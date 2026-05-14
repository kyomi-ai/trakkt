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
use leptos_router::hooks::use_navigate;
use phosphor_leptos::{Icon, IconWeight};
use wasm_bindgen::closure::Closure;
use wasm_bindgen::JsCast;

use crate::components::{
    Alert, AlertVariant,
    Button, ButtonSize, ButtonVariant, ConfirmDialog, EmptyState,
    Modal, ModalSize,
    SearchInput, StyledSelect, TeamIcon, INPUT_CLASS,
};
use crate::pages::board::BoardContent;
use crate::pages::issues::filters::{
    parse_sort_field, sort_field_to_str, sort_issues, LabelFilterDropdown, PriorityFilterDropdown,
    ProjectFilterDropdown, SortDirection, SortDropdown, SortField, StatusFilterDropdown,
};
use crate::pages::issues::issue_row::IssueRow;
use crate::pages::issues::{is_archived, ARCHIVE_DAYS};
use crate::pages::views::ViewFilters;
use crate::server_fns::issues::{create_issue, list_issues};
use crate::server_fns::statuses::list_statuses;
use crate::server_fns::views::{create_view, delete_view, update_view};
use crate::utils::keyboard::is_input_focused;
use trakkt_types::models::{Status, Team};

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

/// Inner implementation shared by `IssueListPage` (no team) and
/// `IssueListForTeam` (team-scoped). All filtering, title, and create-issue
/// logic lives here.
#[component]
fn IssueListInner(
    /// Optional reactive team key. When `Some`, filters issues and statuses by team.
    #[prop(optional, into)]
    team_key: Option<Signal<String>>,
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
    let (status_filter, set_status_filter) = signal(Vec::<String>::new());
    let (priority_filter, set_priority_filter) = signal(Vec::<String>::new());
    let (label_filter, set_label_filter) = signal(Vec::<String>::new());
    let (project_filter, set_project_filter) = signal(Vec::<String>::new());
    let (show_archived, set_show_archived) = signal(false);

    // ── Sort state ─────────────────────────────────────────────────────────
    let (sort_field, set_sort_field) = signal(SortField::Priority);
    let (sort_direction, set_sort_direction) = signal(SortDirection::Asc);

    // ── Active tab state (team-scoped pages only) ─────────────────────────
    // Values: "all", "active", "backlog", "view:{view_id}"
    let (active_tab, set_active_tab) = signal("all".to_string());

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

    // ── Team-scoped views (custom tabs) ──────────────────────────────────
    let team_views = Memo::new(move |_| {
        let team = resolved_team.get();
        let Some(store) = sync_store else { return Vec::new() };
        let Some(ref t) = team else { return Vec::new() };
        let mut views: Vec<trakkt_types::models::View> = store
            .views()
            .get()
            .into_iter()
            .filter(|v| v.team_id.as_deref() == Some(t.team_id.as_str()))
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
        // If the deleted view was the active tab, reset to "All Issues".
        let current_tab = active_tab.get_untracked();
        if current_tab == format!("view:{vid}") {
            set_active_tab.set("all".to_string());
            set_search.set(String::new());
            set_status_filter.set(Vec::new());
            set_priority_filter.set(Vec::new());
            set_label_filter.set(Vec::new());
            set_project_filter.set(Vec::new());
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

    // Reset active tab and sort when navigating between teams.
    Effect::new(move |_| {
        let _ = resolved_team.get();
        set_active_tab.set("all".to_string());
        set_sort_field.set(SortField::Priority);
        set_sort_direction.set(SortDirection::Asc);
    });

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

    // Filtered issue list for list view — applies archive, search, status, priority,
    // label, and project filters.
    let filtered_issues = Memo::new(move |_| {
        let raw = team_issues.get();
        let search_val = search.get().to_lowercase();
        let status_val = status_filter.get();
        let priority_val = priority_filter.get();
        let label_val = label_filter.get();
        let project_val = project_filter.get();
        let archived_visible = show_archived.get();

        raw.into_iter()
            .filter(|issue| {
                // Archive filter: hide archived issues unless the toggle is on.
                if !archived_visible && is_archived(issue, ARCHIVE_DAYS) {
                    return false;
                }
                if !status_val.is_empty() && !status_val.contains(&issue.status_id) {
                    return false;
                }
                if !priority_val.is_empty() {
                    let p_str = issue.priority.to_string();
                    if !priority_val.contains(&p_str) {
                        return false;
                    }
                }
                // Label filter: OR within labels — issue matches if it has ANY of the selected labels.
                if !label_val.is_empty() {
                    let has_match = issue.labels.iter().any(|l| label_val.contains(&l.label_id));
                    if !has_match {
                        return false;
                    }
                }
                // Project filter: OR within projects — issue matches if its project_id is any of the selected.
                if !project_val.is_empty() {
                    match &issue.project_id {
                        Some(pid) if project_val.contains(pid) => {}
                        _ => return false,
                    }
                }
                if !search_val.is_empty() && !issue.title.to_lowercase().contains(&search_val) {
                    return false;
                }
                true
            })
            .collect::<Vec<_>>()
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
    // When a view tab sets status_filter, only show matching board columns.
    let board_filtered_statuses = Memo::new(move |_| {
        let all = board_statuses.get();
        let status_val = status_filter.get();
        if status_val.is_empty() {
            all
        } else {
            all.into_iter()
                .filter(|s| status_val.contains(&s.status_id))
                .collect()
        }
    });

    // ── New Issue modal state ───────────────────────────────────────────────
    let (show_new_issue, set_show_new_issue) = signal(false);

    let on_issue_created = Callback::new(move |()| {
        set_show_new_issue.set(false);
        set_version.update(|v| *v += 1);
    });

    // ── Save View modal state ──────────────────────────────────────────────
    let (show_save_view, set_show_save_view) = signal(false);

    // ── Keyboard navigation state ──────────────────────────────────────────
    let (selected_index, set_selected_index) = signal(Option::<usize>::None);

    // Track the current issue count so keyboard handlers know the bounds.
    let issue_count = RwSignal::new(0usize);

    // Track issue identifiers (e.g. "TRA-42") so Enter can navigate to the selected issue.
    let issue_identifiers = RwSignal::new(Vec::<String>::new());

    // ── j/k/Enter/c keyboard listener (window-level, active on this page) ──
    // Hoist use_navigate to component construction time (not inside closures).
    let nav = use_navigate();
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
                            nav(&format!("/issues/{identifier}"), Default::default());
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

                let on_all = move |_: web_sys::MouseEvent| {
                    set_active_tab.set("all".to_string());
                    set_search.set(String::new());
                    set_status_filter.set(Vec::new());
                    set_priority_filter.set(Vec::new());
                    set_label_filter.set(Vec::new());
                    set_project_filter.set(Vec::new());
                    set_sort_field.set(SortField::Priority);
                    set_sort_direction.set(SortDirection::Asc);
                };

                let on_active = move |_: web_sys::MouseEvent| {
                    set_active_tab.set("active".to_string());
                    set_search.set(String::new());
                    set_priority_filter.set(Vec::new());
                    set_label_filter.set(Vec::new());
                    set_project_filter.set(Vec::new());
                    set_sort_field.set(SortField::Priority);
                    set_sort_direction.set(SortDirection::Asc);
                    let team = resolved_team.get();
                    let status_ids: Vec<String> = if let (Some(store), Some(t)) = (sync_store, &team) {
                        store
                            .statuses()
                            .get()
                            .into_iter()
                            .filter(|s| {
                                s.category == "started"
                                    && (s.team_id.is_none() || s.team_id.as_ref() == Some(&t.team_id))
                            })
                            .map(|s| s.status_id)
                            .collect()
                    } else {
                        Vec::new()
                    };
                    set_status_filter.set(status_ids);
                };

                let on_backlog = move |_: web_sys::MouseEvent| {
                    set_active_tab.set("backlog".to_string());
                    set_search.set(String::new());
                    set_priority_filter.set(Vec::new());
                    set_label_filter.set(Vec::new());
                    set_project_filter.set(Vec::new());
                    set_sort_field.set(SortField::Priority);
                    set_sort_direction.set(SortDirection::Asc);
                    let team = resolved_team.get();
                    let status_ids: Vec<String> = if let (Some(store), Some(t)) = (sync_store, &team) {
                        store
                            .statuses()
                            .get()
                            .into_iter()
                            .filter(|s| {
                                (s.category == "backlog" || s.category == "unstarted")
                                    && (s.team_id.is_none() || s.team_id.as_ref() == Some(&t.team_id))
                            })
                            .map(|s| s.status_id)
                            .collect()
                    } else {
                        Vec::new()
                    };
                    set_status_filter.set(status_ids);
                };

                Some(view! {
                    <div class="px-5 py-1.5 flex items-center gap-1 bg-background shrink-0 flex-wrap">
                        // Default tabs
                        {move || {
                            let tab = active_tab.get();
                            let all_v = if tab == "all" { ButtonVariant::PillActive } else { ButtonVariant::Pill };
                            let active_v = if tab == "active" { ButtonVariant::PillActive } else { ButtonVariant::Pill };
                            let backlog_v = if tab == "backlog" { ButtonVariant::PillActive } else { ButtonVariant::Pill };
                            view! {
                                <Button variant=all_v size=ButtonSize::Pill on:click=on_all>"All Issues"</Button>
                                <Button variant=active_v size=ButtonSize::Pill on:click=on_active>"Active"</Button>
                                <Button variant=backlog_v size=ButtonSize::Pill on:click=on_backlog>"Backlog"</Button>
                            }
                        }}

                        // Custom view tabs
                        {move || {
                            let views = team_views.get();
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
                                    set_context_menu_view_id.set(None);
                                    set_active_tab.set(tab_id_click.clone());
                                    match serde_json::from_str::<ViewFilters>(&filters_json) {
                                        Ok(filters) => {
                                            set_search.set(filters.search);
                                            set_status_filter.set(filters.statuses);
                                            set_priority_filter.set(
                                                filters.priorities.iter().map(|p| p.to_string()).collect()
                                            );
                                            set_label_filter.set(filters.labels);
                                            set_project_filter.set(filters.project_ids);
                                            // Restore sort from saved view (or reset to defaults).
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
                                        Err(e) => {
                                            tracing::warn!("Failed to parse view filters: {e}");
                                        }
                                    }
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
                                        "inline-flex items-center gap-1 group relative px-3 py-1 text-[13px] font-medium rounded-md transition-colors bg-secondary text-foreground"
                                    } else {
                                        "inline-flex items-center gap-1 group relative px-3 py-1 text-[13px] font-medium rounded-md transition-colors text-muted-foreground hover:text-foreground hover:bg-secondary/50"
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
                <div class="bg-background px-5 py-2 flex items-center gap-3 shrink-0">
                    <SearchInput
                        value=Signal::derive(move || search.get())
                        on_input=Callback::new(move |v: String| set_search.set(v))
                        placeholder="Search issues..."
                        class="flex-1 max-w-sm"
                    />
                    <StatusFilterDropdown
                        value=status_filter
                        on_change=Callback::new(move |v: Vec<String>| set_status_filter.set(v))
                        team_id=Signal::derive(move || resolved_team.get().map(|t| t.team_id.clone()))
                    />
                    <PriorityFilterDropdown value=priority_filter on_change=Callback::new(move |v: Vec<String>| set_priority_filter.set(v))/>
                    <LabelFilterDropdown
                        value=label_filter
                        on_change=Callback::new(move |v: Vec<String>| set_label_filter.set(v))
                        team_id=Signal::derive(move || resolved_team.get().map(|t| t.team_id.clone()))
                    />
                    <ProjectFilterDropdown value=project_filter on_change=Callback::new(move |v: Vec<String>| set_project_filter.set(v))/>
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
                                && status_filter.get().is_empty()
                                && priority_filter.get().is_empty()
                                && label_filter.get().is_empty()
                                && project_filter.get().is_empty()
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
            search=Signal::derive(move || search.get())
            status_filter=Signal::derive(move || status_filter.get())
            priority_filter=Signal::derive(move || priority_filter.get())
            label_filter=Signal::derive(move || label_filter.get())
            project_filter=Signal::derive(move || project_filter.get())
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
    // before StyledSelect reconstructs, ensuring clean state on every open.
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
            match create_issue(title_val, desc, prio, None, None, String::new(), None, None, current_parent_id, current_team_id).await {
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
                    <StyledSelect
                        value=priority.get_untracked()
                        options=vec![
                            ("0", "None"),
                            ("1", "Urgent"),
                            ("2", "High"),
                            ("3", "Medium"),
                            ("4", "Low"),
                        ]
                        on_change=move |v: String| set_priority.set(v)
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
#[component]
pub(crate) fn SaveViewModal(
    /// Whether the modal is visible.
    show: Signal<bool>,
    /// Called when the modal should close.
    on_close: Callback<()>,
    /// Current search filter value.
    search: Signal<String>,
    /// Current status filter values (multi-select).
    status_filter: Signal<Vec<String>>,
    /// Current priority filter values (multi-select).
    priority_filter: Signal<Vec<String>>,
    /// Current label filter values (multi-select).
    #[prop(optional, into)]
    label_filter: Option<Signal<Vec<String>>>,
    /// Current project filter values (multi-select).
    #[prop(optional, into)]
    project_filter: Option<Signal<Vec<String>>>,
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

        // Build filters JSON from current filter state.
        let statuses: Vec<String> = status_filter.get_untracked();
        let priority_strings = priority_filter.get_untracked();
        let priorities: Vec<i32> = priority_strings.iter().filter_map(|s| s.parse::<i32>().ok()).collect();

        let search_val = search.get_untracked();
        let team_id_val = team_id.get_untracked().unwrap_or_default();

        let labels: Vec<String> = label_filter.map(|s| s.get_untracked()).unwrap_or_default();
        let project_ids: Vec<String> = project_filter.map(|s| s.get_untracked()).unwrap_or_default();
        let sf_str = sort_field.map(|s| sort_field_to_str(s.get_untracked()).to_string());
        let sd_str = sort_direction.map(|s| s.get_untracked().as_str().to_string());
        let filters = serde_json::json!({
            "statuses": statuses,
            "priorities": priorities,
            "search": search_val,
            "team_id": team_id_val,
            "labels": labels,
            "project_ids": project_ids,
            "sort_field": sf_str,
            "sort_direction": sd_str,
        });

        let display_options = serde_json::json!({
            "view_type": view_mode.get_untracked(),
        });

        let filters_str = filters.to_string();
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
