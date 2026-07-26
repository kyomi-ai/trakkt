// SPDX-License-Identifier: AGPL-3.0-or-later

//! Reactive in-memory store for synced metadata.
//!
//! `SyncStore` is the single source of truth for list pages (issues, labels,
//! teams, projects).
//! It is populated from IndexedDB on startup (hydration) and kept current by
//! the sync engine as delta events arrive over WebSocket.
//!
//! The store itself is **not** cfg-gated to `wasm32`. Page components that
//! read from it compile on both SSR and CSR targets; they receive empty vectors
//! on SSR and wait for `initialized()` to become `true` on the client. Only the
//! IndexedDB hydration call-site is wasm32-only.

use leptos::prelude::*;
use send_wrapper::SendWrapper;

use trakkt_types::models::{Favorite, IssueWithDetails, Label, Notification, Project, Status, Team, View};

#[cfg(target_arch = "wasm32")]
use leptos::task::spawn_local;
#[cfg(target_arch = "wasm32")]
use trakkt_types::sync::entity_types;

// ── Inner storage ─────────────────────────────────────────────────────────────

/// Non-Clone, non-Send inner storage for the store's reactive signals.
///
/// Wrapped in `SendWrapper` so it can be placed in a `StoredValue`, which
/// requires `Send + Sync` even though this crate only ever runs on WASM
/// (single-threaded).
struct SyncStoreInner {
    issues: ArcRwSignal<Vec<IssueWithDetails>>,
    labels: ArcRwSignal<Vec<Label>>,
    statuses: ArcRwSignal<Vec<Status>>,
    teams: ArcRwSignal<Vec<Team>>,
    projects: ArcRwSignal<Vec<Project>>,
    views: ArcRwSignal<Vec<View>>,
    favorites: ArcRwSignal<Vec<Favorite>>,
    notifications: ArcRwSignal<Vec<Notification>>,
    initialized: ArcRwSignal<bool>,
    /// Version counter bumped when an activity sync action arrives.
    /// Used by the timeline component to trigger a refetch.
    activities_version: ArcRwSignal<u32>,
    /// Version counter bumped when an issue_relation sync action arrives.
    /// Used by the relations section to trigger a refetch.
    relations_version: ArcRwSignal<u32>,
    /// Version counter bumped when a comment sync action arrives.
    /// Used by the detail page to trigger a re-read from IndexedDB.
    comments_version: ArcRwSignal<u32>,
    /// Workspace ID for IndexedDB operations.
    ///
    /// Set once after the user context resolves. `remove_*` methods use this
    /// to delete the corresponding IDB entry so stale data is not hydrated
    /// on page refresh.
    workspace_id: ArcRwSignal<Option<String>>,
}

// ── Public handle ─────────────────────────────────────────────────────────────

/// Reactive in-memory store for synced metadata.
///
/// Cheaply `Copy`able — the actual data lives behind a `StoredValue`.
/// Provide at the `Layout` level with [`provide_context`] and access on any
/// child page with `expect_context::<SyncStore>()`.
#[derive(Clone, Copy)]
pub struct SyncStore {
    inner: StoredValue<SendWrapper<SyncStoreInner>>,
}

impl Default for SyncStore {
    fn default() -> Self {
        Self::new()
    }
}

impl SyncStore {
    /// Create a new, empty `SyncStore`.
    ///
    /// All lists start empty and `initialized` starts `false`. The Layout
    /// hydration effect fills the store from IndexedDB once the workspace ID
    /// is available, then marks it initialized.
    pub fn new() -> Self {
        Self {
            inner: StoredValue::new(SendWrapper::new(SyncStoreInner {
                issues: ArcRwSignal::new(Vec::new()),
                labels: ArcRwSignal::new(Vec::new()),
                statuses: ArcRwSignal::new(Vec::new()),
                teams: ArcRwSignal::new(Vec::new()),
                projects: ArcRwSignal::new(Vec::new()),
                views: ArcRwSignal::new(Vec::new()),
                favorites: ArcRwSignal::new(Vec::new()),
                notifications: ArcRwSignal::new(Vec::new()),
                initialized: ArcRwSignal::new(false),
                activities_version: ArcRwSignal::new(0),
                relations_version: ArcRwSignal::new(0),
                comments_version: ArcRwSignal::new(0),
                workspace_id: ArcRwSignal::new(None),
            })),
        }
    }

