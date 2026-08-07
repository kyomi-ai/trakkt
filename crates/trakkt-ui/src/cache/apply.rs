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
//!
//! The counters are tested twice over, and the second time is the one that
//! matters. A version counter holds no data, so a native test that asserts one
//! incremented passes just as well against an arm nothing subscribes to — which
//! is a change that reaches the cache and never the screen. The browser tests at
//! the bottom of this file rebuild the exact source signal each page hands to
//! its `Resource` and assert *that* moved.

use trakkt_types::models::{Favorite, IssueWithDetails, Label, Notification, Project, Status, Team, View};
use trakkt_types::sync::{SyncAction, SyncActionType, entity_types};

use crate::cache::cached_types::cache_rows_written_by;
use crate::cache::idb_writer::{IdbOp, IdbWriter};
use crate::cache::store::SyncStore;
use crate::cache::tab_leader::SyncBroadcastMessage;

// ── Memory half (every tab) ─────────────────────────────────────────────────

/// What one frame reached in the reactive store.
///
/// Reported rather than only logged because "this entity type has an arm" is an
/// invariant with a test behind it: everything the cache persists must reach the
/// store, or the change lands in IndexedDB and nowhere the user can see it until
/// a reload. That guard used to answer the question by reading this module's
/// source as text and counting `match` arms, which credited an arm without ever
/// running it — and walked straight past a rebinding of the name `entity_types`.
/// Asking the function itself is exact, needs no parser, and runs natively.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StoreDispatch {
    /// The frame reached an arm: a cached collection was updated, or a version
    /// counter something subscribes to was bumped.
    Handled,
    /// An insert/update arrived with no `data`, so it was dropped before the
    /// entity match. Nothing downstream of that guard runs, which is why a
    /// service that omits the payload delivers nothing at all.
    MissingPayload,
    /// No arm handles this entity type. The frame ends here.
    Unhandled,
}

/// Apply one `SyncAction` to the reactive store.
///
/// Touches memory only: no IndexedDB, no cursor. The leader pairs this with
/// [`enqueue_cache_writes`]; a follower runs it alone.
///
/// Returns what the frame reached; see [`StoreDispatch`]. Callers that only want
/// the effect can ignore it.
pub fn apply_action_to_memory(store: &SyncStore, action: &SyncAction) -> StoreDispatch {
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
                return StoreDispatch::MissingPayload;
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
                    // fetched on-demand by the timeline component, straight from
                    // the `list_issue_activities` / `list_workspace_activities`
                    // server functions. Bump the version counter so those
                    // reactive dependencies refetch.
                    //
                    // As with milestones and members, `entity_data` is
                    // deliberately not read here, and the payload is still what
                    // makes this arm reachable: the guard above returns on a
                    // data-less insert/update before this match, and both
                    // `insert_activity` and `coalesce_or_insert_activity` in
                    // `crates/trakkt-auth/src/activity_service.rs` used to pass
                    // `None`. Every ACTIVITY frame was therefore dropped and no
                    // other client's timeline moved until it was reloaded —
                    // this arm ran in tests and never in production.
                    //
                    // Bumping the counter is the whole of the frame's job, and
                    // the cache half deliberately does nothing with it: activity
                    // is on `NOT_CACHED` in [`crate::cache::cached_types`], so no
                    // row is written to IndexedDB. Nothing here would read one
                    // back — the `activity` entity type is named only in this
                    // module and in that one — and `sync_bootstrap`
                    // streams eleven types without it
                    // (`apps/server/src/routes/websocket.rs`), so a cached
                    // activity table could only ever be the arbitrary subset
                    // that arrived while some tab was open. Both readers ask the
                    // server instead, which is what this counter makes them do.
                    store.bump_activities_version();
                }
                et if et == entity_types::ISSUE_RELATION => {
                    // Relations are fetched on-demand by the relations section.
                    // Bump the version counter so reactive dependencies refetch.
                    store.bump_relations_version();
                }
                et if et == entity_types::PROJECT_MILESTONE => {
                    // Milestones are fetched on-demand by the project detail
                    // page and the issue metadata sidebar, straight from the
                    // `list_milestones` server function. Bump the version
                    // counter so those reactive dependencies refetch.
                    //
                    // `entity_data` is deliberately not read here, and that is
                    // not an oversight to tidy away. The guard above returns on
                    // a data-less insert/update before this match is reached, so
                    // without a payload on the wire this arm never runs at all —
                    // which is exactly the bug that left milestones frozen after
                    // bootstrap. Carrying it also keeps milestones the same
                    // shape as every other entity on the stream (bootstrap
                    // already streams them with full data), and it is what a
                    // future cached milestone list would need on the delta path
                    // to apply the change without a round trip.
                    store.bump_milestones_version();
                }
                et if et == entity_types::PROJECT_MEMBER => {
                    // Memberships are fetched on-demand by the project detail
                    // page, straight from the `list_project_members` server
                    // function. Bump the version counter so it refetches.
                    //
                    // As with milestones, `entity_data` is deliberately not read
                    // here. The guard above returns on a data-less insert before
                    // this match is reached, so the payload on the wire is what
                    // lets this arm run at all — that is the whole bug. It is
                    // also what a future cached member list would need on the
                    // delta path to apply the change without a round trip.
                    store.bump_project_members_version();
                }
                et if et == entity_types::PROJECT_UPDATE => {
                    // Posted status updates are fetched on-demand by the project
                    // detail page from `list_project_updates` — same on-demand
                    // read path, same payload reasoning as the member arm above.
                    store.bump_project_updates_version();
                }
                et if et == entity_types::ATTACHMENT => {
                    // Attachments are fetched on-demand by the issue detail
                    // page's attachment section, straight from the
                    // `list_issue_attachments` server function. Bump the counter
                    // so it refetches: an upload made in another tab, on another
                    // device, or by an agent otherwise never appears in the list.
                    store.bump_attachments_version();
                }
                et if et == entity_types::ISSUE_ATTACHMENT => {
                    // Linking an existing attachment to an issue changes the very
                    // same list, so it shares the counter — one counter per
                    // reader, not per entity type.
                    //
                    // This arm only runs when the frame carries a payload, and
                    // `attach_to_issue` in
                    // `crates/trakkt-auth/src/attachment_service.rs` records the
                    // link with `None`. So the paths that are live today are the
                    // `Delete` arm below (an unlink) and the attachment insert an
                    // upload emits alongside its link. Sending the junction row
                    // here is the server's half — `add_project_member` in
                    // `project_service.rs` already does exactly that for the
                    // membership its own arm depends on — and it is tracked as
                    // TRA-9979. Handling it here is what leaves that a change to
                    // one service function rather than a second silent gap to
                    // rediscover.
                    store.bump_attachments_version();
                }
                et if et == entity_types::NOTIFICATION_PREFERENCES => {
                    // Preferences are fetched on-demand by the notification
                    // settings page from `get_notification_preferences`. These
                    // frames are scoped to one user, so what this delivers is
                    // that user's own change from another tab or device.
                    store.bump_notification_preferences_version();
                }
                et if et == entity_types::WORKSPACE_SETTINGS => {
                    // The workspace settings page reads through its own
                    // `get_workspace_settings` round trip, so a rename or an
                    // auto-archive change by another admin reaches it only when
                    // this counter tells it to ask again.
                    store.bump_workspace_settings_version();
                }
                other => {
                    tracing::debug!(
                        entity_type = other,
                        "sync_action: unhandled entity type — ignoring"
                    );
                    return StoreDispatch::Unhandled;
                }
            }
            StoreDispatch::Handled
        }
        SyncActionType::Delete => {
            // Remove from the reactive store in memory only. Which types have a
            // cached row is not this half's concern and must not become one: the
            // types on `NOT_CACHED` still have arms here, because the counter
            // they bump is what their reader — a server function, not the cache
            // — subscribes to.
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
                et if et == entity_types::PROJECT_MILESTONE => {
                    // Same on-demand read path as insert/update: the milestone
                    // lists refetch from the server when this bumps.
                    store.bump_milestones_version();
                }
                et if et == entity_types::PROJECT_MEMBER => {
                    // Removing a member is the one membership edit that arrives
                    // as a Delete — there is no row left to send, so this
                    // counter is the only thing that tells the project detail
                    // page its member list is stale.
                    store.bump_project_members_version();
                }
                et if et == entity_types::PROJECT_UPDATE => {
                    // Same on-demand read path as the insert arm. No server path
                    // deletes a posted update today; handling it here is what
                    // stops one from arriving as silence if that changes, which
                    // is the exact failure this ticket fixed for members.
                    store.bump_project_updates_version();
                }
                et if et == entity_types::ATTACHMENT => {
                    // Deleting an attachment removes it from every issue it was
                    // linked to, so the detail page's list has to refetch.
                    store.bump_attachments_version();
                }
                et if et == entity_types::ISSUE_ATTACHMENT => {
                    // Unlinking is the one attachment-link edit that arrives as a
                    // Delete — there is no row left to send. Unlike its insert
                    // twin this arm needs no payload to run, so it is the live
                    // path today: a detach performed elsewhere reaches the list
                    // through here and nowhere else.
                    store.bump_attachments_version();
                }
                et if et == entity_types::NOTIFICATION_PREFERENCES => {
                    // No server path deletes a preferences row today. Handling it
                    // is what stops one from arriving as silence if that changes
                    // — the same reasoning as the posted-update arm above.
                    store.bump_notification_preferences_version();
                }
                et if et == entity_types::WORKSPACE_SETTINGS => {
                    // Likewise: settings are only ever updated today, never
                    // deleted. The settings page has one reactive dependency, and
                    // it must not depend on which action type carried the change.
                    store.bump_workspace_settings_version();
                }
                other => {
                    tracing::debug!(
                        entity_type = other,
                        "sync_action delete: unhandled entity type — ignoring"
                    );
                    return StoreDispatch::Unhandled;
                }
            }
            StoreDispatch::Handled
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
        SyncBroadcastMessage::Action(action) => {
            apply_action_to_memory(store, action);
        }
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
///
/// Both halves are gated on the same
/// [`cache_rows_written_by`](crate::cache::cached_types::cache_rows_written_by):
/// the insert path writes nothing for a type that owns no rows, and the delete
/// path removes exactly the rows the insert path could have written. They cannot
/// disagree, which is what stopped `attachment`, `issue_relation` and
/// `notification_preferences` rows being written by the generic upsert and then
/// removed by no delete arm at all.
pub fn enqueue_cache_writes(writer: &IdbWriter, action: &SyncAction) {
    let entity_type = action.entity_type.as_str();
    let entity_id = &action.entity_id;
    // The rows the cache holds for this type: the gate on the way in, and the
    // exact set to remove on the way out.
    let cache_rows = cache_rows_written_by(entity_type);

    match action.action {
        SyncActionType::Insert | SyncActionType::Update => {
            // A missing data field is reported by `apply_action_to_memory`,
            // which every caller of this function also runs for the same
            // action. Logging it here too would double every warning.
            let Some(ref entity_data) = action.data else {
                return;
            };

            // Types the cache holds no row of are not written at all — see
            // `NOT_CACHED` in `crate::cache::cached_types`, which is also what
            // takes them off the reset wipe and out of the delete below.
            if cache_rows.is_empty() {
                return;
            }

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
            //
            // Derived, not enumerated. This used to be a hand-written match over
            // twelve entity types while the insert path above was generic, and
            // the gap between the two is where rows leaked: a type the upsert
            // persisted but this match had no arm for had its row written and
            // removed by nothing, so it outlived the entity until the next full
            // `SyncReset`. Iterating the same slice the insert path was gated on
            // makes that unrepresentable — a type is either written and removed,
            // or neither.
            //
            // An issue yields two rows, because its body is stored separately;
            // a type on `NOT_CACHED` yields none, and the frame's whole job is
            // then the version counter the memory half bumps. A delete for a row
            // that was never written is a no-op in IndexedDB, which is what makes
            // it safe to queue both of an issue's rows unconditionally.
            for row in cache_rows {
                enqueue_delete(writer, row, entity_id);
            }
        }
    }
}

