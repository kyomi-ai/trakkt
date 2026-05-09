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

use crate::components::{Alert, AlertVariant, EmptyState, SearchInput};
use crate::pages::issues::filters::{PriorityFilterDropdown, StatusFilterDropdown};
use crate::pages::issues::issue_row::IssueRow;
use crate::server_fns::context::UserContext;
use crate::server_fns::issues::list_issues;
use crate::utils::keyboard::is_input_focused;

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
            // No store (SSR) — use server function
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
                            view! { <IssueRow issue=issue.clone() index=idx selected_index=selected_index/> }
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

