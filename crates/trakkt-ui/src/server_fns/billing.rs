// SPDX-License-Identifier: AGPL-3.0-or-later

//! Server functions for Stripe billing management.
//!
//! Provides the backend for the billing settings page: subscription info,
//! checkout session creation, subscription lifecycle (cancel/reactivate),
//! Stripe billing portal access, and invoice history.

use leptos::prelude::*;
use serde::{Deserialize, Serialize};

// ─────────────────────────────────────────────────────────────────────────────
// Shared types (available on both client and server)
// ─────────────────────────────────────────────────────────────────────────────

/// Summary of a workspace's billing state for the settings UI.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BillingInfo {
    pub subscription_status: String,
    pub user_count: i64,
    pub user_limit: Option<i32>,
    pub monthly_cost: Option<f64>,
    pub period_end: Option<String>,
}

/// Result of creating an embedded Stripe Checkout session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CheckoutResult {
    pub client_secret: String,
    pub session_id: String,
}

/// A single invoice record for the billing history UI.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InvoiceInfo {
    pub date: String,
    pub amount: String,
    pub status: String,
    pub pdf_url: Option<String>,
}

// ─────────────────────────────────────────────────────────────────────────────
// Helpers (server-only)
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(feature = "ssr")]
use super::{require_workspace_admin, AuthenticatedContext, IntoServerFnError};

/// Row for fetching workspace billing state.
#[cfg(feature = "ssr")]
#[derive(Debug, sqlx::FromRow)]
struct WorkspaceBillingRow {
    subscription_status: Option<String>,
    subscription_period_end: Option<chrono::DateTime<chrono::Utc>>,
    user_limit: Option<i32>,
}

/// Row for fetching workspace info needed for checkout.
#[cfg(feature = "ssr")]
#[derive(Debug, sqlx::FromRow)]
struct WorkspaceCheckoutRow {
    name: Option<String>,
    admin_email: Option<String>,
}

/// Extract the `StripeService` from the server context, or return an error
/// if billing is not enabled.
#[cfg(feature = "ssr")]
fn require_stripe(
    ctx: &super::ServerContext,
) -> Result<&trakkt_auth::stripe_service::StripeService, ServerFnError> {
    ctx.stripe
        .as_ref()
        .ok_or_else(|| ServerFnError::new("Billing is not enabled"))
}

/// Fetch the workspace's Stripe subscription ID.
#[cfg(feature = "ssr")]
async fn fetch_subscription_id(
    db: &trakkt_core::DbPool,
    ws_id: &str,
) -> Result<String, ServerFnError> {
    #[derive(sqlx::FromRow)]
    struct Row { stripe_subscription_id: Option<String> }

    let row = trakkt_core::db_fetch_one!(
        db, Row,
        "SELECT stripe_subscription_id FROM workspaces WHERE workspace_id = $1",
        ws_id
    ).into_sfn()?;

    row.stripe_subscription_id
        .ok_or_else(|| ServerFnError::new("No active subscription found"))
}

/// Fetch the workspace's Stripe customer ID.
#[cfg(feature = "ssr")]
async fn fetch_customer_id(
    db: &trakkt_core::DbPool,
    ws_id: &str,
) -> Result<String, ServerFnError> {
    #[derive(sqlx::FromRow)]
    struct Row { stripe_customer_id: Option<String> }

    let row = trakkt_core::db_fetch_one!(
        db, Row,
        "SELECT stripe_customer_id FROM workspaces WHERE workspace_id = $1",
        ws_id
    ).into_sfn()?;

    row.stripe_customer_id
        .ok_or_else(|| ServerFnError::new("No billing account found for this workspace"))
}

#[cfg(feature = "ssr")]
const SEAT_PRICE_USD: f64 = 5.0;

// ─────────────────────────────────────────────────────────────────────────────
// Server functions
// ─────────────────────────────────────────────────────────────────────────────