// ── Test support ────────────────────────────────────────────────────────────

/// Fixtures and setup shared by the native tests below and the browser tests
/// after them, so neither target's copy can drift from the other's.
#[cfg(test)]
mod test_support {
    use leptos::prelude::*;
    use trakkt_types::sync::{SyncAction, SyncActionType};

    use crate::cache::store::SyncStore;

    /// One sync frame, with the entity id every fixture here shares.
    pub(super) fn action(
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

    /// Same as [`action`] but with a caller-chosen entity id, for the entity
    /// types whose id is not a bare uuid — a membership is keyed
    /// `project_id:user_id`, an attachment link `issue_id:attachment_id`, and
    /// the add and the remove have to agree on it.
    pub(super) fn action_with_id(
        entity_type: &str,
        entity_id: &str,
        kind: SyncActionType,
        data: Option<serde_json::Value>,
    ) -> SyncAction {
        SyncAction {
            entity_id: entity_id.to_owned(),
            ..action(entity_type, kind, data)
        }
    }

    /// Reactive primitives need an owner on both targets.
    pub(super) fn with_store(test: impl FnOnce(SyncStore)) {
        let owner = Owner::new();
        owner.set();
        test(SyncStore::new());
    }

    /// The payload an ACTIVITY frame carries, built the way the server builds
    /// it.
    ///
    /// Serialized from the real model rather than written out as JSON on
    /// purpose. `insert_activity` and `coalesce_or_insert_activity` in
    /// `crates/trakkt-auth/src/activity_service.rs` read the row back and hand
    /// it to `sync_log_service::sync_payload`, which is `serde_json::to_value`
    /// over exactly this type — so a field renamed on `IssueActivity` changes
    /// this fixture and the wire together instead of leaving the two agreeing
    /// only by hand.
    ///
    /// Not target-gated: the native tests assert what a real frame reaches, and
    /// the browser tests assert what it moves.
    pub(super) fn issue_activity_json() -> serde_json::Value {
        let activity = trakkt_types::models::IssueActivity {
            activity_id: "act-1".to_owned(),
            issue_id: "issue-1".to_owned(),
            workspace_id: "ws-1".to_owned(),
            actor_id: Some("usr-alice".to_owned()),
            actor_name: Some("Alice".to_owned()),
            action_type: "status_changed".to_owned(),
            field: Some("status".to_owned()),
            old_value: Some("Todo".to_owned()),
            new_value: Some("In Progress".to_owned()),
            metadata: None,
            action_source: trakkt_types::enums::ActionSource::User,
            action_source_label: None,
            created_at: "2026-07-26T00:00:00Z".to_owned(),
        };
        serde_json::to_value(&activity)
            .expect("serializing an IssueActivity the way `sync_payload` does")
    }

    // The four entity types this ticket added arms for are exercised only by the
    // browser tests — the counters they bump are meaningless without a reader,
    // and a reader is a reactive signal. So their payloads are wasm-only; built
    // on the native target they are three functions nothing calls.

    /// The payload the notification state changes send, with `read` set by the
    /// caller.
    ///
    /// Serialized from the real model rather than written out as JSON, for the
    /// same reason [`issue_activity_json`] is: every one of
    /// `notification_service`'s six state changes reads the row back and hands
    /// it to `sync_log_service::sync_payload`, which is `serde_json::to_value`
    /// over exactly this type — so a renamed field moves this fixture and the
    /// wire together instead of leaving the two agreeing only by hand.
    #[cfg(target_arch = "wasm32")]
    pub(super) fn notification_json(read: bool) -> serde_json::Value {
        let notification = trakkt_types::models::Notification {
            notification_id: "ntf-1".to_owned(),
            workspace_id: "ws-1".to_owned(),
            user_id: "usr-alice".to_owned(),
            issue_id: "issue-1".to_owned(),
            notification_type: "assigned".to_owned(),
            read,
            issue_title: Some("A leaky issue".to_owned()),
            issue_number: Some(42),
            team_key: Some("TRA".to_owned()),
            actor_id: Some("usr-bob".to_owned()),
            actor_name: Some("Bob".to_owned()),
            action_source: trakkt_types::enums::ActionSource::User,
            action_source_label: None,
            created_at: "2026-07-26T00:00:00Z".to_owned(),
            deleted_at: None,
            context_id: None,
        };
        serde_json::to_value(&notification)
            .expect("serializing a Notification the way `sync_payload` does")
    }

    /// The payload `create_attachment` sends, as the wire carries it.
    #[cfg(target_arch = "wasm32")]
    pub(super) fn attachment_json() -> serde_json::Value {
        serde_json::json!({
            "attachment_id": "att-1",
            "filename": "diagram.png",
            "content_type": "image/png",
            "size_bytes": 4096,
            "created_at": "2026-07-26T00:00:00Z",
        })
    }

    /// The payload `update_notification_preference` sends.
    #[cfg(target_arch = "wasm32")]
    pub(super) fn notification_preferences_json() -> serde_json::Value {
        serde_json::json!({
            "preference_id": "pref-1",
            "user_id": "usr-alice",
            "workspace_id": "ws-1",
            "notify_status_changes": true,
            "notify_comments": false,
            "notify_assignments": true,
            "notify_priority_changes": true,
            "notify_label_changes": true,
            "notify_due_date_changes": true,
            "notify_estimate_changes": true,
            "notify_milestone_changes": true,
            "notify_project_changes": true,
            "notify_team_changes": true,
            "notify_relation_changes": true,
            "notify_own_agent_actions": false,
            "notify_own_api_actions": false,
            "delivery_channel": "in_app",
        })
    }

    /// The snapshot `update_workspace_name`, `update_workspace_settings` and
    /// `set_workspace_default_team` all send — the shape
    /// `WorkspaceSnapshotRow::into_sync_value` builds.
    #[cfg(target_arch = "wasm32")]
    pub(super) fn workspace_settings_json() -> serde_json::Value {
        serde_json::json!({
            "workspace_id": "ws-1",
            "name": "Renamed Workspace",
            "settings": {"default_auto_archive_days": 30},
            "default_team_id": "team-1",
            "updated_at": "2026-07-26T00:00:00Z",
        })
    }
}

// ── Native unit tests ───────────────────────────────────────────────────────

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use futures::channel::mpsc::UnboundedReceiver;
    use leptos::prelude::*;
    use trakkt_types::sync::SyncActionType;

