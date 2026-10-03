// SPDX-License-Identifier: AGPL-3.0-or-later

//! Workspace service — query functions for workspace management.
//!
//! Used by workspace endpoints (4C/4D) and user endpoints that need
//! workspace context. Single-record lookups (`get_workspace`,
//! `get_workspace_user`) live in `user_service.rs`.

use chrono::Utc;
use trakkt_core::enums::TransferStatus;
use trakkt_core::models::{
    OwnershipTransfer, Workspace, WorkspaceInvitation, WorkspaceUser,
};
use trakkt_core::db::DbTx;
use trakkt_core::sql_compat;
use trakkt_core::DbPool;
use serde::{Deserialize, Serialize};

use crate::sync_log_service;
use crate::websocket::WebSocketManager;
use trakkt_types::models::WorkspaceSettingsSnapshot;
use trakkt_types::sync::{SyncActionType, entity_types};

/// Get all active workspace user memberships for a workspace.
///
/// Returns membership records only (not joined user data).
/// The route handler can do a second query for user details if needed,
/// matching the Python pattern.
pub async fn get_workspace_users(
    pool: &DbPool,
    workspace_id: &str,
) -> trakkt_core::Result<Vec<WorkspaceUser>> {
    let is_pg = pool.is_postgres();
    let bt = sql_compat::bool_true(is_pg);
    let sql = format!(
        "SELECT * FROM workspace_users \
         WHERE workspace_id = $1 AND active = {bt}"
    );
    let users = trakkt_core::db_fetch_all!(pool, WorkspaceUser, &sql, workspace_id)?;
    Ok(users)
}

/// Count active members in a workspace.
pub async fn count_workspace_users(
    pool: &DbPool,
    workspace_id: &str,
) -> trakkt_core::Result<i64> {
    let is_pg = pool.is_postgres();
    let bt = sql_compat::bool_true(is_pg);
    let sql = format!(
        "SELECT COUNT(*) as count FROM workspace_users \
         WHERE workspace_id = $1 AND active = {bt}"
    );
    let count: i64 = trakkt_core::db_fetch_scalar!(pool, i64, &sql, workspace_id)?;
    Ok(count)
}

/// Get all workspaces a user belongs to (active memberships).
///
/// Returns pairs of (Workspace, WorkspaceUser) for each membership.
pub async fn get_user_workspaces(
    pool: &DbPool,
    user_id: &str,
) -> trakkt_core::Result<Vec<(Workspace, WorkspaceUser)>> {
    let is_pg = pool.is_postgres();
    let bt = sql_compat::bool_true(is_pg);

    // Get all active workspace memberships
    let memberships_sql = format!(
        "SELECT * FROM workspace_users \
         WHERE user_id = $1 AND active = {bt} \
         ORDER BY created_at ASC"
    );
    let memberships = trakkt_core::db_fetch_all!(pool, WorkspaceUser, &memberships_sql, user_id)?;

    let mut results = Vec::with_capacity(memberships.len());
    for wu in memberships {
        let ws = trakkt_core::db_fetch_optional!(
            pool, Workspace,
            "SELECT * FROM workspaces WHERE workspace_id = $1",
            &wu.workspace_id
        )?;

        if let Some(ws) = ws {
            results.push((ws, wu));
        }
    }

    Ok(results)
}

#[derive(Debug, sqlx::FromRow)]
struct WorkspaceSnapshotRow {
    workspace_id: String,
    name: Option<String>,
    settings: Option<String>,
    /// The workspace-level default team, carried here rather than as an entity
    /// type of its own.
    ///
    /// It is a column on this same `workspaces` row and is semantically a
    /// workspace-level setting — which is exactly what the `workspace_settings`
    /// entity already is. A second entity type for one nullable string on the
    /// row this one already carries would be a second name for the same thing,
    /// and the two could then disagree about which is current.
    ///
    /// The alternative was priced before it was rejected. A new entity type
    /// costs a constant in `entity_types` (`crates/trakkt-types/src/sync.rs`), a
    /// bootstrap stream in `apps/server/src/routes/websocket.rs`, both an
    /// `Update` and a `Delete` arm in `crates/trakkt-ui/src/cache/apply.rs`, a
    /// version counter on the client store, and a `NOT_CACHED`/cache-membership
    /// decision — all to move a field that already travels in this row.
    ///
    /// Adding the field instead is bounded, and the bound was checked rather
    /// than assumed: `apply.rs`'s two `WORKSPACE_SETTINGS` arms read **no**
    /// field off the payload — they only bump `workspace_settings_version`, and
    /// the settings page refetches through `get_workspace_settings`. The
    /// bootstrap reader forwards the snapshot opaquely. Nothing decodes a
    /// cached `workspace_settings` row into a typed struct, so the addition is
    /// purely additive to every reader that exists.
    default_team_id: Option<String>,
    updated_at: String,
}

/// The `SELECT` behind both snapshot readers below.
///
/// One reads it on the pool for the bootstrap, the other on an open transaction
/// for the `sync_log` payload. Same columns and the same JSONB-to-TEXT casts —
/// only the executor differs, so the two cannot drift into reporting different
/// shapes for the same entity.
const WORKSPACE_SNAPSHOT_SQL: &str = r#"SELECT workspace_id,
          name,
          CAST(settings AS TEXT) AS settings,
          default_team_id,
          CAST(updated_at AS TEXT) AS updated_at
   FROM workspaces WHERE workspace_id = $1"#;

