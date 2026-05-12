// SPDX-License-Identifier: AGPL-3.0-or-later

//! My Issues page — shows issues grouped into three sections:
//! 1. Assigned to Me — issues where the current user is the assignee
//! 2. Created by Me — issues created by the user (excluding those already in Assigned)
//! 3. Watching — issues the user is watching (excluding Assigned and Created)
//!
//! Layout follows the same patterns as `issue_list.rs`:
//! - Page header: title (no create button — users create from team pages)
//! - Toolbar: search + filter dropdowns
//! - Content: collapsible sections with issue rows
//!
//! Issue Row follows DESIGN.md "Issue Row Pattern":
//! `px-3 py-[6px] h-9 flex items-center gap-2.5 border-b border-border`
//! hover:bg-surface-alt transition-colors cursor-pointer
//! Order: Priority | Status | Issue ID (with team key) | Title | Labels | Date | Assignee

use std::collections::HashSet;
use std::sync::Arc;

use leptos::prelude::*;
use leptos_router::hooks::use_navigate;
use phosphor_leptos::Icon;
use wasm_bindgen::closure::Closure;
use wasm_bindgen::JsCast;

use crate::components::{Alert, AlertVariant, Button, ButtonSize, ButtonVariant, EmptyState, SearchInput};
use crate::pages::issues::filters::{PriorityFilterDropdown, StatusFilterDropdown};
use crate::pages::issues::issue_list::SaveViewModal;
use crate::pages::issues::issue_row::IssueRow;
use crate::pages::issues::{is_archived, ARCHIVE_DAYS};
use crate::server_fns::context::UserContext;
use crate::server_fns::issues::list_issues;
use crate::server_fns::watchers::list_watched_issue_ids;
use crate::utils::keyboard::is_input_focused;
use trakkt_types::models::IssueWithDetails;

// ─────────────────────────────────────────────────────────────────────────────
// My Issues Page
// ─────────────────────────────────────────────────────────────────────────────

