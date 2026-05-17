// SPDX-License-Identifier: AGPL-3.0-or-later

//! Billing REST endpoint — Stripe webhook handler.
//!
//! This module handles Stripe webhook events for subscription lifecycle
//! management. The endpoint is exempt from auth middleware because Stripe
//! POSTs to it directly — it relies on signature verification instead.

use axum::{
    body::Bytes,
    extract::State,
    http::HeaderMap,
    routing::post,
    Json, Router,
};
use serde_json::{json, Value};
use stripe_shared::{CheckoutSessionMode, Invoice, Subscription, SubscriptionStatus};
use stripe_types::Expandable;
use stripe_webhook::EventObject;

use trakkt_auth::stripe_service::StripeService;
use trakkt_core::db::DbPool;

use crate::state::AppState;

// ===========================================================================
// Router
// ===========================================================================

/// Build the billing router — Stripe webhook endpoint only.
///
/// The webhook endpoint does NOT use auth middleware — it relies on
/// Stripe signature verification instead.
pub fn routes() -> Router<AppState> {
    Router::new().route("/stripe", post(stripe_webhook))
}

// ===========================================================================
// Internal row types
// ===========================================================================

/// Minimal workspace row used by webhook event handlers.
#[derive(Debug, sqlx::FromRow)]
struct WorkspaceRow {
    workspace_id: String,
}

// ===========================================================================
// Helpers
// ===========================================================================

/// Get a reference to the StripeService, or return 400 if not configured.
fn require_stripe(state: &AppState) -> Result<&StripeService, trakkt_core::Error> {
    state.stripe.as_ref().ok_or_else(|| {
        trakkt_core::Error::BadRequest("Billing features are not available".into())
    })
}

/// Load a workspace by its `stripe_subscription_id`.
async fn load_workspace_by_subscription(
    db: &DbPool,
    subscription_id: &str,
) -> Option<WorkspaceRow> {
    trakkt_core::db_fetch_optional!(
        db, WorkspaceRow,
        "SELECT workspace_id FROM workspaces WHERE stripe_subscription_id = $1",
        subscription_id
    )
    .ok()
    .flatten()
}

/// Extract `workspace_id` from subscription metadata.
fn workspace_id_from_metadata(metadata: &std::collections::HashMap<String, String>) -> Option<String> {
    metadata.get("workspace_id").filter(|id| !id.is_empty()).cloned()
}

// ===========================================================================
// Webhook handler
// ===========================================================================