impl WorkspaceSnapshotRow {
    /// The `workspace_settings` entity as the sync protocol carries it.
    ///
    /// Returns the typed [`WorkspaceSettingsSnapshot`] rather than the
    /// `serde_json::json!` literal this used to build. The literal was the
    /// reason `workspace_settings` was the one bootstrap entity with no Rust
    /// type behind it, and therefore the one whose `entity_id` could only be
    /// recovered by looking up a string key — see
    /// [`trakkt_types::sync::SyncEntity`], which this type now implements.
    /// [`workspace_settings_snapshot_in_tx`] encodes it back to a
    /// `serde_json::Value` for the `sync_log` payload, so the shape on the wire
    /// is unchanged: the same five keys, with the same values.
    ///
    /// `Err` only for a `settings` column that is not parseable JSON, which is
    /// a corrupt row rather than an absent one.
    fn into_snapshot(self) -> trakkt_core::Result<WorkspaceSettingsSnapshot> {
        let settings: Option<serde_json::Value> = match self.settings.as_deref() {
            Some(settings) => Some(serde_json::from_str(settings)?),
            None => None,
        };

        Ok(WorkspaceSettingsSnapshot {
            workspace_id: self.workspace_id,
            name: self.name,
            settings,
            default_team_id: self.default_team_id,
            updated_at: self.updated_at,
        })
    }
}

/// Read the snapshot for a `sync_log` payload **on the transaction that just
/// wrote it**, so the payload is the post-update state.
///
/// Running this on the pool instead fails two different ways and neither is
/// obvious. On SQLite the pool is pinned to one connection ([`DbPool::connect`])
/// which the open transaction is holding, so the read waits forever on a
/// connection only the commit can free. On Postgres it does not block at all —
/// it reads the *pre-update* row from outside the transaction and persists that
/// as the payload, so the entry describing a rename carries the old name and
/// every client applies a no-op. The second failure is silent, which is why the
/// executor is the transaction and not the pool.
///
/// # Why a failure here fails the whole update
///
/// This used to degrade to a `None` payload and let the update stand, on the
/// reasoning that the update had already committed and so could not honestly be
/// reported as failed. That reasoning is gone: the update has *not* committed
/// when this runs, and returning `Err` rolls it back with the entry.
///
/// Degrading is now the dishonest option. A `None` payload is not a
/// lower-fidelity entry — `cache/apply.rs` returns on a data-less upsert before
/// it reaches the entity-type match, so clients drop the row outright. The
/// entry would burn a `sync_id` and advance every client's watermark past a
/// change it never delivered, leaving the stale name on screen until a full
/// bootstrap. That is exactly the invisible-mutation failure this conversion
/// exists to remove, and it would be reintroduced deliberately.
///
/// The remaining failure modes are all real faults, none of them "the workspace
/// is fine, we just could not decorate the entry": the query erroring on a
/// connection the commit needs anyway, or a `settings` column that will not
/// parse — which for `update_workspace_settings` means the value it serialized
/// itself moments ago does not round-trip. Failing loudly lets the caller retry
/// against a database that never diverged from its clients.
async fn workspace_settings_snapshot_in_tx(
    tx: &mut DbTx,
    workspace_id: &str,
) -> trakkt_core::Result<serde_json::Value> {
    let row = trakkt_core::tx_fetch_optional!(
        &mut *tx,
        WorkspaceSnapshotRow,
        WORKSPACE_SNAPSHOT_SQL,
        workspace_id
    )?;

    // Unreachable: callers only get here after their own UPDATE reported
    // `rows_affected() > 0` on this same transaction, so the row is there and
    // visible. Stated as an error rather than left to flow on, because the value
    // an absent row would produce is the `None` payload the doc comment above
    // rejects.
    let Some(row) = row else {
        return Err(trakkt_core::Error::Internal(format!(
            "workspace {workspace_id} disappeared between its own UPDATE and the \
             snapshot read on the same transaction"
        )));
    };

    // The `sync_log` payload column is JSON, so the typed snapshot is encoded
    // here rather than at each of the three callers. `to_value` on a struct of
    // strings and an already-parsed `Value` has no failing case, but it is not
    // an infallible signature, so the error propagates like every other.
    Ok(serde_json::to_value(row.into_snapshot()?)?)
}

/// Return the workspace's [`WorkspaceSettingsSnapshot`] for the sync bootstrap
/// protocol.
///
/// `Ok(None)` means the workspace has no row: a real answer, and one the
/// bootstrap streams as "this workspace has no settings entity". `Err` means
/// the snapshot is unknown — either the query failed or the stored `settings`
/// JSON could not be parsed.
///
/// Those two must stay distinguishable to the caller. Collapsing them, as this
/// function used to, hands a bootstrap "there are no settings" for a workspace
/// that has them, and the bootstrap then seals that with a full watermark the
/// client will never revisit.
pub async fn get_workspace_settings_for_sync(
    pool: &DbPool,
    workspace_id: &str,
) -> trakkt_core::Result<Option<WorkspaceSettingsSnapshot>> {
    let row = trakkt_core::db_fetch_optional!(
        pool,
        WorkspaceSnapshotRow,
        WORKSPACE_SNAPSHOT_SQL,
        workspace_id
    )?;

    row.map(WorkspaceSnapshotRow::into_snapshot).transpose()
}

