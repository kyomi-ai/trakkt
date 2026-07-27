// SPDX-License-Identifier: AGPL-3.0-or-later

//! Application of a single `SyncAction`, split into its two independent halves.
//!
//! Only one tab per browser holds the sync leadership lock, and only that tab
//! runs a WebSocket, writes IndexedDB and advances the cursor. Follower tabs
//! learn about the same actions over a `BroadcastChannel` and update **only
//! their own in-memory store** — they must never write the shared cache, which
//! is the corruption this split exists to prevent.
//!
//! So action application is two halves:
//!
//! * [`apply_action_to_memory`] — the reactive store update. Runs in **every**
//!   tab: the leader as it processes the WebSocket stream, followers as the
//!   broadcast arrives. This is what keeps a follower's UI live, including the
//!   activity/relation/comment version counters that drive the on-demand
//!   refetches.
//! * [`enqueue_cache_writes`] — the IndexedDB persistence. Runs in the **leader
//!   only**, and never directly: every op is queued on the single FIFO
//!   [`IdbWriter`](crate::cache::idb_writer::IdbWriter) so the cursor can never
//!   commit ahead of the entity writes it claims.
//!
//! Both halves are pure Rust over the store and the writer queue, so this
//! module is not target-gated and its behaviour — including which entity types
//! bump which version counter — is unit tested natively.

use trakkt_types::models::{Favorite, IssueWithDetails, Label, Notification, Project, Status, Team, View};
use trakkt_types::sync::{SyncAction, SyncActionType, entity_types};

use crate::cache::idb_writer::{IdbOp, IdbWriter};
use crate::cache::store::SyncStore;
use crate::cache::tab_leader::SyncBroadcastMessage;

// ── Memory half (every tab) ─────────────────────────────────────────────────

