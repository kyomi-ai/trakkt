// SPDX-License-Identifier: AGPL-3.0-or-later

//! My Issues page — shows issues assigned to the current user across all teams.
//!
//! Layout follows the same patterns as `issue_list.rs`:
//! - Page header: title (no create button — users create from team pages)
//! - Toolbar: search + filter dropdowns
//! - Content: issue rows, loading skeletons, or empty state
//!
//! Issue Row follows DESIGN.md "Issue Row Pattern":
//! `px-3 py-[6px] h-9 flex items-center gap-2.5 border-b border-border`
//! hover:bg-surface-alt transition-colors cursor-pointer
//! Order: Priority | Status | Issue ID (with team key) | Title | Labels | Date | Assignee

use std::sync::Arc;

use leptos::prelude::*;
use leptos_router::hooks::use_navigate;
use phosphor_leptos::Icon;
use wasm_bindgen::closure::Closure;
use wasm_bindgen::JsCast;

use crate::components::{
    Avatar, EmptyState, IssueStatusBadge, IssueStatusVariant,
    LabelBadge, PriorityIndicator, SearchInput,
};
use crate::pages::issues::filters::{PriorityFilterDropdown, StatusFilterDropdown};
use crate::server_fns::context::UserContext;
use crate::server_fns::issues::list_issues;
use crate::utils::date::format_short_date;
use crate::utils::keyboard::is_input_focused;
use trakkt_types::models::IssueWithDetails;

// ─────────────────────────────────────────────────────────────────────────────
// My Issues Page
// ─────────────────────────────────────────────────────────────────────────────

/// My Issues page — displays issues assigned to the current user across all teams.
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
    let (status_filter, set_status_filter) = signal(String::new());
    let (priority_filter, set_priority_filter) = signal(String::new());

    // ── Data source: SyncStore (real-time) with server function fallback ───
    let sync_store = use_context::<crate::cache::store::SyncStore>();

    // Server function fallback — used for initial load before sync is ready.
    let server_issues = Resource::new(
        || (),
        move |_| async move {
            list_issues(None, None, None, None, None, None, None).await
        },
    );

    // Filtered issue list — reads from SyncStore when initialized, otherwise
    // from the server function result. Filters by assignee first, then applies
    // search/status/priority client-side for instant reactivity.
    let filtered_issues = Memo::new(move |_| {
        let user_id = current_user_id.get();

        // If we don't know the user yet, return empty — we'll update reactively.
        let Some(uid) = user_id else {
            return Vec::new();
        };

        let raw = if let Some(store) = sync_store {
            let issues = store.issues().get();
            if !issues.is_empty() || store.initialized().get() {
                issues
            } else {
                // Store not initialized yet — use server function result
                server_issues.get()
                    .and_then(|r| r.ok())
                    .unwrap_or_default()
            }
        } else {
            // No store (SSR) — use server function
            server_issues.get()
                .and_then(|r| r.ok())
                .unwrap_or_default()
        };

        // Apply client-side filters
        let search_val = search.get().to_lowercase();
        let status_val = status_filter.get();
        let priority_val = priority_filter.get();

        raw.into_iter()
            .filter(|issue| {
                // Primary filter: only issues assigned to the current user
                if issue.assignee_id.as_ref() != Some(&uid) {
                    return false;
                }
                if !status_val.is_empty() && issue.status_id != status_val {
                    return false;
                }
                if !priority_val.is_empty() {
                    if let Ok(p) = priority_val.parse::<i32>() {
                        if issue.priority != p {
                            return false;
                        }
                    }
                }
                if !search_val.is_empty()
                    && !issue.title.to_lowercase().contains(&search_val)
                {
                    return false;
                }
                true
            })
            .collect::<Vec<_>>()
    });

    // ── Keyboard navigation state ──────────────────────────────────────────
    let (selected_index, set_selected_index) = signal(Option::<usize>::None);

    // Track the current issue count so keyboard handlers know the bounds.
    let issue_count = RwSignal::new(0usize);

    // Track issue numbers so Enter can navigate to the selected issue.
    let issue_numbers = RwSignal::new(Vec::<i32>::new());

    // ── j/k/Enter keyboard listener (window-level, active on this page) ────
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
                        let numbers = issue_numbers.get_untracked();
                        if let Some(&number) = numbers.get(idx) {
                            ev.prevent_default();
                            nav(&format!("/issues/{number}"), Default::default());
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
                <StatusFilterDropdown value=status_filter on_change=Callback::new(move |v: String| set_status_filter.set(v))/>
                <PriorityFilterDropdown value=priority_filter on_change=Callback::new(move |v: String| set_priority_filter.set(v))/>
            </div>

            // ── Content area ────────────────────────────────────────────────
            <div class="flex-1 overflow-y-auto">
                {move || {
                    let list = filtered_issues.get();

                    // Update keyboard navigation bounds.
                    issue_count.set(list.len());
                    issue_numbers.set(list.iter().map(|i| i.number).collect());
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
                        view! {
                            <div class="p-4 md:p-6">
                                <EmptyState
                                    icon=empty_icon
                                    title="No issues assigned to you"
                                    description="Issues assigned to you will appear here"
                                />
                            </div>
                        }.into_any()
                    } else {
                        let rows = list.iter().enumerate().map(|(idx, issue)| {
                            view! { <MyIssueRow issue=issue.clone() index=idx selected_index=selected_index/> }
                        }).collect_view();
                        view! {
                            <div role="list">
                                {rows}
                            </div>
                        }.into_any()
                    }
                }}
            </div>
        </div>
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Issue Row (with team key prefix)
// ─────────────────────────────────────────────────────────────────────────────