/// Update workspace display name.
///
/// The UPDATE and its `sync_log` entry are one transaction: a rename that
/// commits without its sync row leaves the old name on every other client, and
/// no later delta reports it — the row a delta would re-read already carries the
/// new name, so nothing marks it as changed.
pub async fn update_workspace_name(
    pool: &DbPool,
    workspace_id: &str,
    name: &str,
    ws_manager: Option<&WebSocketManager>,
) -> trakkt_core::Result<bool> {
    let is_pg = pool.is_postgres();
    let now_expr = sql_compat::now(is_pg);
    let sql = format!(
        "UPDATE workspaces SET name = $1, updated_at = {now_expr} WHERE workspace_id = $2"
    );

    let mut tx = pool.begin().await?;
    let result = trakkt_core::tx_execute!(&mut tx, &sql, name, workspace_id)?;

    if result.rows_affected() == 0 {
        // `tx` is dropped here, which rolls it back (see `DbTx`).
        return Ok(false);
    }

    // Read back on the transaction, not the pool: the new name is not visible
    // outside this transaction yet (see `workspace_settings_snapshot_in_tx`).
    let snapshot = workspace_settings_snapshot_in_tx(&mut tx, workspace_id).await?;

    sync_log_service::commit_and_deliver(
        tx,
        entity_types::WORKSPACE_SETTINGS,
        workspace_id,
        workspace_id,
        sync_log_service::SyncAudience::Workspace,
        SyncActionType::Update,
        Some(snapshot),
        ws_manager,
    )
    .await?;

    Ok(true)
}

/// Update workspace settings JSON (full replace).
///
/// `workspaces.settings` is a Postgres `json` column. Binding `$1` as text
/// and letting Postgres coerce it does NOT work — Postgres refuses the
/// implicit text-to-json cast (`column "settings" is of type json but
/// expression is of type text`). We keep the bind as text (sqlx serializes
/// `String` to TEXT on both backends) and perform the cast in SQL on
/// Postgres. SQLite stores JSON in TEXT columns, so no cast is needed.
///
/// The UPDATE and its `sync_log` entry are one transaction, for the same reason
/// as [`update_workspace_name`]: settings that commit without their sync row are
/// invisible to every future delta.
pub async fn update_workspace_settings(
    pool: &DbPool,
    workspace_id: &str,
    settings: &serde_json::Value,
    ws_manager: Option<&WebSocketManager>,
) -> trakkt_core::Result<bool> {
    let is_pg = pool.is_postgres();
    let now = sql_compat::now(is_pg);
    let settings_str = serde_json::to_string(settings)
        .map_err(|e| trakkt_core::Error::Internal(format!("JSON serialization failed: {e}")))?;
    let sql = if is_pg {
        format!(
            "UPDATE workspaces SET settings = $1::json, updated_at = {now} WHERE workspace_id = $2"
        )
    } else {
        format!(
            "UPDATE workspaces SET settings = $1, updated_at = {now} WHERE workspace_id = $2"
        )
    };
    let mut tx = pool.begin().await?;
    let result = trakkt_core::tx_execute!(&mut tx, &sql, &settings_str, workspace_id)?;

    if result.rows_affected() == 0 {
        // `tx` is dropped here, which rolls it back (see `DbTx`).
        return Ok(false);
    }

    // Read back on the transaction, not the pool: the new settings are not
    // visible outside this transaction yet (see
    // `workspace_settings_snapshot_in_tx`).
    let snapshot = workspace_settings_snapshot_in_tx(&mut tx, workspace_id).await?;

    sync_log_service::commit_and_deliver(
        tx,
        entity_types::WORKSPACE_SETTINGS,
        workspace_id,
        workspace_id,
        sync_log_service::SyncAudience::Workspace,
        SyncActionType::Update,
        Some(snapshot),
        ws_manager,
    )
    .await?;

    Ok(true)
}