    // ── Read-only derived signals ─────────────────────────────────────────────

    /// Reactive signal over the current issue list.
    pub fn issues(&self) -> Signal<Vec<IssueWithDetails>> {
        let sig = self.inner.with_value(|inner| inner.issues.clone());
        Signal::derive(move || sig.get())
    }

    /// Reactive signal over the current label list.
    pub fn labels(&self) -> Signal<Vec<Label>> {
        let sig = self.inner.with_value(|inner| inner.labels.clone());
        Signal::derive(move || sig.get())
    }

    /// Reactive signal over the current status list.
    pub fn statuses(&self) -> Signal<Vec<Status>> {
        let sig = self.inner.with_value(|inner| inner.statuses.clone());
        Signal::derive(move || sig.get())
    }

    /// Reactive signal over the current team list.
    pub fn teams(&self) -> Signal<Vec<Team>> {
        let sig = self.inner.with_value(|inner| inner.teams.clone());
        Signal::derive(move || sig.get())
    }

    /// Reactive signal over the current project list.
    pub fn projects(&self) -> Signal<Vec<Project>> {
        let sig = self.inner.with_value(|inner| inner.projects.clone());
        Signal::derive(move || sig.get())
    }

    /// Reactive signal over the current views list.
    pub fn views(&self) -> Signal<Vec<View>> {
        let sig = self.inner.with_value(|inner| inner.views.clone());
        Signal::derive(move || sig.get())
    }

    /// Reactive signal over the current favorites list.
    pub fn favorites(&self) -> Signal<Vec<Favorite>> {
        let sig = self.inner.with_value(|inner| inner.favorites.clone());
        Signal::derive(move || sig.get())
    }

    /// Reactive signal over the current notifications list.
    pub fn notifications(&self) -> Signal<Vec<Notification>> {
        let sig = self.inner.with_value(|inner| inner.notifications.clone());
        Signal::derive(move || sig.get())
    }

    /// `true` once the store has been hydrated from IndexedDB.
    ///
    /// Pages that read from this store should show a loading state until this
    /// signal becomes `true`.
    pub fn initialized(&self) -> Signal<bool> {
        let sig = self.inner.with_value(|inner| inner.initialized.clone());
        Signal::derive(move || sig.get())
    }

    /// Set the workspace ID used for IndexedDB operations.
    ///
    /// Called once from the Layout after the user context resolves. Until
    /// this is set, `remove_*` methods skip the IDB delete.
    pub fn set_workspace_id(&self, id: String) {
        self.inner.with_value(|inner| inner.workspace_id.set(Some(id)));
    }

    /// Read the current workspace ID, if set.
    pub fn workspace_id(&self) -> Option<String> {
        self.inner.with_value(|inner| inner.workspace_id.get_untracked())
    }

    /// Version counter for activities — bumped on each activity sync action.
    ///
    /// The issue timeline component uses this as a reactive dependency to
    /// trigger a refetch of activities from the server.
    pub fn activities_version(&self) -> Signal<u32> {
        let sig = self.inner.with_value(|inner| inner.activities_version.clone());
        Signal::derive(move || sig.get())
    }

    /// Bump the activities version counter.
    ///
    /// Called by the sync engine when an "activity" entity type arrives via
    /// WebSocket, signaling that the timeline should refetch.
    pub fn bump_activities_version(&self) {
        self.inner.with_value(|inner| {
            inner.activities_version.update(|v| *v += 1);
        });
    }

    /// Version counter for relations — bumped on each issue_relation sync action.
    ///
    /// The relations section component uses this as a reactive dependency to
    /// trigger a refetch of relations from the server.
    pub fn relations_version(&self) -> Signal<u32> {
        let sig = self.inner.with_value(|inner| inner.relations_version.clone());
        Signal::derive(move || sig.get())
    }

    /// Bump the relations version counter.
    ///
    /// Called by the sync engine when an "issue_relation" entity type arrives
    /// via WebSocket, signaling that the relations section should refetch.
    pub fn bump_relations_version(&self) {
        self.inner.with_value(|inner| {
            inner.relations_version.update(|v| *v += 1);
        });
    }