/// My Issues page — displays issues in three grouped sections.
#[component]
pub fn MyIssuesPage() -> impl IntoView {
    // ── Get current user ────────────────────────────────────────────────────
    let user_ctx = expect_context::<LocalResource<Result<UserContext, ServerFnError>>>();

    // Derive the current user's ID reactively.
    let current_user_id = Memo::new(move |_| {
        user_ctx
            .get()
            .and_then(|r| r.ok())
            .map(|ctx| ctx.user_id.clone())
    });

    // ── Filter state ────────────────────────────────────────────────────────
    let (search, set_search) = signal(String::new());
    let (status_filter, set_status_filter) = signal(Vec::<String>::new());
    let (priority_filter, set_priority_filter) = signal(Vec::<String>::new());
    let (show_archived, set_show_archived) = signal(false);
    let (show_save_view, set_show_save_view) = signal(false);

    // ── Error state for server function failures ──────────────────────────
    let error_msg = RwSignal::new(Option::<String>::None);

    // ── Data source: SyncStore (real-time) with server function fallback ───
    let sync_store = use_context::<crate::cache::store::SyncStore>();

    // Server function fallback — used for initial load before sync is ready.
    let server_issues = Resource::new(
        || (),
        move |_| async move {
            list_issues(None, None, None, None, None, None, None, None).await
        },
    );

    // ── Watched issue IDs from server ─────────────────────────────────────
    // Version signal allows refetching when the page remounts or watch state changes.
    let (watcher_version, _set_watcher_version) = signal(0u32);
    let watched_ids_resource = Resource::new(
        move || watcher_version.get(),
        move |_| async move { list_watched_issue_ids().await },
    );

    // ── All issues (raw, unfiltered) ──────────────────────────────────────
    let all_issues = Memo::new(move |_| {
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
        raw
    });

    // ── Filter helper (closure over search/status/priority/archive signals) ─
    let passes_filters = move |issue: &IssueWithDetails| -> bool {
        // Archive filter: hide archived issues unless the toggle is on.
        if !show_archived.get() && is_archived(issue, ARCHIVE_DAYS) {
            return false;
        }

        let search_val = search.get().to_lowercase();
        let status_val = status_filter.get();
        let priority_val = priority_filter.get();

        if !status_val.is_empty() && !status_val.contains(&issue.status_id) {
            return false;
        }
        if !priority_val.is_empty() {
            let p_str = issue.priority.to_string();
            if !priority_val.contains(&p_str) {
                return false;
            }
        }
        if !search_val.is_empty() && !issue.title.to_lowercase().contains(&search_val) {
            return false;
        }
        true
    };

    // ── Section: Assigned to Me ───────────────────────────────────────────
    let assigned_issues = Memo::new(move |_| {
        let Some(uid) = current_user_id.get() else {
            return Vec::new();
        };
        all_issues
            .get()
            .into_iter()
            .filter(|i| i.assignee_id.as_ref() == Some(&uid))
            .filter(|i| passes_filters(i))
            .collect::<Vec<_>>()
    });

    // ── Section: Created by Me (excluding already shown in Assigned) ──────
    let created_issues = Memo::new(move |_| {
        let Some(uid) = current_user_id.get() else {
            return Vec::new();
        };
        let assigned_ids: HashSet<String> = assigned_issues
            .get()
            .iter()
            .map(|i| i.issue_id.clone())
            .collect();
        all_issues
            .get()
            .into_iter()
            .filter(|i| i.creator_id == uid && !assigned_ids.contains(&i.issue_id))
            .filter(|i| passes_filters(i))
            .collect::<Vec<_>>()
    });

    // ── Section: Watching (excluding Assigned and Created) ────────────────
    let watching_issues = Memo::new(move |_| {
        let watched = watched_ids_resource
            .get()
            .and_then(|r| r.ok())
            .unwrap_or_default();
        if watched.is_empty() {
            return Vec::new();
        }
        let watched_set: HashSet<String> = watched.into_iter().collect();
        let assigned_ids: HashSet<String> = assigned_issues
            .get()
            .iter()
            .map(|i| i.issue_id.clone())
            .collect();
        let created_ids: HashSet<String> = created_issues
            .get()
            .iter()
            .map(|i| i.issue_id.clone())
            .collect();
        all_issues
            .get()
            .into_iter()
            .filter(|i| {
                watched_set.contains(&i.issue_id)
                    && !assigned_ids.contains(&i.issue_id)
                    && !created_ids.contains(&i.issue_id)
            })
            .filter(|i| passes_filters(i))
            .collect::<Vec<_>>()
    });

    // ── Keyboard navigation state ──────────────────────────────────────────
    let (selected_index, set_selected_index) = signal(Option::<usize>::None);

    // Track the current issue count so keyboard handlers know the bounds.
    let issue_count = RwSignal::new(0usize);

    // Track issue identifiers (e.g. "TRA-42") so Enter can navigate to the selected issue.
    let issue_identifiers = RwSignal::new(Vec::<String>::new());

    // ── j/k/Enter keyboard listener (window-level, active on this page) ────
    let nav = use_navigate();
    Effect::new(move |_| {
        let Some(window) = web_sys::window() else { return };
        let nav = nav.clone();
        let cb = Closure::<dyn Fn(web_sys::KeyboardEvent)>::new(move |ev: web_sys::KeyboardEvent| {
            if is_input_focused(&ev) {
                return;
            }

            let key = ev.key();
            match key.as_str() {
                "j" => {
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
                    if let Some(idx) = selected_index.get_untracked() {
                        let ids = issue_identifiers.get_untracked();
                        if let Some(identifier) = ids.get(idx) {
                            ev.prevent_default();
                            nav(&format!("/issues/{identifier}"), Default::default());
                        }
                    }
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

    // ── Section collapse state (persisted in localStorage) ────────────────
    let (assigned_collapsed, set_assigned_collapsed) =
        signal(load_collapsed_state("trakkt-myissues-assigned-collapsed"));
    let (created_collapsed, set_created_collapsed) =
        signal(load_collapsed_state("trakkt-myissues-created-collapsed"));
    let (watching_collapsed, set_watching_collapsed) =
        signal(load_collapsed_state("trakkt-myissues-watching-collapsed"));

    // Persist collapse state changes to localStorage.
    Effect::new(move || {
        save_collapsed_state("trakkt-myissues-assigned-collapsed", assigned_collapsed.get());
    });
    Effect::new(move || {
        save_collapsed_state("trakkt-myissues-created-collapsed", created_collapsed.get());
    });
    Effect::new(move || {
        save_collapsed_state("trakkt-myissues-watching-collapsed", watching_collapsed.get());
    });

    // ── Render ──────────────────────────────────────────────────────────────
    view! {
        <div class="bg-background flex flex-col h-full">
            // ── Page header ─────────────────────────────────────────────────
            <div class="page-header h-14 px-5 flex items-center justify-between shrink-0">
                <h1 class="text-sm font-semibold text-foreground">"My Issues"</h1>
            </div>

            // ── Toolbar ─────────────────────────────────────────────────────
            <div class="bg-background px-5 py-2 flex items-center gap-3 shrink-0">
                <SearchInput
                    value=Signal::derive(move || search.get())
                    on_input=Callback::new(move |v: String| set_search.set(v))
                    placeholder="Search issues..."
                    class="flex-1 max-w-sm"
                />
                <StatusFilterDropdown value=status_filter on_change=Callback::new(move |v: Vec<String>| set_status_filter.set(v))/>
                <PriorityFilterDropdown value=priority_filter on_change=Callback::new(move |v: Vec<String>| set_priority_filter.set(v))/>
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
                    })
                    on:click=move |_| set_show_save_view.set(true)
                >
                    <Icon icon=phosphor_leptos::FLOPPY_DISK size="14px"/>
                    "Save view"
                </Button>
            </div>

            // ── Error alert ─────────────────────────────────────────────────
            <Show when=move || error_msg.get().is_some()>
                <div class="mx-4 mt-4">
                    <Alert variant=AlertVariant::Error>
                        {move || error_msg.get().unwrap_or_default()}
                    </Alert>
                </div>
            </Show>

            // ── Content area ────────────────────────────────────────────────
            <div class="flex-1 overflow-y-auto">
                {move || {
                    let assigned = assigned_issues.get();
                    let created = created_issues.get();
                    let watching = watching_issues.get();

                    // Build a flat list of all visible issues for keyboard navigation.
                    let mut all_visible: Vec<&IssueWithDetails> = Vec::new();
                    if !assigned_collapsed.get() {
                        all_visible.extend(assigned.iter());
                    }
                    if !created_collapsed.get() {
                        all_visible.extend(created.iter());
                    }
                    if !watching_collapsed.get() {
                        all_visible.extend(watching.iter());
                    }

                    // Update keyboard navigation bounds.
                    issue_count.set(all_visible.len());
                    issue_identifiers.set(all_visible.iter().map(|i| format!("{}-{}", i.team_key, i.number)).collect());
                    if let Some(idx) = selected_index.get_untracked()
                        && idx >= all_visible.len()
                    {
                        set_selected_index.set(if all_visible.is_empty() { None } else { Some(all_visible.len() - 1) });
                    }

                    // If everything is empty, show empty state.
                    let total = assigned.len() + created.len() + watching.len();
                    if total == 0 {
                        let empty_icon: Arc<dyn Fn() -> AnyView + Send + Sync> = Arc::new(move || {
                            view! {
                                <Icon icon=phosphor_leptos::CLIPBOARD_TEXT weight=phosphor_leptos::IconWeight::Duotone size="48px"/>
                            }.into_any()
                        });
                        return view! {
                            <div class="p-4 md:p-6">
                                <EmptyState
                                    icon=empty_icon
                                    title="No issues to show"
                                    description="Issues assigned to you, created by you, or that you're watching will appear here"
                                />
                            </div>
                        }.into_any();
                    }

                    // Track global index offset for keyboard selection across sections.
                    let mut global_offset = 0usize;

                    let assigned_view = if !assigned.is_empty() {
                        let count = assigned.len();
                        let offset = global_offset;
                        if !assigned_collapsed.get() {
                            global_offset += count;
                        }
                        Some(view! {
                            <CollapsibleSection
                                title="Assigned to Me"
                                count=count
                                collapsed=assigned_collapsed
                                set_collapsed=set_assigned_collapsed
                            >
                                {if !assigned_collapsed.get() {
                                    assigned.iter().enumerate().map(|(idx, issue)| {
                                        let archived = is_archived(issue, ARCHIVE_DAYS);
                                        view! { <IssueRow issue=issue.clone() index=offset+idx selected_index=selected_index archived=archived/> }
                                    }).collect_view().into_any()
                                } else {
                                    view! {}.into_any()
                                }}
                            </CollapsibleSection>
                        })
                    } else {
                        None
                    };

                    let created_view = if !created.is_empty() {
                        let count = created.len();
                        let offset = global_offset;
                        if !created_collapsed.get() {
                            global_offset += count;
                        }
                        Some(view! {
                            <CollapsibleSection
                                title="Created by Me"
                                count=count
                                collapsed=created_collapsed
                                set_collapsed=set_created_collapsed
                            >
                                {if !created_collapsed.get() {
                                    created.iter().enumerate().map(|(idx, issue)| {
                                        let archived = is_archived(issue, ARCHIVE_DAYS);
                                        view! { <IssueRow issue=issue.clone() index=offset+idx selected_index=selected_index archived=archived/> }
                                    }).collect_view().into_any()
                                } else {
                                    view! {}.into_any()
                                }}
                            </CollapsibleSection>
                        })
                    } else {
                        None
                    };

                    let watching_view = if !watching.is_empty() {
                        let count = watching.len();
                        let offset = global_offset;
                        Some(view! {
                            <CollapsibleSection
                                title="Watching"
                                count=count
                                collapsed=watching_collapsed
                                set_collapsed=set_watching_collapsed
                            >
                                {if !watching_collapsed.get() {
                                    watching.iter().enumerate().map(|(idx, issue)| {
                                        let archived = is_archived(issue, ARCHIVE_DAYS);
                                        view! { <IssueRow issue=issue.clone() index=offset+idx selected_index=selected_index archived=archived/> }
                                    }).collect_view().into_any()
                                } else {
                                    view! {}.into_any()
                                }}
                            </CollapsibleSection>
                        })
                    } else {
                        None
                    };

                    view! {
                        <div role="list">
                            {assigned_view}
                            {created_view}
                            {watching_view}
                        </div>
                    }.into_any()
                }}
            </div>
        </div>
        <SaveViewModal
            show=Signal::derive(move || show_save_view.get())
            on_close=Callback::new(move |()| set_show_save_view.set(false))
            search=Signal::derive(move || search.get())
            status_filter=Signal::derive(move || status_filter.get())
            priority_filter=Signal::derive(move || priority_filter.get())
            team_id=Signal::stored(None::<String>)
            view_mode=Signal::stored("list".to_string())
        />
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Collapsible Section
// ─────────────────────────────────────────────────────────────────────────────

/// A collapsible section with a header showing title, count badge, and chevron.
#[component]
fn CollapsibleSection(
    title: &'static str,
    count: usize,
    collapsed: ReadSignal<bool>,
    set_collapsed: WriteSignal<bool>,
    children: Children,
) -> impl IntoView {
    view! {
        <div class="border-b border-border">
            <button
                class="w-full px-5 py-2 flex items-center gap-2 text-sm font-medium text-foreground hover:bg-surface-alt transition-colors"
                on:click=move |_| set_collapsed.update(|c| *c = !*c)
            >
                <span class="text-muted-foreground transition-transform" class:rotate-90=move || !collapsed.get()>
                    <Icon icon=phosphor_leptos::CARET_RIGHT size="14px"/>
                </span>
                <span>{title}</span>
                <span class="text-xs text-muted-foreground bg-surface-alt rounded-full px-1.5 py-0.5 min-w-[20px] text-center">
                    {count}
                </span>
            </button>
            {children()}
        </div>
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// localStorage helpers for collapse state persistence
// ─────────────────────────────────────────────────────────────────────────────

/// Load a boolean collapse state from localStorage. Defaults to `false` (expanded).
fn load_collapsed_state(key: &str) -> bool {
    #[cfg(target_arch = "wasm32")]
    {
        web_sys::window()
            .and_then(|w| w.local_storage().ok().flatten())
            .and_then(|s| s.get_item(key).ok().flatten())
            .map(|v| v == "true")
            .unwrap_or(false)
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = key;
        false
    }
}

/// Save a boolean collapse state to localStorage.
fn save_collapsed_state(key: &str, collapsed: bool) {
    #[cfg(target_arch = "wasm32")]
    {
        if let Some(storage) = web_sys::window()
            .and_then(|w| w.local_storage().ok().flatten())
        {
            let _ = storage.set_item(key, if collapsed { "true" } else { "false" });
        }
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = (key, collapsed);
    }
}