/// Set the workspace-level default team.
///
/// Validates that the team belongs to this workspace before writing.
///
/// The UPDATE and its `sync_log` entry are one transaction, for the same reason
/// as [`update_workspace_name`]: a default-team change that commits without its
/// sync row is invisible to every connected client until the next full
/// bootstrap, and no later delta reports it — the row a delta would re-read
/// already carries the new default, so nothing marks it as changed.
///
/// The validation deliberately runs on the pool, *before* `begin()`.
/// [`crate::team_service::get_team`] takes a `&DbPool`, and SQLite pins the pool
/// to one connection ([`DbPool::connect`]) which an open transaction holds — a
/// pool read inside the span would wait on a connection only the commit can
/// free, until sqlx's 30s acquire timeout fires. Authorization and validation
/// belong ahead of the transaction in any case.
pub async fn set_workspace_default_team(
    pool: &DbPool,
    workspace_id: &str,
    team_id: &str,
    ws_manager: Option<&WebSocketManager>,
) -> trakkt_core::Result<bool> {
    let team = crate::team_service::get_team(pool, team_id)
        .await?
        .ok_or_else(|| trakkt_core::Error::NotFound(format!("team {team_id} not found")))?;
    if team.workspace_id != workspace_id {
        return Err(trakkt_core::Error::BadRequest(
            "Team does not belong to this workspace".into(),
        ));
    }

    let is_pg = pool.is_postgres();
    let now = sql_compat::now(is_pg);
    let sql = format!(
        "UPDATE workspaces SET default_team_id = $1, updated_at = {now} WHERE workspace_id = $2"
    );

    let mut tx = pool.begin().await?;
    let result = trakkt_core::tx_execute!(&mut tx, &sql, team_id, workspace_id)?;

    if result.rows_affected() == 0 {
        // `tx` is dropped here, which rolls it back (see `DbTx`).
        return Ok(false);
    }

    // Read back on the transaction, not the pool: the new default team is not
    // visible outside this transaction yet (see
    // `workspace_settings_snapshot_in_tx`).
    let snapshot = workspace_settings_snapshot_in_tx(&mut tx, workspace_id).await?;

    sync_log_service::commit_and_deliver(
        tx,
        entity_types::WORKSPACE_SETTINGS,
        workspace_id,
        workspace_id,
        sync_log_service::SyncAudience::Workspace,
        SyncActionType::Update,
        Some(snapshot),
        ws_manager,
    )
    .await?;

    Ok(true)
}

/// Get the workspace-level default team ID, if set.
pub async fn get_workspace_default_team_id(
    pool: &DbPool,
    workspace_id: &str,
) -> trakkt_core::Result<Option<String>> {
    #[derive(sqlx::FromRow)]
    struct Row {
        default_team_id: Option<String>,
    }
    let row = trakkt_core::db_fetch_optional!(
        pool,
        Row,
        "SELECT default_team_id FROM workspaces WHERE workspace_id = $1",
        workspace_id
    )?;
    Ok(row.and_then(|r| r.default_team_id))
}

/// Get a workspace with all fields (SELECT *).
///
/// This is functionally identical to `user_service::get_workspace` but lives
/// in workspace_service for domain clarity. Both are thin wrappers over the
/// same query — no duplication of logic, just organizational convenience.
pub async fn get_workspace_full(
    pool: &DbPool,
    workspace_id: &str,
) -> trakkt_core::Result<Option<Workspace>> {
    let ws = trakkt_core::db_fetch_optional!(
        pool, Workspace,
        "SELECT * FROM workspaces WHERE workspace_id = $1",
        workspace_id
    )?;
    Ok(ws)
}


// ===========================================================================
// Phase 4D — Member management
// ===========================================================================

/// A workspace member with joined user data.
///
/// Used by the list_members endpoint to return user details alongside
/// membership info in a single query.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct MemberWithUser {
    // WorkspaceUser fields
    pub wu_id: i32,
    pub workspace_id: String,
    pub user_id: String,
    pub role: String,
    pub active: bool,
    pub wu_created_at: chrono::DateTime<chrono::Utc>,
    // User fields
    pub email: String,
    pub name: Option<String>,
}

/// Get all active workspace members with their user details.
///
/// Performs a single JOIN query rather than N+1 lookups.
pub async fn get_workspace_members_with_users(
    pool: &DbPool,
    workspace_id: &str,
) -> trakkt_core::Result<Vec<MemberWithUser>> {
    let is_pg = pool.is_postgres();
    let bt = sql_compat::bool_true(is_pg);
    let sql = format!(
        "SELECT wu.id AS wu_id, wu.workspace_id, wu.user_id, wu.role, wu.active, \
                wu.created_at AS wu_created_at, u.email, u.name \
         FROM workspace_users wu \
         JOIN users u ON u.user_id = wu.user_id \
         WHERE wu.workspace_id = $1 AND wu.active = {bt} \
         ORDER BY wu.created_at ASC"
    );
    let members = trakkt_core::db_fetch_all!(pool, MemberWithUser, &sql, workspace_id)?;
    Ok(members)
}

/// Update a member's role in a workspace.
pub async fn update_member_role(
    pool: &DbPool,
    workspace_id: &str,
    user_id: &str,
    new_role: &str,
) -> trakkt_core::Result<bool> {
    let is_pg = pool.is_postgres();
    let bt = sql_compat::bool_true(is_pg);
    let sql = format!(
        "UPDATE workspace_users SET role = $1 \
         WHERE workspace_id = $2 AND user_id = $3 AND active = {bt}"
    );
    let result = trakkt_core::db_execute!(pool, &sql, new_role, workspace_id, user_id)?;
    Ok(result.rows_affected() > 0)
}

/// Remove a member from a workspace (hard delete).
pub async fn remove_member(
    pool: &DbPool,
    workspace_id: &str,
    user_id: &str,
) -> trakkt_core::Result<bool> {
    let result = trakkt_core::db_execute!(
        pool,
        "DELETE FROM workspace_users \
         WHERE workspace_id = $1 AND user_id = $2",
        workspace_id, user_id
    )?;
    Ok(result.rows_affected() > 0)
}