    /// Version counter for comments — bumped on each comment sync action.
    ///
    /// The issue detail page uses this as a reactive dependency to trigger
    /// a re-read of comments from IndexedDB.
    pub fn comments_version(&self) -> Signal<u32> {
        let sig = self.inner.with_value(|inner| inner.comments_version.clone());
        Signal::derive(move || sig.get())
    }

    /// Bump the comments version counter.
    ///
    /// Called by the sync engine when a "comment" entity type arrives via
    /// WebSocket, signaling that the detail page should re-read from IDB.
    pub fn bump_comments_version(&self) {
        self.inner.with_value(|inner| {
            inner.comments_version.update(|v| *v += 1);
        });
    }

    // ── Bulk setters (bootstrap / hydration) ─────────────────────────────────

    /// Replace the entire issue list (called during IDB hydration).
    pub fn set_issues(&self, items: Vec<IssueWithDetails>) {
        self.inner.with_value(|inner| inner.issues.set(items));
    }

    /// Replace the entire label list (called during IDB hydration).
    pub fn set_labels(&self, items: Vec<Label>) {
        self.inner.with_value(|inner| inner.labels.set(items));
    }

    /// Replace the entire status list (called during IDB hydration).
    pub fn set_statuses(&self, items: Vec<Status>) {
        self.inner.with_value(|inner| inner.statuses.set(items));
    }

    /// Replace the entire team list (called during IDB hydration).
    pub fn set_teams(&self, items: Vec<Team>) {
        self.inner.with_value(|inner| inner.teams.set(items));
    }

    /// Replace the entire project list (called during IDB hydration).
    pub fn set_projects(&self, items: Vec<Project>) {
        self.inner.with_value(|inner| inner.projects.set(items));
    }

    /// Replace the entire views list (called during IDB hydration).
    pub fn set_views(&self, items: Vec<View>) {
        self.inner.with_value(|inner| inner.views.set(items));
    }

    /// Replace the entire favorites list (called during IDB hydration).
    pub fn set_favorites(&self, items: Vec<Favorite>) {
        self.inner.with_value(|inner| inner.favorites.set(items));
    }

    /// Replace the entire notifications list (called during IDB hydration).
    pub fn set_notifications(&self, items: Vec<Notification>) {
        self.inner.with_value(|inner| inner.notifications.set(items));
    }

    // ── Single-item upserts (live sync) ───────────────────────────────────────

    /// Insert or update an issue by `issue_id`.
    pub fn upsert_issue(&self, item: IssueWithDetails) {
        self.inner.with_value(|inner| {
            inner.issues.update(|list| {
                if let Some(existing) = list.iter_mut().find(|i| i.issue_id == item.issue_id) {
                    *existing = item;
                } else {
                    list.push(item);
                }
            });
        });
    }

    /// Insert or update a label by `label_id`.
    pub fn upsert_label(&self, item: Label) {
        self.inner.with_value(|inner| {
            inner.labels.update(|list| {
                if let Some(existing) = list.iter_mut().find(|l| l.label_id == item.label_id) {
                    *existing = item;
                } else {
                    list.push(item);
                }
            });
        });
    }

    /// Insert or update a status by `status_id`.
    pub fn upsert_status(&self, item: Status) {
        self.inner.with_value(|inner| {
            inner.statuses.update(|list| {
                if let Some(existing) = list.iter_mut().find(|s| s.status_id == item.status_id) {
                    *existing = item;
                } else {
                    list.push(item);
                }
            });
        });
    }

    /// Insert or update a team by `team_id`.
    pub fn upsert_team(&self, item: Team) {
        self.inner.with_value(|inner| {
            inner.teams.update(|list| {
                if let Some(existing) = list.iter_mut().find(|t| t.team_id == item.team_id) {
                    *existing = item;
                } else {
                    list.push(item);
                }
            });
        });
    }

    /// Insert or update a project by `project_id`.
    pub fn upsert_project(&self, item: Project) {
        self.inner.with_value(|inner| {
            inner.projects.update(|list| {
                if let Some(existing) = list.iter_mut().find(|p| p.project_id == item.project_id) {
                    *existing = item;
                } else {
                    list.push(item);
                }
            });
        });
    }

    /// Insert or update a view by `view_id`.
    pub fn upsert_view(&self, item: View) {
        self.inner.with_value(|inner| {
            inner.views.update(|list| {
                if let Some(existing) = list.iter_mut().find(|v| v.view_id == item.view_id) {
                    *existing = item;
                } else {
                    list.push(item);
                }
            });
        });
    }

