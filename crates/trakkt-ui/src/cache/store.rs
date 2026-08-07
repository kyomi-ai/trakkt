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
    /// data from the previous workspace doesn't leak into the new one, and on
    /// the cursor-less connect path in [`crate::cache::sync_engine`] before
    /// every fresh bootstrap.
    ///
    /// That second caller is why the eight collections go through
    /// [`clear_if_populated`] and the nine version counters through
    /// [`rewind_to_zero`], rather than `set`: on a fresh bootstrap they are
    /// already empty and already `0`, and `set` notifies whether or not the
    /// value moved. See [`rewind_to_zero`] for the mechanism and for what an
    /// unchanged-value notification costs the pages subscribed to one.
    ///
    /// `initialized` is the one write here still on `set`. On the cursor-less
    /// bootstrap path it genuinely moves: hydration sets it `true`
    /// (`sync_engine::hydrate_store_from_db`, called by
    /// `sync_engine::hydrate_then_open_gate`, which opens the gate only on its
    /// way out) and the socket is not dialled until that gate opens
    /// (`sync_engine::dial_when_hydrated`), so `reset` always finds it `true`
    /// there. It is reachable as a no-op only through a `SyncReset` or a
    /// follower `Reset` arriving while it is already `false`.
    pub fn reset(&self) {
        self.inner.with_value(|inner| {
            clear_if_populated(&inner.issues);
            clear_if_populated(&inner.labels);
            clear_if_populated(&inner.statuses);
            clear_if_populated(&inner.teams);
            clear_if_populated(&inner.projects);
            clear_if_populated(&inner.views);
            clear_if_populated(&inner.favorites);
            clear_if_populated(&inner.notifications);
            inner.initialized.set(false);
            rewind_to_zero(&inner.activities_version);
            rewind_to_zero(&inner.relations_version);
            rewind_to_zero(&inner.comments_version);
            rewind_to_zero(&inner.milestones_version);
            rewind_to_zero(&inner.project_members_version);
            rewind_to_zero(&inner.project_updates_version);
            rewind_to_zero(&inner.attachments_version);
            rewind_to_zero(&inner.notification_preferences_version);
            rewind_to_zero(&inner.workspace_settings_version);
        });
    }
}

/// Rewind a version counter to zero, waking its subscribers only if it moved.
///
/// # Why not `set(0)`
///
/// `set` notifies unconditionally — it never compares against the value already
/// there. In `reactive_graph-0.2.14`, `Set::set` is `try_update(|n| *n = value)`,
/// `try_update` is `try_maybe_update(|val| (true, fun(val)))`, and that `true`
/// leaves the write guard's `triggerable` in place, so `WriteGuard::drop` calls
/// `notify()` (`traits.rs`, `signal/guards.rs`).
///
/// `maybe_update` is the same write with the notification made conditional: a
/// `false` return calls `untrack()` on the guard, which *takes* the triggerable,
/// and `Drop` then finds nothing to notify. The nine counters are `u32`, so
/// "did it move" is a comparison against `0`.
///
/// # What an unchanged-value notification costs
///
/// Two things, both measured against this tree rather than assumed:
///
/// - A `LocalResource` has no separate source argument, so it subscribes
///   directly to whatever its fetcher reads and its `AsyncDerived` is marked
///   *dirty* by the notification — it refetches. `NotificationsPage`
///   (`pages/settings/notifications.rs`) tracks
///   `notification_preferences_version` inside its fetcher, so an unchanged
///   notification there is a real `get_notification_preferences` round trip.
/// - An `Effect` re-runs its whole body on notification, value unchanged or
///   not. `IssueDetailContent`'s comments effect (`pages/issues/issue_detail.rs`)
///   opens IndexedDB and re-reads every comment of the issue from it.
///
/// `Resource::new` is the case that does *not* refetch, and this is the part to
/// keep rather than round off: `ArcResource::new_with_options`
/// (`leptos_server-0.8.7`) wraps the caller's source closure in an `ArcMemo`,
/// and a memo whose recomputed value compares equal stops the propagation there
/// — `update_if_necessary` returns `false` (`reactive_graph-0.2.14`,
/// `computed/inner.rs`), so the `AsyncDerived` woken by the notification finds
/// nothing to do. The memo's body still re-runs and the async task still wakes;
/// the fetcher does not. That covers the `Resource::new` call sites in
/// `pages/settings/workspace.rs`, `pages/projects/project_detail.rs` and
/// `pages/issues/issue_detail.rs`.
///
/// So TRA-9984's "a wasted server round trip per subscribed page per bootstrap"
/// holds for the two shapes above and overstates the third. Do not simplify this
/// back into the blanket claim: the reason to guard the write is not that every
/// subscriber pays a round trip, it is that the store cannot know which shape is
/// listening.
fn rewind_to_zero(counter: &ArcRwSignal<u32>) {
    counter.maybe_update(|value| {
        let moved = *value != 0;
        *value = 0;
        moved
    });
}