/// Count admins in a workspace.
pub async fn count_admins(
    pool: &DbPool,
    workspace_id: &str,
) -> trakkt_core::Result<i64> {
    let is_pg = pool.is_postgres();
    let bt = sql_compat::bool_true(is_pg);
    let sql = format!(
        "SELECT COUNT(*) as count FROM workspace_users \
         WHERE workspace_id = $1 AND role = 'workspace_admin' AND active = {bt}"
    );
    let count: i64 = trakkt_core::db_fetch_scalar!(pool, i64, &sql, workspace_id)?;
    Ok(count)
}

/// Create a new workspace membership.
pub async fn create_workspace_user(
    pool: &DbPool,
    workspace_id: &str,
    user_id: &str,
    role: &str,
) -> trakkt_core::Result<()> {
    let is_pg = pool.is_postgres();
    let bt = sql_compat::bool_true(is_pg);
    let sql = format!(
        "INSERT INTO workspace_users (workspace_id, user_id, role, active) \
         VALUES ($1, $2, $3, {bt})"
    );
    trakkt_core::db_execute!(pool, &sql, workspace_id, user_id, role)?;
    Ok(())
}

// ===========================================================================
// Phase 4D — Invitation management
// ===========================================================================

/// Create a workspace invitation and return the inserted record.
pub async fn create_invitation(
    pool: &DbPool,
    invitation_id: &str,
    workspace_id: &str,
    email: &str,
    role: &str,
    invited_by: &str,
    expires_at: chrono::DateTime<chrono::Utc>,
) -> trakkt_core::Result<WorkspaceInvitation> {
    trakkt_core::db_execute!(
        pool,
        "INSERT INTO workspace_invitations \
         (invitation_id, workspace_id, email, role, invited_by_user_id, status, expires_at) \
         VALUES ($1, $2, $3, $4, $5, 'pending', $6)",
        invitation_id, workspace_id, email, role, invited_by, &expires_at
    )?;

    get_invitation(pool, invitation_id)
        .await?
        .ok_or_else(|| trakkt_core::Error::Internal("Invitation created but not found".into()))
}

/// Get an invitation by ID.
pub async fn get_invitation(
    pool: &DbPool,
    invitation_id: &str,
) -> trakkt_core::Result<Option<WorkspaceInvitation>> {
    let inv = trakkt_core::db_fetch_optional!(
        pool, WorkspaceInvitation,
        "SELECT * FROM workspace_invitations WHERE invitation_id = $1",
        invitation_id
    )?;
    Ok(inv)
}

/// Get an invitation by ID scoped to a workspace.
pub async fn get_invitation_in_workspace(
    pool: &DbPool,
    invitation_id: &str,
    workspace_id: &str,
) -> trakkt_core::Result<Option<WorkspaceInvitation>> {
    let inv = trakkt_core::db_fetch_optional!(
        pool, WorkspaceInvitation,
        "SELECT * FROM workspace_invitations \
         WHERE invitation_id = $1 AND workspace_id = $2",
        invitation_id, workspace_id
    )?;
    Ok(inv)
}

/// Get all pending invitations for a workspace.
pub async fn get_pending_invitations_for_workspace(
    pool: &DbPool,
    workspace_id: &str,
) -> trakkt_core::Result<Vec<WorkspaceInvitation>> {
    let invitations = trakkt_core::db_fetch_all!(
        pool, WorkspaceInvitation,
        "SELECT * FROM workspace_invitations \
         WHERE workspace_id = $1 AND status = 'pending' \
         ORDER BY created_at DESC",
        workspace_id
    )?;
    Ok(invitations)
}

/// Get all pending invitations addressed to a specific email.
pub async fn get_pending_invitations_for_email(
    pool: &DbPool,
    email: &str,
) -> trakkt_core::Result<Vec<WorkspaceInvitation>> {
    let invitations = trakkt_core::db_fetch_all!(
        pool, WorkspaceInvitation,
        "SELECT * FROM workspace_invitations \
         WHERE LOWER(email) = LOWER($1) AND status = 'pending' \
         ORDER BY created_at DESC",
        email
    )?;
    Ok(invitations)
}

/// Count pending invitations for a workspace.
pub async fn count_pending_invitations(
    pool: &DbPool,
    workspace_id: &str,
) -> trakkt_core::Result<i64> {
    let count: i64 = trakkt_core::db_fetch_scalar!(
        pool, i64,
        "SELECT COUNT(*) as count FROM workspace_invitations \
         WHERE workspace_id = $1 AND status = 'pending'",
        workspace_id
    )?;
    Ok(count)
}

/// Check whether a user with the given email is already a member of the workspace.
pub async fn check_existing_member_by_email(
    pool: &DbPool,
    workspace_id: &str,
    email: &str,
) -> trakkt_core::Result<bool> {
    let is_pg = pool.is_postgres();
    let bt = sql_compat::bool_true(is_pg);
    let sql = format!(
        "SELECT COUNT(*) as count FROM workspace_users wu \
         JOIN users u ON u.user_id = wu.user_id \
         WHERE wu.workspace_id = $1 AND LOWER(u.email) = LOWER($2) AND wu.active = {bt}"
    );
    let count: i64 = trakkt_core::db_fetch_scalar!(pool, i64, &sql, workspace_id, email)?;
    Ok(count > 0)
}

