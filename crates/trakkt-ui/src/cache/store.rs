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
//!
//! The store holds no IndexedDB handle and performs no cache write. A UI delete
//! removes the entity from memory here and hands the durable half to
//! [`DeleteRoute`], which reaches the one tab that owns every cache write. See
//! [`crate::cache::delete_route`].

use std::cell::RefCell;

use leptos::prelude::*;
use send_wrapper::SendWrapper;

use trakkt_types::models::{Favorite, IssueWithDetails, Label, Notification, Project, Status, Team, View};
use trakkt_types::sync::entity_types;

use crate::cache::delete_route::DeleteRoute;
use crate::cache::tab_leader::CachedEntity;

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
    /// Version counter bumped when a project_milestone sync action arrives.
    /// Used by the project detail page and the issue metadata sidebar to
    /// trigger a refetch of the milestone list from the server.
    milestones_version: ArcRwSignal<u32>,
    /// Version counter bumped when a project_member sync action arrives.
    /// Used by the project detail page to trigger a refetch of the member list
    /// from the server.
    project_members_version: ArcRwSignal<u32>,
    /// Version counter bumped when a project_update sync action arrives.
    /// Used by the project detail page to trigger a refetch of the posted
    /// status updates from the server.
    project_updates_version: ArcRwSignal<u32>,
    /// Version counter bumped when an attachment or issue_attachment sync
    /// action arrives. Used by the issue detail page's attachment list to
    /// trigger a refetch from the server.
    attachments_version: ArcRwSignal<u32>,
    /// Version counter bumped when a notification_preferences sync action
    /// arrives. Used by the notification settings page to trigger a refetch
    /// from the server.
    notification_preferences_version: ArcRwSignal<u32>,
    /// Version counter bumped when a workspace_settings sync action arrives.
    /// Used by the workspace settings page to trigger a refetch from the
    /// server.
    workspace_settings_version: ArcRwSignal<u32>,
    /// Where this tab's `remove_*` methods send the matching cache delete.
    ///
    /// Set by the Layout once the tab's sync role is known, and set again if a
    /// follower is promoted to leader. Not a signal: nothing renders from it,
    /// and the deletes that read it run from event handlers, not from reactive
    /// closures. See [`DeleteRoute`].
    delete_route: RefCell<DeleteRoute>,
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
                milestones_version: ArcRwSignal::new(0),
                project_members_version: ArcRwSignal::new(0),
                project_updates_version: ArcRwSignal::new(0),
                attachments_version: ArcRwSignal::new(0),
                notification_preferences_version: ArcRwSignal::new(0),
                workspace_settings_version: ArcRwSignal::new(0),
                delete_route: RefCell::new(DeleteRoute::default()),
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

    /// Set where this tab's `remove_*` cache deletes are routed.
    ///
    /// Called by the Layout: to the broadcast channel while this tab is a
    /// follower, and to the writer queue once it holds the leadership lock.
    pub fn set_delete_route(&self, route: DeleteRoute) {
        self.inner
            .with_value(|inner| *inner.delete_route.borrow_mut() = route);
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

    /// Version counter for milestones — bumped on each project_milestone sync
    /// action.
    ///
    /// Milestones are not held in this store: the project detail page and the
    /// issue metadata sidebar both read them straight from the `list_milestones`
    /// server function. This counter is the reactive dependency that tells them
    /// to ask again.
    pub fn milestones_version(&self) -> Signal<u32> {
        let sig = self.inner.with_value(|inner| inner.milestones_version.clone());
        Signal::derive(move || sig.get())
    }

    /// Bump the milestones version counter.
    ///
    /// Called by the sync engine when a "project_milestone" entity type arrives
    /// via WebSocket, signaling that the milestone lists should refetch.
    pub fn bump_milestones_version(&self) {
        self.inner.with_value(|inner| {
            inner.milestones_version.update(|v| *v += 1);
        });
    }

    /// Version counter for project members — bumped on each project_member sync
    /// action.
    ///
    /// Memberships are not held in this store: the project detail page reads
    /// them straight from the `list_project_members` server function. This
    /// counter is the reactive dependency that tells it to ask again.
    pub fn project_members_version(&self) -> Signal<u32> {
        let sig = self
            .inner
            .with_value(|inner| inner.project_members_version.clone());
        Signal::derive(move || sig.get())
    }

    /// Bump the project members version counter.
    ///
    /// Called by the sync engine when a "project_member" entity type arrives via
    /// WebSocket, signaling that the member list should refetch.
    pub fn bump_project_members_version(&self) {
        self.inner.with_value(|inner| {
            inner.project_members_version.update(|v| *v += 1);
        });
    }

    /// Version counter for project status updates — bumped on each
    /// project_update sync action.
    ///
    /// Posted updates are not held in this store: the project detail page reads
    /// them straight from the `list_project_updates` server function. This
    /// counter is the reactive dependency that tells it to ask again.
    pub fn project_updates_version(&self) -> Signal<u32> {
        let sig = self
            .inner
            .with_value(|inner| inner.project_updates_version.clone());
        Signal::derive(move || sig.get())
    }

    /// Bump the project updates version counter.
    ///
    /// Called by the sync engine when a "project_update" entity type arrives via
    /// WebSocket, signaling that the posted update list should refetch.
    pub fn bump_project_updates_version(&self) {
        self.inner.with_value(|inner| {
            inner.project_updates_version.update(|v| *v += 1);
        });
    }

    /// Version counter for attachments — bumped on each attachment and
    /// issue_attachment sync action.
    ///
    /// Attachments are not held in this store: the issue detail page reads them
    /// straight from the `list_issue_attachments` server function. This counter
    /// is the reactive dependency that tells it to ask again.
    ///
    /// One counter covers both entity types because they invalidate exactly one
    /// reader between them — that list changes when an attachment is uploaded or
    /// deleted (`attachment`) and when one is linked to or unlinked from an
    /// issue (`issue_attachment`).
    pub fn attachments_version(&self) -> Signal<u32> {
        let sig = self.inner.with_value(|inner| inner.attachments_version.clone());
        Signal::derive(move || sig.get())
    }

    /// Bump the attachments version counter.
    ///
    /// Called by the sync engine when an "attachment" or "issue_attachment"
    /// entity type arrives via WebSocket, signaling that the issue's attachment
    /// list should refetch.
    pub fn bump_attachments_version(&self) {
        self.inner.with_value(|inner| {
            inner.attachments_version.update(|v| *v += 1);
        });
    }

    /// Version counter for notification preferences — bumped on each
    /// notification_preferences sync action.
    ///
    /// Preferences are not held in this store: the notification settings page
    /// reads them straight from the `get_notification_preferences` server
    /// function. This counter is the reactive dependency that tells it to ask
    /// again — the frames are scoped to a single user, so what it carries is
    /// that user's own change made on another tab or another device.
    pub fn notification_preferences_version(&self) -> Signal<u32> {
        let sig = self
            .inner
            .with_value(|inner| inner.notification_preferences_version.clone());
        Signal::derive(move || sig.get())
    }

    /// Bump the notification preferences version counter.
    ///
    /// Called by the sync engine when a "notification_preferences" entity type
    /// arrives via WebSocket, signaling that the settings page should refetch.
    pub fn bump_notification_preferences_version(&self) {
        self.inner.with_value(|inner| {
            inner.notification_preferences_version.update(|v| *v += 1);
        });
    }

    /// Version counter for workspace settings — bumped on each
    /// workspace_settings sync action.
    ///
    /// The workspace settings page reads its data through its own
    /// `get_workspace_settings` server function rather than from this store, so
    /// this counter is the reactive dependency that tells it to ask again after
    /// another admin renames the workspace or changes its auto-archive default.
    pub fn workspace_settings_version(&self) -> Signal<u32> {
        let sig = self
            .inner
            .with_value(|inner| inner.workspace_settings_version.clone());
        Signal::derive(move || sig.get())
    }

    /// Bump the workspace settings version counter.
    ///
    /// Called by the sync engine when a "workspace_settings" entity type
    /// arrives via WebSocket, signaling that the settings page should refetch.
    pub fn bump_workspace_settings_version(&self) {
        self.inner.with_value(|inner| {
            inner.workspace_settings_version.update(|v| *v += 1);
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

    /// Route the removal of `entities` to the tab that owns the shared cache.
    ///
    /// The store never writes IndexedDB itself: one tab per browser owns every
    /// cache write, and it may not be this one. See [`DeleteRoute`].
    fn delete_from_cache(&self, entities: Vec<CachedEntity>) {
        // Cloned out before dispatching so the route cannot be borrowed across
        // the call — the sink reaches transports this store knows nothing about.
        let route = self
            .inner
            .with_value(|inner| inner.delete_route.borrow().clone());
        route.delete(entities);
    }

    // ── Single-item removes, memory only (sync engine) ───────────────────────
    //
    // The sync engine enqueues the matching cache deletes on the writer queue
    // itself, so they stay ordered against the cursor that claims to cover them.
    // It calls these memory-only variants for the other half.
    //
    // Followers call them too, from the leader's broadcast: a follower updates
    // memory and nothing else.

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
    //
    // Both halves of a delete the user just clicked: the in-memory removal
    // lands here and now, so the list updates without waiting on anything, and
    // the cache delete is routed to the tab that owns the shared cache.
    //
    // Only entity types the UI actually deletes have a method here. The sync
    // stream's own deletes do not come through these — they are applied by
    // `cache::apply`, which pairs `remove_*_in_memory` above with a write it
    // enqueues on the leader's queue itself.

    /// Remove a team by `team_id`, from memory and from the shared cache.
    ///
    /// Used by both "delete team" and "leave team". The latter is why the cache
    /// delete has to be durable: leaving a team emits a `TEAM`/`Update` sync
    /// action, never a delete, so nothing else will ever evict the row.
    pub fn remove_team(&self, team_id: &str) {
        self.remove_team_in_memory(team_id);
        self.delete_from_cache(vec![CachedEntity::new(entity_types::TEAM, team_id)]);
    }

    /// Remove a project by `project_id`, from memory and from the shared cache.
    pub fn remove_project(&self, project_id: &str) {
        self.remove_project_in_memory(project_id);
        self.delete_from_cache(vec![CachedEntity::new(entity_types::PROJECT, project_id)]);
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
            inner.milestones_version.set(0);
            inner.project_members_version.set(0);
            inner.project_updates_version.set(0);
            inner.attachments_version.set(0);
            inner.notification_preferences_version.set(0);
            inner.workspace_settings_version.set(0);
        });
    }
}