/// A single issue row in the My Issues list.
///
/// Same layout as `IssueRow` in `issue_list.rs`, showing the team key prefix
/// in the issue identifier (e.g. "ENG-42") for cross-team context.
///
/// Row height: 36px (h-9), padding: px-3 py-[6px], gap: gap-2.5
#[component]
fn MyIssueRow(
    issue: IssueWithDetails,
    /// This row's index in the list.
    index: usize,
    /// The currently keyboard-selected index (None = no selection).
    #[prop(into)]
    selected_index: Signal<Option<usize>>,
) -> impl IntoView {
    let number = issue.number;
    let issue_key = format!("{}-{}", issue.team_key, issue.number);
    let issue_href = format!("/issues/{number}");
    let status = IssueStatusVariant::parse(&issue.status_category);
    let row_ref = NodeRef::<leptos::html::A>::new();

    let is_selected = Memo::new(move |_| selected_index.get() == Some(index));

    // Scroll the selected row into view when keyboard-navigated.
    Effect::new(move || {
        if is_selected.get() && let Some(el) = row_ref.get() {
            let opts = web_sys::ScrollIntoViewOptions::new();
            opts.set_block(web_sys::ScrollLogicalPosition::Nearest);
            el.scroll_into_view_with_scroll_into_view_options(&opts);
        }
    });

    let row_class = move || {
        if is_selected.get() {
            "h-9 px-3 py-[6px] flex items-center gap-2.5 border-b border-border bg-primary/5 ring-1 ring-primary/20 focus-visible:outline-none transition-colors cursor-pointer no-underline text-inherit"
        } else {
            "h-9 px-3 py-[6px] flex items-center gap-2.5 border-b border-border hover:bg-surface-alt focus-visible:bg-surface-alt focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring transition-colors cursor-pointer no-underline text-inherit"
        }
    };

    view! {
        <a
            node_ref=row_ref
            href=issue_href
            class=row_class
            role="listitem"
            tabindex="0"
        >
            // Priority icon (first — most important for triage scanning)
            <PriorityIndicator priority=issue.priority/>

            // Status icon
            <IssueStatusBadge status=status/>

            // Issue ID with team key (Geist Mono)
            <span class="font-mono text-xs text-muted-foreground shrink-0">
                {issue_key}
            </span>

            // Title
            <span class="text-sm font-medium text-foreground flex-1 truncate">
                {issue.title.clone()}
            </span>

            // Labels
            <div class="hidden sm:flex items-center gap-1 shrink-0">
                {issue.labels.iter().map(|label| {
                    view! {
                        <LabelBadge
                            name=label.name.clone()
                            color=label.color.clone()
                        />
                    }
                }).collect_view()}
            </div>

            // Date (Geist Mono)
            <span class="font-mono text-xs text-muted-foreground shrink-0 hidden sm:inline">
                {format_short_date(&issue.created_at)}
            </span>

            // Assignee avatar (18px)
            {if issue.assignee_name.is_some() {
                view! {
                    <Avatar name=issue.assignee_name.clone().unwrap_or_default()/>
                }.into_any()
            } else {
                // Empty placeholder to keep alignment
                view! {
                    <span class="w-[18px] h-[18px] shrink-0"></span>
                }.into_any()
            }}
        </a>
    }
}

