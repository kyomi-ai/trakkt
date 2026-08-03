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
///
/// # Contract for every getter on this type
///
/// **Resolve a getter once, at component setup. Never bind one inside a closure
/// that re-runs.**
///
/// Every read-only getter below — the eight collection getters, `initialized()`,
/// and all nine `*_version()` counters — returns a **newly built** [`Signal`] on
/// each call.
/// The underlying value is a long-lived `ArcRwSignal` held by this store, but
/// the `Signal` handed back is a fresh `Signal::derive` wrapper, and a `Signal`
/// is an arena item registered with **whichever owner is current at the moment
/// of the call** (`reactive_graph-0.2.14`, `ArenaItem::new_with_storage` →
/// `Owner::register`). When that owner is cleaned up, the arena slot is removed
/// (`Owner::cleanup`), and any later read of that wrapper panics with "you tried
/// to access a reactive value ... but it has already been disposed".
///
/// The owner does not have to be *torn down* for this to happen. `Effect` and
/// `Memo` both call `Owner::with_cleanup` on **every re-run**
/// (`effect/effect.rs`, `computed/inner.rs`), so a wrapper resolved inside an
/// effect or memo body is disposed the next time that body runs. So is one
/// resolved inside a component that a `Transition`/`Suspense` boundary rebuilds.
/// A wrapper that outlives its resolution point — stored in a struct, captured
/// by a longer-lived closure, or passed as a component prop — is the shape that
/// panicked `/settings/notifications` and `/settings/workspace` and forced the
/// revert of #282.
///
/// Reading a getter *inline* (`store.foo_version().get()`, wrapper built and
/// consumed in one expression) does not panic, because the wrapper never
/// outlives the expression — but it abandons one arena item per evaluation and
/// is one refactor away from the panicking shape, so it is not the form to
/// reach for either.
///
/// The safe form, used by every `*_version()` call site in `pages/` and
/// enforced there by `no_page_resolves_a_version_counter_inline`:
///
/// ```ignore
/// // at component setup, outside every closure:
/// let version = sync_store.map(|s| s.activities_version());
/// // inside the closure that re-runs, read the already-built Signal:
/// let source = move || (team_key.clone(), version.map(|v| v.get()).unwrap_or(0));
/// ```
///
/// Each claim above is checked by the tests in this module's `wasm_tests`.
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

/// Checks for the getter contract documented on [`SyncStore`].
///
/// That contract is the reason `pages/` resolves every `*_version()` counter at
/// component setup rather than inside the closure that reads it. It has now cost
/// three review cycles (#282/#283, TRA-9977, TRA-9991) while living only in
/// review logs and call-site comments, so the claims it makes are checked here,
/// next to the getters they are about.
///
/// Run with:
/// `wasm-pack test --headless --firefox crates/trakkt-ui --lib --features hydrate`
#[cfg(all(test, target_arch = "wasm32"))]
mod wasm_tests {
    use super::*;
    use wasm_bindgen_test::{wasm_bindgen_test, wasm_bindgen_test_configure};

    wasm_bindgen_test_configure!(run_in_browser);

    /// A counter resolved under a subtree's owner dies when that subtree does.
    ///
    /// This is the whole reason the contract exists, and the mechanism behind
    /// the `/settings/notifications` and `/settings/workspace` panics that got
    /// #282 reverted. `Owner::cleanup` is not only a teardown path: `Effect` and
    /// `Memo` run their bodies through `Owner::with_cleanup`, so an owner is
    /// cleaned up on **every re-run**. A wrapper resolved inside such a body and
    /// kept past that run is reading a removed arena slot.
    ///
    /// If this test ever stops panicking, the getters no longer hand back an
    /// owner-scoped wrapper and the contract on [`SyncStore`] is stale — rewrite
    /// it rather than deleting this.
    #[wasm_bindgen_test]
    #[should_panic(expected = "already been disposed")]
    fn a_counter_resolved_under_a_subtree_owner_is_disposed_with_that_subtree() {
        let root = Owner::new();
        root.set();
        let store = SyncStore::new();

        // Stands in for an effect/memo body, or a component inside a suspense
        // boundary — anything that runs under an owner it does not outlive.
        let subtree = Owner::new();
        let counter = subtree.with(|| store.activities_version());

        subtree.cleanup();
        let _ = counter.get_untracked();
    }