/// Check whether a pending invitation already exists for the given email in the workspace.
pub async fn check_pending_invitation(
    pool: &DbPool,
    workspace_id: &str,
    email: &str,
) -> trakkt_core::Result<bool> {
    let count: i64 = trakkt_core::db_fetch_scalar!(
        pool, i64,
        "SELECT COUNT(*) as count FROM workspace_invitations \
         WHERE workspace_id = $1 AND LOWER(email) = LOWER($2) AND status = 'pending'",
        workspace_id, email
    )?;
    Ok(count > 0)
}

/// Update an invitation's status.
pub async fn update_invitation_status(
    pool: &DbPool,
    invitation_id: &str,
    status: &str,
) -> trakkt_core::Result<bool> {
    let result = trakkt_core::db_execute!(
        pool,
        "UPDATE workspace_invitations SET status = $1 WHERE invitation_id = $2",
        status, invitation_id
    )?;
    Ok(result.rows_affected() > 0)
}

/// Accept an invitation: set status to 'accepted', record who accepted and when.
pub async fn accept_invitation(
    pool: &DbPool,
    invitation_id: &str,
    user_id: &str,
) -> trakkt_core::Result<bool> {
    let is_pg = pool.is_postgres();
    let now = sql_compat::now(is_pg);
    let sql = format!(
        "UPDATE workspace_invitations SET \
         status = 'accepted', accepted_at = {now}, accepted_by_user_id = $1 \
         WHERE invitation_id = $2"
    );
    let result = trakkt_core::db_execute!(pool, &sql, user_id, invitation_id)?;
    Ok(result.rows_affected() > 0)
}

/// Accept an invitation for a newly-created user (self-hosted SMTP-less flow).
///
/// Adds the user to the workspace and marks the invitation as accepted.
/// Used during one-step signup when the user has a pending invitation.
pub async fn accept_invitation_for_user(
    pool: &DbPool,
    invitation_id: &str,
    user_id: &str,
) -> trakkt_core::Result<()> {
    let invitation = get_invitation(pool, invitation_id)
        .await?
        .ok_or_else(|| trakkt_core::Error::NotFound("Invitation not found".into()))?;

    let db_role = invitation.role.as_ref();
    create_workspace_user(pool, &invitation.workspace_id, user_id, db_role).await?;
    accept_invitation(pool, invitation_id, user_id).await?;

    // Add the user to the workspace's default team.
    if let Ok(default_team) = crate::team_service::get_default_team(pool, &invitation.workspace_id).await
        && let Err(e) = crate::team_service::add_team_member(
            pool,
            &default_team.team_id,
            user_id,
            "member",
            &invitation.workspace_id,
        ).await
    {
        tracing::warn!(error = %e, "Failed to add invited user to default team");
    }

    Ok(())
}

// ===========================================================================
// Phase 4D — Ownership transfer
// ===========================================================================

/// Create an ownership transfer request and return the inserted record.
pub async fn create_ownership_transfer(
    pool: &DbPool,
    transfer_id: &str,
    workspace_id: &str,
    from_user_id: &str,
    to_user_id: &str,
    expires_at: chrono::DateTime<chrono::Utc>,
) -> trakkt_core::Result<OwnershipTransfer> {
    trakkt_core::db_execute!(
        pool,
        "INSERT INTO ownership_transfers \
         (transfer_id, workspace_id, from_user_id, to_user_id, status, expires_at) \
         VALUES ($1, $2, $3, $4, 'pending', $5)",
        transfer_id, workspace_id, from_user_id, to_user_id, &expires_at
    )?;

    get_ownership_transfer(pool, transfer_id)
        .await?
        .ok_or_else(|| trakkt_core::Error::Internal("Transfer created but not found".into()))
}

/// Get an ownership transfer by ID.
pub async fn get_ownership_transfer(
    pool: &DbPool,
    transfer_id: &str,
) -> trakkt_core::Result<Option<OwnershipTransfer>> {
    let transfer = trakkt_core::db_fetch_optional!(
        pool, OwnershipTransfer,
        "SELECT * FROM ownership_transfers WHERE transfer_id = $1",
        transfer_id
    )?;
    Ok(transfer)
}

/// Get a pending transfer for a specific workspace.
pub async fn get_pending_transfer_for_workspace(
    pool: &DbPool,
    workspace_id: &str,
) -> trakkt_core::Result<Option<OwnershipTransfer>> {
    let transfer = trakkt_core::db_fetch_optional!(
        pool, OwnershipTransfer,
        "SELECT * FROM ownership_transfers \
         WHERE workspace_id = $1 AND status = 'pending' \
         ORDER BY created_at DESC LIMIT 1",
        workspace_id
    )?;
    Ok(transfer)
}

/// Get all pending transfers where the given user is the recipient.
pub async fn get_pending_transfers_for_user(
    pool: &DbPool,
    user_id: &str,
) -> trakkt_core::Result<Vec<OwnershipTransfer>> {
    let transfers = trakkt_core::db_fetch_all!(
        pool, OwnershipTransfer,
        "SELECT * FROM ownership_transfers \
         WHERE to_user_id = $1 AND status = 'pending' \
         ORDER BY created_at DESC",
        user_id
    )?;
    Ok(transfers)
}