/// Load billing info for the current workspace.
///
/// Returns `None` when billing is not enabled (self-hosted / development).
#[server(prefix = "/leptos-api")]
pub async fn get_billing_info() -> Result<Option<BillingInfo>, ServerFnError> {
    let ac = AuthenticatedContext::extract().await?;

    if ac.ctx.stripe.is_none() {
        return Ok(None);
    }

    let row: WorkspaceBillingRow = trakkt_core::db_fetch_one!(
        ac.db(),
        WorkspaceBillingRow,
        "SELECT subscription_status, subscription_period_end, user_limit \
         FROM workspaces WHERE workspace_id = $1",
        &ac.ws_id
    )
    .into_sfn()?;

    let user_count =
        trakkt_auth::billing_service::get_billable_user_count(ac.db(), &ac.ws_id)
            .await
            .into_sfn()?;

    let status = row.subscription_status.as_deref().unwrap_or_else(|| {
        tracing::warn!(workspace_id = %ac.ws_id, "workspace has NULL subscription_status");
        "free"
    });

    let monthly_cost = if status == "active" || status == "trialing" {
        Some(user_count as f64 * SEAT_PRICE_USD)
    } else {
        None
    };

    let period_end = row.subscription_period_end.map(|dt| dt.to_rfc3339());

    Ok(Some(BillingInfo {
        subscription_status: status.to_string(),
        user_count,
        user_limit: row.user_limit,
        monthly_cost,
        period_end,
    }))
}

/// Create an embedded Stripe Checkout session for a new subscription.
///
/// Requires workspace admin. Creates a Stripe customer if one doesn't
/// already exist for this workspace (delegated to billing_service).
#[server(prefix = "/leptos-api")]
pub async fn create_checkout(quantity: u64) -> Result<CheckoutResult, ServerFnError> {
    let ac = AuthenticatedContext::extract().await?;
    require_workspace_admin(&ac.auth)?;
    let stripe = require_stripe(&ac.ctx)?;

    let row: WorkspaceCheckoutRow = trakkt_core::db_fetch_one!(
        ac.db(),
        WorkspaceCheckoutRow,
        "SELECT name, admin_email FROM workspaces WHERE workspace_id = $1",
        &ac.ws_id
    )
    .into_sfn()?;

    let email = row.admin_email.as_deref().unwrap_or(&ac.auth.email);
    let ws_name = row.name.as_deref().unwrap_or("Trakkt Workspace");

    let customer_id = trakkt_auth::billing_service::ensure_customer_exists(
        ac.db(), stripe, &ac.ws_id, email, ws_name,
    )
    .await
    .into_sfn()?;

    let params = trakkt_auth::stripe_service::EmbeddedCheckoutParams {
        customer_id,
        price_id: stripe.price_id().to_string(),
        workspace_id: ac.ws_id.clone(),
        quantity,
    };

    let result = stripe
        .create_embedded_checkout_session(&params)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "Failed to create checkout session");
            ServerFnError::new(format!("Failed to create checkout session: {e}"))
        })?;

    Ok(CheckoutResult {
        client_secret: result.client_secret,
        session_id: result.session_id,
    })
}

/// Cancel the workspace's subscription at the end of the current billing period.
///
/// Requires workspace admin. The subscription remains active until the period
/// ends, then status transitions to "cancelled" via webhook.
#[server(prefix = "/leptos-api")]
pub async fn cancel_billing_subscription() -> Result<(), ServerFnError> {
    let ac = AuthenticatedContext::extract().await?;
    require_workspace_admin(&ac.auth)?;
    let stripe = require_stripe(&ac.ctx)?;
    let sub_id = fetch_subscription_id(ac.db(), &ac.ws_id).await?;

    stripe.cancel_subscription(&sub_id).await.map_err(|e| {
        tracing::error!(error = %e, "Failed to cancel subscription");
        ServerFnError::new(format!("Failed to cancel subscription: {e}"))
    })?;

    Ok(())
}

/// Reactivate a subscription that was scheduled for cancellation.
///
/// Requires workspace admin. Removes the `cancel_at_period_end` flag on
/// the Stripe subscription.
#[server(prefix = "/leptos-api")]
pub async fn reactivate_billing_subscription() -> Result<(), ServerFnError> {
    let ac = AuthenticatedContext::extract().await?;
    require_workspace_admin(&ac.auth)?;
    let stripe = require_stripe(&ac.ctx)?;
    let sub_id = fetch_subscription_id(ac.db(), &ac.ws_id).await?;

    stripe
        .reactivate_subscription(&sub_id)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "Failed to reactivate subscription");
            ServerFnError::new(format!("Failed to reactivate subscription: {e}"))
        })?;

    Ok(())
}