    /// The hoisted form survives every rebuild beneath it — the fix, asserted.
    ///
    /// Same store, same getter, same reads; the only difference from the test
    /// above is *where* the getter was called. This is what
    /// `sync_store.map(|s| s.activities_version())` at component setup buys.
    #[wasm_bindgen_test]
    fn a_counter_resolved_at_setup_still_reads_after_the_subtrees_beneath_it_rebuild() {
        let root = Owner::new();
        root.set();
        let store = SyncStore::new();

        // Resolved once, at "page setup".
        let counter = store.activities_version();

        // Three rebuilds of a subtree that reads it.
        let subtree = Owner::new();
        for _ in 0..3 {
            subtree.cleanup();
            subtree.with(|| {
                let _ = counter.get_untracked();
            });
        }

        store.bump_activities_version();
        assert_eq!(
            counter.get_untracked(),
            1,
            "a counter resolved at component setup must keep reading after the subtrees \
             below it are rebuilt. If it does not, every page that keys a Resource on a \
             sync counter stops refetching the moment its suspense boundary rebuilds"
        );
    }

    /// Reading a getter inline does not panic — recorded so it is not re-derived.
    ///
    /// `store.foo_version().get()` builds a wrapper and consumes it in the same
    /// expression, so the wrapper is never read after its owner is cleaned up
    /// and the panic above cannot occur. What it does do is abandon one arena
    /// item per evaluation, under whichever owner is current at the time.
    ///
    /// This is worth an explicit test because the inline form reads at a glance
    /// exactly like the form that *does* panic, and the difference has twice
    /// been mis-stated in review — in both directions. Being safe today is not a
    /// reason to write it: binding the result instead of consuming it inline is
    /// a one-line refactor away from the disposed-value panic, which is why
    /// `pages/` no longer contains the shape.
    #[wasm_bindgen_test]
    fn reading_a_counter_inline_does_not_panic_but_abandons_a_wrapper_per_run() {
        let root = Owner::new();
        root.set();
        let store = SyncStore::new();
        let sync_store = Some(store);

        // The pre-TRA-9991 shape, verbatim.
        let inline = Signal::derive(move || {
            sync_store
                .map(|s| s.activities_version().get())
                .unwrap_or(0)
        });

        let subtree = Owner::new();
        for _ in 0..3 {
            subtree.cleanup();
            assert_eq!(subtree.with(|| inline.get_untracked()), 0);
        }

        store.bump_activities_version();
        assert_eq!(
            subtree.with(|| inline.get_untracked()),
            1,
            "the inline form reads correctly; it is a per-evaluation allocation, not a \
             live panic. Any claim that it panics on its own is wrong and should be \
             checked against this test before it costs another cycle"
        );
    }

    /// Every counter a page keys a `Resource` on moves when its entity syncs.
    ///
    /// TRA-9991 rewrote the source closures of six such resources
    /// (`MetadataSidebar`, `RelationsSection`, `IssueTimeline` in
    /// `pages/issues/issue_detail.rs`; the milestone, update and member
    /// resources in `pages/projects/project_detail.rs`) to read a counter
    /// resolved at setup instead of resolving one inline. That is meant to be
    /// behaviour-preserving, and this is what says so: each source is rebuilt in
    /// the post-fix shape, and a bump of its own counter — and only its own —
    /// must move it.
    #[wasm_bindgen_test]
    fn each_hoisted_page_source_still_moves_when_its_own_counter_bumps() {
        let root = Owner::new();
        root.set();
        let store = SyncStore::new();
        let sync_store = Some(store);

        // The post-fix shape, one per rewritten call site.
        let milestones = sync_store.map(|s| s.milestones_version());
        let relations = sync_store.map(|s| s.relations_version());
        let activities = sync_store.map(|s| s.activities_version());
        let updates = sync_store.map(|s| s.project_updates_version());
        let members = sync_store.map(|s| s.project_members_version());

        let read = move || {
            (
                milestones.map(|v| v.get_untracked()).unwrap_or(0),
                relations.map(|v| v.get_untracked()).unwrap_or(0),
                activities.map(|v| v.get_untracked()).unwrap_or(0),
                updates.map(|v| v.get_untracked()).unwrap_or(0),
                members.map(|v| v.get_untracked()).unwrap_or(0),
            )
        };

        assert_eq!(read(), (0, 0, 0, 0, 0));

        store.bump_milestones_version();
        assert_eq!(
            read(),
            (1, 0, 0, 0, 0),
            "a project_milestone frame must move the milestone sources on the issue \
             sidebar and the project page, and nothing else"
        );

        store.bump_relations_version();
        assert_eq!(
            read(),
            (1, 1, 0, 0, 0),
            "an issue_relation frame must move the relations section's source, and \
             nothing else"
        );

        store.bump_activities_version();
        assert_eq!(
            read(),
            (1, 1, 1, 0, 0),
            "an activity frame must move the issue timeline's source, and nothing else"
        );

        store.bump_project_updates_version();
        assert_eq!(
            read(),
            (1, 1, 1, 1, 0),
            "a project_update frame must move the posted-updates source, and nothing else"
        );

        store.bump_project_members_version();
        assert_eq!(
            read(),
            (1, 1, 1, 1, 1),
            "a project_member frame must move the member-list source, and nothing else"
        );
    }
}

