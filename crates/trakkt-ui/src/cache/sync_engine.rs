// SPDX-License-Identifier: AGPL-3.0-or-later

//! Client-side sync engine for the offline-first sync protocol.
//!
//! The sync engine manages three sync phases over the shared WebSocket:
//!
//! 1. **Bootstrap** (`sync_bootstrap`): sent on first connect when no local
//!    cursor exists — the server sends the full workspace dataset as a stream
//!    of `sync_action` messages followed by `sync_complete`.
//!
//! 2. **Delta** (`sync_delta`): sent on reconnect when a cursor exists — the
//!    server sends only actions that occurred after `last_sync_id`.
//!
//! 3. **Reset** (`sync_reset`): the server signals that local state is
//!    irrecoverably stale (e.g. cursor too old). The engine nukes IndexedDB,
//!    resets the reactive store, and re-bootstraps.
//!
//! ## Reconnect handling
//!
//! The engine watches `WebSocketClient::connection_state` and re-sends the
//! appropriate request on every transition to `Connected`. The on_message
//! callback survives reconnects, so there is no need to re-register it.
//!
//! ## Thread safety / `!Send` types
//!
//! This module is `wasm32`-only. All async tasks use `spawn_local` (single-
//! threaded WASM event loop).

use leptos::prelude::*;
use leptos::task::spawn_local;

use trakkt_types::models::{Comment, Favorite, IssueWithDetails, Label, Notification, Project, Status, Team, View};
use trakkt_types::sync::{SyncAction, SyncActionType, SyncResponse, entity_types};

use crate::cache::db;
use crate::cache::store::SyncStore;
use crate::cache::websocket::{ConnectionState, WebSocketClient};

const ALL_CACHED_ENTITY_TYPES: &[&str] = &[
    entity_types::ISSUE,
    entity_types::LABEL,
    entity_types::STATUS,
    entity_types::TEAM,
    entity_types::PROJECT,
    entity_types::PROJECT_MILESTONE,
    entity_types::VIEW,
    entity_types::FAVORITE,
    entity_types::NOTIFICATION,
    entity_types::COMMENT,
    entity_types::WORKSPACE_SETTINGS,
];

// ── Public entry point ──────────────────────────────────────────────────────