/// Create a Stripe Customer Portal session and return the URL.
///
/// Requires workspace admin. The portal lets the customer manage payment
/// methods, view invoices, and update billing details directly on Stripe.
#[server(prefix = "/leptos-api")]
pub async fn create_billing_portal_session() -> Result<String, ServerFnError> {
    let ac = AuthenticatedContext::extract().await?;
    require_workspace_admin(&ac.auth)?;
    let stripe = require_stripe(&ac.ctx)?;
    let customer_id = fetch_customer_id(ac.db(), &ac.ws_id).await?;

    let return_url = format!("{}/settings/billing", ac.ctx.config.base_url);

    let portal_url = stripe
        .create_portal_session(&customer_id, &return_url)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "Failed to create billing portal session");
            ServerFnError::new(format!("Failed to create billing portal session: {e}"))
        })?;

    Ok(portal_url)
}

/// List recent invoices for the workspace's Stripe customer.
///
/// Requires workspace admin. Returns an empty list if no customer exists
/// or billing is not enabled.
#[server(prefix = "/leptos-api")]
pub async fn get_billing_invoices() -> Result<Vec<InvoiceInfo>, ServerFnError> {
    let ac = AuthenticatedContext::extract().await?;
    require_workspace_admin(&ac.auth)?;
    let stripe = require_stripe(&ac.ctx)?;

    let customer_id = match fetch_customer_id(ac.db(), &ac.ws_id).await {
        Ok(id) => id,
        Err(_) => return Ok(Vec::new()),
    };

    let invoices = stripe.list_invoices(&customer_id, 12).await.map_err(|e| {
        tracing::error!(error = %e, "Failed to list invoices from Stripe");
        ServerFnError::new(format!("Failed to list invoices: {e}"))
    })?;

    let result = invoices
        .into_iter()
        .map(|inv| {
            let date = inv
                .created
                .and_then(|ts| chrono::DateTime::from_timestamp(ts, 0))
                .map(|dt| dt.format("%Y-%m-%d").to_string())
                .unwrap_or_else(|| {
                    tracing::warn!("Invoice has no created timestamp");
                    String::new()
                });

            let amount = format_currency(inv.amount_paid, &inv.currency);

            let status = inv.status.unwrap_or_else(|| {
                tracing::warn!("Invoice has no status");
                "unknown".to_string()
            });

            InvoiceInfo {
                date,
                amount,
                status,
                pdf_url: inv.invoice_pdf,
            }
        })
        .collect();

    Ok(result)
}

/// Returns the Stripe publishable key for Stripe.js initialization.
///
/// Returns `None` if billing is not enabled.
#[server(prefix = "/leptos-api")]
pub async fn get_stripe_publishable_key() -> Result<Option<String>, ServerFnError> {
    let ctx = super::extract_context()?;

    Ok(ctx.stripe.as_ref().map(|s| s.publishable_key().to_string()))
}

// ─────────────────────────────────────────────────────────────────────────────
// Internal helpers (server-only)
// ─────────────────────────────────────────────────────────────────────────────

/// Format a Stripe amount (in cents) as a currency string.
///
/// Stripe stores amounts in the smallest currency unit (e.g. cents for USD).
#[cfg(feature = "ssr")]
fn format_currency(amount_cents: i64, currency: &str) -> String {
    let major = amount_cents / 100;
    let minor = (amount_cents % 100).unsigned_abs();
    let symbol = match currency.to_lowercase().as_str() {
        "usd" => "$",
        "eur" => "\u{20ac}",
        "gbp" => "\u{00a3}",
        _ => "",
    };
    if symbol.is_empty() {
        format!("{major}.{minor:02} {}", currency.to_uppercase())
    } else {
        format!("{symbol}{major}.{minor:02}")
    }
}