/// Update a transfer's status and set completed_at = NOW().
pub async fn update_transfer_status(
    pool: &DbPool,
    transfer_id: &str,
    status: &str,
) -> trakkt_core::Result<bool> {
    let is_pg = pool.is_postgres();
    let now = sql_compat::now(is_pg);
    let sql = format!(
        "UPDATE ownership_transfers SET status = $1, completed_at = {now} \
         WHERE transfer_id = $2"
    );
    let result = trakkt_core::db_execute!(pool, &sql, status, transfer_id)?;
    Ok(result.rows_affected() > 0)
}

/// Complete an ownership transfer in a transaction:
/// 1. Update workspace owner_user_id
/// 2. Ensure new owner has workspace_admin role
/// 3. Mark transfer as accepted with completed_at
///
/// All three or none. A workspace whose owner moved while the transfer stayed
/// `pending` can be accepted a second time; a new owner without the
/// `workspace_admin` role owns a workspace they cannot administer.
///
/// Runs on [`DbTx`] rather than a hand-written `match` over both pool variants.
/// The two arms it replaced held the same three statements written out twice,
/// so every edit had to be made in both places to stay correct, and only one of
/// them was ever exercised by a given test run.
pub async fn complete_ownership_transfer(
    pool: &DbPool,
    transfer_id: &str,
    workspace_id: &str,
    new_owner_id: &str,
) -> trakkt_core::Result<bool> {
    let is_pg = pool.is_postgres();
    let now = sql_compat::now(is_pg);
    let bt = sql_compat::bool_true(is_pg);

    let update_owner_sql = format!(
        "UPDATE workspaces SET owner_user_id = $1, updated_at = {now} \
         WHERE workspace_id = $2"
    );
    let update_role_sql = format!(
        "UPDATE workspace_users SET role = 'workspace_admin' \
         WHERE workspace_id = $1 AND user_id = $2 AND active = {bt}"
    );
    let update_transfer_sql = format!(
        "UPDATE ownership_transfers SET status = 'accepted', completed_at = {now} \
         WHERE transfer_id = $1"
    );

    let mut tx = pool.begin().await?;
    trakkt_core::tx_execute!(&mut tx, &update_owner_sql, new_owner_id, workspace_id)?;
    trakkt_core::tx_execute!(&mut tx, &update_role_sql, workspace_id, new_owner_id)?;
    trakkt_core::tx_execute!(&mut tx, &update_transfer_sql, transfer_id)?;
    tx.commit().await?;

    Ok(true)
}

/// Update workspace owner_user_id directly (used by ownership transfer).
pub async fn update_workspace_owner(
    pool: &DbPool,
    workspace_id: &str,
    new_owner_id: &str,
) -> trakkt_core::Result<bool> {
    let is_pg = pool.is_postgres();
    let now = sql_compat::now(is_pg);
    let sql = format!(
        "UPDATE workspaces SET owner_user_id = $1, updated_at = {now} \
         WHERE workspace_id = $2"
    );
    let result = trakkt_core::db_execute!(pool, &sql, new_owner_id, workspace_id)?;
    Ok(result.rows_affected() > 0)
}

// ─── Orchestration ─────────────────────────────────────────────────────────

/// Enriched ownership transfer for display on the accept-ownership page.
///
/// All fields are pre-formatted strings so this type can be used directly
/// as a server-function return type (no chrono / enum deps on the client).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OwnershipTransferDetail {
    pub transfer_id: String,
    pub workspace_name: String,
    pub from_user_email: String,
    pub expires_at: String,
    pub status: String,
}

/// Fetch an ownership transfer for a specific recipient, auto-expiring if past
/// its deadline.
///
/// Returns `None` if the transfer doesn't exist, isn't pending, is expired, or
/// `recipient_id` doesn't match `to_user_id`.
pub async fn get_transfer_for_recipient(
    pool: &DbPool,
    transfer_id: &str,
    recipient_id: &str,
) -> trakkt_core::Result<Option<OwnershipTransferDetail>> {
    let Some(transfer) = get_ownership_transfer(pool, transfer_id).await? else {
        return Ok(None);
    };

    if transfer.to_user_id != recipient_id {
        return Ok(None);
    }

    if transfer.status != TransferStatus::Pending {
        return Ok(None);
    }

    if transfer.expires_at < Utc::now() {
        let _ = update_transfer_status(pool, transfer_id, "expired").await;
        return Ok(None);
    }

    let workspace = get_workspace_full(pool, &transfer.workspace_id).await?;
    let workspace_name = workspace
        .and_then(|w| w.name)
        .unwrap_or_else(|| "Unnamed Workspace".to_string());

    let from_user =
        crate::user_service::get_user_by_id(pool, &transfer.from_user_id).await?;
    let from_user_email = from_user.map(|u| u.email).unwrap_or_default();

    Ok(Some(OwnershipTransferDetail {
        transfer_id: transfer.transfer_id,
        workspace_name,
        from_user_email,
        expires_at: transfer.expires_at.to_rfc3339(),
        status: transfer.status.to_string(),
    }))
}

