// SPDX-License-Identifier: AGPL-3.0-or-later

//! Stripe service — wraps the `async-stripe` crate for all Stripe API
//! interactions: customers, checkout sessions, subscriptions, webhooks,
//! invoices, and billing portal sessions.

use serde::{Deserialize, Serialize};
use stripe::{Client, StripeError};
use stripe_billing::{
    invoice::ListInvoice,
    subscription::{RetrieveSubscription, UpdateSubscription},
    subscription_item::UpdateSubscriptionItem,
};
use stripe_checkout::checkout_session::{
    CreateCheckoutSession, CreateCheckoutSessionAutomaticTax, CreateCheckoutSessionCustomText,
    CreateCheckoutSessionCustomerUpdate, CreateCheckoutSessionCustomerUpdateAddress,
    CreateCheckoutSessionCustomerUpdateName, CreateCheckoutSessionLineItems,
    CreateCheckoutSessionPaymentMethodTypes, CreateCheckoutSessionSubscriptionData,
    CreateCheckoutSessionTaxIdCollection, CustomTextPositionParam, RetrieveCheckoutSession,
};
use stripe_checkout::CheckoutSessionMode;
use stripe_core::customer::CreateCustomer;
use stripe_webhook::Webhook;

// ─── Public types ───────────────────────────────────────────────────────────

/// Parameters for creating an embedded checkout session (subscription).
#[derive(Debug)]
pub struct EmbeddedCheckoutParams {
    pub customer_id: String,
    pub price_id: String,
    pub workspace_id: String,
    pub quantity: u64,
}

/// Result of creating an embedded checkout session.
#[derive(Debug, Serialize, Deserialize)]
pub struct EmbeddedCheckoutResult {
    pub client_secret: String,
    pub session_id: String,
}

/// A simplified invoice record for API responses.
#[derive(Debug, Serialize, Deserialize)]
pub struct InvoiceData {
    pub invoice_id: String,
    pub amount_paid: i64,
    pub currency: String,
    pub status: Option<String>,
    pub hosted_invoice_url: Option<String>,
    pub invoice_pdf: Option<String>,
    pub created: Option<i64>,
}

/// Status of a checkout session (for verifying completion).
#[derive(Debug, Serialize, Deserialize)]
pub struct CheckoutSessionStatus {
    pub status: String,
    pub payment_status: String,
}

// ─── Retry classification ───────────────────────────────────────────────────

/// Returns `true` if the Stripe error is transient and the operation may succeed
/// on retry.
///
/// Retryable conditions:
/// - `Timeout` — network timeout talking to Stripe.
/// - `ClientError` — network-level error (connection reset, DNS failure, etc.).
/// - `Stripe(_, status)` where status is 429 (rate limit), 502, 503, or 504
///   (gateway/upstream errors).
///
/// Permanent errors that must not be retried:
/// - `Stripe(_, 400..=404)` — bad request, auth failure, not found.
/// - `JSONDeserialize` — response parsing error (not a transient condition).
/// - `ConfigError` — client misconfiguration.
fn is_stripe_transient(e: &StripeError) -> bool {
    match e {
        StripeError::Timeout => true,
        StripeError::ClientError(_) => true,
        StripeError::Stripe(_, status) => {
            trakkt_core::retry::is_transient_http_status(*status)
        }
        StripeError::JSONDeserialize(_) | StripeError::ConfigError(_) => false,
    }
}

// ─── Service ────────────────────────────────────────────────────────────────

/// Wraps the `async-stripe` `Client` and provides typed methods for
/// all Stripe operations used by Trakkt's billing system.
#[derive(Clone)]
pub struct StripeService {
    client: Client,
    price_id: String,
    webhook_secret: String,
    publishable_key: String,
}

impl StripeService {
    /// Create a new `StripeService` from explicit configuration values.
    pub fn new(
        secret_key: &str,
        webhook_secret: &str,
        price_id: &str,
        publishable_key: &str,
    ) -> Self {
        let client = Client::new(secret_key);
        Self {
            client,
            price_id: price_id.to_string(),
            webhook_secret: webhook_secret.to_string(),
            publishable_key: publishable_key.to_string(),
        }
    }