/// Empty one of the store's collections, waking its subscribers only if there
/// was something in it.
///
/// The counterpart to [`rewind_to_zero`] for the eight collections
/// [`SyncStore::reset`] clears, and it exists for exactly the same reason: on
/// the cursor-less connect path `reset` runs before every fresh bootstrap, and
/// on a first page load the lists are already empty when it does — hydration
/// finishes before the socket is dialled, and an empty cache hydrates to empty
/// lists. `set(Vec::new())` notified every list page anyway. See
/// [`rewind_to_zero`] for the `maybe_update` mechanism and for the measured
/// cost of an unchanged-value notification, including the part that is *not*
/// true of every subscriber:
///
/// - A `LocalResource` subscribes directly to what its fetcher reads and is
///   marked dirty by the notification, so it really does refetch from the
///   server (`leptos_server-0.8.7`, `ArcLocalResource::new`, which passes the
///   fetcher to `ArcAsyncDerived::new_unsync` with no separate source).
/// - An `Effect` re-runs its whole body on notification regardless of value.
/// - A `Resource::new` does **not** refetch: `ArcResource::new_with_options`
///   (`leptos_server-0.8.7`) wraps the caller's source closure in an `ArcMemo`,
///   and a memo whose recomputed value compares equal stops the propagation
///   there (`reactive_graph-0.2.14`, `computed/inner.rs`, `update_if_necessary`
///   returns `false`). Its body re-runs and its async task wakes; the fetcher
///   does not. The ticket's "a wasted server round trip per subscribed page"
///   is therefore true of the first two shapes and an overstatement for the
///   third — do not restore it to the blanket claim.
///
/// # Why `!is_empty()` and not `PartialEq`
///
/// The value written is always an empty `Vec`, so "did it move" is exactly "was
/// it non-empty". That needs no `PartialEq` on the element type — which matters,
/// because comparing the old and new lists would mean an element-wise compare of
/// every issue in the workspace on a path whose whole point is to do less work.
///
/// The write is `*list = Vec::new()` rather than `list.clear()` so the old
/// allocation is dropped exactly as `set(Vec::new())` dropped it. `clear` would
/// retain the capacity, which is a different memory profile across a workspace
/// switch than the code this replaced.
fn clear_if_populated<T>(collection: &ArcRwSignal<Vec<T>>)
where
    T: Send + Sync + 'static,
{
    collection.maybe_update(|list| {
        let moved = !list.is_empty();
        *list = Vec::new();
        moved
    });
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
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU32, Ordering};

    use super::*;
    use wasm_bindgen_test::{wasm_bindgen_test, wasm_bindgen_test_configure};

    wasm_bindgen_test_configure!(run_in_browser);

    // ── Probing what `reset()` wakes ────────────────────────────────────────

    /// A `*_version()` getter, as a value so the nine can be walked in a loop.
    type CounterGetter = fn(&SyncStore) -> Signal<u32>;
    /// A `bump_*_version()` method, likewise.
    type CounterBump = fn(&SyncStore);

    /// The nine version counters [`SyncStore::reset`] rewinds.
    ///
    /// Each entry is the getter a page resolves at setup and the bump the sync
    /// engine calls when that entity's frame arrives. All nine are listed so a
    /// fix applied to eight of them fails, naming the one that was missed.
    const COUNTERS: [(&str, CounterGetter, CounterBump); 9] = [
        (
            "activities_version",
            SyncStore::activities_version,
            SyncStore::bump_activities_version,
        ),
        (
            "relations_version",
            SyncStore::relations_version,
            SyncStore::bump_relations_version,
        ),
        (
            "comments_version",
            SyncStore::comments_version,
            SyncStore::bump_comments_version,
        ),
        (
            "milestones_version",
            SyncStore::milestones_version,
            SyncStore::bump_milestones_version,
        ),
        (
            "project_members_version",
            SyncStore::project_members_version,
            SyncStore::bump_project_members_version,
        ),
        (
            "project_updates_version",
            SyncStore::project_updates_version,
            SyncStore::bump_project_updates_version,
        ),
        (
            "attachments_version",
            SyncStore::attachments_version,
            SyncStore::bump_attachments_version,
        ),
        (
            "notification_preferences_version",
            SyncStore::notification_preferences_version,
            SyncStore::bump_notification_preferences_version,
        ),
        (
            "workspace_settings_version",
            SyncStore::workspace_settings_version,
            SyncStore::bump_workspace_settings_version,
        ),
    ];

    /// One subscriber attached to one counter, plus a way to read that counter
    /// without going through it.
    struct Probe {
        name: &'static str,
        /// How many times the subscriber's body has run.
        ///
        /// Not a signal: an instrument that is itself an arena item is
        /// reachable by the very notifications under test. `SaveLog` in
        /// `wasm_test_support` keeps its counts in `Rc<Cell<_>>` for that
        /// reason; this is `Arc<AtomicU32>` only because `Memo::new` requires
        /// `Send + Sync`, which `Rc` is not. WASM is single-threaded, so the
        /// ordering is immaterial.
        runs: Arc<AtomicU32>,
        /// A `Memo` over the counter — the subscriber.
        ///
        /// This is the node `Resource::new` interposes itself:
        /// `ArcResource::new_with_options` (`leptos_server-0.8.7`) wraps the
        /// page's source closure in an `ArcMemo`, so on a page keyed to a
        /// counter this body is the first thing a notification reaches. It
        /// re-runs on notification alone, whatever the value — which is exactly
        /// what these tests need to see, since the value is `0` either way.
        ///
        /// A memo is lazy, so a notification does not run the body on the spot;
        /// it marks the memo dirty and the body runs on the next read. That is
        /// what production does too — the `AsyncDerived` woken by the notify
        /// calls `update_if_necessary`, which forces the memo. So [`Probe::read`]
        /// below is the poll, not the observation: the observation is whether
        /// the body ran during it.
        memo: Memo<u32>,
        /// The counter read straight, bypassing the memo, so "was rewound" is
        /// observable independently of "was notified".
        raw: Signal<u32>,
        bump: CounterBump,
    }

    impl Probe {
        /// Poll the subscriber. Its body runs during this call if, and only if,
        /// something notified the counter since the last poll.
        fn read(&self) -> u32 {
            self.memo.get_untracked()
        }

        fn runs(&self) -> u32 {
            self.runs.load(Ordering::Relaxed)
        }
    }

    /// Attach a subscriber to every counter in [`COUNTERS`].
    ///
    /// Both getters are resolved here, at "component setup" and under the
    /// caller's owner, which is the form the getter contract on [`SyncStore`]
    /// requires and the form `pages/` uses.
    fn probe_every_counter(store: &SyncStore) -> Vec<Probe> {
        COUNTERS
            .iter()
            .map(|&(name, getter, bump)| {
                let runs = Arc::new(AtomicU32::new(0));
                let version = getter(store);
                let memo = {
                    let runs = Arc::clone(&runs);
                    Memo::new(move |_| {
                        runs.fetch_add(1, Ordering::Relaxed);
                        version.get()
                    })
                };
                Probe {
                    name,
                    runs,
                    memo,
                    raw: getter(store),
                    bump,
                }
            })
            .collect()
    }

    /// `reset()` on a store whose counters are already zero must wake nobody.
    ///
    /// The cursor-less connect path in `cache/sync_engine.rs` calls `reset()`
    /// immediately before every `sync_bootstrap`, so on a plain page load it
    /// runs against counters that have never been bumped. `set(0)` notified
    /// there anyway, and each of those notifications is a page's subscriber
    /// re-running for a value that did not change.
    ///
    /// **This asserts on the subscriber running, not on the counter's value,
    /// and that is the whole point of the test.** The values are `0` before and
    /// after either way, so a value assertion passes against the unfixed
    /// `set(0)` and proves nothing. The run count is the only thing that tells
    /// the two apart. Note the read below happens in both this test and
    /// `resetting_a_bumped_store_…`: identical polls, different body-run
    /// counts, so the difference is the notification and not the poll.
    #[wasm_bindgen_test]
    fn resetting_an_already_zero_store_wakes_no_subscriber() {
        let root = Owner::new();
        root.set();
        let store = SyncStore::new();
        let probes = probe_every_counter(&store);

        // Prime. A memo is lazy, so until it is read its body has never run and
        // "did not run again" would be true of a subscriber that was never
        // wired up at all.
        for probe in &probes {
            assert_eq!(probe.read(), 0, "fixture: {} starts at zero", probe.name);
            assert_eq!(
                probe.runs(),
                1,
                "fixture: the subscriber on {} must have run once by now, or the rest of \
                 this test is watching nothing",
                probe.name
            );
        }

        store.reset();

        for probe in &probes {
            assert_eq!(
                probe.read(),
                0,
                "{} must still read zero after a reset that found it at zero",
                probe.name
            );
            assert_eq!(
                probe.runs(),
                1,
                "the subscriber on {} re-ran for a reset that changed nothing. Every page \
                 keyed to that counter woke on a fresh bootstrap for a value that did not \
                 move — a `LocalResource` refetches from the server, an `Effect` re-reads \
                 IndexedDB. Rewind the counter with `rewind_to_zero`, not `set(0)`",
                probe.name
            );
        }
    }

    /// `reset()` on a store that has been bumped must still rewind **and**
    /// still notify.
    ///
    /// The other direction of the same guard. Without this, the fix could
    /// degrade to "never notify" — a `rewind_to_zero` that always returned
    /// `false`, or one that skipped the write — and
    /// `resetting_an_already_zero_store_wakes_no_subscriber` would stay green
    /// while a workspace switch left every page showing the previous
    /// workspace's data.
    ///
    /// The two halves are asserted through different paths on purpose: the
    /// rewind through `raw`, which reads the counter itself, and the
    /// notification through the subscriber's run count. A guard that stopped
    /// notifying would still pass the rewind half, so the failure names which
    /// half broke.
    #[wasm_bindgen_test]
    fn resetting_a_bumped_store_rewinds_the_counters_and_wakes_their_subscribers() {
        let root = Owner::new();
        root.set();
        let store = SyncStore::new();
        let probes = probe_every_counter(&store);

        for probe in &probes {
            assert_eq!(probe.read(), 0, "fixture: {} starts at zero", probe.name);
        }

        // A frame for each entity type arrives, as it does over the sync
        // stream.
        for probe in &probes {
            (probe.bump)(&store);
        }
        for probe in &probes {
            assert_eq!(
                probe.read(),
                1,
                "fixture: {} must move when its own bump runs",
                probe.name
            );
            assert_eq!(
                probe.runs(),
                2,
                "fixture: the subscriber on {} must have re-run for the bump, or this test \
                 cannot tell a missing notification from a subscriber that never worked",
                probe.name
            );
        }

        store.reset();

        for probe in &probes {
            assert_eq!(
                probe.raw.get_untracked(),
                0,
                "{} was not rewound by reset(). A workspace switch leaves the new \
                 workspace's pages keyed to the previous one's counter",
                probe.name
            );
            let through_subscriber = probe.read();
            assert_eq!(
                probe.runs(),
                3,
                "the subscriber on {} did not re-run for a reset that moved it from 1 to \
                 0. The guard has become an unconditional skip: the counter rewinds and \
                 nothing subscribed to it ever hears about it",
                probe.name
            );
            assert_eq!(
                through_subscriber,
                0,
                "the subscriber on {} re-ran but did not see the rewind",
                probe.name
            );
        }
    }

    // ── Probing what `reset()` wakes: the collections ───────────────────────

    /// One subscriber attached to one of the store's collections.
    ///
    /// The same instrument as [`Probe`], reading the collection's **length**
    /// rather than its contents: `reset` writes an empty `Vec`, so the length is
    /// all the assertions need, and reading it with `with` avoids cloning the
    /// whole list on every poll.
    struct CollectionProbe {
        name: &'static str,
        /// Runs of the subscriber's body. Not a signal — see [`Probe::runs`].
        runs: Arc<AtomicU32>,
        memo: Memo<usize>,
        /// The length read straight off the collection, bypassing the memo, so
        /// "was cleared" is observable independently of "was notified".
        raw: Signal<usize>,
        /// Put one entity into the collection, as a sync frame does.
        seed: Box<dyn Fn()>,
    }

    impl CollectionProbe {
        /// Poll the subscriber. Its body runs during this call if, and only if,
        /// something notified the collection since the last poll.
        fn read(&self) -> usize {
            self.memo.get_untracked()
        }

        fn runs(&self) -> u32 {
            self.runs.load(Ordering::Relaxed)
        }
    }

    fn probe_collection<T>(
        name: &'static str,
        items: Signal<Vec<T>>,
        seed: impl Fn() + 'static,
    ) -> CollectionProbe
    where
        T: Send + Sync + 'static,
    {
        let runs = Arc::new(AtomicU32::new(0));
        let memo = {
            let runs = Arc::clone(&runs);
            Memo::new(move |_| {
                runs.fetch_add(1, Ordering::Relaxed);
                items.with(|list| list.len())
            })
        };
        CollectionProbe {
            name,
            runs,
            memo,
            raw: Signal::derive(move || items.with(|list| list.len())),
            seed: Box::new(seed),
        }
    }

    /// Attach a subscriber to every collection [`SyncStore::reset`] clears.
    ///
    /// All eight are listed for the same reason all nine counters are: a guard
    /// applied to seven of them must fail, naming the one left on `set`.
    fn probe_every_collection(store: &SyncStore) -> Vec<CollectionProbe> {
        let store = *store;
        vec![
            probe_collection("issues", store.issues(), move || {
                store.upsert_issue(an_issue())
            }),
            probe_collection("labels", store.labels(), move || {
                store.upsert_label(a_label())
            }),
            probe_collection("statuses", store.statuses(), move || {
                store.upsert_status(a_status())
            }),
            probe_collection("teams", store.teams(), move || store.upsert_team(a_team())),
            probe_collection("projects", store.projects(), move || {
                store.upsert_project(a_project())
            }),
            probe_collection("views", store.views(), move || store.upsert_view(a_view())),
            probe_collection("favorites", store.favorites(), move || {
                store.upsert_favorite(a_favorite())
            }),
            probe_collection("notifications", store.notifications(), move || {
                store.upsert_notification(a_notification())
            }),
        ]
    }

    // ── Fixtures ────────────────────────────────────────────────────────────
    //
    // One entity per collection. Nothing reads a field of any of them: the
    // guard's predicate is `!list.is_empty()`, so all these exist to do is make
    // a list non-empty. They are written out in full rather than built from
    // JSON because the models carry no `Default`, and a fixture that stops
    // compiling when a field is added is the cheaper failure.

    fn a_label() -> Label {
        Label {
            label_id: "lbl-1".to_owned(),
            workspace_id: "ws-1".to_owned(),
            team_id: None,
            name: "bug".to_owned(),
            color: "#0D9488".to_owned(),
            created_at: "2026-08-07T00:00:00Z".to_owned(),
        }
    }

    fn an_issue() -> IssueWithDetails {
        IssueWithDetails {
            issue_id: "iss-1".to_owned(),
            workspace_id: "ws-1".to_owned(),
            team_id: "tea-1".to_owned(),
            team_key: "TRA".to_owned(),
            number: 42,
            title: "An issue the reset has to drop".to_owned(),
            description: None,
            status_id: "sta-1".to_owned(),
            status_name: "Todo".to_owned(),
            status_category: "unstarted".to_owned(),
            priority: 0,
            assignee_id: None,
            assignee_name: None,
            creator_id: "usr-alice".to_owned(),
            creator_name: None,
            due_date: None,
            project_id: None,
            project_name: None,
            milestone_id: None,
            estimate: None,
            parent_identifier: None,
            parent_title: None,
            sort_order: None,
            created_at: "2026-08-07T00:00:00Z".to_owned(),
            updated_at: "2026-08-07T00:00:00Z".to_owned(),
            started_at: None,
            completed_at: None,
            released_at: None,
            archived_at: None,
            has_children: false,
            is_blocked: false,
            is_blocking: false,
            has_relations: false,
            labels: Vec::new(),
        }
    }

    fn a_status() -> Status {
        Status {
            status_id: "sta-1".to_owned(),
            workspace_id: "ws-1".to_owned(),
            team_id: None,
            name: "Todo".to_owned(),
            category: "unstarted".to_owned(),
            position: 0,
            color: None,
            created_at: "2026-08-07T00:00:00Z".to_owned(),
        }
    }

    fn a_team() -> Team {
        Team {
            team_id: "tea-1".to_owned(),
            workspace_id: "ws-1".to_owned(),
            name: "Engineering".to_owned(),
            key: "TRA".to_owned(),
            description: None,
            icon: None,
            icon_type: None,
            icon_name: None,
            icon_color: None,
            member_count: 1,
            settings: None,
            created_at: "2026-08-07T00:00:00Z".to_owned(),
        }
    }

    fn a_project() -> Project {
        Project {
            project_id: "prj-1".to_owned(),
            workspace_id: "ws-1".to_owned(),
            name: "Q3 Launch".to_owned(),
            description: None,
            icon: None,
            color: None,
            status: "planned".to_owned(),
            lead_id: None,
            lead_name: None,
            start_date: None,
            target_date: None,
            sort_order: 0.0,
            created_at: "2026-08-07T00:00:00Z".to_owned(),
            updated_at: "2026-08-07T00:00:00Z".to_owned(),
            archived_at: None,
        }
    }

    fn a_view() -> View {
        View {
            view_id: "viw-1".to_owned(),
            workspace_id: "ws-1".to_owned(),
            team_id: None,
            created_by: "usr-alice".to_owned(),
            name: "My open issues".to_owned(),
            icon: None,
            filters: "{}".to_owned(),
            display_options: "{}".to_owned(),
            sort_order: 0.0,
            position: 0,
            is_shared: false,
            created_at: "2026-08-07T00:00:00Z".to_owned(),
            updated_at: "2026-08-07T00:00:00Z".to_owned(),
        }
    }

    fn a_favorite() -> Favorite {
        Favorite {
            favorite_id: "fav-1".to_owned(),
            user_id: "usr-alice".to_owned(),
            workspace_id: "ws-1".to_owned(),
            target_type: "issue".to_owned(),
            target_id: "iss-1".to_owned(),
            sort_order: 0.0,
            created_at: "2026-08-07T00:00:00Z".to_owned(),
        }
    }

    fn a_notification() -> Notification {
        Notification {
            notification_id: "ntf-1".to_owned(),
            workspace_id: "ws-1".to_owned(),
            user_id: "usr-alice".to_owned(),
            issue_id: "iss-1".to_owned(),
            notification_type: "assigned".to_owned(),
            read: false,
            issue_title: None,
            issue_number: None,
            team_key: None,
            actor_id: None,
            actor_name: None,
            action_source: trakkt_types::enums::ActionSource::User,
            action_source_label: None,
            created_at: "2026-08-07T00:00:00Z".to_owned(),
            deleted_at: None,
            context_id: None,
        }
    }

    /// `reset()` on a store whose collections are already empty must wake
    /// nobody.
    ///
    /// The collections half of
    /// `resetting_an_already_zero_store_wakes_no_subscriber`, and the same trap
    /// applies: on the cursor-less bootstrap path the lists are already empty
    /// when `reset` runs — hydration finishes before the socket is dialled and
    /// an empty cache hydrates to empty lists — so **the lists are empty before
    /// and after either way**. Asserting they are empty passes against `main`'s
    /// `set(Vec::new())`. Only the subscriber's run count tells the two apart.
    ///
    /// This is the assertion that catches a collection left on `set` when the
    /// other seven were converted, which is why all eight are probed.
    #[wasm_bindgen_test]
    fn resetting_an_already_empty_store_wakes_no_subscriber() {
        let root = Owner::new();
        root.set();
        let store = SyncStore::new();
        let probes = probe_every_collection(&store);

        // Prime. A memo is lazy, so until it is read its body has never run.
        for probe in &probes {
            assert_eq!(probe.read(), 0, "fixture: {} starts empty", probe.name);
            assert_eq!(
                probe.runs(),
                1,
                "fixture: the subscriber on {} must have run once by now, or the rest of \
                 this test is watching nothing",
                probe.name
            );
        }

        store.reset();

        for probe in &probes {
            assert_eq!(
                probe.read(),
                0,
                "{} must still be empty after a reset that found it empty",
                probe.name
            );
            assert_eq!(
                probe.runs(),
                1,
                "the subscriber on {} re-ran for a reset that changed nothing. Every list \
                 page reading that collection woke on a fresh bootstrap for a value that \
                 did not move — which is the `Transition` rebuild with no data behind it \
                 that TRA-9984 is about. Clear it with `clear_if_populated`, not \
                 `set(Vec::new())`",
                probe.name
            );
        }
    }

    /// `reset()` on a populated store must still clear **and** still notify.
    ///
    /// The other direction, as for the counters. Without it the guard could
    /// degrade to "never notify" and
    /// `resetting_an_already_empty_store_wakes_no_subscriber` would stay green
    /// while a workspace switch left every list page rendering the previous
    /// workspace's issues.
    ///
    /// The two halves go through different paths on purpose — the clear through
    /// `raw`, which reads the collection itself, the notification through the
    /// subscriber's run count — so a mutation can fail one without the other.
    #[wasm_bindgen_test]
    fn resetting_a_populated_store_clears_the_collections_and_wakes_their_subscribers() {
        let root = Owner::new();
        root.set();
        let store = SyncStore::new();
        let probes = probe_every_collection(&store);

        for probe in &probes {
            assert_eq!(probe.read(), 0, "fixture: {} starts empty", probe.name);
        }

        // One entity of each type arrives, as it does over the sync stream.
        for probe in &probes {
            (probe.seed)();
        }
        for probe in &probes {
            assert_eq!(
                probe.read(),
                1,
                "fixture: {} must hold the entity that was just upserted into it",
                probe.name
            );
            assert_eq!(
                probe.runs(),
                2,
                "fixture: the subscriber on {} must have re-run for the upsert, or this \
                 test cannot tell a missing notification from a subscriber that never \
                 worked",
                probe.name
            );
        }

        store.reset();

        for probe in &probes {
            assert_eq!(
                probe.raw.get_untracked(),
                0,
                "{} was not cleared by reset(). The previous workspace's entities are \
                 still in memory for the next one's pages to render",
                probe.name
            );
            let through_subscriber = probe.read();
            assert_eq!(
                probe.runs(),
                3,
                "the subscriber on {} did not re-run for a reset that emptied it. The \
                 guard has become an unconditional skip: the collection is cleared and \
                 every page still showing its contents is never told",
                probe.name
            );
            assert_eq!(
                through_subscriber, 0,
                "the subscriber on {} re-ran but did not see the clear",
                probe.name
            );
        }
    }

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