/// POST /webhooks/stripe — Stripe webhook handler (no auth).
pub async fn stripe_webhook(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Json<Value>, trakkt_core::Error> {
    let stripe_service = require_stripe(&state)?;

    let sig_header = headers
        .get("stripe-signature")
        .and_then(|v| v.to_str().ok())
        .ok_or_else(|| trakkt_core::Error::BadRequest("Missing Stripe signature".into()))?;

    // Convert raw bytes to a string for signature verification
    let payload = std::str::from_utf8(&body).map_err(|_| {
        trakkt_core::Error::BadRequest("Invalid UTF-8 in webhook payload".into())
    })?;

    // Verify webhook signature and parse typed event
    let event = stripe_service
        .construct_webhook_event(payload, sig_header)
        .map_err(|e| {
            tracing::error!(
                error = %e,
                error_debug = ?e,
                sig_header = %sig_header,
                payload_len = payload.len(),
                "Stripe webhook signature verification failed"
            );
            trakkt_core::Error::BadRequest("Invalid signature".into())
        })?;

    let event_type = event.type_.to_string();
    tracing::info!(event_type = %event_type, "Received Stripe webhook");

    // Dispatch on typed event object
    match event.data.object {
        EventObject::CheckoutSessionCompleted(session) => {
            handle_checkout_completed(&state, &session).await;
        }
        EventObject::CustomerSubscriptionCreated(sub) => {
            handle_subscription_created_or_updated(&state, &sub).await;
        }
        EventObject::CustomerSubscriptionUpdated(sub) => {
            handle_subscription_created_or_updated(&state, &sub).await;
        }
        EventObject::CustomerSubscriptionDeleted(sub) => {
            handle_subscription_deleted(&state, &sub).await;
        }
        EventObject::InvoicePaymentSucceeded(inv) => {
            handle_invoice_payment_succeeded(&state, &inv).await;
        }
        EventObject::InvoicePaymentFailed(inv) => {
            handle_invoice_payment_failed(&state, &inv).await;
        }
        _ => {
            tracing::debug!(event_type = %event_type, "Unhandled Stripe event type");
        }
    }

    // Always return 200 to acknowledge receipt (Stripe retries on non-200)
    Ok(Json(json!({})))
}

// ===========================================================================
// Event handlers
// ===========================================================================

/// Handle `checkout.session.completed` — set customer/subscription IDs and activate.
///
/// Only processes subscription checkouts. Payment-mode checkouts are ignored
/// because Trakkt only uses Stripe for subscriptions.
async fn handle_checkout_completed(state: &AppState, session: &stripe_shared::CheckoutSession) {
    // Only handle subscription checkouts
    if session.mode != CheckoutSessionMode::Subscription {
        tracing::debug!("Checkout session is not subscription mode — skipping");
        return;
    }

    let metadata = match &session.metadata {
        Some(m) => m,
        None => {
            tracing::warn!("Checkout session has no metadata");
            return;
        }
    };

    let workspace_id = match workspace_id_from_metadata(metadata) {
        Some(id) => id,
        None => {
            tracing::warn!("Checkout session has no workspace_id in metadata — skipping");
            return;
        }
    };

    // Extract customer ID
    let customer_id = match &session.customer {
        Some(Expandable::Id(id)) => id.to_string(),
        Some(Expandable::Object(c)) => c.id.to_string(),
        None => {
            tracing::error!(
                workspace_id = %workspace_id,
                "Checkout session has no customer — cannot activate subscription"
            );
            return;
        }
    };

    // Extract subscription ID
    let subscription_id = match &session.subscription {
        Some(Expandable::Id(id)) => id.to_string(),
        Some(Expandable::Object(sub)) => sub.id.to_string(),
        None => {
            tracing::error!(
                workspace_id = %workspace_id,
                "Checkout session has no subscription — cannot activate"
            );
            return;
        }
    };

    let result = trakkt_core::db_execute!(
        &state.db,
        "UPDATE workspaces SET \
             stripe_customer_id = $1, \
             stripe_subscription_id = $2, \
             subscription_status = 'active' \
         WHERE workspace_id = $3",
        &customer_id,
        &subscription_id,
        &workspace_id
    );

    match result {
        Ok(_) => {
            tracing::info!(
                workspace_id = %workspace_id,
                customer_id = %customer_id,
                subscription_id = %subscription_id,
                "checkout.session.completed — activated subscription"
            );
        }
        Err(e) => {
            tracing::error!(
                workspace_id = %workspace_id,
                "Failed to activate subscription from checkout: {e}"
            );
        }
    }
}

/// Handle `customer.subscription.created` and `customer.subscription.updated`.
///
/// Extracts subscription status, user limit (quantity), and period dates,
/// then updates the workspace accordingly.
async fn handle_subscription_created_or_updated(state: &AppState, subscription: &Subscription) {
    let workspace_id = match workspace_id_from_metadata(&subscription.metadata) {
        Some(id) => id,
        None => {
            tracing::warn!("Subscription event missing workspace_id in metadata — skipping");
            return;
        }
    };

    // Determine subscription status
    let status = if subscription.cancel_at_period_end {
        "cancelled"
    } else {
        match subscription.status {
            SubscriptionStatus::Active => "active",
            SubscriptionStatus::Trialing => "trialing",
            SubscriptionStatus::PastDue => "past_due",
            SubscriptionStatus::Canceled | SubscriptionStatus::Unpaid => "cancelled",
            SubscriptionStatus::Incomplete
            | SubscriptionStatus::IncompleteExpired
            | SubscriptionStatus::Paused
            | SubscriptionStatus::Unknown(_)
            | _ => "past_due",
        }
    };

    // Get quantity (user_limit) from first subscription item
    let user_limit: i32 = subscription
        .items
        .data
        .first()
        .and_then(|item| item.quantity)
        .unwrap_or(1) as i32;

    // Get period dates from the first subscription item
    // (current_period_start/end are on SubscriptionItem in the 1.0 crate)
    let (period_start, period_end) = if let Some(first_item) = subscription.items.data.first() {
        (
            chrono::DateTime::from_timestamp(first_item.current_period_start, 0),
            chrono::DateTime::from_timestamp(first_item.current_period_end, 0),
        )
    } else {
        (None, None)
    };

    let result = trakkt_core::db_execute!(
        &state.db,
        "UPDATE workspaces SET \
             subscription_status = $1, \
             user_limit = $2, \
             subscription_period_start = $3, \
             subscription_period_end = $4 \
         WHERE workspace_id = $5",
        status,
        user_limit,
        period_start,
        period_end,
        &workspace_id
    );

    match result {
        Ok(_) => {
            tracing::info!(
                workspace_id = %workspace_id,
                status,
                user_limit,
                "Subscription created/updated"
            );
        }
        Err(e) => {
            tracing::error!(
                workspace_id = %workspace_id,
                "Failed to update workspace from subscription event: {e}"
            );
        }
    }
}

/// Handle `customer.subscription.deleted` — revert workspace to free tier.
async fn handle_subscription_deleted(state: &AppState, subscription: &Subscription) {
    let workspace_id = match workspace_id_from_metadata(&subscription.metadata) {
        Some(id) => id,
        None => {
            tracing::warn!("Subscription deleted event missing workspace_id in metadata — skipping");
            return;
        }
    };

    let result = trakkt_core::db_execute!(
        &state.db,
        "UPDATE workspaces SET \
             subscription_status = 'free', \
             stripe_subscription_id = NULL, \
             subscription_period_start = NULL, \
             subscription_period_end = NULL, \
             user_limit = 1 \
         WHERE workspace_id = $1",
        &workspace_id
    );

    match result {
        Ok(_) => {
            tracing::info!(
                workspace_id = %workspace_id,
                "Subscription deleted — reverted to free tier"
            );
        }
        Err(e) => {
            tracing::error!(
                workspace_id = %workspace_id,
                "Failed to revert workspace to free tier: {e}"
            );
        }
    }
}

/// Handle `invoice.payment_succeeded` — ensure subscription is active.
///
/// This catches the case where a workspace was `past_due` (failed payment)
/// and a retry payment succeeds, restoring it to `active`.
async fn handle_invoice_payment_succeeded(state: &AppState, invoice: &Invoice) {
    let subscription_id = match &invoice.subscription {
        Some(Expandable::Id(id)) => id.to_string(),
        Some(Expandable::Object(sub)) => sub.id.to_string(),
        None => {
            tracing::debug!("Invoice has no subscription — skipping");
            return;
        }
    };

    let workspace = match load_workspace_by_subscription(&state.db, &subscription_id).await {
        Some(w) => w,
        None => {
            tracing::warn!(
                subscription_id = %subscription_id,
                "No workspace found for subscription in payment_succeeded event"
            );
            return;
        }
    };

    let result = trakkt_core::db_execute!(
        &state.db,
        "UPDATE workspaces SET subscription_status = 'active' WHERE workspace_id = $1",
        &workspace.workspace_id
    );

    match result {
        Ok(_) => {
            tracing::info!(
                workspace_id = %workspace.workspace_id,
                "Payment succeeded — ensured subscription is active"
            );
        }
        Err(e) => {
            tracing::error!(
                workspace_id = %workspace.workspace_id,
                "Failed to update subscription status to active: {e}"
            );
        }
    }
}

/// Handle `invoice.payment_failed` — mark subscription as past_due.
async fn handle_invoice_payment_failed(state: &AppState, invoice: &Invoice) {
    let subscription_id = match &invoice.subscription {
        Some(Expandable::Id(id)) => id.to_string(),
        Some(Expandable::Object(sub)) => sub.id.to_string(),
        None => {
            tracing::debug!("Invoice has no subscription — skipping");
            return;
        }
    };

    let workspace = match load_workspace_by_subscription(&state.db, &subscription_id).await {
        Some(w) => w,
        None => {
            tracing::warn!(
                subscription_id = %subscription_id,
                "No workspace found for subscription in payment_failed event"
            );
            return;
        }
    };

    let result = trakkt_core::db_execute!(
        &state.db,
        "UPDATE workspaces SET subscription_status = 'past_due' WHERE workspace_id = $1",
        &workspace.workspace_id
    );

    match result {
        Ok(_) => {
            tracing::warn!(
                workspace_id = %workspace.workspace_id,
                "Payment failed — marked subscription as past_due"
            );
        }
        Err(e) => {
            tracing::error!(
                workspace_id = %workspace.workspace_id,
                "Failed to update subscription status to past_due: {e}"
            );
        }
    }
}