/// Remove a member from a workspace, enforcing all business rules.
///
/// Returns an `Err` with a user-facing message on any rule violation.
pub async fn remove_workspace_member(
    pool: &DbPool,
    workspace_id: &str,
    owner_user_id: &str,
    requesting_user_id: &str,
    target_user_id: &str,
) -> trakkt_core::Result<()> {
    if target_user_id == owner_user_id {
        return Err(trakkt_core::Error::BadRequest(
            "Cannot remove workspace owner. Transfer ownership first.".into(),
        ));
    }

    if target_user_id == requesting_user_id {
        let admin_count = count_admins(pool, workspace_id).await?;
        if admin_count < 2 {
            return Err(trakkt_core::Error::BadRequest(
                "Cannot remove yourself: you are the only admin".into(),
            ));
        }
    }

    let target = crate::user_service::get_workspace_user(pool, workspace_id, target_user_id).await?;
    if target.is_none() {
        return Err(trakkt_core::Error::NotFound(
            "Member not found in workspace".into(),
        ));
    }

    remove_member(pool, workspace_id, target_user_id).await?;
    Ok(())
}

/// Parameters for `invite_workspace_member`.
pub struct InviteWorkspaceMemberParams<'a> {
    pub pool: &'a DbPool,
    pub workspace_id: &'a str,
    pub email: &'a str,
    pub db_role: &'a str,
    pub invited_by: &'a str,
    pub invitation_id: &'a str,
    pub expires_at: chrono::DateTime<chrono::Utc>,
    pub user_limit: Option<i64>,
}

/// Create a workspace invitation, enforcing duplicate and user-limit checks.
///
/// `email` is expected to be already trimmed and lowercased by the caller.
pub async fn invite_workspace_member(
    params: InviteWorkspaceMemberParams<'_>,
) -> trakkt_core::Result<()> {
    let InviteWorkspaceMemberParams {
        pool, workspace_id, email, db_role, invited_by, invitation_id, expires_at, user_limit,
    } = params;
    let is_member = check_existing_member_by_email(pool, workspace_id, email).await?;
    if is_member {
        return Err(trakkt_core::Error::BadRequest(
            "User is already a member of this workspace".into(),
        ));
    }

    let has_pending = check_pending_invitation(pool, workspace_id, email).await?;
    if has_pending {
        return Err(trakkt_core::Error::BadRequest(
            "Invitation already pending for this email".into(),
        ));
    }

    if let Some(limit) = user_limit {
        let current_users = count_workspace_users(pool, workspace_id).await?;
        let pending = count_pending_invitations(pool, workspace_id).await?;
        if current_users + pending >= limit {
            return Err(trakkt_core::Error::BadRequest(
                "Workspace user limit reached. Upgrade your plan to add more users."
                    .into(),
            ));
        }
    }

    create_invitation(
        pool,
        invitation_id,
        workspace_id,
        email,
        db_role,
        invited_by,
        expires_at,
    )
    .await?;
    Ok(())
}

/// A resolved ownership transfer with sender and recipient emails.
#[derive(Debug, Clone)]
pub struct ResolvedTransfer {
    pub transfer_id: String,
    pub from_user_id: String,
    pub from_user_email: String,
    pub to_user_id: String,
    pub to_user_email: String,
    pub status: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub expires_at: chrono::DateTime<chrono::Utc>,
    pub is_initiator: bool,
    pub is_recipient: bool,
}

/// List all pending ownership transfers relevant to a user.
///
/// Combines transfers where the user is the recipient with any transfer they
/// initiated as workspace owner, deduplicates, and resolves email addresses.
pub async fn list_ownership_transfers_for_user(
    pool: &DbPool,
    workspace_id: &str,
    user_id: &str,
) -> trakkt_core::Result<Vec<ResolvedTransfer>> {
    let mut transfers = get_pending_transfers_for_user(pool, user_id).await?;

    if let Some(initiated) =
        get_pending_transfer_for_workspace(pool, workspace_id).await?
        && !transfers
            .iter()
            .any(|t| t.transfer_id == initiated.transfer_id)
        {
            transfers.push(initiated);
        }

    let mut result = Vec::with_capacity(transfers.len());
    for transfer in &transfers {
        let from_email =
            crate::user_service::get_user_by_id(pool, &transfer.from_user_id)
                .await?
                .map(|u| u.email)
                .unwrap_or_default();

        let to_email =
            crate::user_service::get_user_by_id(pool, &transfer.to_user_id)
                .await?
                .map(|u| u.email)
                .unwrap_or_default();

        result.push(ResolvedTransfer {
            transfer_id: transfer.transfer_id.clone(),
            from_user_id: transfer.from_user_id.clone(),
            from_user_email: from_email,
            to_user_id: transfer.to_user_id.clone(),
            to_user_email: to_email,
            status: transfer.status.to_string(),
            created_at: transfer.created_at,
            expires_at: transfer.expires_at,
            is_initiator: transfer.from_user_id == user_id,
            is_recipient: transfer.to_user_id == user_id,
        });
    }
    Ok(result)
}