    /// Create a `StripeService` from environment variables.
    ///
    /// Returns `None` if `STRIPE_SECRET_KEY` is not set — billing is disabled.
    ///
    /// Required env vars when billing is enabled:
    /// - `STRIPE_SECRET_KEY`
    /// - `STRIPE_WEBHOOK_SECRET`
    /// - `STRIPE_PRICE_ID`
    /// - `STRIPE_PUBLISHABLE_KEY`
    pub fn new_from_env() -> Option<Self> {
        let secret_key = std::env::var("STRIPE_SECRET_KEY").ok()?;
        let webhook_secret = match std::env::var("STRIPE_WEBHOOK_SECRET") {
            Ok(v) => v,
            Err(_) => {
                tracing::warn!("STRIPE_SECRET_KEY set but STRIPE_WEBHOOK_SECRET missing");
                return None;
            }
        };
        let price_id = match std::env::var("STRIPE_PRICE_ID") {
            Ok(v) => v,
            Err(_) => {
                tracing::warn!("STRIPE_SECRET_KEY set but STRIPE_PRICE_ID missing");
                return None;
            }
        };
        let publishable_key = match std::env::var("STRIPE_PUBLISHABLE_KEY") {
            Ok(v) => v,
            Err(_) => {
                tracing::warn!("STRIPE_SECRET_KEY set but STRIPE_PUBLISHABLE_KEY missing");
                return None;
            }
        };

        Some(Self::new(&secret_key, &webhook_secret, &price_id, &publishable_key))
    }

    /// Returns the Stripe publishable key (used client-side for Stripe.js).
    pub fn publishable_key(&self) -> &str {
        &self.publishable_key
    }

    /// Returns the configured Stripe price ID.
    pub fn price_id(&self) -> &str {
        &self.price_id
    }

    // ── Customer ─────────────────────────────────────────────────────────

    /// Create a Stripe customer for a workspace.
    ///
    /// Returns the Stripe customer ID.
    pub async fn create_customer(
        &self,
        email: &str,
        workspace_id: &str,
        workspace_name: &str,
    ) -> Result<String, StripeError> {
        let description = format!("Trakkt Workspace: {workspace_name}");
        let metadata: std::collections::HashMap<String, String> = [
            ("workspace_id".to_string(), workspace_id.to_string()),
            ("workspace_name".to_string(), workspace_name.to_string()),
        ]
        .into_iter()
        .collect();

        let customer = trakkt_core::retry::retry_with_backoff_classified(
            || async {
                CreateCustomer::new()
                    .email(email)
                    .description(&description)
                    .metadata(metadata.clone())
                    .send(&self.client)
                    .await
            },
            is_stripe_transient,
        )
        .await?;

        tracing::info!(
            customer_id = %customer.id,
            workspace_id,
            "Created Stripe customer"
        );

        Ok(customer.id.to_string())
    }

    // ── Embedded Checkout ─────────────────────────────────────────────────