/// Source-level guard for the getter contract documented on [`SyncStore`].
///
/// Runs on the host (`cargo test --workspace`), not in the browser — it reads
/// source text rather than executing anything.
#[cfg(test)]
mod source_guard {
    use std::path::{Path, PathBuf};

    /// No page or component may resolve a `*_version()` counter inline.
    ///
    /// # Why a source check and not a behavioural one
    ///
    /// Because a behavioural one is not possible. The inline form
    /// (`store.foo_version().get()`) and the hoisted form are
    /// runtime-indistinguishable at a call site: the inline wrapper is consumed
    /// in the expression that builds it, so it is never read after its owner is
    /// cleaned up and it cannot raise the disposed-value panic.
    /// `reading_a_counter_inline_does_not_panic_but_abandons_a_wrapper_per_run`
    /// in this file's `wasm_tests` pins that. Reverting any of the six call
    /// sites TRA-9991 changed leaves the whole browser suite green, which is
    /// exactly why the shape kept coming back — nothing but review caught it.
    ///
    /// So what this enforces is a style rule with teeth: keep the fragile form
    /// out of `pages/` and `components/` so the safe form is the only one anyone
    /// copies. The inline form is one refactor away from the panicking shape —
    /// bind its result instead of consuming it, and the wrapper starts
    /// outliving the closure that built it.
    ///
    /// # What it does not catch
    ///
    /// The genuinely dangerous shape: `let v = s.foo_version();` *inside* a
    /// closure that re-runs. That binds the wrapper under a short-lived owner
    /// and keeps it, which is the panic
    /// `a_counter_resolved_under_a_subtree_owner_is_disposed_with_that_subtree`
    /// demonstrates. Detecting it needs closure-scope tracking over Rust source,
    /// which a substring scan cannot do; nothing in the tree writes that shape
    /// today, and the contract on [`SyncStore`] is what stands between it and
    /// the next author. This check is the cheap half, not the whole guard.
    #[test]
    fn no_page_resolves_a_version_counter_inline() {
        // `store.rs` itself is excluded on purpose: its `wasm_tests` module
        // keeps a copy of the banned expression as a pinned counter-example.
        let roots = ["src/pages", "src/components"];
        let mut offenders = Vec::new();

        for root in roots {
            let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(root);
            assert!(
                dir.is_dir(),
                "expected {} to exist — this guard scans nothing if the tree moved, and \
                 a guard that scans nothing passes forever",
                dir.display()
            );
            visit(&dir, &mut offenders);
        }

        assert!(
            offenders.is_empty(),
            "these files resolve a SyncStore version counter inside the expression that \
             reads it:\n  {}\nEach `*_version()` call builds a fresh owner-registered \
             `Signal` wrapper (see the getter contract on `SyncStore`). Resolve it once at \
             component setup instead:\n    let version = sync_store.map(|s| \
             s.activities_version());\nand read `version.map(|v| v.get()).unwrap_or(0)` \
             inside the closure.",
            offenders.join("\n  ")
        );
    }

    fn visit(dir: &Path, offenders: &mut Vec<String>) {
        let entries = std::fs::read_dir(dir)
            .unwrap_or_else(|e| panic!("reading {} while scanning for the inline shape: {e}", dir.display()));
        for entry in entries {
            let entry = entry.unwrap_or_else(|e| panic!("reading an entry under {}: {e}", dir.display()));
            let path = entry.path();
            if path.is_dir() {
                visit(&path, offenders);
            } else if path.extension().is_some_and(|e| e == "rs") {
                let source = std::fs::read_to_string(&path)
                    .unwrap_or_else(|e| panic!("reading {} while scanning for the inline shape: {e}", path.display()));
                // Whitespace-stripped so the multi-line form
                // (`.map(|s| s.project_updates_version()\n.get())`) is caught
                // too — the pre-TRA-9991 `project_detail.rs` was written that
                // way, and a line-by-line scan would have missed it.
                let packed: String = source.split_whitespace().collect();
                if packed.contains("_version().get(") || packed.contains("_version().read(")
                    || packed.contains("_version().with(") || packed.contains("_version().track(")
                {
                    offenders.push(path.display().to_string());
                }
            }
        }
    }
}