/// Apply one `SyncAction` to the reactive store.
///
/// Touches memory only: no IndexedDB, no cursor. The leader pairs this with
/// [`enqueue_cache_writes`]; a follower runs it alone.
pub fn apply_action_to_memory(store: &SyncStore, action: &SyncAction) {
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

            // Update the reactive store.
            match entity_type {
                et if et == entity_types::ISSUE => {
                    match serde_json::from_value::<IssueWithDetails>(entity_data.clone()) {
                        Ok(mut item) => {
                            item.description = None;
                            store.upsert_issue(item);
                        }
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
                    // Comments are not stored in the reactive signal — loaded
                    // on-demand by the detail page from IndexedDB. Bump the
                    // version counter so reactive dependencies re-read from IDB.
                    store.bump_comments_version();
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
            // Remove from the reactive store in memory only. Entity types with
            // no cached rows (activity, issue_relation) only bump their version
            // counter, exactly as before.
            match entity_type {
                et if et == entity_types::ISSUE => store.remove_issue_in_memory(entity_id),
                et if et == entity_types::LABEL => store.remove_label_in_memory(entity_id),
                et if et == entity_types::STATUS => store.remove_status_in_memory(entity_id),
                et if et == entity_types::TEAM => store.remove_team_in_memory(entity_id),
                et if et == entity_types::PROJECT => store.remove_project_in_memory(entity_id),
                et if et == entity_types::VIEW => store.remove_view_in_memory(entity_id),
                et if et == entity_types::FAVORITE => store.remove_favorite_in_memory(entity_id),
                et if et == entity_types::NOTIFICATION => {
                    store.remove_notification_in_memory(entity_id);
                }
                et if et == entity_types::COMMENT => {
                    // Comments live only in IndexedDB — the detail page reads
                    // them on demand and re-reads when the version bumps.
                    store.bump_comments_version();
                }
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

/// Handle one message received from another tab of this browser.
///
/// Two halves again, split by role. Every tab replays the leader's published
/// actions into its own memory. The leader additionally performs the cache
/// deletes a follower asks for, because a follower has no writer to perform them
/// with — `writer` is `Some` on exactly the tab that owns the cache, which is
/// what makes "one writer owns every cache write" hold for UI-initiated deletes
/// as well as for the sync stream.
///
/// One entry point rather than two so a caller cannot wire up half of it.
pub fn apply_broadcast(
    store: &SyncStore,
    writer: Option<&IdbWriter>,
    message: &SyncBroadcastMessage,
) {
    if let Some(writer) = writer {
        enqueue_broadcast_cache_writes(writer, message);
    }
    apply_broadcast_to_memory(store, message);
}

/// Queue the cache writes another tab asked this one to perform.
///
/// Leader-only, and only [`SyncBroadcastMessage::CacheDelete`] carries any: the
/// other variants report writes this tab has already made, and a
/// `BroadcastChannel` never delivers to the object that posted.
fn enqueue_broadcast_cache_writes(writer: &IdbWriter, message: &SyncBroadcastMessage) {
    match message {
        SyncBroadcastMessage::CacheDelete { entities } => {
            for entity in entities {
                enqueue_delete(writer, &entity.entity_type, &entity.entity_id);
            }
        }
        SyncBroadcastMessage::Action(_)
        | SyncBroadcastMessage::Complete { .. }
        | SyncBroadcastMessage::Reset => {}
    }
}

/// Apply one broadcast message from the leader tab to a follower's store.
///
/// The follower mirrors the leader's in-memory state exactly: the leader posts
/// each action after its cache write commits, so replaying them here converges
/// on the same store contents without the follower ever touching IndexedDB.
fn apply_broadcast_to_memory(store: &SyncStore, message: &SyncBroadcastMessage) {
    match message {
        SyncBroadcastMessage::Action(action) => apply_action_to_memory(store, action),
        SyncBroadcastMessage::CacheDelete { .. } => {
            // Nothing to do in memory. The tab that asked for the delete already
            // dropped the entity from its own store — that is what made the UI
            // react immediately — and every other tab converges through the
            // server's own sync action for the same change.
        }
        SyncBroadcastMessage::Complete { last_sync_id } => {
            // Mirrors the leader's own `sync_complete` handling. By the time
            // this is posted the cursor and every entity of the stream are in
            // the shared cache, so on-demand IDB reads (comments, activities)
            // see the same data the leader does.
            store.set_initialized(true);
            tracing::debug!(last_sync_id, "follower: leader finished a sync stream");
        }
        SyncBroadcastMessage::Reset => {
            // The leader is nuking the shared cache and re-bootstrapping. Drop
            // in-memory state; the re-bootstrap stream refills it action by
            // action over this same channel.
            tracing::info!("follower: leader signalled sync_reset — clearing in-memory store");
            store.reset();
        }
    }
}

// ── Cache half (leader only) ────────────────────────────────────────────────

/// Queue the removal of one cached entity record.
fn enqueue_delete(writer: &IdbWriter, entity_type: &str, entity_id: &str) {
    writer.enqueue(IdbOp::Delete {
        entity_type: entity_type.to_owned(),
        entity_id: entity_id.to_owned(),
    });
}

/// Queue the persistent cache writes for one `SyncAction`.
///
/// Leader-only. Nothing is written here directly: every op is appended to the
/// FIFO writer queue, ordered against the cursor that will claim it.
pub fn enqueue_cache_writes(writer: &IdbWriter, action: &SyncAction) {
    let entity_type = action.entity_type.as_str();
    let entity_id = &action.entity_id;

    match action.action {
        SyncActionType::Insert | SyncActionType::Update => {
            // A missing data field is reported by `apply_action_to_memory`,
            // which every caller of this function also runs for the same
            // action. Logging it here too would double every warning.
            let Some(ref entity_data) = action.data else {
                return;
            };

            // For issues: split the description into a separate issue_content
            // entity in IDB so it is not bulk-loaded during hydration. The
            // main issue record stored in IDB has its description stripped.
            let (idb_data, content_json) = if entity_type == entity_types::ISSUE {
                let mut data = entity_data.clone();
                let description = data.as_object_mut().and_then(|obj| obj.remove("description"));
                let content = description.and_then(|d| {
                    if d.is_null() {
                        None
                    } else {
                        Some(serde_json::json!({"description": d}).to_string())
                    }
                });
                (data, content)
            } else {
                (entity_data.clone(), None)
            };

            // Queue the persistent write behind everything already streamed.
            let json_str = match serde_json::to_string(&idb_data) {
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
            writer.enqueue(IdbOp::Upsert {
                entity_type: entity_type.to_owned(),
                entity_id: entity_id.clone(),
                json: json_str,
                ts: action.timestamp.clone(),
            });
            // Issue descriptions are stored as a separate entity so hydration
            // does not bulk-load them.
            if let Some(content) = content_json {
                writer.enqueue(IdbOp::Upsert {
                    entity_type: entity_types::ISSUE_CONTENT.to_owned(),
                    entity_id: entity_id.clone(),
                    json: content,
                    ts: action.timestamp.clone(),
                });
            }
        }
        SyncActionType::Delete => {
            // Queue the cache delete so it stays ordered against the cursor.
            // Entity types with no cached rows (activity, issue_relation) have
            // nothing to queue — they only bump a version counter, which is the
            // memory half's job.
            match entity_type {
                et if et == entity_types::ISSUE => {
                    enqueue_delete(writer, entity_types::ISSUE, entity_id);
                    enqueue_delete(writer, entity_types::ISSUE_CONTENT, entity_id);
                }
                et if et == entity_types::LABEL => {
                    enqueue_delete(writer, entity_types::LABEL, entity_id);
                }
                et if et == entity_types::STATUS => {
                    enqueue_delete(writer, entity_types::STATUS, entity_id);
                }
                et if et == entity_types::TEAM => {
                    enqueue_delete(writer, entity_types::TEAM, entity_id);
                }
                et if et == entity_types::PROJECT => {
                    enqueue_delete(writer, entity_types::PROJECT, entity_id);
                }
                et if et == entity_types::VIEW => {
                    enqueue_delete(writer, entity_types::VIEW, entity_id);
                }
                et if et == entity_types::FAVORITE => {
                    enqueue_delete(writer, entity_types::FAVORITE, entity_id);
                }
                et if et == entity_types::NOTIFICATION => {
                    enqueue_delete(writer, entity_types::NOTIFICATION, entity_id);
                }
                et if et == entity_types::COMMENT => {
                    enqueue_delete(writer, entity_types::COMMENT, entity_id);
                }
                // Unhandled types are reported by the memory half.
                _ => {}
            }
        }
    }
}

// ── Native unit tests ───────────────────────────────────────────────────────

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use futures::channel::mpsc::UnboundedReceiver;
    use leptos::prelude::*;
    use trakkt_types::sync::SyncActionType;

    use crate::cache::idb_writer::channel;
    use crate::cache::tab_leader::CachedEntity;

    use super::*;

    /// A one-line description of a queued op, for order-sensitive assertions.
    fn describe(op: &IdbOp) -> String {
        match op {
            IdbOp::Upsert {
                entity_type,
                entity_id,
                json,
                ..
            } => format!("upsert:{entity_type}:{entity_id}:{json}"),
            IdbOp::Delete {
                entity_type,
                entity_id,
            } => format!("delete:{entity_type}:{entity_id}"),
            IdbOp::DeleteAllOfType { entity_type } => format!("delete_all:{entity_type}"),
            IdbOp::SetCursor { cursor } => format!("set_cursor:{cursor}"),
            IdbOp::SetSchemaHash => "set_schema_hash".to_owned(),
            IdbOp::Flush(_) => "flush".to_owned(),
            IdbOp::Notify(_) => "notify".to_owned(),
        }
    }

    fn drain(mut ops: UnboundedReceiver<IdbOp>) -> Vec<String> {
        let mut seen = Vec::new();
        while let Ok(op) = ops.try_recv() {
            seen.push(describe(&op));
        }
        seen
    }

    /// Run the cache half alone and report what it queued.
    fn cache_ops(action: &SyncAction) -> Vec<String> {
        let (writer, ops) = channel();
        enqueue_cache_writes(&writer, action);
        drop(writer);
        drain(ops)
    }

    fn issue_json(description: serde_json::Value) -> serde_json::Value {
        serde_json::json!({
            "issue_id": "issue-1",
            "workspace_id": "ws-1",
            "team_id": "team-1",
            "team_key": "TRA",
            "number": 1,
            "title": "A title",
            "description": description,
            "status_id": "status-1",
            "status_name": "Todo",
            "status_category": "unstarted",
            "priority": 2,
            "assignee_id": null,
            "assignee_name": null,
            "creator_id": "user-1",
            "creator_name": null,
            "due_date": null,
            "project_id": null,
            "project_name": null,
            "milestone_id": null,
            "estimate": null,
            "parent_identifier": null,
            "parent_title": null,
            "sort_order": null,
            "created_at": "2026-07-26T00:00:00Z",
            "updated_at": "2026-07-26T00:00:00Z",
            "started_at": null,
            "completed_at": null,
            "released_at": null,
            "archived_at": null,
            "has_children": false,
            "is_blocked": false,
            "is_blocking": false,
            "has_relations": false,
            "labels": [],
        })
    }

    fn action(
        entity_type: &str,
        kind: SyncActionType,
        data: Option<serde_json::Value>,
    ) -> SyncAction {
        SyncAction {
            sync_id: 1,
            entity_type: entity_type.to_owned(),
            entity_id: "issue-1".to_owned(),
            workspace_id: "ws-1".to_owned(),
            action: kind,
            data,
            timestamp: "2026-07-26T00:00:00Z".to_owned(),
        }
    }

    /// Reactive primitives need an owner on the native target.
    fn with_store(test: impl FnOnce(SyncStore)) {
        let owner = Owner::new();
        owner.set();
        test(SyncStore::new());
    }

    // ── Memory half ─────────────────────────────────────────────────────────

    #[test]
    fn an_issue_upsert_lands_in_memory_without_its_description() {
        with_store(|store| {
            apply_action_to_memory(
                &store,
                &action(
                    entity_types::ISSUE,
                    SyncActionType::Update,
                    Some(issue_json(serde_json::json!("the body"))),
                ),
            );

            let issues = store.issues().get_untracked();
            assert_eq!(issues.len(), 1, "the issue should be in the store");
            assert_eq!(issues[0].title, "A title");
            assert_eq!(
                issues[0].description, None,
                "descriptions are read on demand, never held in the list store"
            );
        });
    }

    #[test]
    fn a_comment_action_bumps_only_the_comments_version() {
        with_store(|store| {
            apply_action_to_memory(
                &store,
                &action(
                    entity_types::COMMENT,
                    SyncActionType::Insert,
                    Some(serde_json::json!({"comment_id": "c-1"})),
                ),
            );

            assert_eq!(
                store.comments_version().get_untracked(),
                1,
                "the detail page re-reads comments from IDB when this bumps"
            );
            assert_eq!(store.activities_version().get_untracked(), 0);
            assert_eq!(store.relations_version().get_untracked(), 0);
        });
    }

    #[test]
    fn an_activity_action_bumps_only_the_activities_version() {
        with_store(|store| {
            apply_action_to_memory(
                &store,
                &action(
                    entity_types::ACTIVITY,
                    SyncActionType::Insert,
                    Some(serde_json::json!({"activity_id": "a-1"})),
                ),
            );

            assert_eq!(
                store.activities_version().get_untracked(),
                1,
                "the issue timeline refetches when this bumps"
            );
            assert_eq!(store.comments_version().get_untracked(), 0);
            assert_eq!(store.relations_version().get_untracked(), 0);
        });
    }

    #[test]
    fn a_relation_action_bumps_only_the_relations_version() {
        with_store(|store| {
            apply_action_to_memory(
                &store,
                &action(
                    entity_types::ISSUE_RELATION,
                    SyncActionType::Insert,
                    Some(serde_json::json!({"relation_id": "r-1"})),
                ),
            );

            assert_eq!(
                store.relations_version().get_untracked(),
                1,
                "the relations section refetches when this bumps"
            );
            assert_eq!(store.comments_version().get_untracked(), 0);
            assert_eq!(store.activities_version().get_untracked(), 0);
        });
    }

    #[test]
    fn version_counters_also_bump_on_delete() {
        with_store(|store| {
            for entity_type in [
                entity_types::COMMENT,
                entity_types::ACTIVITY,
                entity_types::ISSUE_RELATION,
            ] {
                apply_action_to_memory(
                    &store,
                    &action(entity_type, SyncActionType::Delete, None),
                );
            }

            assert_eq!(store.comments_version().get_untracked(), 1);
            assert_eq!(store.activities_version().get_untracked(), 1);
            assert_eq!(store.relations_version().get_untracked(), 1);
        });
    }

    #[test]
    fn deleting_an_issue_removes_it_from_memory() {
        with_store(|store| {
            apply_action_to_memory(
                &store,
                &action(
                    entity_types::ISSUE,
                    SyncActionType::Update,
                    Some(issue_json(serde_json::Value::Null)),
                ),
            );
            assert_eq!(store.issues().get_untracked().len(), 1);

            apply_action_to_memory(
                &store,
                &action(entity_types::ISSUE, SyncActionType::Delete, None),
            );
            assert!(store.issues().get_untracked().is_empty());
        });
    }

    #[test]
    fn an_upsert_without_data_changes_nothing() {
        with_store(|store| {
            apply_action_to_memory(
                &store,
                &action(entity_types::ISSUE, SyncActionType::Update, None),
            );
            assert!(store.issues().get_untracked().is_empty());
        });

        assert!(
            cache_ops(&action(entity_types::ISSUE, SyncActionType::Update, None)).is_empty(),
            "a dataless upsert must not reach the cache either"
        );
    }

    // ── Cache half ──────────────────────────────────────────────────────────

    #[test]
    fn an_issue_upsert_queues_the_body_as_a_separate_record() {
        let ops = cache_ops(&action(
            entity_types::ISSUE,
            SyncActionType::Update,
            Some(issue_json(serde_json::json!("the body"))),
        ));

        assert_eq!(ops.len(), 2, "expected the issue and its content, got {ops:?}");
        assert!(
            ops[0].starts_with("upsert:issue:issue-1:"),
            "expected the issue record first, got {:?}",
            ops[0]
        );
        assert!(
            !ops[0].contains("the body"),
            "the description must be stripped from the bulk-hydrated record: {:?}",
            ops[0]
        );
        assert_eq!(
            ops[1],
            r#"upsert:issue_content:issue-1:{"description":"the body"}"#
        );
    }

    #[test]
    fn an_issue_with_no_body_queues_only_the_issue() {
        let ops = cache_ops(&action(
            entity_types::ISSUE,
            SyncActionType::Update,
            Some(issue_json(serde_json::Value::Null)),
        ));

        assert_eq!(ops.len(), 1, "expected only the issue record, got {ops:?}");
    }

    #[test]
    fn deleting_an_issue_queues_both_of_its_records() {
        let ops = cache_ops(&action(entity_types::ISSUE, SyncActionType::Delete, None));

        assert_eq!(
            ops,
            vec!["delete:issue:issue-1", "delete:issue_content:issue-1"],
            "the body is a separate record and must be deleted with the issue"
        );
    }

    #[test]
    fn entity_types_with_no_cached_rows_queue_nothing() {
        for entity_type in [entity_types::ACTIVITY, entity_types::ISSUE_RELATION] {
            assert!(
                cache_ops(&action(entity_type, SyncActionType::Delete, None)).is_empty(),
                "{entity_type} has no cached rows to delete"
            );
        }
    }

    #[test]
    fn a_comment_delete_queues_the_cache_delete() {
        assert_eq!(
            cache_ops(&action(entity_types::COMMENT, SyncActionType::Delete, None)),
            vec!["delete:comment:issue-1"],
            "comments live only in IndexedDB, so the row must go"
        );
    }

    // ── Follower dispatch ───────────────────────────────────────────────────

    #[test]
    fn a_follower_applies_broadcast_actions_to_memory() {
        with_store(|store| {
            apply_broadcast_to_memory(
                &store,
                &SyncBroadcastMessage::Action(action(
                    entity_types::ISSUE,
                    SyncActionType::Update,
                    Some(issue_json(serde_json::Value::Null)),
                )),
            );

            assert_eq!(
                store.issues().get_untracked().len(),
                1,
                "a follower's list pages update from the broadcast alone"
            );
        });
    }

    #[test]
    fn a_follower_marks_the_store_initialized_on_completion() {
        with_store(|store| {
            assert!(!store.initialized().get_untracked());

            apply_broadcast_to_memory(
                &store,
                &SyncBroadcastMessage::Complete { last_sync_id: 12 },
            );

            assert!(
                store.initialized().get_untracked(),
                "followers must leave the loading state when the leader finishes a stream"
            );
        });
    }

    /// Run the broadcast handler as the leader and report what it queued.
    fn broadcast_cache_ops(message: &SyncBroadcastMessage) -> Vec<String> {
        let mut queued = Vec::new();
        with_store(|store| {
            let (writer, ops) = channel();
            apply_broadcast(&store, Some(&writer), message);
            drop(writer);
            queued = drain(ops);
        });
        queued
    }

    #[test]
    fn the_leader_queues_the_deletes_a_follower_asks_for() {
        assert_eq!(
            broadcast_cache_ops(&SyncBroadcastMessage::CacheDelete {
                entities: vec![
                    CachedEntity::new(entity_types::ISSUE, "issue-1"),
                    CachedEntity::new(entity_types::ISSUE_CONTENT, "issue-1"),
                ],
            }),
            vec!["delete:issue:issue-1", "delete:issue_content:issue-1"],
            "a follower has no writer of its own — the leader is what makes its \
             delete durable"
        );
    }

    #[test]
    fn the_leader_queues_nothing_for_messages_that_report_its_own_writes() {
        for message in [
            SyncBroadcastMessage::Action(action(
                entity_types::ISSUE,
                SyncActionType::Delete,
                None,
            )),
            SyncBroadcastMessage::Complete { last_sync_id: 3 },
            SyncBroadcastMessage::Reset,
        ] {
            assert!(
                broadcast_cache_ops(&message).is_empty(),
                "{message:?} describes a write that already happened — re-queueing it \
                 would replay the leader's own stream back into the cache"
            );
        }
    }

    #[test]
    fn a_follower_ignores_the_delete_requests_it_overhears() {
        with_store(|store| {
            apply_action_to_memory(
                &store,
                &action(
                    entity_types::ISSUE,
                    SyncActionType::Update,
                    Some(issue_json(serde_json::Value::Null)),
                ),
            );

            // Delivered to every other tab on the channel, not just the leader.
            apply_broadcast(
                &store,
                None,
                &SyncBroadcastMessage::CacheDelete {
                    entities: vec![CachedEntity::new(entity_types::ISSUE, "issue-1")],
                },
            );

            assert_eq!(
                store.issues().get_untracked().len(),
                1,
                "a tab that owns no writer has nothing to do with another tab's delete \
                 request — its own store converges through the server's sync action"
            );
        });
    }

    #[test]
    fn a_follower_clears_memory_on_reset() {
        with_store(|store| {
            apply_broadcast_to_memory(
                &store,
                &SyncBroadcastMessage::Action(action(
                    entity_types::ISSUE,
                    SyncActionType::Update,
                    Some(issue_json(serde_json::Value::Null)),
                )),
            );
            assert_eq!(store.issues().get_untracked().len(), 1);

            apply_broadcast_to_memory(&store, &SyncBroadcastMessage::Reset);

            assert!(
                store.issues().get_untracked().is_empty(),
                "the leader wiped the shared cache — the follower must not keep rendering it"
            );
        });
    }
}
