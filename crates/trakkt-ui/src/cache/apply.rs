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
                    // is on `NOT_CACHED`, so no row is written to IndexedDB.
                    // Nothing here would read one back — the `activity` entity
                    // type is named only in this module — and `sync_bootstrap`
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

/// Entity types that arrive on the sync stream and are deliberately **not**
/// written to the local cache.
///
/// [`enqueue_cache_writes`] otherwise persists any action carrying a payload,
/// with no regard for whether anything reads it back. That default is right for
/// every type but the two named here, and both are here for the same reason: a
/// payload arrives, and nothing in this client will ever read the row back.
///
/// # `release`
///
/// `release` has no reader in this client at all: no route, no page, no server
/// function, no hydration step and no on-demand cache read mentions releases —
/// they are created and listed exclusively through the API and MCP tools. Nor
/// could the cached rows serve a future one, because `sync_bootstrap` does not
/// stream releases: a cache filled only by whichever deltas happened to arrive
/// while some tab was open is an arbitrary subset, never a list. The
/// user-visible half of publishing a release does reach the UI, but as the
/// `issue` updates the same transaction emits for every issue it contains.
///
/// # `activity`
///
/// Activities are read, and never from here. Both readers — the issue timeline
/// and the workspace feed — call a server function (`list_issue_activities`,
/// `list_workspace_activities`) when the store's activity counter bumps, so what
/// the frame has to do is bump that counter, which is the memory half's job in
/// [`apply_action_to_memory`]. The payload exists so that arm can run at all: the
/// data-less guard returns before the entity match, which is why every activity
/// frame was dropped before TRA-9987 gave the two write sites in
/// `crates/trakkt-auth/src/activity_service.rs` a real one.
///
/// Persisting the row is the part with no reader. Nothing in `trakkt-ui` reads
/// an activity out of IndexedDB — the `activity` entity type is named only in
/// this module and, until this entry, in `ALL_CACHED_ENTITY_TYPES` — and
/// `sync_bootstrap` streams eleven types without it
/// (`apps/server/src/routes/websocket.rs`), so as with releases the cache could
/// only ever hold whichever deltas happened to arrive while a tab was open.
/// Writing them anyway is unbounded growth in step with every status change,
/// comment and field edit in the workspace, evicted by nothing: this module's
/// delete half queues no cache delete for an activity, so only a full
/// `SyncReset` or a no-cursor cold start clears them.
///
/// # Both halves, together
///
/// Skipping a type here is what lets it come off `ALL_CACHED_ENTITY_TYPES` (see
/// `crates/trakkt-ui/src/cache/sync_engine.rs`) without breaking the invariant
/// that everything the cache can hold is wiped by a reset — the two are checked
/// against each other by
/// `every_entity_type_the_cache_persists_is_wiped_by_a_reset`. The entry and the
/// removal are one change, not two: an entry without the removal promises a wipe
/// for rows that are never written, and a removal without the entry leaves rows
/// nothing ever clears.
///
/// Giving either type a cached reader means undoing both halves: stream it from
/// the bootstrap, drop this entry, and put it back on that array.
const NOT_CACHED: &[&str] = &[entity_types::RELEASE, entity_types::ACTIVITY];

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

            // Types nothing reads back are not written at all. See `NOT_CACHED`.
            if NOT_CACHED.contains(&entity_type) {
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
                et if et == entity_types::PROJECT_MILESTONE => {
                    // Milestone inserts/updates are persisted by the generic
                    // upsert above (bootstrap streams them, and now so does
                    // delta), so the delete has to be persisted too or the
                    // removed row outlives the milestone in the cache.
                    enqueue_delete(writer, entity_types::PROJECT_MILESTONE, entity_id);
                }
                et if et == entity_types::PROJECT_MEMBER => {
                    // A membership add is persisted by the generic upsert above
                    // (it carries a payload), so the remove has to be persisted
                    // too or the removed member outlives the membership in the
                    // cache. Both sides derive the same `project_id:user_id`
                    // entity id, so this deletes exactly the row the add wrote.
                    enqueue_delete(writer, entity_types::PROJECT_MEMBER, entity_id);
                }
                et if et == entity_types::PROJECT_UPDATE => {
                    // Same pairing as above: posted updates are persisted by the
                    // generic upsert, so a delete has to remove the row rather
                    // than leave it behind.
                    enqueue_delete(writer, entity_types::PROJECT_UPDATE, entity_id);
                }
                // Unhandled types are reported by the memory half.
                _ => {}
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

    /// The snapshot `update_workspace_name` and `update_workspace_settings`
    /// both send — the shape `WorkspaceSnapshotRow::into_sync_value` builds.
    #[cfg(target_arch = "wasm32")]
    pub(super) fn workspace_settings_json() -> serde_json::Value {
        serde_json::json!({
            "workspace_id": "ws-1",
            "name": "Renamed Workspace",
            "settings": {"default_auto_archive_days": 30},
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

    /// What the delete path queues for the two types whose deletes it ignores.
    ///
    /// The name and the message both claimed these types have no cached rows.
    /// That is true of `activity`, and is now true by construction rather than by
    /// accident: it used to hold because every activity frame carried a `None`
    /// payload, so the insert path returned before it could persist anything;
    /// TRA-9987 gave those frames a payload and put `activity` on `NOT_CACHED` in
    /// the same change, so the insert path still writes nothing and this delete
    /// still has nothing to remove. `an_activity_upsert_is_not_persisted` is the
    /// half that would fail if that entry were dropped — without it this test
    /// would keep passing while activity rows accumulated unevicted, which is
    /// exactly how a green test hides a leak.
    ///
    /// It remains false of `issue_relation`, whose inserts carry a payload and
    /// are persisted by the generic upsert. Its rows outlive the relation until
    /// the next reset.
    ///
    /// That behaviour is left exactly as it was on purpose. The fix is not a
    /// third arm here: the insert path persists everything with a payload while
    /// this delete path is a hand-written list, which is the same list-drift
    /// `every_entity_type_the_cache_persists_reaches_the_store` was added for,
    /// one level down. Deriving the delete from the persist path, with its own
    /// guard, is its own change — so this records what is true today rather than
    /// half-correcting it.
    #[test]
    fn entity_types_with_no_cached_rows_queue_nothing() {
        for entity_type in [entity_types::ACTIVITY, entity_types::ISSUE_RELATION] {
            assert!(
                cache_ops(&action(entity_type, SyncActionType::Delete, None)).is_empty(),
                "{entity_type}'s delete queues nothing today — for `activity` because \
                 `NOT_CACHED` stops the row being written in the first place, for \
                 `issue_relation` because its rows are persisted and never removed"
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

    #[test]
    fn a_release_upsert_is_not_persisted() {
        // Nothing in this client reads releases back — no route, no page, no
        // hydration step, no on-demand cache read — and no bootstrap streams
        // them, so a cached release row could never be more than an arbitrary
        // fragment. Writing rows nothing reads is the reason `release` is on
        // `NOT_CACHED` and off `ALL_CACHED_ENTITY_TYPES`, and the reason it has
        // no store arm either.
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
    /// The counter is resolved once here, outside the closure, as
    /// `AttachmentsSection` and `NotificationsPage` do it. That is not how
    /// `IssueTimeline` itself reads it today — issue_detail.rs:2358-2360 calls
    /// `s.activities_version()` inside the `Signal::derive` closure, allocating
    /// a fresh owner-registered wrapper per evaluation. That difference is about
    /// where the wrapper is built, not about what the source depends on, so this
    /// rebuild still observes the same counter the page does.
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
}