/// Start the sync engine. Call **once** from the Layout after connecting the
/// WebSocket.
///
/// Registers the message callback on the `WebSocketClient` to process
/// `sync_action`, `sync_complete`, and `sync_reset` messages. Watches the
/// connection state signal to send bootstrap or delta requests on every
/// connect/reconnect.
pub fn start_sync_engine(
    ws: &WebSocketClient,
    store: &SyncStore,
    workspace_id: &str,
) {
    // ── Register message handler ────────────────────────────────────────────
    let store_msg = *store;
    let wid_msg = workspace_id.to_owned();
    let ws_for_msg = ws.clone();
    ws.set_on_message(move |msg: SyncResponse| {
        match msg {
            SyncResponse::SyncAction(action) => {
                apply_sync_action(&store_msg, &wid_msg, &action);
            }
            SyncResponse::SyncComplete { last_sync_id } => {
                let wid = wid_msg.clone();
                let store_complete = store_msg;
                spawn_local(async move {
                    match db::init_cache_db(&wid).await {
                        Ok(cache_db) => {
                            if let Err(e) =
                                db::set_last_sync_id(&cache_db, &wid, &last_sync_id.to_string())
                                    .await
                            {
                                tracing::warn!("sync_complete: failed to persist cursor: {e}");
                            }
                            if let Err(e) = db::set_meta(&cache_db, "schemaHash", db::SCHEMA_HASH).await {
                                tracing::warn!("sync_complete: failed to persist schema hash: {e}");
                            }
                        }
                        Err(e) => {
                            tracing::warn!("sync_complete: failed to open cache db: {e}");
                        }
                    }
                    store_complete.set_initialized(true);
                    tracing::info!(last_sync_id, "sync_complete: store initialized");
                });
            }
            SyncResponse::SyncReset => {
                tracing::info!("sync_reset: nuking local cache and re-bootstrapping");
                store_msg.reset();
                let wid = wid_msg.clone();
                let ws_for_reset = ws_for_msg.clone();
                spawn_local(async move {
                    match db::init_cache_db(&wid).await {
                        Ok(cache_db) => {
                            for et in ALL_CACHED_ENTITY_TYPES {
                                if let Err(e) =
                                    db::delete_all_of_type(&cache_db, et, &wid).await
                                {
                                    tracing::warn!(
                                        entity_type = et,
                                        "sync_reset: delete_all_of_type failed: {e}"
                                    );
                                }
                            }
                            if let Err(e) =
                                db::set_last_sync_id(&cache_db, &wid, "0").await
                            {
                                tracing::warn!("sync_reset: failed to reset cursor: {e}");
                            }
                        }
                        Err(e) => {
                            tracing::warn!("sync_reset: failed to open cache db: {e}");
                        }
                    }
                    if !ws_for_reset.send(serde_json::json!({"type": "sync_bootstrap"})) {
                        tracing::warn!("sync_reset: failed to send bootstrap request");
                    }
                });
            }
        }
    });

    // ── Watch connection state to send bootstrap or delta on connect ────────
    let ws_for_state = ws.clone();
    let wid_state = workspace_id.to_owned();
    let store_state = *store;

    Effect::new(move |_| {
        let state = ws_for_state.connection_state.get();
        if state != ConnectionState::Connected {
            return;
        }

        let wid = wid_state.clone();
        let ws_send = ws_for_state.clone();
        spawn_local(async move {
            let cache_db = match db::init_cache_db(&wid).await {
                Ok(db) => Some(db),
                Err(e) => {
                    tracing::warn!("sync: failed to open cache db: {e}");
                    None
                }
            };

            let idb_cursor = match cache_db {
                Some(ref db) => match db::get_last_sync_id(db, &wid).await {
                    Ok(Some(s)) => match s.parse::<i64>() {
                        Ok(v) => v,
                        Err(e) => {
                            tracing::warn!("sync: failed to parse cursor {s:?}: {e}");
                            0
                        }
                    },
                    Ok(None) => 0,
                    Err(e) => {
                        tracing::warn!("sync: failed to read cursor from IDB: {e}");
                        0
                    }
                },
                None => 0,
            };

            if idb_cursor > 0 {
                tracing::info!(idb_cursor, "sync: cursor found — sending sync_delta");
                if !ws_send.send(serde_json::json!({
                    "type": "sync_delta",
                    "last_sync_id": idb_cursor
                })) {
                    tracing::warn!("sync: failed to send sync_delta");
                }
            } else {
                store_state.reset();

                if let Some(ref cache_db) = cache_db {
                    for et in ALL_CACHED_ENTITY_TYPES {
                        if let Err(e) =
                            db::delete_all_of_type(cache_db, et, &wid).await
                        {
                            tracing::warn!(
                                entity_type = et,
                                "bootstrap: failed to clear entity cache: {e}"
                            );
                        }
                    }
                }

                tracing::info!("sync: no cursor — sending sync_bootstrap");
                if !ws_send.send(serde_json::json!({"type": "sync_bootstrap"})) {
                    tracing::warn!("sync: failed to send sync_bootstrap");
                }
            }
        });
    });
}

// ── Hydration ───────────────────────────────────────────────────────────────