    /// Insert or update a favorite by `favorite_id`.
    pub fn upsert_favorite(&self, item: Favorite) {
        self.inner.with_value(|inner| {
            inner.favorites.update(|list| {
                if let Some(existing) = list.iter_mut().find(|f| f.favorite_id == item.favorite_id) {
                    *existing = item;
                } else {
                    list.push(item);
                }
            });
        });
    }

    /// Insert or update a notification by `notification_id`.
    pub fn upsert_notification(&self, item: Notification) {
        self.inner.with_value(|inner| {
            inner.notifications.update(|list| {
                if let Some(existing) = list.iter_mut().find(|n| n.notification_id == item.notification_id) {
                    *existing = item;
                } else {
                    list.push(item);
                }
            });
        });
    }

    /// Spawn an async task to delete an entity from IndexedDB.
    ///
    /// No-op when `workspace_id` has not been set yet (SSR or pre-auth).
    /// On WASM, opens the IDB cache and deletes the key. The delete is
    /// idempotent — removing a non-existent key is a silent no-op.
    #[cfg(target_arch = "wasm32")]
    fn delete_from_idb(&self, entity_type: &'static str, entity_id: &str) {
        let Some(wid) = self.workspace_id() else {
            return;
        };
        let eid = entity_id.to_owned();
        spawn_local(async move {
            match super::db::init_cache_db(&wid).await {
                Ok(cache_db) => {
                    if let Err(e) = super::db::delete(&cache_db, entity_type, &eid, &wid).await {
                        tracing::warn!(
                            entity_type,
                            entity_id = %eid,
                            "SyncStore: IDB delete failed: {e}"
                        );
                    }
                }
                Err(e) => {
                    tracing::warn!("SyncStore: failed to open cache db for delete: {e}");
                }
            }
        });
    }

    // ── Single-item removes, memory only (sync engine) ───────────────────────
    //
    // The sync engine must not let the store spawn its own IndexedDB delete:
    // that write would race the sync cursor, which is exactly the durability
    // hole the FIFO writer queue exists to close. It calls these memory-only
    // variants and enqueues the matching deletes on the writer queue itself, so
    // they stay ordered with every other cache write.
    //
    // UI-initiated deletes keep using the `remove_*` methods below, which pair
    // the memory removal with the IndexedDB delete exactly as before.

    /// Remove an issue from the in-memory list only.
    pub fn remove_issue_in_memory(&self, issue_id: &str) {
        self.inner.with_value(|inner| {
            inner.issues.update(|list| {
                list.retain(|i| i.issue_id != issue_id);
            });
        });
    }

    /// Remove a label from the in-memory list only.
    pub fn remove_label_in_memory(&self, label_id: &str) {
        self.inner.with_value(|inner| {
            inner.labels.update(|list| {
                list.retain(|l| l.label_id != label_id);
            });
        });
    }

    /// Remove a status from the in-memory list only.
    pub fn remove_status_in_memory(&self, status_id: &str) {
        self.inner.with_value(|inner| {
            inner.statuses.update(|list| {
                list.retain(|s| s.status_id != status_id);
            });
        });
    }

    /// Remove a team from the in-memory list only.
    pub fn remove_team_in_memory(&self, team_id: &str) {
        self.inner.with_value(|inner| {
            inner.teams.update(|list| {
                list.retain(|t| t.team_id != team_id);
            });
        });
    }

    /// Remove a project from the in-memory list only.
    pub fn remove_project_in_memory(&self, project_id: &str) {
        self.inner.with_value(|inner| {
            inner.projects.update(|list| {
                list.retain(|p| p.project_id != project_id);
            });
        });
    }

    /// Remove a view from the in-memory list only.
    pub fn remove_view_in_memory(&self, view_id: &str) {
        self.inner.with_value(|inner| {
            inner.views.update(|list| {
                list.retain(|v| v.view_id != view_id);
            });
        });
    }

    /// Remove a favorite from the in-memory list only.
    pub fn remove_favorite_in_memory(&self, favorite_id: &str) {
        self.inner.with_value(|inner| {
            inner.favorites.update(|list| {
                list.retain(|f| f.favorite_id != favorite_id);
            });
        });
    }