    use crate::cache::cached_types::{all_cached_entity_types, side_effect_only_cache_types};
    use crate::cache::idb_writer::channel;
    use crate::cache::tab_leader::CachedEntity;

    use super::test_support::*;
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
                    Some(issue_activity_json()),
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
    fn an_activity_action_without_a_payload_reaches_nothing() {
        // The whole of TRA-9987 in one assertion. Both activity writers sent
        // `None`, so this — not the test above — was the production path, and
        // the arm the test above exercises never ran outside this file.
        with_store(|store| {
            apply_action_to_memory(
                &store,
                &action(entity_types::ACTIVITY, SyncActionType::Insert, None),
            );

            assert_eq!(
                store.activities_version().get_untracked(),
                0,
                "the data-less guard returns before the entity match, so a \
                 payload-less activity insert never reaches its arm at all — \
                 this is why the server has to send one"
            );
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

    fn milestone_json() -> serde_json::Value {
        serde_json::json!({
            "milestone_id": "ms-1",
            "project_id": "proj-1",
            "name": "Beta",
            "description": null,
            "target_date": "2026-09-01",
            "sort_order": 0,
            "created_at": "2026-07-26T00:00:00Z",
        })
    }

    #[test]
    fn a_milestone_action_bumps_only_the_milestones_version() {
        with_store(|store| {
            apply_action_to_memory(
                &store,
                &action(
                    entity_types::PROJECT_MILESTONE,
                    SyncActionType::Insert,
                    Some(milestone_json()),
                ),
            );

            assert_eq!(
                store.milestones_version().get_untracked(),
                1,
                "the project detail page and the issue metadata sidebar refetch \
                 `list_milestones` when this bumps — without it a milestone \
                 created elsewhere never appears"
            );
            assert_eq!(store.comments_version().get_untracked(), 0);
            assert_eq!(store.activities_version().get_untracked(), 0);
            assert_eq!(store.relations_version().get_untracked(), 0);
        });
    }

    #[test]
    fn a_milestone_update_bumps_the_milestones_version_and_touches_no_list() {
        with_store(|store| {
            apply_action_to_memory(
                &store,
                &action(
                    entity_types::PROJECT_MILESTONE,
                    SyncActionType::Update,
                    Some(milestone_json()),
                ),
            );

            assert_eq!(
                store.milestones_version().get_untracked(),
                1,
                "a rename or a re-date has to reach the milestone lists"
            );
            // Milestones are not a cached list in this store: a payload shaped
            // like a milestone must not be mistaken for any entity that is.
            assert!(store.projects().get_untracked().is_empty());
            assert!(store.issues().get_untracked().is_empty());
        });
    }

    #[test]
    fn a_milestone_action_without_a_payload_reaches_nothing() {
        with_store(|store| {
            apply_action_to_memory(
                &store,
                &action(entity_types::PROJECT_MILESTONE, SyncActionType::Insert, None),
            );

            assert_eq!(
                store.milestones_version().get_untracked(),
                0,
                "the data-less guard returns before the entity match, so a \
                 payload-less milestone insert never reaches its arm at all — \
                 this is why the server has to send one"
            );
        });
    }

    // ── Project members and posted updates (TRA-9940) ───────────────────────
    //
    // Neither was on the sync protocol at all: both were reported as an Update
    // to the parent project, which a membership edit or a posted update leaves
    // byte-identical. The project detail page reads both from server functions,
    // so a version counter is the only thing that can tell it to ask again.

    fn project_member_json() -> serde_json::Value {
        serde_json::json!({
            "project_id": "proj-1",
            "user_id": "usr-bob",
            "role": "member",
            "created_at": "2026-07-26T00:00:00Z",
        })
    }

    fn project_update_json() -> serde_json::Value {
        serde_json::json!({
            "update_id": "upd-1",
            "project_id": "proj-1",
            "user_id": "usr-alice",
            "health": "at_risk",
            "body": "Blocked on the vendor",
            "created_at": "2026-07-26T00:00:00Z",
        })
    }

    #[test]
    fn a_project_member_action_bumps_only_the_project_members_version() {
        with_store(|store| {
            apply_action_to_memory(
                &store,
                &action(
                    entity_types::PROJECT_MEMBER,
                    SyncActionType::Insert,
                    Some(project_member_json()),
                ),
            );

            assert_eq!(
                store.project_members_version().get_untracked(),
                1,
                "the project detail page refetches `list_project_members` when \
                 this bumps — without it a member added elsewhere never appears"
            );
            assert_eq!(store.project_updates_version().get_untracked(), 0);
            assert_eq!(store.milestones_version().get_untracked(), 0);
            assert_eq!(store.comments_version().get_untracked(), 0);
            assert_eq!(store.activities_version().get_untracked(), 0);
            assert_eq!(store.relations_version().get_untracked(), 0);
            // A membership payload carries a `project_id`. It must not be
            // mistaken for the project itself — that misrouting is the shape of
            // the bug this replaced.
            assert!(store.projects().get_untracked().is_empty());
            assert!(store.issues().get_untracked().is_empty());
        });
    }

    #[test]
    fn a_project_update_action_bumps_only_the_project_updates_version() {
        with_store(|store| {
            apply_action_to_memory(
                &store,
                &action(
                    entity_types::PROJECT_UPDATE,
                    SyncActionType::Insert,
                    Some(project_update_json()),
                ),
            );

            assert_eq!(
                store.project_updates_version().get_untracked(),
                1,
                "the project detail page refetches `list_project_updates` when \
                 this bumps — without it an update posted elsewhere never appears"
            );
            assert_eq!(store.project_members_version().get_untracked(), 0);
            assert_eq!(store.milestones_version().get_untracked(), 0);
            assert_eq!(store.comments_version().get_untracked(), 0);
            assert_eq!(store.activities_version().get_untracked(), 0);
            assert_eq!(store.relations_version().get_untracked(), 0);
            assert!(store.projects().get_untracked().is_empty());
            assert!(store.issues().get_untracked().is_empty());
        });
    }

    #[test]
    fn a_project_member_action_without_a_payload_reaches_nothing() {
        with_store(|store| {
            apply_action_to_memory(
                &store,
                &action(entity_types::PROJECT_MEMBER, SyncActionType::Insert, None),
            );

            assert_eq!(
                store.project_members_version().get_untracked(),
                0,
                "the data-less guard returns before the entity match, so a \
                 payload-less member insert never reaches its arm at all — this \
                 is why the server has to send one"
            );
        });
    }

    #[test]
    fn a_project_update_action_without_a_payload_reaches_nothing() {
        with_store(|store| {
            apply_action_to_memory(
                &store,
                &action(entity_types::PROJECT_UPDATE, SyncActionType::Insert, None),
            );

            assert_eq!(
                store.project_updates_version().get_untracked(),
                0,
                "the data-less guard returns before the entity match, so a \
                 payload-less posted-update insert never reaches its arm at all \
                 — this is why the server has to send one"
            );
        });
    }

    #[test]
    fn version_counters_also_bump_on_delete() {
        with_store(|store| {
            for entity_type in [
                entity_types::COMMENT,
                entity_types::ACTIVITY,
                entity_types::ISSUE_RELATION,
                entity_types::PROJECT_MILESTONE,
                entity_types::PROJECT_MEMBER,
                entity_types::PROJECT_UPDATE,
            ] {
                apply_action_to_memory(
                    &store,
                    &action(entity_type, SyncActionType::Delete, None),
                );
            }

            assert_eq!(store.comments_version().get_untracked(), 1);
            assert_eq!(store.activities_version().get_untracked(), 1);
            assert_eq!(store.relations_version().get_untracked(), 1);
            assert_eq!(
                store.milestones_version().get_untracked(),
                1,
                "a deleted milestone has to disappear from the lists too — a \
                 delete carries no payload, so the counter is all there is"
            );
            assert_eq!(
                store.project_members_version().get_untracked(),
                1,
                "removing a member is the *only* way a membership edit arrives \
                 as a Delete, and it carries no payload — this counter is the \
                 whole signal that the member list went stale"
            );
            assert_eq!(
                store.project_updates_version().get_untracked(),
                1,
                "no server path deletes a posted update today; handling it here \
                 is what keeps one from arriving as silence if that changes"
            );
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

    /// A type the cache holds no row of queues nothing on either half.
    ///
    /// The two halves are one statement now, and that is the change: this used
    /// to be true of `activity` by construction and false of `issue_relation` by
    /// accident. `issue_relation` inserts carry a payload and were persisted by
    /// the generic upsert, while this delete path — a hand-written list of arms —
    /// had no arm for them, so the rows outlived the relation until the next full
    /// reset. Both are now on `NOT_CACHED`, so neither is written and neither has
    /// anything to remove; `a_not_cached_type_is_never_persisted` is the half
    /// that fails if the write path stops honouring it, without which this test
    /// would go on passing while rows accumulated unevicted.
    #[test]
    fn a_not_cached_type_queues_no_delete() {
        for entity_type in [entity_types::ACTIVITY, entity_types::ISSUE_RELATION] {
            assert!(
                cache_ops(&action(entity_type, SyncActionType::Delete, None)).is_empty(),
                "{entity_type} owns no cache row, so its delete has nothing to queue"
            );
        }
    }

    #[test]
    fn a_member_add_and_remove_use_the_same_cache_key() {
        // The server derives `project_id:user_id` for both the Insert and the
        // Delete. If the two ever disagreed the remove would delete nothing and
        // the departed member's row would outlive the membership.
        let key = "proj-1:usr-bob";

        let upserts = cache_ops(&action_with_id(
            entity_types::PROJECT_MEMBER,
            key,
            SyncActionType::Insert,
            Some(project_member_json()),
        ));
        assert_eq!(upserts.len(), 1, "expected one member record, got {upserts:?}");
        assert!(
            upserts[0].starts_with("upsert:project_member:proj-1:usr-bob:"),
            "got {:?}",
            upserts[0]
        );
        assert!(
            upserts[0].contains("\"user_id\":\"usr-bob\""),
            "the persisted row is the payload the server sent: {:?}",
            upserts[0]
        );

        assert_eq!(
            cache_ops(&action_with_id(
                entity_types::PROJECT_MEMBER,
                key,
                SyncActionType::Delete,
                None
            )),
            vec!["delete:project_member:proj-1:usr-bob"],
            "member adds are persisted by the generic upsert, so the remove has \
             to be persisted too — against the very same key"
        );
    }

    #[test]
    fn a_project_update_delete_queues_the_cache_delete() {
        assert_eq!(
            cache_ops(&action_with_id(
                entity_types::PROJECT_UPDATE,
                "upd-1",
                SyncActionType::Delete,
                None
            )),
            vec!["delete:project_update:upd-1"],
            "posted updates are persisted by the generic upsert, so a delete \
             has to remove the row rather than leave it behind"
        );
    }

    #[test]
    fn a_milestone_delete_queues_the_cache_delete() {
        assert_eq!(
            cache_ops(&action(
                entity_types::PROJECT_MILESTONE,
                SyncActionType::Delete,
                None
            )),
            vec!["delete:project_milestone:issue-1"],
            "milestone inserts/updates are persisted by the generic upsert, so \
             the delete has to be persisted too"
        );
    }

    #[test]
    fn a_milestone_upsert_queues_the_row_it_carries() {
        let ops = cache_ops(&action(
            entity_types::PROJECT_MILESTONE,
            SyncActionType::Update,
            Some(milestone_json()),
        ));

        assert_eq!(ops.len(), 1, "expected one milestone record, got {ops:?}");
        assert!(
            ops[0].starts_with("upsert:project_milestone:issue-1:"),
            "got {:?}",
            ops[0]
        );
        assert!(
            ops[0].contains("\"name\":\"Beta\""),
            "the persisted row is the payload the server sent: {:?}",
            ops[0]
        );
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

    /// Every type on `NOT_CACHED`, through the real write path.
    ///
    /// One assertion per type rather than a loop over the constant, because a
    /// loop over `NOT_CACHED` would agree with it by construction and would go
    /// on passing if a type were quietly removed from it. These name the types
    /// the determination was made about; the payloads are the shapes their
    /// services really send, so none of them can pass by sending something the
    /// write path would have skipped anyway.
    #[test]
    fn a_not_cached_type_is_never_persisted() {
        let cases: [(&str, serde_json::Value); 4] = [
            (
                entity_types::ATTACHMENT,
                serde_json::json!({
                    "attachment_id": "att-1",
                    "filename": "diagram.png",
                    "content_type": "image/png",
                    "size_bytes": 4096,
                    "created_at": "2026-07-26T00:00:00Z",
                }),
            ),
            (
                entity_types::ISSUE_ATTACHMENT,
                serde_json::json!({
                    "issue_id": "issue-1",
                    "attachment_id": "att-1",
                    "created_at": "2026-07-26T00:00:00Z",
                }),
            ),
            (
                entity_types::ISSUE_RELATION,
                serde_json::json!({
                    "relation_id": "rel-1",
                    "issue_id": "issue-1",
                    "related_issue_id": "issue-2",
                    "relation_type": "blocks",
                }),
            ),
            (
                entity_types::NOTIFICATION_PREFERENCES,
                serde_json::json!({
                    "preference_id": "pref-1",
                    "user_id": "usr-alice",
                    "workspace_id": "ws-1",
                    "notify_comments": false,
                }),
            ),
        ];

        for (entity_type, payload) in cases {
            assert!(
                cache_ops(&action_with_id(
                    entity_type,
                    "entity-1",
                    SyncActionType::Insert,
                    Some(payload),
                ))
                .is_empty(),
                "{entity_type} must not reach IndexedDB: its reader refetches from a server \
                 function when the version counter bumps, and no bootstrap streams the type, \
                 so the row could only ever be written"
            );
        }
    }

    #[test]
    fn a_release_upsert_is_not_persisted() {
        // Nothing in this client reads releases back — no route, no page, no
        // hydration step, no on-demand cache read — and no bootstrap streams
        // them, so a cached release row could never be more than an arbitrary
        // fragment. Writing rows nothing reads is the reason `release` is on
        // `NOT_CACHED`, and the reason it has no store arm either.
        assert!(
            cache_ops(&action_with_id(
                entity_types::RELEASE,
                "rel-1",
                SyncActionType::Insert,
                Some(serde_json::json!({
                    "release_id": "rel-1",
                    "name": "v1.2.0",
                    "issue_count": 3,
                })),
            ))
            .is_empty(),
            "a release must not reach IndexedDB — it is wiped by no reset and read by \
             nothing, so the row would only ever be written"
        );
    }

    #[test]
    fn an_activity_upsert_is_not_persisted() {
        // The other half of the payload this ticket added. The payload has to be
        // there for the memory arm to run — the data-less guard returns before
        // the entity match — but it must not turn into an IndexedDB row: nothing
        // in this client reads an activity back, and no bootstrap streams them,
        // so a cached activity table could only ever be the arbitrary subset that
        // arrived while a tab was open.
        //
        // Without `activity` on `NOT_CACHED` this is not a no-op that goes
        // unnoticed. The delete half queues nothing for an activity, so nothing
        // ever evicts one, and the rows grow in step with every status change,
        // comment and field edit in the whole workspace until a full `SyncReset`.
        //
        // The frame used here is the same real payload the memory-half tests
        // assert *does* reach the timeline counter, so this cannot pass by
        // sending something the write path would have skipped anyway.
        assert!(
            cache_ops(&action_with_id(
                entity_types::ACTIVITY,
                "act-1",
                SyncActionType::Insert,
                Some(issue_activity_json()),
            ))
            .is_empty(),
            "an activity must not reach IndexedDB — it is evicted by no delete and read \
             by nothing, so the row would only ever be written"
        );
    }

    #[test]
    fn a_coalesced_activity_update_is_not_persisted_either() {
        // The coalescing path reports a repeated description save as an Update
        // of the row already written. `enqueue_cache_writes` handles Insert and
        // Update in one arm, so this shares the skip — stated separately because
        // "insert is skipped" and "update is skipped" is the pair a future
        // per-action-type split would break.
        assert!(
            cache_ops(&action_with_id(
                entity_types::ACTIVITY,
                "act-1",
                SyncActionType::Update,
                Some(issue_activity_json()),
            ))
            .is_empty(),
            "a coalesced activity update must not reach IndexedDB either"
        );
    }

    #[test]
    fn the_activity_skip_does_not_touch_the_comments_a_comment_frame_carries() {
        // Adding a comment records a `comment_added` activity *and* a comment.
        // Comments are cached and read back from IndexedDB by the issue detail
        // page, so the skip has to be scoped to the activity entity and not to
        // the interaction that produced it — the same distinction
        // `a_release_upsert_still_persists_nothing_when_it_carries_the_issues_it_names`
        // draws for releases.
        let ops = cache_ops(&action_with_id(
            entity_types::COMMENT,
            "cmt-1",
            SyncActionType::Insert,
            Some(serde_json::json!({
                "comment_id": "cmt-1",
                "issue_id": "issue-1",
                "body": "Looks good",
            })),
        ));
        assert_eq!(
            ops.len(),
            1,
            "the comment an activity accompanies is still cached, got {ops:?}"
        );
    }

    #[test]
    fn a_release_upsert_still_persists_nothing_when_it_carries_the_issues_it_names() {
        // Publishing a release emits an `issue` update per issue it contains,
        // alongside the release itself. Those are ordinary issue frames and must
        // keep working — the skip is scoped to the release entity, not to the
        // transaction that produced it.
        let ops = cache_ops(&action(
            entity_types::ISSUE,
            SyncActionType::Update,
            Some(issue_json(serde_json::Value::Null)),
        ));
        assert_eq!(
            ops.len(),
            1,
            "the issue updates a release emits are still cached, got {ops:?}"
        );
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

    // ── The three lists, checked against each other by execution ─────────────
    //
    // "What is written", "what is wiped" and "what is removed per entity" are
    // one mapping now (`cache::cached_types`), but the write path is still a
    // separate function that could stop honouring it. These drive every declared
    // entity type through the real `enqueue_cache_writes` and the real
    // `apply_action_to_memory` and compare what came out, so a reintroduced
    // hand-written list fails here rather than being agreed with.
    //
    // Nothing is restated: the universe comes from `entity_types::ALL`, which is
    // emitted by the same macro invocation that declares the constants, so a
    // newly declared type appears in these tests on the commit that declares it.
    //
    // They replace two `wasm32`-only tests that answered the same questions by
    // `include_str!`-ing this file and `trakkt-types/src/sync.rs` and parsing
    // `match` arms out of the text. That approach credited an arm it could read
    // without ever running it, and its own doc comment recorded the hole it could
    // not close: a `use some::other::module as entity_types;` in this file left
    // every arm parsing and being credited while comparing against different
    // strings. Executing the code closes that, and runs under `cargo test`.

    /// Every cache row an insert of `entity_type` really queues.
    fn rows_persisted_by_an_insert_of(entity_type: &str) -> Vec<String> {
        cache_ops(&action_with_id(
            entity_type,
            "entity-1",
            SyncActionType::Insert,
            // A non-null `description` is what makes the issue arm split its
            // body into a second record, so this payload reaches that branch
            // too. Types that decode it into nothing still persist the row —
            // the write path stores the JSON verbatim.
            Some(serde_json::json!({"description": "a body"})),
        ))
        .iter()
        .filter_map(|op| op.strip_prefix("upsert:"))
        .filter_map(|op| op.split(':').next())
        .map(str::to_owned)
        .collect()
    }

    /// Every cache row a delete of `entity_type` really queues.
    fn rows_deleted_by_a_delete_of(entity_type: &str) -> Vec<String> {
        cache_ops(&action_with_id(
            entity_type,
            "entity-1",
            SyncActionType::Delete,
            None,
        ))
        .iter()
        .filter_map(|op| op.strip_prefix("delete:"))
        .filter_map(|op| op.split(':').next())
        .map(str::to_owned)
        .collect()
    }

    /// The write path persists exactly the rows `cache_rows_written_by` claims.
    #[test]
    fn the_write_path_persists_exactly_the_rows_the_cache_claims_to_hold() {
        for entity_type in entity_types::ALL {
            let claimed: Vec<String> = cache_rows_written_by(entity_type)
                .iter()
                .map(|row| (*row).to_owned())
                .collect();
            assert_eq!(
                rows_persisted_by_an_insert_of(entity_type),
                claimed,
                "an insert of `{entity_type}` persisted rows other than the ones \
                 `cache_rows_written_by` names. The reset wipe and the per-entity delete \
                 are both derived from that function, so anything the write path stores \
                 outside it is wiped by nothing and removed by nothing."
            );
        }
    }

    /// The invariant `SyncReset` rests on: nothing the cache can hold survives
    /// the wipe.
    ///
    /// The replacement for the TRA-9940 guard test, and stronger in three ways:
    /// it runs natively, it reads the universe from `entity_types::ALL` instead
    /// of parsing source, and it checks the delete path too — which the original
    /// did not, because the delete path was not part of the list it policed.
    #[test]
    fn every_entity_type_the_cache_persists_is_wiped_by_a_reset() {
        let wiped = all_cached_entity_types();

        let mut unwiped: Vec<String> = Vec::new();
        for entity_type in entity_types::ALL {
            for row in rows_persisted_by_an_insert_of(entity_type) {
                if !wiped.contains(&row.as_str()) && !unwiped.contains(&row) {
                    unwiped.push(row);
                }
            }
        }

        assert!(
            unwiped.is_empty(),
            "These entity types are written to IndexedDB but never wiped: {unwiped:?}.\n\
             `enqueue_cache_writes` queues an upsert for them, while `SyncReset` and the \
             no-cursor cold start only clear what `all_cached_entity_types` returns — so \
             their rows outlive the reset that is supposed to leave a clean slate, and \
             nothing else ever removes them.\n\
             Both come from `cache_rows_written_by` in crates/trakkt-ui/src/cache/cached_types.rs. \
             If they disagree, the write path has stopped being driven by it."
        );
    }

    /// The list the original ticket missed: what a `Delete` removes.
    ///
    /// The insert path was generic and this one was a hand-written twelve-arm
    /// `match`, so a persisted type with no arm had its row written and removed
    /// by nothing — `attachment`, `issue_relation`, `notification_preferences`
    /// and `workspace_settings` were all in that state, and `issue_attachment`
    /// would have joined them the moment TRA-9979 gave it a payload. A reset was
    /// the only thing that ever cleared them.
    #[test]
    fn a_delete_removes_every_row_the_write_path_persists() {
        for entity_type in entity_types::ALL {
            let persisted = rows_persisted_by_an_insert_of(entity_type);
            let deleted = rows_deleted_by_a_delete_of(entity_type);

            assert_eq!(
                deleted, persisted,
                "a `{entity_type}` delete removes {deleted:?} but its insert persists \
                 {persisted:?}. A row the write path stores and the delete path leaves \
                 behind outlives the entity it describes until the next full `SyncReset` \
                 — which is the leak this pairing exists to make unrepresentable."
            );
        }
    }

    /// The invariant the UI rests on: nothing the cache holds is invisible.
    ///
    /// Being persisted is not the same as being seen. `apply_action_to_memory`
    /// routes each frame to the store by entity type, and a type it has no arm
    /// for ends at a `tracing::debug!`: the row lands in IndexedDB, no signal
    /// fires, and nothing on screen moves until a reload.
    ///
    /// Asked of the function rather than of its source, so an arm only counts
    /// when it actually runs. The payload is deliberately not a decodable entity
    /// — the arms that deserialise log a warning and carry on, and what is under
    /// test is that the frame reached an arm at all.
    #[test]
    fn every_entity_type_the_cache_persists_reaches_the_store() {
        let exempt = side_effect_only_cache_types();

        with_store(|store| {
            for entity_type in entity_types::ALL {
                // A type no frame carries — `issue_content` — cannot have an
                // arm, and demanding one would be demanding dead code.
                if cache_rows_written_by(entity_type).is_empty() || exempt.contains(entity_type) {
                    continue;
                }

                for kind in [SyncActionType::Insert, SyncActionType::Delete] {
                    let data = match kind {
                        SyncActionType::Delete => None,
                        _ => Some(serde_json::json!({})),
                    };
                    assert_eq!(
                        apply_action_to_memory(
                            &store,
                            &action_with_id(entity_type, "entity-1", kind.clone(), data)
                        ),
                        StoreDispatch::Handled,
                        "`{entity_type}` is cached by this client but its {kind:?} frame \
                         reaches nothing in the reactive store.\n\
                         `enqueue_cache_writes` persists it and a reset wipes it, so it is a \
                         real row — but `apply_action_to_memory` has no arm for it, so the \
                         change is in IndexedDB and nowhere the user can see it.\n\
                         Fix it there, with an arm that updates a cached collection or bumps \
                         a version counter *something actually subscribes to* — a counter no \
                         page reads is this same bug with a passing test. If nothing in this \
                         client reads the type at all, the honest fix is the other one: add \
                         it to `NOT_CACHED` in crates/trakkt-ui/src/cache/cached_types.rs, \
                         which takes it off the write path, the wipe and the delete at once."
                    );
                }
            }
        });
    }

    /// The data-less guard runs ahead of the entity match, which is why a
    /// service that omits the payload delivers nothing at all — the defect
    /// TRA-9987 fixed for activities and TRA-9979 still owes for attachment
    /// links. Stated against the reported outcome so the distinction between
    /// "no payload" and "no arm" cannot collapse.
    #[test]
    fn a_data_less_upsert_is_reported_as_such_rather_than_as_unhandled() {
        with_store(|store| {
            assert_eq!(
                apply_action_to_memory(
                    &store,
                    &action(entity_types::ISSUE, SyncActionType::Update, None)
                ),
                StoreDispatch::MissingPayload
            );
            assert_eq!(
                apply_action_to_memory(
                    &store,
                    &action("not_an_entity_type", SyncActionType::Update, Some(serde_json::json!({})))
                ),
                StoreDispatch::Unhandled
            );
            assert_eq!(
                apply_action_to_memory(
                    &store,
                    &action("not_an_entity_type", SyncActionType::Delete, None)
                ),
                StoreDispatch::Unhandled
            );
        });
    }
}

// ── Browser tests: the frames a reader actually observes ────────────────────

/// Tests that an applied frame changes what a page reads, run in a browser.
///
/// The counters these entity types bump hold no data, so "the counter went up"
/// is not evidence of anything — an arm that bumped a counter nobody subscribed
/// to would pass such a test while the screen stayed frozen, which is the bug
/// this file was fixed for. So each test builds the *same source signal* its
/// page hands to `Resource::new` and asserts that value moved. A Leptos
/// `Resource` refetches exactly when its source changes, so a moved source is a
/// refetch, and a refetch is the change reaching the page.
///
/// Run with:
/// `wasm-pack test --headless --firefox crates/trakkt-ui --lib --features hydrate`
#[cfg(all(test, target_arch = "wasm32"))]
mod wasm_tests {
    use leptos::prelude::*;
    use trakkt_types::sync::SyncActionType;
    use wasm_bindgen_test::{wasm_bindgen_test, wasm_bindgen_test_configure};

    use super::test_support::*;
    use super::*;

    wasm_bindgen_test_configure!(run_in_browser);

    /// The source `AttachmentsSection` hands to `Resource::new`, rebuilt here.
    ///
    /// See `AttachmentsSection` in `crates/trakkt-ui/src/pages/issues/issue_detail.rs`:
    /// `(team_key, number, version.get(), ws_version.get())`. The page's own
    /// `version` — its uploads and detaches — is held still here, so the only
    /// thing that can move this tuple is a frame from the sync stream.
    /// Each counter is resolved once, outside the closure, exactly as the pages
    /// do it — the getters build a fresh owner-registered `Signal` wrapper per
    /// call, so calling one from inside a closure that re-runs is a different
    /// (and wrong) shape from the one under test.
    fn attachment_list_source(store: SyncStore) -> Signal<(String, i32, u32, u32)> {
        let version = store.attachments_version();
        Signal::derive(move || ("TRA".to_owned(), 42, 0, version.get()))
    }

    /// The source `IssueTimeline` hands to `Resource::new`, rebuilt here.
    ///
    /// See `IssueTimeline` in `crates/trakkt-ui/src/pages/issues/issue_detail.rs`:
    /// `(team_key, number, activities_version.get())`. Nothing else can move it
    /// — the `issue` frame an edit emits alongside its activity updates the
    /// store's issue collection, and `IssueDetailContent` is keyed on
    /// `(team_key, number)` by the `<For>` that renders it, so neither rebuilds
    /// this resource. The counter is the only path a new timeline row has.
    ///
    /// The counter is resolved once here, outside the closure, which is how
    /// `IssueTimeline` reads it too — TRA-9991 hoisted it out of the source
    /// closure, so this rebuild now matches the page shape for shape. See the
    /// getter contract on [`SyncStore`].
    fn issue_timeline_source(store: SyncStore) -> Signal<(String, i32, u32)> {
        let version = store.activities_version();
        Signal::derive(move || ("TRA".to_owned(), 42, version.get()))
    }

    /// The source `NotificationsPage` reads inside its `LocalResource` fetcher.
    fn notification_preferences_source(store: SyncStore) -> Signal<u32> {
        let version = store.notification_preferences_version();
        Signal::derive(move || version.get())
    }

    /// The source `WorkspacePage` hands to `Resource::new`.
    fn workspace_settings_source(store: SyncStore) -> Signal<u32> {
        let version = store.workspace_settings_version();
        Signal::derive(move || version.get())
    }

    /// Apply `action` and report what each of the four page sources read
    /// before and after, so every test can assert its own reader moved *and*
    /// that it did not drag the other three along with it.
    struct Observed {
        attachments: bool,
        activities: bool,
        notification_preferences: bool,
        workspace_settings: bool,
    }

    fn observe(action: &SyncAction) -> Observed {
        let mut observed = Observed {
            attachments: false,
            activities: false,
            notification_preferences: false,
            workspace_settings: false,
        };
        with_store(|store| {
            let attachments = attachment_list_source(store);
            let activities = issue_timeline_source(store);
            let prefs = notification_preferences_source(store);
            let settings = workspace_settings_source(store);

            let before = (
                attachments.get_untracked(),
                activities.get_untracked(),
                prefs.get_untracked(),
                settings.get_untracked(),
            );

            apply_action_to_memory(&store, action);

            observed.attachments = attachments.get_untracked() != before.0;
            observed.activities = activities.get_untracked() != before.1;
            observed.notification_preferences = prefs.get_untracked() != before.2;
            observed.workspace_settings = settings.get_untracked() != before.3;
        });
        observed
    }

    // ── attachment ──────────────────────────────────────────────────────────

    #[wasm_bindgen_test]
    fn an_attachment_upload_refetches_the_issue_attachment_list() {
        let observed = observe(&action_with_id(
            entity_types::ATTACHMENT,
            "att-1",
            SyncActionType::Insert,
            Some(attachment_json()),
        ));

        assert!(
            observed.attachments,
            "the issue detail page's attachment list must refetch when an attachment is \
             uploaded — without it a file added by another tab, another user or an agent \
             never appears on the issue"
        );
        assert!(!observed.activities);
        assert!(!observed.notification_preferences);
        assert!(!observed.workspace_settings);
    }

    #[wasm_bindgen_test]
    fn deleting_an_attachment_refetches_the_issue_attachment_list() {
        let observed = observe(&action_with_id(
            entity_types::ATTACHMENT,
            "att-1",
            SyncActionType::Delete,
            None,
        ));

        assert!(
            observed.attachments,
            "a deleted attachment is gone from every issue it was linked to — the list has \
             to ask again, and a delete carries no payload so this counter is all there is"
        );
        assert!(!observed.activities);
        assert!(!observed.notification_preferences);
        assert!(!observed.workspace_settings);
    }

    // ── issue_attachment ────────────────────────────────────────────────────

    #[wasm_bindgen_test]
    fn linking_an_attachment_to_an_issue_refetches_the_list() {
        let observed = observe(&action_with_id(
            entity_types::ISSUE_ATTACHMENT,
            "issue-1:att-1",
            SyncActionType::Insert,
            Some(serde_json::json!({
                "issue_id": "issue-1",
                "attachment_id": "att-1",
                "created_at": "2026-07-26T00:00:00Z",
            })),
        ));

        assert!(
            observed.attachments,
            "linking an existing attachment changes the very list an upload changes, so it \
             shares the counter"
        );
        assert!(!observed.activities);
        assert!(!observed.notification_preferences);
        assert!(!observed.workspace_settings);
    }

    #[wasm_bindgen_test]
    fn unlinking_an_attachment_from_an_issue_refetches_the_list() {
        let observed = observe(&action_with_id(
            entity_types::ISSUE_ATTACHMENT,
            "issue-1:att-1",
            SyncActionType::Delete,
            None,
        ));

        assert!(
            observed.attachments,
            "an unlink is the only attachment-link edit that arrives as a Delete, and the \
             attachment itself still exists — nothing else tells the list it went stale"
        );
        assert!(!observed.activities);
        assert!(!observed.notification_preferences);
        assert!(!observed.workspace_settings);
    }

    #[wasm_bindgen_test]
    fn a_link_frame_without_a_payload_still_reaches_nothing() {
        // Not a gap in this module: the data-less guard is deliberate, and it is
        // what stops a payload-less frame from burning a sync id and advancing
        // every client's watermark past a change it never delivered.
        //
        // It does mean `attach_to_issue` in `crates/trakkt-auth/src/attachment_service.rs`
        // has a live gap of its own: it records the link with `None`, so the
        // link half of the arm above cannot run for a link made through the API
        // or an agent. Uploads are unaffected — they emit an `attachment` insert
        // with a payload alongside — and an unlink needs no payload. Sending the
        // junction row here is the server's half, exactly as `add_project_member`
        // already does. That is tracked as TRA-9979; this test is what makes the
        // gap visible rather than silent until it lands.
        let observed = observe(&action_with_id(
            entity_types::ISSUE_ATTACHMENT,
            "issue-1:att-1",
            SyncActionType::Insert,
            None,
        ));

        assert!(
            !observed.attachments,
            "the data-less guard returns before the entity match, so a payload-less link \
             insert never reaches its arm at all — this is why the server has to send one"
        );
    }

    // ── activity ────────────────────────────────────────────────────────────

    #[wasm_bindgen_test]
    fn an_activity_insert_refetches_the_issue_timeline() {
        let observed = observe(&action_with_id(
            entity_types::ACTIVITY,
            "act-1",
            SyncActionType::Insert,
            Some(issue_activity_json()),
        ));

        assert!(
            observed.activities,
            "the issue detail page's timeline must refetch when an activity is recorded — \
             a status change, a comment or a field edit made by another user, another tab \
             or an agent otherwise never appears on the issue until it is reloaded"
        );
        assert!(!observed.attachments);
        assert!(!observed.notification_preferences);
        assert!(!observed.workspace_settings);
    }

    #[wasm_bindgen_test]
    fn a_coalesced_activity_update_refetches_the_issue_timeline() {
        // `coalesce_or_insert_activity` reports a repeated description save
        // inside its 60s window as an Update of the row already written, not as
        // a second insert. The timeline has one reactive dependency, and which
        // action type carried the change must not decide whether it fires.
        let observed = observe(&action_with_id(
            entity_types::ACTIVITY,
            "act-1",
            SyncActionType::Update,
            Some(issue_activity_json()),
        ));

        assert!(
            observed.activities,
            "a coalesced activity moves the row's timestamp, so its position in the \
             timeline changes — the page has to ask again"
        );
        assert!(!observed.attachments);
        assert!(!observed.notification_preferences);
        assert!(!observed.workspace_settings);
    }

    #[wasm_bindgen_test]
    fn an_activity_frame_without_a_payload_reaches_no_reader() {
        // The bug this ticket fixed, stated against the reader rather than
        // against the counter. Both `insert_activity` and
        // `coalesce_or_insert_activity` passed `None` to `commit_and_deliver`,
        // so every ACTIVITY frame on the wire looked like this one: the
        // data-less guard dropped it before the entity match, and the arm below
        // it ran only in the native test above. A test that asserted the arm was
        // entered passed throughout.
        let observed = observe(&action_with_id(
            entity_types::ACTIVITY,
            "act-1",
            SyncActionType::Insert,
            None,
        ));

        assert!(
            !observed.activities,
            "the data-less guard returns before the entity match, so a payload-less \
             activity insert never reaches its arm at all — this is why the server has to \
             send one"
        );
    }

    #[wasm_bindgen_test]
    fn deleting_an_activity_refetches_the_issue_timeline() {
        // No server path deletes an activity on its own today; a cascade from an
        // issue delete emits one, and the timeline of a deleted issue is not on
        // screen. Handling it is what stops a future one from arriving as
        // silence — and a delete carries no payload, so this counter is all
        // there is.
        let observed = observe(&action_with_id(
            entity_types::ACTIVITY,
            "act-1",
            SyncActionType::Delete,
            None,
        ));

        assert!(
            observed.activities,
            "the timeline has one reactive dependency, and which action type carried the \
             change must not decide whether it fires"
        );
        assert!(!observed.attachments);
        assert!(!observed.notification_preferences);
        assert!(!observed.workspace_settings);
    }

    // ── notification_preferences ────────────────────────────────────────────

    #[wasm_bindgen_test]
    fn a_preference_change_refetches_the_notification_settings_page() {
        let observed = observe(&action_with_id(
            entity_types::NOTIFICATION_PREFERENCES,
            "pref-1",
            SyncActionType::Update,
            Some(notification_preferences_json()),
        ));

        assert!(
            observed.notification_preferences,
            "these frames are scoped to one user, so this is that user toggling a preference \
             on another tab or another device — the settings page has to ask again or the \
             two go on disagreeing"
        );
        assert!(!observed.attachments);
        assert!(!observed.activities);
        assert!(!observed.workspace_settings);
    }

    #[wasm_bindgen_test]
    fn deleting_a_preferences_row_refetches_the_notification_settings_page() {
        // No server path deletes a preferences row today. Handling it is what
        // stops one from arriving as silence if that changes.
        let observed = observe(&action_with_id(
            entity_types::NOTIFICATION_PREFERENCES,
            "pref-1",
            SyncActionType::Delete,
            None,
        ));

        assert!(
            observed.notification_preferences,
            "the settings page has one reactive dependency, and which action type carried \
             the change must not decide whether it fires"
        );
        assert!(!observed.attachments);
        assert!(!observed.activities);
        assert!(!observed.workspace_settings);
    }

    // ── workspace_settings ──────────────────────────────────────────────────

    #[wasm_bindgen_test]
    fn a_workspace_rename_refetches_the_workspace_settings_page() {
        let observed = observe(&action_with_id(
            entity_types::WORKSPACE_SETTINGS,
            "ws-1",
            SyncActionType::Update,
            Some(workspace_settings_json()),
        ));

        assert!(
            observed.workspace_settings,
            "the page reads its data through `get_workspace_settings`, not from this store, \
             so without this it shows what it read on mount until it is navigated away from \
             and back — while another admin's rename sits in the cache unseen"
        );
        assert!(!observed.attachments);
        assert!(!observed.activities);
        assert!(!observed.notification_preferences);
    }

    #[wasm_bindgen_test]
    fn deleting_workspace_settings_refetches_the_workspace_settings_page() {
        // As with preferences: settings are only ever updated today, never
        // deleted, and the page must not depend on that staying true.
        let observed = observe(&action_with_id(
            entity_types::WORKSPACE_SETTINGS,
            "ws-1",
            SyncActionType::Delete,
            None,
        ));

        assert!(
            observed.workspace_settings,
            "one reactive dependency, either action type"
        );
        assert!(!observed.attachments);
        assert!(!observed.activities);
        assert!(!observed.notification_preferences);
    }

    // ── release ─────────────────────────────────────────────────────────────

    #[wasm_bindgen_test]
    fn a_release_frame_reaches_no_reader_because_there_is_none() {
        // The other half of the determination `NOT_CACHED` records. Releases are
        // created and listed exclusively through the API and MCP tools; no route,
        // page or server function in this crate reads them. Adding a counter
        // would give this file a subscriber-less signal, which is the same defect
        // with a passing test — so the frame is deliberately left to fall
        // through, and it is not written to the cache either.
        //
        // What a user *can* see of a release does arrive: the same transaction
        // emits an `issue` update for every issue it contains, and those take the
        // ordinary issue path.
        let observed = observe(&action_with_id(
            entity_types::RELEASE,
            "rel-1",
            SyncActionType::Insert,
            Some(serde_json::json!({
                "release_id": "rel-1",
                "name": "v1.2.0",
                "issue_count": 3,
            })),
        ));

        assert!(!observed.attachments);
        assert!(!observed.activities);
        assert!(!observed.notification_preferences);
        assert!(!observed.workspace_settings);
    }

    // ── notification (TRA-9974) ─────────────────────────────────────────────

    #[wasm_bindgen_test]
    fn a_notification_update_frame_reaches_the_store() {
        // The NOTIFICATION update arm and `notification`'s absence from
        // `NOT_CACHED` were both established by reading the code. TRA-9938 and
        // TRA-9940 each found a client arm missing for a type that looked
        // supported that way, so this asserts it instead.
        with_store(|store| {
            // The tab already holds the notification unread, as a bootstrap or
            // an insert frame would have left it.
            let held_before: trakkt_types::models::Notification =
                serde_json::from_value(notification_json(false))
                    .expect("the unread notification the second tab starts with");
            store.upsert_notification(held_before);

            apply_action_to_memory(
                &store,
                &action_with_id(
                    entity_types::NOTIFICATION,
                    "ntf-1",
                    SyncActionType::Update,
                    Some(notification_json(true)),
                ),
            );

            let held = store.notifications().get_untracked();
            assert_eq!(
                held.len(),
                1,
                "the update must replace the row it already held, not append a \
                 second copy of the same notification: {held:?}"
            );
            assert_eq!(held[0].notification_id, "ntf-1");
            assert!(
                held[0].read,
                "the read state has to reach the store — this signal is what the \
                 inbox and the unread badge render, so a frame that stops short \
                 of it leaves the other tab showing the notification unread, \
                 which is the bug TRA-9974 reports"
            );
        });
    }

    #[wasm_bindgen_test]
    fn a_soft_deleted_notification_frame_keeps_the_row_and_stamps_it() {
        // A soft-delete arrives as an `Update` carrying `deleted_at`, not as a
        // `Delete`. That distinction is what `issue_service::delete_issue`'s
        // cascade comment depends on: the row stays in the client's cache, so
        // the cascade that later destroys it still has to send a `Delete`.
        with_store(|store| {
            let live: trakkt_types::models::Notification =
                serde_json::from_value(notification_json(false))
                    .expect("the live notification the tab starts with");
            store.upsert_notification(live);

            let mut dismissed = notification_json(false);
            dismissed["deleted_at"] = serde_json::Value::String("2026-07-26T01:00:00Z".to_owned());

            apply_action_to_memory(
                &store,
                &action_with_id(
                    entity_types::NOTIFICATION,
                    "ntf-1",
                    SyncActionType::Update,
                    Some(dismissed),
                ),
            );

            let held = store.notifications().get_untracked();
            assert_eq!(
                held.len(),
                1,
                "an update never evicts — only the delete arm calls \
                 `remove_notification_in_memory`: {held:?}"
            );
            assert_eq!(
                held[0].deleted_at.as_deref(),
                Some("2026-07-26T01:00:00Z"),
                "the dismissal has to reach the row itself, since that is what \
                 the inbox filters on"
            );
        });
    }
}