/// Read all entity types from IndexedDB and populate the store.
///
/// Called once at startup before the WebSocket connects, so the UI can
/// render cached data immediately while the sync engine catches up.
pub async fn hydrate_store_from_db(
    cache_db: &db::CacheDb,
    workspace_id: &str,
    store: &SyncStore,
) {
    fn deser<T: serde::de::DeserializeOwned>(
        entries: &[(String, String, String)],
        entity_type: &str,
    ) -> Vec<T> {
        let mut items = Vec::with_capacity(entries.len());
        for (id, json, _ts) in entries {
            match serde_json::from_str(json) {
                Ok(item) => items.push(item),
                Err(e) => tracing::warn!(
                    entity_type,
                    entity_id = %id,
                    "hydration deser failed: {e}"
                ),
            }
        }
        items
    }

    if let Ok(entries) = db::read_all(cache_db, entity_types::ISSUE, workspace_id).await {
        store.set_issues(deser::<IssueWithDetails>(&entries, entity_types::ISSUE));
    }
    if let Ok(entries) = db::read_all(cache_db, entity_types::LABEL, workspace_id).await {
        store.set_labels(deser::<Label>(&entries, entity_types::LABEL));
    }
    if let Ok(entries) = db::read_all(cache_db, entity_types::STATUS, workspace_id).await {
        store.set_statuses(deser::<Status>(&entries, entity_types::STATUS));
    }
    if let Ok(entries) = db::read_all(cache_db, entity_types::TEAM, workspace_id).await {
        store.set_teams(deser::<Team>(&entries, entity_types::TEAM));
    }
    if let Ok(entries) = db::read_all(cache_db, entity_types::PROJECT, workspace_id).await {
        store.set_projects(deser::<Project>(&entries, entity_types::PROJECT));
    }
    if let Ok(entries) = db::read_all(cache_db, entity_types::VIEW, workspace_id).await {
        store.set_views(deser::<View>(&entries, entity_types::VIEW));
    }
    if let Ok(entries) = db::read_all(cache_db, entity_types::FAVORITE, workspace_id).await {
        store.set_favorites(deser::<Favorite>(&entries, entity_types::FAVORITE));
    }
    if let Ok(entries) = db::read_all(cache_db, entity_types::NOTIFICATION, workspace_id).await {
        store.set_notifications(deser::<Notification>(&entries, entity_types::NOTIFICATION));
    }
    if let Ok(entries) = db::read_all(cache_db, entity_types::COMMENT, workspace_id).await {
        store.set_comments(deser::<Comment>(&entries, entity_types::COMMENT));
    }

    store.set_initialized(true);
    if let Ok(Some(cursor)) = db::get_last_sync_id(cache_db, workspace_id).await {
        tracing::debug!(cursor, "hydrated from IDB with cursor — will delta-sync");
    }
}

// ── Internal helpers ────────────────────────────────────────────────────────

