// SPDX-License-Identifier: AGPL-3.0-or-later

//! Billing helper functions — gate checks and user count queries for
//! Stripe-based subscription enforcement.

use trakkt_core::DbPool;
use trakkt_core::sql_compat;

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