    /// Remove a notification from the in-memory list only.
    pub fn remove_notification_in_memory(&self, notification_id: &str) {
        self.inner.with_value(|inner| {
            inner.notifications.update(|list| {
                list.retain(|n| n.notification_id != notification_id);
            });
        });
    }

    // ── Single-item removes (UI-initiated deletes) ───────────────────────────

    /// Remove an issue by `issue_id`, and from IndexedDB.
    pub fn remove_issue(&self, issue_id: &str) {
        self.remove_issue_in_memory(issue_id);
        #[cfg(target_arch = "wasm32")]
        self.delete_from_idb(entity_types::ISSUE, issue_id);
        #[cfg(target_arch = "wasm32")]
        self.delete_from_idb(entity_types::ISSUE_CONTENT, issue_id);
    }

    /// Remove a label by `label_id`, and from IndexedDB.
    pub fn remove_label(&self, label_id: &str) {
        self.remove_label_in_memory(label_id);
        #[cfg(target_arch = "wasm32")]
        self.delete_from_idb(entity_types::LABEL, label_id);
    }

    /// Remove a status by `status_id`, and from IndexedDB.
    pub fn remove_status(&self, status_id: &str) {
        self.remove_status_in_memory(status_id);
        #[cfg(target_arch = "wasm32")]
        self.delete_from_idb(entity_types::STATUS, status_id);
    }

    /// Remove a team by `team_id`, and from IndexedDB.
    pub fn remove_team(&self, team_id: &str) {
        self.remove_team_in_memory(team_id);
        #[cfg(target_arch = "wasm32")]
        self.delete_from_idb(entity_types::TEAM, team_id);
    }

    /// Remove a project by `project_id`, and from IndexedDB.
    pub fn remove_project(&self, project_id: &str) {
        self.remove_project_in_memory(project_id);
        #[cfg(target_arch = "wasm32")]
        self.delete_from_idb(entity_types::PROJECT, project_id);
    }

    /// Remove a view by `view_id`, and from IndexedDB.
    pub fn remove_view(&self, view_id: &str) {
        self.remove_view_in_memory(view_id);
        #[cfg(target_arch = "wasm32")]
        self.delete_from_idb(entity_types::VIEW, view_id);
    }

    /// Remove a favorite by `favorite_id`, and from IndexedDB.
    pub fn remove_favorite(&self, favorite_id: &str) {
        self.remove_favorite_in_memory(favorite_id);
        #[cfg(target_arch = "wasm32")]
        self.delete_from_idb(entity_types::FAVORITE, favorite_id);
    }

    /// Remove a notification by `notification_id`, and from IndexedDB.
    pub fn remove_notification(&self, notification_id: &str) {
        self.remove_notification_in_memory(notification_id);
        #[cfg(target_arch = "wasm32")]
        self.delete_from_idb(entity_types::NOTIFICATION, notification_id);
    }

    /// Remove a comment by `comment_id` (deletes from IndexedDB).
    ///
    /// Comments are not held in memory — the detail page reads them from
    /// IndexedDB on demand.
    pub fn remove_comment(&self, _comment_id: &str) {
        #[cfg(target_arch = "wasm32")]
        self.delete_from_idb(entity_types::COMMENT, _comment_id);
    }

    // ── State transitions ─────────────────────────────────────────────────────

    /// Mark the store as fully hydrated from IndexedDB.
    ///
    /// Called after all entity types have been read from IDB (or after the sync
    /// engine confirms an existing cursor). Pages waiting on `initialized()`
    /// will update reactively.
    pub fn set_initialized(&self, value: bool) {
        self.inner.with_value(|inner| inner.initialized.set(value));
    }

    /// Clear all lists and reset initialized to false.
    ///
    /// Called before hydrating from a different workspace's cache so stale
    /// data from the previous workspace doesn't leak into the new one.
    pub fn reset(&self) {
        self.inner.with_value(|inner| {
            inner.issues.set(Vec::new());
            inner.labels.set(Vec::new());
            inner.statuses.set(Vec::new());
            inner.teams.set(Vec::new());
            inner.projects.set(Vec::new());
            inner.views.set(Vec::new());
            inner.favorites.set(Vec::new());
            inner.notifications.set(Vec::new());
            inner.initialized.set(false);
            inner.activities_version.set(0);
            inner.relations_version.set(0);
            inner.comments_version.set(0);
        });
    }
}