    /// Create an embedded Stripe Checkout session for a new subscription.
    ///
    /// Unlike hosted checkout, the user stays on our site. The returned
    /// `client_secret` is passed to Stripe.js `initEmbeddedCheckout()`.
    pub async fn create_embedded_checkout_session(
        &self,
        params: &EmbeddedCheckoutParams,
    ) -> Result<EmbeddedCheckoutResult, StripeError> {
        let line_items = vec![CreateCheckoutSessionLineItems {
            price: Some(params.price_id.clone()),
            quantity: Some(params.quantity),
            ..Default::default()
        }];

        let sub_metadata: std::collections::HashMap<String, String> = [
            ("workspace_id".to_string(), params.workspace_id.clone()),
            ("brand".to_string(), "trakkt".to_string()),
        ]
        .into_iter()
        .collect();

        let sub_data = CreateCheckoutSessionSubscriptionData {
            description: Some("Trakkt Pro".to_string()),
            metadata: Some(sub_metadata),
            ..Default::default()
        };

        let session_metadata: std::collections::HashMap<String, String> = [
            ("workspace_id".to_string(), params.workspace_id.clone()),
            ("brand".to_string(), "trakkt".to_string()),
        ]
        .into_iter()
        .collect();

        let session = trakkt_core::retry::retry_with_backoff_classified(
            || async {
                CreateCheckoutSession::new()
                    .customer(&params.customer_id)
                    .mode(CheckoutSessionMode::Subscription)
                    .ui_mode(stripe_shared::CheckoutSessionUiMode::Embedded)
                    .redirect_on_completion(
                        stripe_shared::CheckoutSessionRedirectOnCompletion::Never,
                    )
                    .payment_method_types(vec![CreateCheckoutSessionPaymentMethodTypes::Card])
                    .line_items(line_items.clone())
                    .subscription_data(sub_data.clone())
                    .metadata(session_metadata.clone())
                    .custom_text(CreateCheckoutSessionCustomText {
                        submit: Some(CustomTextPositionParam::new("Subscribe to Trakkt Pro")),
                        ..Default::default()
                    })
                    .automatic_tax(CreateCheckoutSessionAutomaticTax {
                        enabled: true,
                        liability: None,
                    })
                    .tax_id_collection(CreateCheckoutSessionTaxIdCollection::new(true))
                    .customer_update(CreateCheckoutSessionCustomerUpdate {
                        name: Some(CreateCheckoutSessionCustomerUpdateName::Auto),
                        address: Some(CreateCheckoutSessionCustomerUpdateAddress::Auto),
                        ..Default::default()
                    })
                    .send(&self.client)
                    .await
            },
            is_stripe_transient,
        )
        .await?;

        tracing::info!(
            session_id = %session.id,
            workspace_id = %params.workspace_id,
            "Created embedded checkout session"
        );

        let client_secret = session.client_secret.ok_or_else(|| {
            StripeError::ClientError(
                "Embedded checkout session created but no client_secret returned".into(),
            )
        })?;

        Ok(EmbeddedCheckoutResult {
            session_id: session.id.to_string(),
            client_secret,
        })
    }

    // ── Subscription management ──────────────────────────────────────────

    /// Update the seat count on a subscription.
    ///
    /// Retrieves the subscription, finds the first item, and sets its quantity.
    pub async fn update_seat_count(
        &self,
        subscription_id: &str,
        total_users: u64,
    ) -> Result<(), StripeError> {
        let subscription = trakkt_core::retry::retry_with_backoff_classified(
            || async { RetrieveSubscription::new(subscription_id).send(&self.client).await },
            is_stripe_transient,
        )
        .await?;

        let first_item = subscription.items.data.first().ok_or_else(|| {
            StripeError::ClientError("Subscription has no items".into())
        })?;

        let first_item_id = first_item.id.clone();
        trakkt_core::retry::retry_with_backoff_classified(
            || {
                let first_item_id = first_item_id.clone();
                async move {
                    UpdateSubscriptionItem::new(first_item_id)
                        .quantity(total_users)
                        .send(&self.client)
                        .await
                }
            },
            is_stripe_transient,
        )
        .await?;

        tracing::info!(subscription_id, total_users, "Updated seat count");

        Ok(())
    }

    /// Get the current seat count (quantity on the first subscription item).
    pub async fn get_subscription_quantity(
        &self,
        subscription_id: &str,
    ) -> Result<u64, StripeError> {
        let subscription = trakkt_core::retry::retry_with_backoff_classified(
            || async { RetrieveSubscription::new(subscription_id).send(&self.client).await },
            is_stripe_transient,
        )
        .await?;

        let first_item = subscription.items.data.first().ok_or_else(|| {
            StripeError::ClientError("Subscription has no items".into())
        })?;

        Ok(first_item.quantity.unwrap_or(1))
    }

    /// Cancel a subscription at the end of the current billing period.
    pub async fn cancel_subscription(
        &self,
        subscription_id: &str,
    ) -> Result<(), StripeError> {
        trakkt_core::retry::retry_with_backoff_classified(
            || async {
                UpdateSubscription::new(subscription_id)
                    .cancel_at_period_end(true)
                    .send(&self.client)
                    .await
            },
            is_stripe_transient,
        )
        .await?;

        tracing::info!(subscription_id, "Scheduled subscription cancellation at period end");
        Ok(())
    }