/// Apply a single `SyncAction` to the reactive store and IndexedDB.
fn apply_sync_action(store: &SyncStore, workspace_id: &str, action: &SyncAction) {
    let entity_type = action.entity_type.as_str();
    let entity_id = &action.entity_id;

    match action.action {
        SyncActionType::Insert | SyncActionType::Update => {
            let Some(ref entity_data) = action.data else {
                tracing::warn!(
                    action = ?action.action,
                    entity_type,
                    entity_id,
                    "sync_action insert/update: missing data field — skipping"
                );
                return;
            };

            // Persist to IndexedDB (best-effort async write).
            let json_str = match serde_json::to_string(entity_data) {
                Ok(s) => s,
                Err(e) => {
                    tracing::warn!(
                        entity_type,
                        entity_id,
                        "sync_action: failed to re-serialize entity data: {e}"
                    );
                    return;
                }
            };
            let et = entity_type.to_owned();
            let eid = entity_id.clone();
            let wid = workspace_id.to_owned();
            let ts = action.timestamp.clone();
            spawn_local(async move {
                match db::init_cache_db(&wid).await {
                    Ok(cache_db) => {
                        if let Err(e) =
                            db::upsert(&cache_db, &et, &eid, &wid, &json_str, &ts).await
                        {
                            tracing::warn!(
                                entity_type = %et,
                                entity_id = %eid,
                                "sync_action upsert to IDB failed: {e}"
                            );
                        }
                    }
                    Err(e) => {
                        tracing::warn!("sync_action: failed to open cache db: {e}");
                    }
                }
            });

            // Update the reactive store.
            match entity_type {
                et if et == entity_types::ISSUE => {
                    match serde_json::from_value::<IssueWithDetails>(entity_data.clone()) {
                        Ok(item) => store.upsert_issue(item),
                        Err(e) => tracing::warn!(
                            entity_type,
                            entity_id,
                            "sync_action: failed to deserialize issue: {e}"
                        ),
                    }
                }
                et if et == entity_types::LABEL => {
                    match serde_json::from_value::<Label>(entity_data.clone()) {
                        Ok(item) => store.upsert_label(item),
                        Err(e) => tracing::warn!(
                            entity_type,
                            entity_id,
                            "sync_action: failed to deserialize label: {e}"
                        ),
                    }
                }
                et if et == entity_types::STATUS => {
                    match serde_json::from_value::<Status>(entity_data.clone()) {
                        Ok(item) => store.upsert_status(item),
                        Err(e) => tracing::warn!(
                            entity_type,
                            entity_id,
                            "sync_action: failed to deserialize status: {e}"
                        ),
                    }
                }
                et if et == entity_types::TEAM => {
                    match serde_json::from_value::<Team>(entity_data.clone()) {
                        Ok(item) => store.upsert_team(item),
                        Err(e) => tracing::warn!(
                            entity_type,
                            entity_id,
                            "sync_action: failed to deserialize team: {e}"
                        ),
                    }
                }
                et if et == entity_types::PROJECT => {
                    match serde_json::from_value::<Project>(entity_data.clone()) {
                        Ok(item) => store.upsert_project(item),
                        Err(e) => tracing::warn!(
                            entity_type,
                            entity_id,
                            "sync_action: failed to deserialize project: {e}"
                        ),
                    }
                }
                et if et == entity_types::VIEW => {
                    match serde_json::from_value::<View>(entity_data.clone()) {
                        Ok(item) => store.upsert_view(item),
                        Err(e) => tracing::warn!(
                            entity_type,
                            entity_id,
                            "sync_action: failed to deserialize view: {e}"
                        ),
                    }
                }
                et if et == entity_types::FAVORITE => {
                    match serde_json::from_value::<Favorite>(entity_data.clone()) {
                        Ok(item) => store.upsert_favorite(item),
                        Err(e) => tracing::warn!(
                            entity_type,
                            entity_id,
                            "sync_action: failed to deserialize favorite: {e}"
                        ),
                    }
                }
                et if et == entity_types::NOTIFICATION => {
                    match serde_json::from_value::<Notification>(entity_data.clone()) {
                        Ok(item) => store.upsert_notification(item),
                        Err(e) => tracing::warn!(
                            entity_type,
                            entity_id,
                            "sync_action: failed to deserialize notification: {e}"
                        ),
                    }
                }
                et if et == entity_types::COMMENT => {
                    match serde_json::from_value::<Comment>(entity_data.clone()) {
                        Ok(item) => store.upsert_comment(item),
                        Err(e) => tracing::warn!(
                            entity_type,
                            entity_id,
                            "sync_action: failed to deserialize comment: {e}"
                        ),
                    }
                }
                et if et == entity_types::ACTIVITY => {
                    // Activities are not stored in the SyncStore — they are
                    // fetched on-demand by the timeline component. Bump the
                    // version counter so reactive dependencies refetch.
                    store.bump_activities_version();
                }
                et if et == entity_types::ISSUE_RELATION => {
                    // Relations are fetched on-demand by the relations section.
                    // Bump the version counter so reactive dependencies refetch.
                    store.bump_relations_version();
                }
                other => {
                    tracing::debug!(
                        entity_type = other,
                        "sync_action: unhandled entity type — ignoring"
                    );
                }
            }
        }
        SyncActionType::Delete => {
            // Remove from the reactive store (also handles IDB deletion).
            match entity_type {
                et if et == entity_types::ISSUE => store.remove_issue(entity_id),
                et if et == entity_types::LABEL => store.remove_label(entity_id),
                et if et == entity_types::STATUS => store.remove_status(entity_id),
                et if et == entity_types::TEAM => store.remove_team(entity_id),
                et if et == entity_types::PROJECT => store.remove_project(entity_id),
                et if et == entity_types::VIEW => store.remove_view(entity_id),
                et if et == entity_types::FAVORITE => store.remove_favorite(entity_id),
                et if et == entity_types::NOTIFICATION => store.remove_notification(entity_id),
                et if et == entity_types::COMMENT => store.remove_comment(entity_id),
                et if et == entity_types::ACTIVITY => store.bump_activities_version(),
                et if et == entity_types::ISSUE_RELATION => store.bump_relations_version(),
                other => {
                    tracing::debug!(
                        entity_type = other,
                        "sync_action delete: unhandled entity type — ignoring"
                    );
                }
            }
        }
    }
}
