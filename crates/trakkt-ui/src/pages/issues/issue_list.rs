// SPDX-License-Identifier: AGPL-3.0-or-later

//! Issue list page — the first thing users see after login.
//!
//! Layout follows DESIGN.md "Issue List Page" spec:
//! - Page header: title + view toggle + "New Issue" button
//! - Toolbar: search + filter dropdowns (list mode only)
//! - Content: issue rows (list mode) or Kanban board (board mode)
//!
//! Issue Row follows DESIGN.md "Issue Row Pattern":
//! `px-3 py-[6px] h-9 flex items-center gap-2.5 border-b border-border`
//! hover:bg-surface-alt transition-colors cursor-pointer
//! Order: Priority | Status | Issue ID | Title | Labels | Date | Assignee
//!
//! ## View toggle
//!
//! A segmented control in the page header allows switching between list and
//! board views. The preference is persisted to localStorage per team (or
//! globally for the workspace-level page).

use std::sync::Arc;

use leptos::prelude::*;
use leptos_router::hooks::{use_location, use_navigate};
use leptos_router::location::State;
use leptos_router::NavigateOptions;
use phosphor_leptos::{Icon, IconWeight};
use wasm_bindgen::closure::Closure;
use wasm_bindgen::JsCast;

use crate::components::{
    Alert, AlertVariant,
    Button, ButtonSize, ButtonVariant, ConfirmDialog, EmptyState,
    Modal, ModalSize,
    SearchInput, Select, SelectVariant, TeamIcon, INPUT_CLASS,
};
use crate::pages::board::BoardContent;
use crate::pages::issues::filters::{
    apply_clause, parse_sort_field, sort_field_to_str, sort_issues, FilterBar, SortDirection,
    SortDropdown, SortField,
};
use crate::pages::issues::issue_row::IssueRow;
use crate::pages::issues::{is_archived, ARCHIVE_DAYS};
use crate::pages::views::{FilterClause, LegacyViewFilters, ViewFilters};
use crate::server_fns::issues::{create_issue, get_archived_issues, list_issues};
use crate::server_fns::statuses::list_statuses;
use crate::server_fns::views::{create_view, delete_view, update_view};
use crate::types::IssueNavState;
use crate::utils::keyboard::is_input_focused;
use trakkt_types::models::{IssueWithDetails, Status, Team};

// ─────────────────────────────────────────────────────────────────────────────
// localStorage helpers for view mode
// ─────────────────────────────────────────────────────────────────────────────

/// Read the saved view mode from localStorage.
#[cfg(target_arch = "wasm32")]
fn read_view_mode(key: &str) -> Option<String> {
    let storage = web_sys::window()?.local_storage().ok()??;
    storage.get_item(key).ok()?
}

/// Write the view mode to localStorage.
#[cfg(target_arch = "wasm32")]
fn write_view_mode(key: &str, mode: &str) {
    if let Some(storage) = web_sys::window().and_then(|w| w.local_storage().ok().flatten()) {
        let _ = storage.set_item(key, mode);
    }
}