    /// Reactivate a subscription that was scheduled for cancellation
    /// (remove the `cancel_at_period_end` flag).
    pub async fn reactivate_subscription(
        &self,
        subscription_id: &str,
    ) -> Result<(), StripeError> {
        trakkt_core::retry::retry_with_backoff_classified(
            || async {
                UpdateSubscription::new(subscription_id)
                    .cancel_at_period_end(false)
                    .send(&self.client)
                    .await
            },
            is_stripe_transient,
        )
        .await?;

        tracing::info!(subscription_id, "Reactivated subscription");
        Ok(())
    }

    // ── Billing Portal ───────────────────────────────────────────────────

    /// Create a Stripe Customer Portal session.
    ///
    /// Returns the portal URL for the customer to manage billing.
    pub async fn create_portal_session(
        &self,
        customer_id: &str,
        return_url: &str,
    ) -> Result<String, StripeError> {
        let portal = trakkt_core::retry::retry_with_backoff_classified(
            || async {
                stripe_billing::billing_portal_session::CreateBillingPortalSession::new()
                    .customer(customer_id)
                    .return_url(return_url)
                    .send(&self.client)
                    .await
            },
            is_stripe_transient,
        )
        .await?;

        tracing::info!(
            customer_id,
            session_id = %portal.id,
            "Created billing portal session"
        );

        Ok(portal.url)
    }

    // ── Invoices ─────────────────────────────────────────────────────────

    /// List recent invoices for a Stripe customer.
    pub async fn list_invoices(
        &self,
        customer_id: &str,
        limit: u64,
    ) -> Result<Vec<InvoiceData>, StripeError> {
        let invoices = trakkt_core::retry::retry_with_backoff_classified(
            || async {
                ListInvoice::new()
                    .customer(customer_id)
                    .limit(limit as i64)
                    .send(&self.client)
                    .await
            },
            is_stripe_transient,
        )
        .await?;

        let result = invoices
            .data
            .iter()
            .map(|inv| InvoiceData {
                invoice_id: inv.id.as_ref().map(|id| id.to_string()).unwrap_or_else(|| {
                    tracing::warn!("Invoice from Stripe has no id");
                    String::new()
                }),
                amount_paid: inv.amount_paid,
                currency: inv.currency.to_string(),
                status: inv.status.as_ref().map(|s| format!("{s:?}").to_lowercase()),
                hosted_invoice_url: inv.hosted_invoice_url.clone(),
                invoice_pdf: inv.invoice_pdf.clone(),
                created: Some(inv.created),
            })
            .collect();

        Ok(result)
    }

    // ── Checkout session status ──────────────────────────────────────────

    /// Retrieve a checkout session's status (for verifying completion).
    pub async fn retrieve_checkout_session_status(
        &self,
        session_id: &str,
    ) -> Result<CheckoutSessionStatus, StripeError> {
        let session = trakkt_core::retry::retry_with_backoff_classified(
            || async {
                RetrieveCheckoutSession::new(session_id.to_string())
                    .send(&self.client)
                    .await
            },
            is_stripe_transient,
        )
        .await?;

        let status = match session.status {
            Some(s) => s.as_str().to_string(),
            None => {
                tracing::warn!(session_id, "Checkout session has no status field");
                "unknown".to_string()
            }
        };
        let payment_status = session.payment_status.as_str().to_string();

        Ok(CheckoutSessionStatus {
            status,
            payment_status,
        })
    }

    // ── Webhook verification ─────────────────────────────────────────────

    /// Verify a Stripe webhook signature and parse the event.
    ///
    /// Uses the library's `Webhook::construct_event` which handles HMAC-SHA256
    /// verification and event deserialization from Stripe's current OpenAPI spec.
    pub fn construct_webhook_event(
        &self,
        payload: &str,
        sig_header: &str,
    ) -> Result<stripe_webhook::Event, stripe_webhook::WebhookError> {
        Webhook::construct_event(payload, sig_header, &self.webhook_secret)
    }
}
