// SPDX-License-Identifier: AGPL-3.0-or-later

//! Billing helper functions — gate checks and user count queries for
//! Stripe-based subscription enforcement.

use trakkt_core::DbPool;
use trakkt_core::sql_compat;

use crate::stripe_service::StripeService;

/// Returns `true` if Stripe billing is enabled (STRIPE_SECRET_KEY is set).
pub fn billing_enabled() -> bool {
    std::env::var("STRIPE_SECRET_KEY").is_ok()
}

/// Returns `true` if the workspace is allowed to invite new users.
///
/// When billing is disabled (self-hosted / development), invitations are
/// always allowed. When billing is enabled, only workspaces with an active
/// subscription may invite.
pub fn can_invite_users(subscription_status: Option<&str>) -> bool {
    if !billing_enabled() {
        return true;
    }
    matches!(subscription_status, Some("active") | Some("trialing"))
}

/// Count the number of active (confirmed) members in a workspace.
///
/// This is the billable user count used for seat-based subscription
/// quantity updates.
pub async fn get_billable_user_count(db: &DbPool, workspace_id: &str) -> trakkt_core::Result<i64> {
    let is_pg = db.is_postgres();
    let bt = sql_compat::bool_true(is_pg);
    let sql = format!(
        "SELECT COUNT(*) FROM workspace_users \
         WHERE workspace_id = $1 AND active = {bt}"
    );
    let count: i64 = trakkt_core::db_fetch_scalar!(db, i64, &sql, workspace_id)?;
    Ok(count)
}

// ─── Customer management ──────────────────────────────────────────────────

/// Ensure a Stripe customer exists for the workspace, creating one if needed.
///
/// Returns the Stripe customer ID. If one already exists in the database,
/// returns it directly. Otherwise creates a new customer via Stripe API
/// and persists the ID.
pub async fn ensure_customer_exists(
    db: &DbPool,
    stripe: &StripeService,
    workspace_id: &str,
    email: &str,
    workspace_name: &str,
) -> trakkt_core::Result<String> {
    #[derive(sqlx::FromRow)]
    struct CustRow { stripe_customer_id: Option<String> }

    let row = trakkt_core::db_fetch_one!(
        db, CustRow,
        "SELECT stripe_customer_id FROM workspaces WHERE workspace_id = $1",
        workspace_id
    )?;

    if let Some(existing) = row.stripe_customer_id {
        return Ok(existing);
    }

    let new_id = stripe.create_customer(email, workspace_id, workspace_name)
        .await
        .map_err(|e| trakkt_core::Error::Internal(format!("Stripe customer creation failed: {e}")))?;

    let is_pg = db.is_postgres();
    let now = sql_compat::now(is_pg);
    let sql = format!(
        "UPDATE workspaces SET stripe_customer_id = $1, updated_at = {now} WHERE workspace_id = $2"
    );
    trakkt_core::db_execute!(db, &sql, &new_id, workspace_id)?;

    Ok(new_id)
}

// ─── Seat sync ─────────────────────────────────────────────────────────────

/// Minimal row for fetching a workspace's Stripe subscription ID.
#[derive(sqlx::FromRow)]
struct WorkspaceSubscription {
    stripe_subscription_id: Option<String>,
}

/// Sync the Stripe subscription quantity with the actual workspace member count.
///
/// No-op if the workspace has no active subscription. Only calls Stripe API
/// if the local count differs from Stripe's quantity.
pub async fn sync_seat_count(
    db: &DbPool,
    stripe: &StripeService,
    workspace_id: &str,
) -> trakkt_core::Result<()> {
    // 1. Get workspace's stripe_subscription_id
    let row = trakkt_core::db_fetch_optional!(
        db, WorkspaceSubscription,
        "SELECT stripe_subscription_id FROM workspaces WHERE workspace_id = $1",
        workspace_id
    )?;

    let subscription_id = match row.and_then(|r| r.stripe_subscription_id) {
        Some(id) => id,
        // No subscription — free/solo workspace, nothing to sync
        None => return Ok(()),
    };

    // 2. Count active workspace members
    let user_count = get_billable_user_count(db, workspace_id).await?;

    // 3. Get current Stripe quantity
    let stripe_quantity = match stripe.get_subscription_quantity(&subscription_id).await {
        Ok(q) => q,
        Err(e) => {
            tracing::warn!(
                workspace_id,
                error = %e,
                "Failed to get subscription quantity from Stripe"
            );
            return Ok(());
        }
    };

    // 4. If different → update Stripe and local user_limit
    if user_count as u64 == stripe_quantity {
        return Ok(());
    }

    match stripe.update_seat_count(&subscription_id, user_count as u64).await {
        Ok(()) => {
            let is_pg = db.is_postgres();
            let now = sql_compat::now(is_pg);
            let sql = format!(
                "UPDATE workspaces SET user_limit = $1, updated_at = {now} WHERE workspace_id = $2"
            );
            trakkt_core::db_execute!(db, &sql, user_count as i32, workspace_id)?;
            tracing::info!(
                workspace_id,
                old_quantity = stripe_quantity,
                new_quantity = user_count,
                "Synced seat count with Stripe"
            );
        }
        Err(e) => {
            tracing::warn!(
                workspace_id,
                error = %e,
                "Failed to sync seat count with Stripe"
            );
        }
    }

    Ok(())
}