/// Build the localStorage key for the view mode.
fn view_mode_storage_key(team_key: &Option<Signal<String>>) -> String {
    match team_key {
        Some(sig) => {
            let key = sig.get_untracked();
            if key.is_empty() {
                "trakkt-view-mode-global".to_string()
            } else {
                format!("trakkt-view-mode-{key}")
            }
        }
        None => "trakkt-view-mode-global".to_string(),
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// URL query param helpers for view state persistence
// ─────────────────────────────────────────────────────────────────────────────

/// Parsed view state from URL query parameters.
struct ParsedViewState {
    /// Active tab: "issues", "active", "backlog", or "view:<uuid>"
    view: String,
    /// Filter clauses deserialized from the `filters` JSON param.
    filters: Vec<FilterClause>,
    /// Sort field (if present in URL).
    sort: Option<SortField>,
    /// Sort direction (if present in URL).
    sort_dir: Option<SortDirection>,
}

/// Parse URL query parameters into view state.
///
/// Handles both the new format (`view=active&filters=...&sort=...&sort_dir=...`)
/// and the legacy format (`status=in_progress` / `status=backlog`) for backward
/// compatibility with sidebar links.
///
/// The `query` string comes from `use_location().search` which does NOT include
/// the leading `?`.
fn parse_query_params(query: &str) -> ParsedViewState {
    let mut view = None::<String>;
    let mut filters_raw = None::<String>;
    let mut sort_raw = None::<String>;
    let mut sort_dir_raw = None::<String>;
    let mut legacy_status = None::<String>;

    for pair in query.split('&') {
        if pair.is_empty() {
            continue;
        }
        if let Some((key, value)) = pair.split_once('=') {
            match key {
                "view" => view = Some(value.to_string()),
                "filters" => filters_raw = Some(value.to_string()),
                "sort" => sort_raw = Some(value.to_string()),
                "sort_dir" => sort_dir_raw = Some(value.to_string()),
                "status" => legacy_status = Some(value.to_string()),
                _ => {}
            }
        }
    }

    // Backward compatibility: translate legacy `?status=` params into new format.
    // Only applies when no `view` param is present (i.e., old-style sidebar links).
    if view.is_none()
        && let Some(ref status) = legacy_status
    {
        match status.as_str() {
            "in_progress" => view = Some("active".to_string()),
            "backlog" => view = Some("backlog".to_string()),
            _ => {}
        }
    }

    // Parse the `filters` JSON param.
    let filters = match filters_raw {
        Some(encoded) => {
            let decoded = percent_encoding::percent_decode_str(&encoded)
                .decode_utf8_lossy()
                .to_string();
            match serde_json::from_str::<Vec<FilterClause>>(&decoded) {
                Ok(clauses) => clauses,
                Err(e) => {
                    tracing::warn!("Failed to parse filters from URL: {e}");
                    Vec::new()
                }
            }
        }
        None => Vec::new(),
    };

    let sort = sort_raw.as_deref().and_then(parse_sort_field);
    let sort_dir = sort_dir_raw.as_deref().and_then(SortDirection::parse);

    ParsedViewState {
        view: view.unwrap_or_else(|| "issues".to_string()),
        filters,
        sort,
        sort_dir,
    }
}

/// Build a URL query string from the current view state.
///
/// Only includes parameters that differ from defaults (clean URL = no params).
/// Defaults: view=issues, no filters, sort=priority, sort_dir=asc.
fn build_query_string(
    view: &str,
    clauses: &[FilterClause],
    sort: SortField,
    direction: SortDirection,
) -> String {
    let mut params = Vec::<String>::new();

    // View param — omit when "issues" (default).
    if view != "issues" {
        params.push(format!("view={view}"));
    }

    // Filters — omit when empty.
    if !clauses.is_empty() {
        match serde_json::to_string(clauses) {
            Ok(json) => {
                let encoded = percent_encoding::utf8_percent_encode(
                    &json,
                    percent_encoding::NON_ALPHANUMERIC,
                )
                .to_string();
                params.push(format!("filters={encoded}"));
            }
            Err(e) => {
                tracing::warn!("Failed to serialize filter clauses to URL: {e}");
            }
        }
    }

    // Sort — omit when default (priority).
    if sort != SortField::Priority {
        params.push(format!("sort={}", sort_field_to_str(sort)));
    }

    // Sort direction — omit when default (asc).
    if direction != SortDirection::Asc {
        params.push(format!("sort_dir={}", direction.as_str()));
    }

    params.join("&")
}

/// Compute status IDs matching the given categories, filtered by team.
///
/// Shared by the init Effect (for "active"/"backlog" tabs) to avoid
/// duplicating the SyncStore lookup logic from the tab click handlers.
fn compute_status_ids(
    sync_store: Option<crate::cache::store::SyncStore>,
    team: &Option<Team>,
    categories: &[&str],
) -> Vec<String> {
    let Some(store) = sync_store else {
        return Vec::new();
    };
    store
        .statuses()
        .get_untracked()
        .into_iter()
        .filter(|s| {
            categories.contains(&s.category.as_str())
                && (s.team_id.is_none()
                    || team.as_ref().map(|t| &t.team_id) == s.team_id.as_ref())
        })
        .map(|s| s.status_id)
        .collect()
}

/// Load filter/sort state from a saved view's JSON in the SyncStore.
///
/// Used by the init Effect for `view:<uuid>` tabs. Same deserialization
/// logic as the custom view click handler (handles both new and legacy
/// filter formats).
fn apply_view_filters_from_store(
    sync_store: Option<crate::cache::store::SyncStore>,
    team: &Option<Team>,
    view_id: &str,
    filter_clauses: &RwSignal<Vec<FilterClause>>,
    set_sort_field: &WriteSignal<SortField>,
    set_sort_direction: &WriteSignal<SortDirection>,
) {
    let Some(store) = sync_store else { return };
    let view = store
        .views()
        .get_untracked()
        .into_iter()
        .find(|v| {
            v.view_id == view_id
                && match team {
                    Some(t) => v.team_id.as_deref() == Some(t.team_id.as_str()),
                    None => v.team_id.is_none(),
                }
        });
    let Some(v) = view else {
        tracing::warn!("View {view_id} not found in SyncStore");
        return;
    };
    let filters_json = &v.filters;
    let is_new_format = filters_json.contains("\"clauses\"");
    if is_new_format {
        match serde_json::from_str::<ViewFilters>(filters_json) {
            Ok(filters) => {
                filter_clauses.set(filters.clauses);
                set_sort_field.set(
                    filters
                        .sort_field
                        .as_deref()
                        .and_then(parse_sort_field)
                        .unwrap_or(SortField::Priority),
                );
                set_sort_direction.set(
                    filters
                        .sort_direction
                        .as_deref()
                        .and_then(SortDirection::parse)
                        .unwrap_or(SortDirection::Asc),
                );
            }
            Err(e) => tracing::warn!("Failed to parse view filters: {e}"),
        }
    } else {
        match serde_json::from_str::<LegacyViewFilters>(filters_json) {
            Ok(legacy) => {
                let converted = legacy.into_view_filters();
                filter_clauses.set(converted.clauses);
                set_sort_field.set(
                    converted
                        .sort_field
                        .as_deref()
                        .and_then(parse_sort_field)
                        .unwrap_or(SortField::Priority),
                );
                set_sort_direction.set(
                    converted
                        .sort_direction
                        .as_deref()
                        .and_then(SortDirection::parse)
                        .unwrap_or(SortDirection::Asc),
                );
            }
            Err(e) => tracing::warn!("Failed to parse legacy view filters: {e}"),
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Issue List Page
// ─────────────────────────────────────────────────────────────────────────────

/// Main issue list page — zero-arg entry point used by the router.
///
/// Shows all workspace issues (no team filtering). For team-scoped views,
/// use `<IssueListForTeam team_key=.../>` instead.
#[component]
pub fn IssueListPage() -> impl IntoView {
    view! { <IssueListInner/> }
}

/// Team-scoped issue list page — reads the `:key` route param internally.
///
/// Filters issues, statuses, and new issue creation to the resolved team.
/// Follows the same pattern as `ProjectDetailPage` and `IssueDetailPage`:
/// page components own their param extraction rather than receiving props.
#[component]
pub fn IssueListForTeam() -> impl IntoView {
    let params = leptos_router::hooks::use_params_map();
    let team_key = Signal::derive(move || params.read().get("key").unwrap_or_default());
    view! { <IssueListInner team_key=team_key/> }
}

/// Inner implementation shared by `IssueListPage` (no team),
/// `IssueListForTeam` (team-scoped), and `WorkspaceViewPage` (saved view).
/// All filtering, title, and create-issue logic lives here.
#[component]
pub(crate) fn IssueListInner(
    /// Optional reactive team key. When `Some`, filters issues and statuses by team.
    #[prop(optional, into)]
    team_key: Option<Signal<String>>,
    /// When set, the component loads this view on mount (used by /views/:view_id route).
    #[prop(optional, into)]
    initial_view_id: Option<Signal<String>>,
) -> impl IntoView {
    // ── View mode state ────────────────────────────────────────────────────
    let storage_key = view_mode_storage_key(&team_key);
    let initial_mode = {
        #[cfg(target_arch = "wasm32")]
        { read_view_mode(&storage_key).unwrap_or_else(|| "list".to_string()) }
        #[cfg(not(target_arch = "wasm32"))]
        { "list".to_string() }
    };
    let (view_mode, set_view_mode) = signal(initial_mode);
    let storage_key_for_effect = storage_key.clone();
    Effect::new(move |_| {
        let mode = view_mode.get();
        #[cfg(target_arch = "wasm32")]
        write_view_mode(&storage_key_for_effect, &mode);
        #[cfg(not(target_arch = "wasm32"))]
        let _ = (&storage_key_for_effect, &mode);
    });

    // ── Filter state ────────────────────────────────────────────────────────
    let (search, set_search) = signal(String::new());
    let filter_clauses = RwSignal::new(Vec::<FilterClause>::new());
    let (show_archived, set_show_archived) = signal(false);

    // ── Server-fetched archived issues (fetched on demand when toggle is ON) ──
    let archived_issues_signal = RwSignal::new(Vec::<IssueWithDetails>::new());

    // ── Sort state ─────────────────────────────────────────────────────────
    let (sort_field, set_sort_field) = signal(SortField::Priority);
    let (sort_direction, set_sort_direction) = signal(SortDirection::Asc);

    // ── Active tab state (team-scoped pages only) ─────────────────────────
    // Values: "issues", "active", "backlog", "view:{view_id}"
    let (active_tab, set_active_tab) = signal("issues".to_string());

    // ── URL state persistence ───────────────────────────────────────────
    // Prevents circular Effect loops: init reads URL → sets signals →
    // URL-update Effect would write URL → re-trigger init. The init Effect
    // sets this to `true` before writing signals, and back to `false` after.
    let skip_url_update = RwSignal::new(false);
    // When `true`, the URL-update Effect uses `replace: false` (push state)
    // so the browser back button works for tab switches. Tab click handlers
    // set this before changing signals; the Effect reads it and resets.
    let push_next_nav = RwSignal::new(false);

    // ── Error state for server function failures ──────────────────────────
    let error_msg = RwSignal::new(Option::<String>::None);

    // ── Context menu / rename / delete state for custom view tabs ────────
    let (context_menu_view_id, set_context_menu_view_id) = signal(Option::<String>::None);
    let (renaming_view_id, set_renaming_view_id) = signal(Option::<String>::None);
    let (rename_value, set_rename_value) = signal(String::new());
    let (confirm_delete_view_id, set_confirm_delete_view_id) = signal(Option::<String>::None);

    // ── Data source: SyncStore (real-time) with server function fallback ───
    let sync_store = use_context::<crate::cache::store::SyncStore>();
    let (version, set_version) = signal(0u32);

    // ── Resolve team from SyncStore ─────────────────────────────────────────
    let resolved_team: Memo<Option<Team>> = Memo::new(move |_| {
        let key = team_key?.get();
        if key.is_empty() {
            return None;
        }
        let key_lower = key.to_lowercase();
        let store = sync_store?;
        store
            .teams()
            .get()
            .into_iter()
            .find(|t| t.key.to_lowercase() == key_lower)
    });

    // ── Fetch archived issues from server when toggle is ON ─────────────
    Effect::new(move |_| {
        let showing = show_archived.get();
        if !showing {
            archived_issues_signal.set(Vec::new());
            return;
        }
        let team_id = match resolved_team.get() {
            Some(t) => t.team_id.clone(),
            None => return,
        };
        leptos::task::spawn_local(async move {
            match get_archived_issues(team_id, None, None).await {
                Ok(issues) => archived_issues_signal.set(issues),
                Err(e) => {
                    tracing::warn!("Failed to fetch archived issues: {e}");
                    archived_issues_signal.set(Vec::new());
                }
            }
        });
    });

    // ── Custom views (team-scoped or workspace-scoped) ─────────────────
    // Exclude views whose names collide with the hardcoded preset tabs
    // (Issues, Active, Backlog) to prevent duplicates.
    const PRESET_TAB_NAMES: &[&str] = &["Issues", "Active", "Backlog"];
    let custom_views = Memo::new(move |_| {
        let Some(store) = sync_store else { return Vec::new() };
        let team = resolved_team.get();
        let mut views: Vec<trakkt_types::models::View> = store
            .views()
            .get()
            .into_iter()
            .filter(|v| {
                let scope_match = match &team {
                    Some(t) => v.team_id.as_deref() == Some(t.team_id.as_str()),
                    None => v.team_id.is_none(),
                };
                scope_match
                    && !PRESET_TAB_NAMES
                        .iter()
                        .any(|p| p.eq_ignore_ascii_case(&v.name))
            })
            .collect();
        views.sort_by_key(|v| v.position);
        views
    });

    // ── Rename submit handler for custom view tabs ────────────────────────
    let handle_tab_rename_submit = move || {
        let name = rename_value.get_untracked().trim().to_string();
        let view_id = renaming_view_id.get_untracked();
        set_renaming_view_id.set(None);
        if name.is_empty() {
            return;
        }
        let Some(vid) = view_id else { return };
        leptos::task::spawn_local(async move {
            match update_view(vid, Some(name), None, None, None, None, None, None).await {
                Ok(_) => error_msg.set(None),
                Err(e) => error_msg.set(Some(format!("Failed to rename view: {e}"))),
            }
        });
    };

    // ── Delete handler for custom view tabs ─────────────────────────────
    let handle_tab_delete = move || {
        let Some(vid) = confirm_delete_view_id.get_untracked() else { return };
        set_confirm_delete_view_id.set(None);
        // If the deleted view was the active tab, reset to "Issues".
        let current_tab = active_tab.get_untracked();
        if current_tab == format!("view:{vid}") {
            set_active_tab.set("issues".to_string());
            set_search.set(String::new());
            filter_clauses.set(Vec::new());
            set_sort_field.set(SortField::Priority);
            set_sort_direction.set(SortDirection::Asc);
        }
        leptos::task::spawn_local(async move {
            match delete_view(vid).await {
                Ok(_) => error_msg.set(None),
                Err(e) => error_msg.set(Some(format!("Failed to delete view: {e}"))),
            }
        });
    };

    // ── Init Effect: read URL query params and restore view state ─────────
    // Subscribes to resolved_team (team switch resets URL) and location_search
    // (URL changes from sidebar links / direct navigation).
    // Sets `skip_url_update` to prevent the URL-update Effect from firing
    // during init signal writes.
    {
        let location_search = use_location().search;
        Effect::new(move |_| {
            let _ = resolved_team.get();
            let query = location_search.get();

            // Suppress URL-update Effect while we restore signals from the URL.
            skip_url_update.set(true);

            let parsed = parse_query_params(&query);

            // Always reset search on URL-driven state change.
            set_search.set(String::new());

            // Apply sort (or fall back to defaults).
            set_sort_field.set(parsed.sort.unwrap_or(SortField::Priority));
            set_sort_direction.set(parsed.sort_dir.unwrap_or(SortDirection::Asc));

            match parsed.view.as_str() {
                "issues" => {
                    set_active_tab.set("issues".to_string());
                    if !parsed.filters.is_empty() {
                        filter_clauses.set(parsed.filters);
                    } else {
                        let status_ids = compute_status_ids(
                            sync_store,
                            &resolved_team.get_untracked(),
                            &["backlog", "unstarted", "started"],
                        );
                        if let Some(store) = sync_store
                            && !store.initialized().get()
                        {
                            skip_url_update.set(false);
                            return;
                        }
                        filter_clauses.set(vec![FilterClause {
                            field: "status".to_string(),
                            operator: "any_of".to_string(),
                            values: status_ids,
                        }]);
                    }
                }
                "active" => {
                    set_active_tab.set("active".to_string());
                    if !parsed.filters.is_empty() {
                        filter_clauses.set(parsed.filters);
                    } else {
                        let status_ids = compute_status_ids(
                            sync_store,
                            &resolved_team.get_untracked(),
                            &["unstarted", "started"],
                        );
                        if let Some(store) = sync_store
                            && !store.initialized().get()
                        {
                            skip_url_update.set(false);
                            return;
                        }
                        filter_clauses.set(vec![FilterClause {
                            field: "status".to_string(),
                            operator: "any_of".to_string(),
                            values: status_ids,
                        }]);
                    }
                }
                "backlog" => {
                    set_active_tab.set("backlog".to_string());
                    if !parsed.filters.is_empty() {
                        filter_clauses.set(parsed.filters);
                    } else {
                        let status_ids = compute_status_ids(
                            sync_store,
                            &resolved_team.get_untracked(),
                            &["backlog"],
                        );
                        if let Some(store) = sync_store
                            && !store.initialized().get()
                        {
                            skip_url_update.set(false);
                            return;
                        }
                        filter_clauses.set(vec![FilterClause {
                            field: "status".to_string(),
                            operator: "any_of".to_string(),
                            values: status_ids,
                        }]);
                    }
                }
                view_str if view_str.starts_with("view:") => {
                    let view_id = &view_str[5..];
                    set_active_tab.set(view_str.to_string());
                    if !parsed.filters.is_empty() {
                        filter_clauses.set(parsed.filters);
                    } else {
                        apply_view_filters_from_store(
                            sync_store,
                            &resolved_team.get_untracked(),
                            view_id,
                            &filter_clauses,
                            &set_sort_field,
                            &set_sort_direction,
                        );
                    }
                }
                _ => {
                    set_active_tab.set("issues".to_string());
                    if !parsed.filters.is_empty() {
                        filter_clauses.set(parsed.filters);
                    } else {
                        let status_ids = compute_status_ids(
                            sync_store,
                            &resolved_team.get_untracked(),
                            &["backlog", "unstarted", "started"],
                        );
                        if let Some(store) = sync_store
                            && !store.initialized().get()
                        {
                            skip_url_update.set(false);
                            return;
                        }
                        filter_clauses.set(vec![FilterClause {
                            field: "status".to_string(),
                            operator: "any_of".to_string(),
                            values: status_ids,
                        }]);
                    }
                }
            }

            skip_url_update.set(false);
        });
    }

    // ── Initial view loading (from /views/:view_id route) ───────────────
    // Uses a signal guard instead of prev.is_some() because the store may not
    // be initialized on first run — the effect must re-fire when initialized
    // flips to true, but only apply the view once.
    let init_view_applied = RwSignal::new(false);
    if let Some(ref init_view) = initial_view_id {
        let init_view = *init_view;
        Effect::new(move |_| {
            if init_view_applied.get_untracked() {
                return;
            }
            let vid = init_view.get();
            if vid.is_empty() {
                return;
            }
            if let Some(store) = sync_store {
                if !store.initialized().get() {
                    return;
                }
                let views = store.views().get();
                if views.iter().any(|v| v.view_id == vid) {
                    init_view_applied.set(true);
                    skip_url_update.set(true);
                    apply_view_filters_from_store(
                        sync_store,
                        &resolved_team.get_untracked(),
                        &vid,
                        &filter_clauses,
                        &set_sort_field,
                        &set_sort_direction,
                    );
                    set_active_tab.set(format!("view:{vid}"));
                    skip_url_update.set(false);
                }
            }
        });
    }

    // ── URL-update Effect: serialize view state into query params ─────────
    // Watches all view state signals and updates the URL when they change.
    // Uses `replace: true` by default (silent update), or `replace: false`
    // (push state) when `push_next_nav` is set by a tab click handler.
    {
        let nav_for_url = use_navigate();
        let pathname = use_location().pathname;
        Effect::new(move |_| {
            // Read all tracked signals to subscribe.
            let tab = active_tab.get();
            let clauses = filter_clauses.get();
            let sf = sort_field.get();
            let sd = sort_direction.get();

            if skip_url_update.get_untracked() {
                return;
            }

            // Update URL on team-scoped pages and workspace view pages.
            if team_key.is_none() && initial_view_id.is_none() {
                return;
            }

            let query = build_query_string(&tab, &clauses, sf, sd);
            let path = pathname.get_untracked();
            let new_url = if query.is_empty() {
                path
            } else {
                format!("{path}?{query}")
            };

            let should_push = push_next_nav.get_untracked();
            if should_push {
                push_next_nav.set(false);
            }

            nav_for_url(&new_url, NavigateOptions {
                resolve: false,
                replace: !should_push,
                scroll: false,
                ..Default::default()
            });
        });
    }

    // Close context menu on click outside.
    Effect::new(move |_| {
        let Some(document) = web_sys::window().and_then(|w| w.document()) else { return };
        let cb = Closure::<dyn Fn(web_sys::MouseEvent)>::new(move |_: web_sys::MouseEvent| {
            if context_menu_view_id.get_untracked().is_some() {
                set_context_menu_view_id.set(None);
            }
        });
        let _ = document.add_event_listener_with_callback("mousedown", cb.as_ref().unchecked_ref());
        let cb_cleanup = send_wrapper::SendWrapper::new(cb);
        on_cleanup(move || {
            let Some(document) = web_sys::window().and_then(|w| w.document()) else { return };
            let cb = cb_cleanup.take();
            let _ = document.remove_event_listener_with_callback("mousedown", cb.as_ref().unchecked_ref());
        });
    });

    // Server function fallback — used for initial load before sync is ready.
    // When on a team-scoped page, pass the resolved team_id so the server
    // returns only that team's issues (important for SSR initial load).
    let server_issues = Resource::new(
        move || (version.get(), resolved_team.get().map(|t| t.team_id.clone())),
        move |(_, team_id)| async move { list_issues(team_id, None, None, None, None, None, None, None).await },
    );

    // ── All issues (unfiltered, scoped to team if applicable) ──────────────
    // Shared between list and board views.
    let team_issues = Memo::new(move |_| {
        let raw = if let Some(store) = sync_store {
            let issues = store.issues().get();
            if !issues.is_empty() || store.initialized().get() {
                issues
            } else {
                match server_issues.get() {
                    Some(Ok(items)) => {
                        error_msg.set(None);
                        items
                    }
                    Some(Err(e)) => {
                        error_msg.set(Some(format!("Failed to load issues: {e}")));
                        Vec::new()
                    }
                    None => Vec::new(),
                }
            }
        } else {
            match server_issues.get() {
                Some(Ok(items)) => {
                    error_msg.set(None);
                    items
                }
                Some(Err(e)) => {
                    error_msg.set(Some(format!("Failed to load issues: {e}")));
                    Vec::new()
                }
                None => Vec::new(),
            }
        };

        // When a team_key was requested but hasn't resolved yet, return empty (loading state).
        let team_key_present = team_key.is_some_and(|s| !s.get().is_empty());
        if team_key_present && resolved_team.get().is_none() {
            return vec![];
        }

        // Filter by team when on a team page.
        let team = resolved_team.get();
        raw.into_iter()
            .filter(|issue| {
                if let Some(ref t) = team {
                    issue.team_id == t.team_id
                } else {
                    true
                }
            })
            .collect::<Vec<_>>()
    });

    // Filtered issue list for list view — applies archive, search, and composable
    // filter clauses. When show_archived is ON, merges in server-fetched archived
    // issues (deduplicating by issue_id).
    let filtered_issues = Memo::new(move |_| {
        let raw = team_issues.get();
        let search_val = search.get().to_lowercase();
        let clauses = filter_clauses.get();
        let archived_visible = show_archived.get();

        let passes_filters = |issue: &IssueWithDetails| -> bool {
            // Search filter (not a clause — always visible in toolbar).
            // Matches against title, full identifier (e.g. "TRA-148"), or number (e.g. "148").
            if !search_val.is_empty() {
                let identifier = format!("{}-{}", issue.team_key, issue.number).to_lowercase();
                let number_str = issue.number.to_string();
                if !issue.title.to_lowercase().contains(&search_val)
                    && !identifier.contains(&search_val)
                    && !number_str.contains(&search_val)
                {
                    return false;
                }
            }
            // Apply each composable filter clause.
            for clause in &clauses {
                if !apply_clause(clause, issue) {
                    return false;
                }
            }
            true
        };

        let mut result: Vec<IssueWithDetails> = raw
            .into_iter()
            .filter(|issue| {
                // Archive filter: hide locally-archived issues unless the toggle is on.
                if !archived_visible && is_archived(issue, ARCHIVE_DAYS) {
                    return false;
                }
                passes_filters(issue)
            })
            .collect();

        // When showing archived, merge in server-fetched archived issues
        // (deduplicating by issue_id against what's already in the list).
        if archived_visible {
            let existing_ids: std::collections::HashSet<String> =
                result.iter().map(|i| i.issue_id.clone()).collect();
            let server_archived = archived_issues_signal.get();
            for issue in server_archived {
                if !existing_ids.contains(&issue.issue_id) && passes_filters(&issue) {
                    result.push(issue);
                }
            }
        }

        result
    });

    // Sorted issue list — applies the selected sort field and direction on
    // top of the filtered results. All rendering reads from `sorted_issues`
    // instead of `filtered_issues`.
    let sorted_issues = Memo::new(move |_| {
        let mut issues = filtered_issues.get();
        sort_issues(&mut issues, sort_field.get(), sort_direction.get());
        issues
    });

    // ── Statuses (for board view) ──────────────────────────────────────────
    // Board view needs statuses for column rendering. Loaded from SyncStore
    // with server function fallback, filtered by team.
    let statuses_resource = Resource::new(
        || (),
        move |_| async move { list_statuses(None).await },
    );

    let board_statuses = Memo::new(move |_| {
        let all = if let Some(store) = sync_store {
            let s = store.statuses().get();
            if !s.is_empty() || store.initialized().get() {
                s
            } else {
                statuses_resource
                    .get()
                    .and_then(|r| r.ok())
                    .unwrap_or_default()
            }
        } else {
            statuses_resource
                .get()
                .and_then(|r| r.ok())
                .unwrap_or_default()
        };

        let mut statuses: Vec<Status> = match resolved_team.get() {
            Some(ref t) => all
                .into_iter()
                .filter(|s| s.team_id.is_none() || s.team_id.as_ref() == Some(&t.team_id))
                .collect(),
            None => all,
        };
        statuses.sort_by(|a, b| {
            let cat_rank = |c: &str| match c {
                "backlog" => 0,
                "unstarted" => 1,
                "started" => 2,
                "completed" => 3,
                "cancelled" => 4,
                _ => 5,
            };
            cat_rank(&a.category)
                .cmp(&cat_rank(&b.category))
                .then(a.position.cmp(&b.position))
        });
        statuses
    });

    // ── Board-filtered statuses ──────────────────────────────────────────
    // When filter clauses include a status "any_of" clause, only show matching
    // board columns.
    let board_filtered_statuses = Memo::new(move |_| {
        let all = board_statuses.get();
        let clauses = filter_clauses.get();
        // Find the first status "any_of" clause (if any).
        let status_values: Option<&Vec<String>> = clauses.iter().find_map(|c| {
            if c.field == "status" && c.operator == "any_of" && !c.values.is_empty() {
                Some(&c.values)
            } else {
                None
            }
        });
        match status_values {
            Some(vals) => all.into_iter().filter(|s| vals.contains(&s.status_id)).collect(),
            None => all,
        }
    });

    // ── New Issue modal state ───────────────────────────────────────────────
    let (show_new_issue, set_show_new_issue) = signal(false);

    if let Some(trigger) = use_context::<crate::components::CreateIssueTrigger>() {
        Effect::new(move || {
            if trigger.0.get() {
                trigger.0.set(false);
                set_show_new_issue.set(true);
            }
        });
    }

    let on_issue_created = Callback::new(move |()| {
        set_show_new_issue.set(false);
        set_version.update(|v| *v += 1);
    });

    // ── Save View modal state ──────────────────────────────────────────────
    let (show_save_view, set_show_save_view) = signal(false);

    // Auto-open save-view modal when ?new_view=1 is in URL.
    // Strips the param after opening so refresh/share doesn't re-trigger.
    {
        let loc = use_location();
        let nav_cleanup = use_navigate();
        let opened = RwSignal::new(false);
        Effect::new(move |_| {
            if opened.get_untracked() {
                return;
            }
            let query = loc.search.get();
            let has_new_view = query
                .split('&')
                .any(|p| p == "new_view=1");
            if has_new_view {
                opened.set(true);
                set_show_save_view.set(true);
                let clean_query: String = query
                    .split('&')
                    .filter(|p| *p != "new_view=1")
                    .collect::<Vec<_>>()
                    .join("&");
                let path = loc.pathname.get_untracked();
                let url = if clean_query.is_empty() {
                    path
                } else {
                    format!("{path}?{clean_query}")
                };
                nav_cleanup(&url, NavigateOptions {
                    resolve: false,
                    replace: true,
                    scroll: false,
                    ..Default::default()
                });
            }
        });
    }

    // ── Keyboard navigation state ──────────────────────────────────────────
    let (selected_index, set_selected_index) = signal(Option::<usize>::None);

    // Track the current issue count so keyboard handlers know the bounds.
    let issue_count = RwSignal::new(0usize);

    // Track issue identifiers (e.g. "TRA-42") so Enter can navigate to the selected issue.
    let issue_identifiers = RwSignal::new(Vec::<String>::new());

    // ── j/k/Enter/c keyboard listener (window-level, active on this page) ──
    // Hoist use_navigate and use_location to component construction time (not inside closures).
    let nav = use_navigate();
    let location = use_location();
    Effect::new(move |_| {
        let Some(window) = web_sys::window() else { return };
        let nav = nav.clone();
        let cb = Closure::<dyn Fn(web_sys::KeyboardEvent)>::new(move |ev: web_sys::KeyboardEvent| {
            // Skip keyboard shortcuts when the user is typing in an input,
            // textarea, select, or contenteditable element.
            if is_input_focused(&ev) {
                return;
            }

            let key = ev.key();
            match key.as_str() {
                "j" => {
                    // Only active in list mode.
                    if view_mode.get_untracked() != "list" { return; }
                    ev.prevent_default();
                    let count = issue_count.get_untracked();
                    if count == 0 { return; }
                    set_selected_index.update(|idx| {
                        *idx = Some(match *idx {
                            None => 0,
                            Some(i) => (i + 1).min(count - 1),
                        });
                    });
                }
                "k" => {
                    if view_mode.get_untracked() != "list" { return; }
                    ev.prevent_default();
                    let count = issue_count.get_untracked();
                    if count == 0 { return; }
                    set_selected_index.update(|idx| {
                        *idx = Some(match *idx {
                            None => 0,
                            Some(i) => i.saturating_sub(1),
                        });
                    });
                }
                "Enter" => {
                    if view_mode.get_untracked() != "list" { return; }
                    if let Some(idx) = selected_index.get_untracked() {
                        let ids = issue_identifiers.get_untracked();
                        if let Some(identifier) = ids.get(idx) {
                            ev.prevent_default();
                            let path = location.pathname.get_untracked();
                            let search = location.search.get_untracked();
                            let nav_state = IssueNavState::from_current_path(&path, &search);
                            let json = nav_state.to_json();
                            nav(&format!("/issues/{identifier}"), NavigateOptions {
                                state: State::from(wasm_bindgen::JsValue::from_str(&json)),
                                ..Default::default()
                            });
                        }
                    }
                }
                "c" => {
                    ev.prevent_default();
                    set_show_new_issue.set(true);
                }
                _ => {}
            }
        });
        let _ = window.add_event_listener_with_callback(
            "keydown",
            cb.as_ref().unchecked_ref(),
        );
        let cb_cleanup = send_wrapper::SendWrapper::new(cb);
        on_cleanup(move || {
            let Some(window) = web_sys::window() else { return };
            let cb = cb_cleanup.take();
            let _ = window.remove_event_listener_with_callback(
                "keydown",
                cb.as_ref().unchecked_ref(),
            );
        });
    });

    let is_list_mode = Memo::new(move |_| view_mode.get() == "list");
    let is_board_mode = Memo::new(move |_| view_mode.get() == "board");

    // Team key as a plain String for passing to BoardContent.
    let team_key_string = Memo::new(move |_| {
        team_key.and_then(|s| {
            let v = s.get();
            if v.is_empty() { None } else { Some(v) }
        })
    });

    // ── Render ──────────────────────────────────────────────────────────────
    view! {
        <div class="bg-background flex flex-col h-full">
            // ── Page header ─────────────────────────────────────────────────
            <div class="page-header h-14 px-5 flex items-center justify-between shrink-0">
                <h1 class="flex items-center gap-2 text-sm font-semibold text-foreground">
                    {move || {
                        if let Some(team) = resolved_team.get() {
                            let name = team.name.clone();
                            view! {
                                <TeamIcon team=team size="20px"/>
                                <span>{name}</span>
                            }.into_any()
                        } else if initial_view_id.is_some() {
                            // On /views/:view_id — show the view's name from the store.
                            let view_name = move || {
                                let vid = initial_view_id.as_ref().map(|s| s.get()).unwrap_or_default();
                                if vid.is_empty() {
                                    return "Issues".to_string();
                                }
                                sync_store
                                    .and_then(|store| {
                                        store.views().get().into_iter().find(|v| v.view_id == vid)
                                    })
                                    .map(|v| v.name.clone())
                                    .unwrap_or_else(|| "Issues".to_string())
                            };
                            view! { <span>{view_name}</span> }.into_any()
                        } else {
                            view! { <span>"Issues"</span> }.into_any()
                        }
                    }}
                </h1>
                <div class="flex items-center gap-3">
                    // View toggle (segmented control)
                    <div class="flex items-center border border-border rounded-md overflow-hidden">
                        {move || {
                            let lv = if is_list_mode.get() { ButtonVariant::PillActive } else { ButtonVariant::Pill };
                            let bv = if is_board_mode.get() { ButtonVariant::PillActive } else { ButtonVariant::Pill };
                            view! {
                                <Button variant=lv size=ButtonSize::Pill on:click=move |_| set_view_mode.set("list".to_string()) aria_label="List view">
                                    <Icon icon=phosphor_leptos::LIST size="14px"/>
                                </Button>
                                <Button variant=bv size=ButtonSize::Pill on:click=move |_| set_view_mode.set("board".to_string()) aria_label="Board view">
                                    <Icon icon=phosphor_leptos::KANBAN size="14px"/>
                                </Button>
                            }
                        }}
                    </div>

                    <Button
                        on:click=move |_| set_show_new_issue.set(true)
                    >
                        <Icon icon=phosphor_leptos::PLUS size="14px"/>
                        "New Issue"
                    </Button>
                </div>
            </div>

            // ── Tab bar (team-scoped pages only) ───────────────────────────
            {move || {
                team_key?;

                let on_issues = move |_: web_sys::MouseEvent| {
                    push_next_nav.set(true);
                    set_active_tab.set("issues".to_string());
                    set_search.set(String::new());
                    set_sort_field.set(SortField::Priority);
                    set_sort_direction.set(SortDirection::Asc);
                    let team = resolved_team.get();
                    let status_ids: Vec<String> = if let (Some(store), Some(t)) = (sync_store, &team) {
                        store
                            .statuses()
                            .get()
                            .into_iter()
                            .filter(|s| {
                                (s.category == "backlog" || s.category == "unstarted" || s.category == "started")
                                    && (s.team_id.is_none() || s.team_id.as_ref() == Some(&t.team_id))
                            })
                            .map(|s| s.status_id)
                            .collect()
                    } else {
                        Vec::new()
                    };
                    filter_clauses.set(vec![FilterClause {
                        field: "status".to_string(),
                        operator: "any_of".to_string(),
                        values: status_ids,
                    }]);
                };

                let on_active = move |_: web_sys::MouseEvent| {
                    push_next_nav.set(true);
                    set_active_tab.set("active".to_string());
                    set_search.set(String::new());
                    set_sort_field.set(SortField::Priority);
                    set_sort_direction.set(SortDirection::Asc);
                    let team = resolved_team.get();
                    let status_ids: Vec<String> = if let (Some(store), Some(t)) = (sync_store, &team) {
                        store
                            .statuses()
                            .get()
                            .into_iter()
                            .filter(|s| {
                                (s.category == "unstarted" || s.category == "started")
                                    && (s.team_id.is_none() || s.team_id.as_ref() == Some(&t.team_id))
                            })
                            .map(|s| s.status_id)
                            .collect()
                    } else {
                        Vec::new()
                    };
                    filter_clauses.set(vec![FilterClause {
                        field: "status".to_string(),
                        operator: "any_of".to_string(),
                        values: status_ids,
                    }]);
                };

                let on_backlog = move |_: web_sys::MouseEvent| {
                    push_next_nav.set(true);
                    set_active_tab.set("backlog".to_string());
                    set_search.set(String::new());
                    set_sort_field.set(SortField::Priority);
                    set_sort_direction.set(SortDirection::Asc);
                    let team = resolved_team.get();
                    let status_ids: Vec<String> = if let (Some(store), Some(t)) = (sync_store, &team) {
                        store
                            .statuses()
                            .get()
                            .into_iter()
                            .filter(|s| {
                                s.category == "backlog"
                                    && (s.team_id.is_none() || s.team_id.as_ref() == Some(&t.team_id))
                            })
                            .map(|s| s.status_id)
                            .collect()
                    } else {
                        Vec::new()
                    };
                    filter_clauses.set(vec![FilterClause {
                        field: "status".to_string(),
                        operator: "any_of".to_string(),
                        values: status_ids,
                    }]);
                };

                Some(view! {
                    <div class="px-5 py-1.5 flex items-center gap-1 bg-background shrink-0 flex-wrap">
                        // Default tabs
                        {move || {
                            let tab = active_tab.get();
                            let issues_v = if tab == "issues" { ButtonVariant::PillActive } else { ButtonVariant::Pill };
                            let active_v = if tab == "active" { ButtonVariant::PillActive } else { ButtonVariant::Pill };
                            let backlog_v = if tab == "backlog" { ButtonVariant::PillActive } else { ButtonVariant::Pill };
                            view! {
                                <Button variant=issues_v size=ButtonSize::Pill on:click=on_issues>"Issues"</Button>
                                <Button variant=active_v size=ButtonSize::Pill on:click=on_active>"Active"</Button>
                                <Button variant=backlog_v size=ButtonSize::Pill on:click=on_backlog>"Backlog"</Button>
                            }
                        }}

                        // Custom view tabs
                        {move || {
                            let views = custom_views.get();
                            let current_tab = active_tab.get();
                            let open_menu_id = context_menu_view_id.get();
                            let current_renaming = renaming_view_id.get();
                            views.into_iter().map(|v| {
                                let view_id = v.view_id.clone();
                                let tab_id = format!("view:{}", v.view_id);
                                let is_active = current_tab == tab_id;
                                let is_renaming = current_renaming.as_deref() == Some(v.view_id.as_str());
                                let is_menu_open = open_menu_id.as_deref() == Some(v.view_id.as_str());
                                let filters_json = v.filters.clone();
                                let name = v.name.clone();
                                let tab_id_click = tab_id;

                                let on_view_click = move |_: web_sys::MouseEvent| {
                                    if renaming_view_id.get_untracked().is_some() {
                                        return;
                                    }
                                    push_next_nav.set(true);
                                    set_context_menu_view_id.set(None);
                                    set_active_tab.set(tab_id_click.clone());
                                    // Determine format by probing for the "clauses" key.
                                    // ViewFilters has #[serde(default)] on all fields, so
                                    // serde_json always succeeds — we must check explicitly.
                                    let is_new_format = filters_json.contains("\"clauses\"");
                                    if is_new_format {
                                        match serde_json::from_str::<ViewFilters>(&filters_json) {
                                            Ok(filters) => {
                                                filter_clauses.set(filters.clauses);
                                                set_sort_field.set(
                                                    filters.sort_field.as_deref()
                                                        .and_then(parse_sort_field)
                                                        .unwrap_or(SortField::Priority)
                                                );
                                                set_sort_direction.set(
                                                    filters.sort_direction.as_deref()
                                                        .and_then(SortDirection::parse)
                                                        .unwrap_or(SortDirection::Asc)
                                                );
                                            }
                                            Err(e) => tracing::warn!("Failed to parse view filters: {e}"),
                                        }
                                    } else {
                                        match serde_json::from_str::<LegacyViewFilters>(&filters_json) {
                                            Ok(legacy) => {
                                                let converted = legacy.into_view_filters();
                                                filter_clauses.set(converted.clauses);
                                                set_sort_field.set(
                                                    converted.sort_field.as_deref()
                                                        .and_then(parse_sort_field)
                                                        .unwrap_or(SortField::Priority)
                                                );
                                                set_sort_direction.set(
                                                    converted.sort_direction.as_deref()
                                                        .and_then(SortDirection::parse)
                                                        .unwrap_or(SortDirection::Asc)
                                                );
                                            }
                                            Err(e) => tracing::warn!("Failed to parse legacy view filters: {e}"),
                                        }
                                    }
                                    set_search.set(String::new());
                                };

                                let vid_ctx = view_id.clone();
                                let on_contextmenu = move |ev: web_sys::MouseEvent| {
                                    ev.prevent_default();
                                    set_context_menu_view_id.set(Some(vid_ctx.clone()));
                                };

                                if is_renaming {
                                    // Inline rename input replacing the tab text
                                    let handle_submit = handle_tab_rename_submit;
                                    view! {
                                        <div class="relative flex items-center">
                                            <form
                                                class="flex items-center"
                                                on:submit=move |ev: web_sys::SubmitEvent| {
                                                    ev.prevent_default();
                                                    handle_submit();
                                                }
                                            >
                                                <input
                                                    type="text"
                                                    autofocus=true
                                                    class="w-36 h-7 px-2 py-0.5 text-[13px] rounded-md border border-border bg-background text-foreground focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
                                                    prop:value=move || rename_value.get()
                                                    on:input=move |ev| set_rename_value.set(event_target_value(&ev))
                                                    on:blur=move |_| handle_submit()
                                                />
                                            </form>
                                        </div>
                                    }.into_any()
                                } else {
                                    // Normal tab: name + "..." button on hover + optional dropdown
                                    let active_class = if is_active {
                                        "inline-flex items-center gap-1 group relative px-3 py-1 text-[13px] font-medium rounded-md transition-colors bg-secondary text-foreground cursor-pointer"
                                    } else {
                                        "inline-flex items-center gap-1 group relative px-3 py-1 text-[13px] font-medium rounded-md transition-colors text-muted-foreground hover:text-foreground hover:bg-secondary/50 cursor-pointer"
                                    };

                                    let vid_dots = view_id.clone();
                                    let vid_rename = view_id.clone();
                                    let name_for_rename = name.clone();
                                    let vid_delete = view_id.clone();

                                    view! {
                                        <div
                                            class=active_class
                                            role="tab"
                                            tabindex=0
                                            on:click=on_view_click
                                            on:contextmenu=on_contextmenu
                                        >
                                            {name}
                                            // "..." button — visible on hover or when menu is open
                                            <button
                                                class=move || {
                                                    if is_menu_open {
                                                        "p-0.5 rounded text-muted-foreground hover:text-foreground transition-colors focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
                                                    } else {
                                                        "p-0.5 rounded text-muted-foreground hover:text-foreground transition-colors opacity-0 group-hover:opacity-100 focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
                                                    }
                                                }
                                                on:mousedown=move |ev: web_sys::MouseEvent| ev.stop_propagation()
                                                on:click=move |ev: web_sys::MouseEvent| {
                                                    ev.stop_propagation();
                                                    if context_menu_view_id.get_untracked().as_deref() == Some(vid_dots.as_str()) {
                                                        set_context_menu_view_id.set(None);
                                                    } else {
                                                        set_context_menu_view_id.set(Some(vid_dots.clone()));
                                                    }
                                                }
                                                title="View actions"
                                            >
                                                <Icon icon=phosphor_leptos::DOTS_THREE weight=IconWeight::Bold size="14px"/>
                                            </button>

                                            // Context menu dropdown
                                            {if is_menu_open {
                                                let vid_rename = vid_rename.clone();
                                                let name_for_rename = name_for_rename.clone();
                                                let vid_delete = vid_delete.clone();
                                                Some(view! {
                                                    <div class="absolute left-0 top-full mt-1 w-40 bg-popover border border-border rounded-lg shadow-lg py-1 z-50">
                                                        <button
                                                            class="w-full text-left px-4 py-2 text-sm text-foreground hover:bg-secondary transition-colors focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
                                                            on:mousedown=move |ev: web_sys::MouseEvent| ev.stop_propagation()
                                                            on:click=move |ev: web_sys::MouseEvent| {
                                                                ev.stop_propagation();
                                                                set_context_menu_view_id.set(None);
                                                                set_rename_value.set(name_for_rename.clone());
                                                                set_renaming_view_id.set(Some(vid_rename.clone()));
                                                            }
                                                        >
                                                            "Rename"
                                                        </button>
                                                        <button
                                                            class="w-full text-left px-4 py-2 text-sm text-destructive hover:bg-secondary transition-colors focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
                                                            on:mousedown=move |ev: web_sys::MouseEvent| ev.stop_propagation()
                                                            on:click=move |ev: web_sys::MouseEvent| {
                                                                ev.stop_propagation();
                                                                set_context_menu_view_id.set(None);
                                                                set_confirm_delete_view_id.set(Some(vid_delete.clone()));
                                                            }
                                                        >
                                                            "Delete"
                                                        </button>
                                                    </div>
                                                })
                                            } else {
                                                None
                                            }}
                                        </div>
                                    }.into_any()
                                }
                            }).collect_view()
                        }}

                        // + button to save a new view
                        <Button
                            variant=ButtonVariant::GhostMuted
                            size=ButtonSize::IconXs
                            on:click=move |_| set_show_save_view.set(true)
                            aria_label="Save current filters as a view"
                        >
                            <Icon icon=phosphor_leptos::PLUS size="14px"/>
                        </Button>
                    </div>
                })
            }}

            // ── Toolbar (list view only) ────────────────────────────────────
            <Show when=move || view_mode.get() == "list">
                <div class="bg-background px-5 py-2 flex items-center gap-3 shrink-0 flex-wrap">
                    <SearchInput
                        value=Signal::derive(move || search.get())
                        on_input=Callback::new(move |v: String| set_search.set(v))
                        placeholder="Search issues..."
                        class="flex-1 max-w-sm"
                    />
                    <FilterBar
                        clauses=filter_clauses
                        team_id=Signal::derive(move || resolved_team.get().map(|t| t.team_id.clone()))
                    />
                    <SortDropdown
                        field=Signal::derive(move || sort_field.get())
                        direction=Signal::derive(move || sort_direction.get())
                        on_change=Callback::new(move |(f, d): (SortField, SortDirection)| {
                            set_sort_field.set(f);
                            set_sort_direction.set(d);
                        })
                    />
                    <button
                        class=move || {
                            if show_archived.get() {
                                "px-2 py-1 text-xs rounded-md border border-primary bg-primary/10 text-primary transition-colors flex items-center gap-1"
                            } else {
                                "px-2 py-1 text-xs rounded-md border border-border text-muted-foreground hover:text-foreground transition-colors flex items-center gap-1"
                            }
                        }
                        on:click=move |_| set_show_archived.update(|v| *v = !*v)
                        title="Show archived issues"
                    >
                        <Icon icon=phosphor_leptos::ARCHIVE size="14px"/>
                        {move || if show_archived.get() { "Hide archived" } else { "Show archived" }}
                    </button>
                    // "Save view" — always visible, disabled when no filters active
                    <Button
                        variant=ButtonVariant::Ghost
                        size=ButtonSize::Sm
                        disabled=Signal::derive(move || {
                            search.get().is_empty()
                                && filter_clauses.get().is_empty()
                        })
                        on:click=move |_| set_show_save_view.set(true)
                    >
                        <Icon icon=phosphor_leptos::FLOPPY_DISK size="14px"/>
                        "Save view"
                    </Button>
                </div>
            </Show>

            // ── Error alert ─────────────────────────────────────────────────
            <Show when=move || error_msg.get().is_some()>
                <div class="mx-4 mt-4">
                    <Alert variant=AlertVariant::Error>
                        {move || error_msg.get().unwrap_or_default()}
                    </Alert>
                </div>
            </Show>

            // ── Content area ────────────────────────────────────────────────
            {move || {
                if view_mode.get() == "board" {
                    if let Some(key) = team_key_string.get() {
                        view! {
                            <BoardContent
                                issues=team_issues
                                statuses=board_filtered_statuses
                                sync_store=sync_store
                                team_key=key
                            />
                        }.into_any()
                    } else {
                        view! {
                            <BoardContent
                                issues=team_issues
                                statuses=board_filtered_statuses
                                sync_store=sync_store
                            />
                        }.into_any()
                    }
                } else {
                    view! {
                        <div class="flex-1 overflow-y-auto">
                            {move || {
                                let list = sorted_issues.get();

                                // Update keyboard navigation bounds.
                                issue_count.set(list.len());
                                issue_identifiers.set(list.iter().map(|i| format!("{}-{}", i.team_key, i.number)).collect());
                                if let Some(idx) = selected_index.get_untracked()
                                    && idx >= list.len()
                                {
                                    set_selected_index.set(if list.is_empty() { None } else { Some(list.len() - 1) });
                                }

                                if list.is_empty() {
                                    let empty_icon: Arc<dyn Fn() -> AnyView + Send + Sync> = Arc::new(move || {
                                        view! {
                                            <Icon icon=phosphor_leptos::CLIPBOARD_TEXT weight=phosphor_leptos::IconWeight::Duotone size="48px"/>
                                        }.into_any()
                                    });
                                    let empty_action: Arc<dyn Fn() -> AnyView + Send + Sync> = Arc::new(move || {
                                        view! {
                                            <Button on:click=move |_| set_show_new_issue.set(true)>
                                                <Icon icon=phosphor_leptos::PLUS size="14px"/>
                                                "New Issue"
                                            </Button>
                                        }.into_any()
                                    });
                                    view! {
                                        <div class="p-4 md:p-6">
                                            <EmptyState
                                                icon=empty_icon
                                                title="No issues yet"
                                                description="Create your first issue to get started"
                                                action=empty_action
                                            />
                                        </div>
                                    }.into_any()
                                } else {
                                    let rows = list.iter().enumerate().map(|(idx, issue)| {
                                        let archived = is_archived(issue, ARCHIVE_DAYS);
                                        view! { <IssueRow issue=issue.clone() index=idx selected_index=selected_index archived=archived/> }
                                    }).collect_view();
                                    view! {
                                        <div role="list">
                                            {rows}
                                        </div>
                                    }.into_any()
                                }
                            }}
                        </div>
                    }.into_any()
                }
            }}
        </div>

        // ── New Issue modal ─────────────────────────────────────────────────
        <NewIssueModal
            show=Signal::derive(move || show_new_issue.get())
            on_close=Callback::new(move |()| set_show_new_issue.set(false))
            on_created=on_issue_created
            team_id=Signal::derive(move || resolved_team.get().map(|t| t.team_id.clone()))
        />

        // ── Save View modal ────────────────────────────────────────────────
        <SaveViewModal
            show=Signal::derive(move || show_save_view.get())
            on_close=Callback::new(move |()| set_show_save_view.set(false))
            filter_clauses=Signal::derive(move || filter_clauses.get())
            team_id=Signal::derive(move || resolved_team.get().map(|t| t.team_id.clone()))
            view_mode=Signal::derive(move || view_mode.get())
            sort_field=Signal::derive(move || sort_field.get())
            sort_direction=Signal::derive(move || sort_direction.get())
        />

        // ── Delete View confirmation dialog ────────────────────────────────
        <ConfirmDialog
            open=Signal::derive(move || confirm_delete_view_id.get().is_some())
            title="Delete view?"
            message="This saved view will be permanently deleted."
            confirm_text="Delete"
            on_confirm=Callback::new(move |()| {
                handle_tab_delete();
            })
            on_cancel=Callback::new(move |()| set_confirm_delete_view_id.set(None))
        />
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// New Issue Modal
// ─────────────────────────────────────────────────────────────────────────────

/// Modal form for creating a new issue.
///
/// Fields: title (required), description (textarea), priority (select).
/// Uses the `create_issue` server function via spawn_local.
#[component]
pub(crate) fn NewIssueModal(
    /// Whether the modal is visible.
    show: Signal<bool>,
    /// Called when the modal should close (cancel, escape, backdrop click).
    on_close: Callback<()>,
    /// Called after an issue is successfully created.
    on_created: Callback<()>,
    /// When on a team page, the team_id to assign to newly created issues.
    #[prop(optional, into)]
    team_id: Option<Signal<Option<String>>>,
    /// When creating a sub-issue, the parent issue ID to attach.
    #[prop(optional, into)]
    parent_issue_id: Option<Signal<Option<String>>>,
    /// When creating a sub-issue, the parent's display title (e.g. "TEAM-42 Fix bug").
    #[prop(optional, into)]
    parent_title: Option<Signal<Option<String>>>,
) -> impl IntoView {
    let (title, set_title) = signal(String::new());
    let (description, set_description) = signal(String::new());
    let (priority, set_priority) = signal("0".to_string());
    let (submitting, set_submitting) = signal(false);
    let (error_msg, set_error_msg) = signal(Option::<String>::None);

    // Reset form state when modal opens — signals are reset synchronously
    // before Select reconstructs, ensuring clean state on every open.
    Effect::new(move || {
        if show.get() {
            set_title.set(String::new());
            set_description.set(String::new());
            set_priority.set("0".to_string());
            set_error_msg.set(None);
            set_submitting.set(false);
        }
    });

    let handle_submit = move || {
        let title_val = title.get_untracked();
        if title_val.trim().is_empty() {
            return;
        }

        let desc_val = description.get_untracked();
        let desc = if desc_val.trim().is_empty() { None } else { Some(desc_val) };
        let prio = priority.get_untracked().parse::<i32>().unwrap_or(0);
        let current_team_id = team_id.and_then(|s| s.get_untracked());
        let current_parent_id = parent_issue_id.and_then(|s| s.get_untracked());

        set_submitting.set(true);
        set_error_msg.set(None);

        leptos::task::spawn_local(async move {
            match create_issue(title_val, desc, prio, None, None, String::new(), None, None, current_parent_id, current_team_id, None).await {
                Ok(_) => {
                    set_submitting.set(false);
                    on_created.run(());
                }
                Err(e) => {
                    set_submitting.set(false);
                    set_error_msg.set(Some(format!("Failed to create issue: {e}")));
                }
            }
        });
    };

    let title_empty = Memo::new(move |_| title.get().trim().is_empty());

    // Modal footer — extracted as Arc<dyn Fn() -> AnyView> per codebase pattern (see team.rs).
    let handle_submit_for_footer = handle_submit;
    let modal_footer: Arc<dyn Fn() -> AnyView + Send + Sync> = Arc::new(move || {
        let submit = handle_submit_for_footer;
        view! {
            <Button
                variant=ButtonVariant::Ghost
                on:click=move |_| on_close.run(())
            >
                "Cancel"
            </Button>
            <Button
                disabled=Signal::derive(move || submitting.get() || title_empty.get())
                on:click=move |_| submit()
            >
                {move || if submitting.get() {
                    "Creating..."
                } else if parent_issue_id.is_some_and(|s| s.get().is_some()) {
                    "Create Sub-issue"
                } else {
                    "Create Issue"
                }}
            </Button>
        }.into_any()
    });

    let modal_title = Signal::derive(move || {
        if parent_issue_id.is_some_and(|s| s.get().is_some()) {
            "New Sub-issue".to_string()
        } else {
            "New Issue".to_string()
        }
    });

    view! {
        <Modal
            show=show
            on_close=on_close
            title=modal_title
            size=ModalSize::Lg
            footer=modal_footer
        >
            <form
                on:submit=move |ev: web_sys::SubmitEvent| {
                    ev.prevent_default();
                    handle_submit();
                }
                class="space-y-4"
            >
                // Error message
                <Show when=move || error_msg.get().is_some()>
                    <crate::components::Alert variant=crate::components::AlertVariant::Error>
                        <crate::components::AlertDescription>
                            {move || error_msg.get().unwrap_or_default()}
                        </crate::components::AlertDescription>
                    </crate::components::Alert>
                </Show>

                // Parent indicator (shown when creating a sub-issue)
                {move || parent_title.and_then(|s| s.get()).map(|title| {
                    view! {
                        <div class="flex items-center gap-1.5 text-xs text-muted-foreground mb-2">
                            <Icon icon=phosphor_leptos::ARROW_BEND_DOWN_RIGHT size="12px"/>
                            <span>"Sub-issue of "</span>
                            <span class="font-medium text-foreground">{title}</span>
                        </div>
                    }
                })}

                // Title
                <div class="space-y-2">
                    <label for="issue-title" class="text-sm font-medium text-foreground">
                        "Title"
                    </label>
                    <input
                        id="issue-title"
                        type="text"
                        required=true
                        autofocus=true
                        placeholder="Issue title"
                        class=INPUT_CLASS
                        prop:value=move || title.get()
                        on:input=move |ev| set_title.set(event_target_value(&ev))
                    />
                </div>

                // Description (WYSIWYG)
                <div class="space-y-2">
                    <label class="text-sm font-medium text-foreground">
                        "Description"
                    </label>
                    <div class="border border-border rounded-md overflow-hidden" style="min-height: 120px;">
                        <kode_leptos::TreeWysiwygEditor
                            content=Signal::stored(String::new())
                            on_change=Arc::new(move |text: String| {
                                set_description.set(text);
                            })
                            show_fixed_toolbar=false
                            show_floating_toolbar=true
                            theme={
                                let theme_state = use_context::<crate::components::theme::ThemeState>();
                                Signal::derive(move || {
                                    let mut theme = super::issue_detail::trakkt_kode_theme();
                                    theme.content_padding = Some("0.75rem 1rem");
                                    if let Some(ts) = theme_state
                                        && ts.effective.get() == "dark"
                                    {
                                        theme.syntax = kode_leptos::SyntaxTheme::OneDark;
                                    }
                                    theme
                                })
                            }
                        />
                    </div>
                </div>

                // Priority
                <div class="space-y-2">
                    <label class="text-sm font-medium text-foreground">
                        "Priority"
                    </label>
                    <Select
                        value=priority
                        options=Signal::derive(|| vec![
                            ("0".to_string(), "None".to_string()),
                            ("1".to_string(), "Urgent".to_string()),
                            ("2".to_string(), "High".to_string()),
                            ("3".to_string(), "Medium".to_string()),
                            ("4".to_string(), "Low".to_string()),
                        ])
                        on_change=Callback::new(move |v: String| set_priority.set(v))
                        variant=SelectVariant::Form
                    />
                </div>
            </form>
        </Modal>
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Save View Modal
// ─────────────────────────────────────────────────────────────────────────────

/// Modal form for saving the current filter set as a named view.
///
/// Serializes the composable filter clauses using the new `ViewFilters` format.
#[component]
pub(crate) fn SaveViewModal(
    /// Whether the modal is visible.
    show: Signal<bool>,
    /// Called when the modal should close.
    on_close: Callback<()>,
    /// Current composable filter clauses.
    filter_clauses: Signal<Vec<FilterClause>>,
    /// Current team_id if on a team page.
    team_id: Signal<Option<String>>,
    /// Current view mode (list/board).
    view_mode: Signal<String>,
    /// Current sort field.
    #[prop(optional, into)]
    sort_field: Option<Signal<SortField>>,
    /// Current sort direction.
    #[prop(optional, into)]
    sort_direction: Option<Signal<SortDirection>>,
) -> impl IntoView {
    let (name, set_name) = signal(String::new());
    let (submitting, set_submitting) = signal(false);
    let (error_msg, set_error_msg) = signal(Option::<String>::None);

    // Reset form state when modal opens.
    Effect::new(move || {
        if show.get() {
            set_name.set(String::new());
            set_error_msg.set(None);
            set_submitting.set(false);
        }
    });

    let handle_submit = move || {
        let name_val = name.get_untracked().trim().to_string();
        if name_val.is_empty() {
            return;
        }

        let team_id_val = team_id.get_untracked().unwrap_or_default();

        // Serialize using the new ViewFilters format.
        let filters = ViewFilters {
            clauses: filter_clauses.get_untracked(),
            sort_field: sort_field.map(|s| sort_field_to_str(s.get_untracked()).to_string()),
            sort_direction: sort_direction.map(|s| s.get_untracked().as_str().to_string()),
        };
        let filters_str = match serde_json::to_string(&filters) {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!("Failed to serialize view filters: {e}");
                set_error_msg.set(Some(format!("Failed to serialize filters: {e}")));
                return;
            }
        };

        let display_options = serde_json::json!({
            "view_type": view_mode.get_untracked(),
        });
        let display_str = display_options.to_string();

        set_submitting.set(true);
        set_error_msg.set(None);

        leptos::task::spawn_local(async move {
            let view_team_id = if team_id_val.is_empty() { None } else { Some(team_id_val.clone()) };
            match create_view(name_val, None, filters_str, display_str, false, view_team_id, 0).await {
                Ok(_) => {
                    set_submitting.set(false);
                    on_close.run(());
                }
                Err(e) => {
                    set_submitting.set(false);
                    set_error_msg.set(Some(format!("Failed to save view: {e}")));
                }
            }
        });
    };

    let name_empty = Memo::new(move |_| name.get().trim().is_empty());

    let handle_submit_for_footer = handle_submit;
    let modal_footer: Arc<dyn Fn() -> AnyView + Send + Sync> = Arc::new(move || {
        let submit = handle_submit_for_footer;
        view! {
            <Button
                variant=ButtonVariant::Ghost
                on:click=move |_| on_close.run(())
            >
                "Cancel"
            </Button>
            <Button
                disabled=Signal::derive(move || submitting.get() || name_empty.get())
                on:click=move |_| submit()
            >
                {move || if submitting.get() { "Saving..." } else { "Save View" }}
            </Button>
        }.into_any()
    });

    view! {
        <Modal
            show=show
            on_close=on_close
            title="Save View"
            size=ModalSize::Sm
            footer=modal_footer
        >
            <form
                on:submit=move |ev: web_sys::SubmitEvent| {
                    ev.prevent_default();
                    handle_submit();
                }
                class="space-y-4"
            >
                // Error message
                <Show when=move || error_msg.get().is_some()>
                    <crate::components::Alert variant=crate::components::AlertVariant::Error>
                        <crate::components::AlertDescription>
                            {move || error_msg.get().unwrap_or_default()}
                        </crate::components::AlertDescription>
                    </crate::components::Alert>
                </Show>

                // View name
                <div class="space-y-2">
                    <label for="view-name" class="text-sm font-medium text-foreground">
                        "Name"
                    </label>
                    <input
                        id="view-name"
                        type="text"
                        required=true
                        autofocus=true
                        placeholder="e.g. High priority bugs"
                        class=INPUT_CLASS
                        prop:value=move || name.get()
                        on:input=move |ev| set_name.set(event_target_value(&ev))
                    />
                </div>
            </form>
        </Modal>
    }
}
