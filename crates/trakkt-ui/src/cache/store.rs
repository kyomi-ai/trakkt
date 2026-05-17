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

use trakkt_types::models::{Comment, Favorite, IssueWithDetails, Label, Notification, Project, Status, Team, View};

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
    comments: ArcRwSignal<Vec<Comment>>,
    initialized: ArcRwSignal<bool>,
    /// Version counter bumped when an activity sync action arrives.
    /// Used by the timeline component to trigger a refetch.
    activities_version: ArcRwSignal<u32>,
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
                comments: ArcRwSignal::new(Vec::new()),
                initialized: ArcRwSignal::new(false),
                activities_version: ArcRwSignal::new(0),
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

    /// Reactive signal over the current comments list.
    pub fn comments(&self) -> Signal<Vec<Comment>> {
        let sig = self.inner.with_value(|inner| inner.comments.clone());
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

    /// Replace the entire comments list (called during IDB hydration).
    pub fn set_comments(&self, items: Vec<Comment>) {
        self.inner.with_value(|inner| inner.comments.set(items));
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

    /// Insert or update a comment by `comment_id`.
    pub fn upsert_comment(&self, item: Comment) {
        self.inner.with_value(|inner| {
            inner.comments.update(|list| {
                if let Some(existing) = list.iter_mut().find(|c| c.comment_id == item.comment_id) {
                    *existing = item;
                } else {
                    list.push(item);
                }
            });
        });
    }

    // ── Single-item removes (delete sync) ────────────────────────────────────

    /// Remove an issue by `issue_id`.
    pub fn remove_issue(&self, issue_id: &str) {
        self.inner.with_value(|inner| {
            inner.issues.update(|list| {
                list.retain(|i| i.issue_id != issue_id);
            });
        });
    }

    /// Remove a label by `label_id`.
    pub fn remove_label(&self, label_id: &str) {
        self.inner.with_value(|inner| {
            inner.labels.update(|list| {
                list.retain(|l| l.label_id != label_id);
            });
        });
    }

    /// Remove a status by `status_id`.
    pub fn remove_status(&self, status_id: &str) {
        self.inner.with_value(|inner| {
            inner.statuses.update(|list| {
                list.retain(|s| s.status_id != status_id);
            });
        });
    }

    /// Remove a team by `team_id`.
    pub fn remove_team(&self, team_id: &str) {
        self.inner.with_value(|inner| {
            inner.teams.update(|list| {
                list.retain(|t| t.team_id != team_id);
            });
        });
    }

    /// Remove a project by `project_id`.
    pub fn remove_project(&self, project_id: &str) {
        self.inner.with_value(|inner| {
            inner.projects.update(|list| {
                list.retain(|p| p.project_id != project_id);
            });
        });
    }

    /// Remove a view by `view_id`.
    pub fn remove_view(&self, view_id: &str) {
        self.inner.with_value(|inner| {
            inner.views.update(|list| {
                list.retain(|v| v.view_id != view_id);
            });
        });
    }

    /// Remove a favorite by `favorite_id`.
    pub fn remove_favorite(&self, favorite_id: &str) {
        self.inner.with_value(|inner| {
            inner.favorites.update(|list| {
                list.retain(|f| f.favorite_id != favorite_id);
            });
        });
    }

    /// Remove a notification by `notification_id`.
    pub fn remove_notification(&self, notification_id: &str) {
        self.inner.with_value(|inner| {
            inner.notifications.update(|list| {
                list.retain(|n| n.notification_id != notification_id);
            });
        });
    }

    /// Remove a comment by `comment_id`.
    pub fn remove_comment(&self, comment_id: &str) {
        self.inner.with_value(|inner| {
            inner.comments.update(|list| {
                list.retain(|c| c.comment_id != comment_id);
            });
        });
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
            inner.comments.set(Vec::new());
            inner.initialized.set(false);
            inner.activities_version.set(0);
        });
    }
}
