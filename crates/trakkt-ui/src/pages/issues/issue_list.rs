// SPDX-License-Identifier: AGPL-3.0-or-later

//! Issue list page — the first thing users see after login.
//!
//! Layout follows DESIGN.md "Issue List Page" spec:
//! - Page header: title + "New Issue" button
//! - Toolbar: search + filter dropdowns
//! - Content: issue rows, loading skeletons, or empty state
//!
//! Issue Row follows DESIGN.md "Issue Row Pattern":
//! `px-3 py-[6px] h-9 flex items-center gap-2.5 border-b border-border`
//! hover:bg-surface-alt transition-colors cursor-pointer
//! Order: Priority | Status | Issue ID | Title | Labels | Date | Assignee

use std::sync::Arc;

use leptos::prelude::*;
use leptos::children::ChildrenFn;
use leptos_router::hooks::use_navigate;
use phosphor_leptos::Icon;
use wasm_bindgen::closure::Closure;
use wasm_bindgen::JsCast;

use crate::components::{
    Avatar, Button, ButtonVariant, EmptyState, IssueStatusBadge, IssueStatusVariant,
    LabelBadge, Modal, ModalSize, PriorityIndicator, SearchInput, StyledSelect,
    DropdownTrigger, DropdownMenu, DropdownItem,
    INPUT_CLASS,
};
use crate::server_fns::issues::{create_issue, list_issues};
use crate::server_fns::statuses::list_statuses;
use trakkt_types::models::IssueWithDetails;

// ─────────────────────────────────────────────────────────────────────────────
// Helpers
// ─────────────────────────────────────────────────────────────────────────────

/// Formats a datetime string into a short date like "May 8".
/// Expects ISO 8601 format (e.g. "2026-05-08T..."). Falls back to the
/// first 10 characters if parsing fails.
fn format_short_date(datetime: &str) -> String {
    // Extract the date portion (YYYY-MM-DD).
    let date_part = if datetime.len() >= 10 { &datetime[..10] } else { datetime };
    let parts: Vec<&str> = date_part.split('-').collect();
    if parts.len() == 3 {
        let month = match parts[1] {
            "01" => "Jan",
            "02" => "Feb",
            "03" => "Mar",
            "04" => "Apr",
            "05" => "May",
            "06" => "Jun",
            "07" => "Jul",
            "08" => "Aug",
            "09" => "Sep",
            "10" => "Oct",
            "11" => "Nov",
            "12" => "Dec",
            _ => return date_part.to_string(),
        };
        // Strip leading zero from the day.
        let day = parts[2].trim_start_matches('0');
        format!("{month} {day}")
    } else {
        date_part.to_string()
    }
}

/// Returns `true` if the keyboard event target is an input, textarea, select,
/// or contenteditable element — meaning single-key shortcuts (j/k/c) should
/// NOT fire so they don't interfere with text editing.
fn is_input_focused(ev: &web_sys::KeyboardEvent) -> bool {
    use wasm_bindgen::JsCast;
    let Some(target) = ev.target() else { return false };
    let Some(el) = target.dyn_ref::<web_sys::HtmlElement>() else { return false };
    let tag = el.tag_name().to_uppercase();
    if matches!(tag.as_str(), "INPUT" | "TEXTAREA" | "SELECT") {
        return true;
    }
    // Check for contenteditable (kode editor, rich text fields).
    if el.is_content_editable() {
        return true;
    }
    false
}

// ─────────────────────────────────────────────────────────────────────────────
// Issue List Page
// ─────────────────────────────────────────────────────────────────────────────

/// Main issue list page — displays all issues with search, filter, and create.
#[component]
pub fn IssueListPage() -> impl IntoView {
    // ── Filter state ────────────────────────────────────────────────────────
    let (search, set_search) = signal(String::new());
    let (status_filter, set_status_filter) = signal(String::new());
    let (priority_filter, set_priority_filter) = signal(String::new());

    // ── Data source: SyncStore (real-time) with server function fallback ───
    let sync_store = use_context::<crate::cache::store::SyncStore>();
    let (version, set_version) = signal(0u32);

    // Server function fallback — used for initial load before sync is ready.
    let server_issues = Resource::new(
        move || version.get(),
        move |_| async move {
            list_issues(None, None, None, None, None, None, None).await
        },
    );

    // Filtered issue list — reads from SyncStore when initialized, otherwise
    // from the server function result. Filters are applied client-side for
    // instant reactivity (no round-trip to the server on filter change).
    let filtered_issues = Memo::new(move |_| {
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

    // ── New Issue modal state ───────────────────────────────────────────────
    let (show_new_issue, set_show_new_issue) = signal(false);

    let on_issue_created = Callback::new(move |()| {
        set_show_new_issue.set(false);
        set_version.update(|v| *v += 1);
    });

    // ── Keyboard navigation state ──────────────────────────────────────────
    let (selected_index, set_selected_index) = signal(Option::<usize>::None);

    // Track the current issue count so keyboard handlers know the bounds.
    let issue_count = RwSignal::new(0usize);

    // Track issue numbers so Enter can navigate to the selected issue.
    let issue_numbers = RwSignal::new(Vec::<i32>::new());

    // ── j/k/Enter/c keyboard listener (window-level, active on this page) ──
    Effect::new(move |_| {
        let Some(window) = web_sys::window() else { return };
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
                            let nav = use_navigate();
                            nav(&format!("/issues/{number}"), Default::default());
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

    // ── Render ──────────────────────────────────────────────────────────────
    view! {
        <div class="bg-background flex flex-col h-full">
            // ── Page header ─────────────────────────────────────────────────
            <div class="page-header h-14 px-5 flex items-center justify-between shrink-0">
                <h1 class="text-sm font-semibold text-foreground">"Issues"</h1>
                <Button
                    on:click=move |_| set_show_new_issue.set(true)
                >
                    <Icon icon=phosphor_leptos::PLUS size="14px"/>
                    "New Issue"
                </Button>
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

        // ── New Issue modal ─────────────────────────────────────────────────
        <NewIssueModal
            show=Signal::derive(move || show_new_issue.get())
            on_close=Callback::new(move |()| set_show_new_issue.set(false))
            on_created=on_issue_created
        />
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Issue Row
// ─────────────────────────────────────────────────────────────────────────────

/// A single issue row in the list.
///
/// DESIGN.md Issue Row Pattern:
/// ```text
/// [priority] [status] TRK-42  Fix login redirect loop  [bug] [auth]  May 8  @j
/// ```
///
/// Row height: 36px (h-9), padding: px-3 py-[6px], gap: gap-2.5
///
/// Supports keyboard navigation highlighting: when `selected_index` matches
/// `index`, the row renders with a distinct selected background.
#[component]
fn IssueRow(
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

            // Issue ID (Geist Mono)
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

// ─────────────────────────────────────────────────────────────────────────────
// Filter Dropdowns (DESIGN.md § Dropdowns)
// ─────────────────────────────────────────────────────────────────────────────

#[component]
fn StatusFilterDropdown(
    #[prop(into)] value: Signal<String>,
    on_change: Callback<String>,
) -> impl IntoView {
    let (open, set_open) = signal(false);
    let trigger_ref = NodeRef::<leptos::html::Div>::new();

    // Fetch statuses dynamically from the server.
    let statuses_resource = Resource::new(
        || (),
        move |_| async move { list_statuses(None).await },
    );

    // Resolved statuses — empty until loaded.
    let statuses = Memo::new(move |_| {
        statuses_resource
            .get()
            .and_then(|r| r.ok())
            .unwrap_or_default()
    });

    // Display name for the current selection (looked up from loaded statuses).
    let display = Memo::new(move |_| {
        let v = value.get();
        if v.is_empty() {
            None
        } else {
            statuses
                .get()
                .iter()
                .find(|s| s.status_id == v)
                .map(|s| s.name.clone())
        }
    });

    // Icon variant for the current selection.
    let current_variant = Memo::new(move |_| {
        let v = value.get();
        if v.is_empty() {
            None
        } else {
            statuses
                .get()
                .iter()
                .find(|s| s.status_id == v)
                .map(|s| IssueStatusVariant::parse(&s.category))
        }
    });

    view! {
        <div node_ref=trigger_ref>
            <DropdownTrigger
                label="All statuses"
                value=Signal::derive(move || display.get())
                icon=Arc::new(move || {
                    current_variant.get().map(|v| {
                        view! { <IssueStatusBadge status=v size=12/> }.into_any()
                    }).unwrap_or_else(|| view! { <span/> }.into_any())
                }) as ChildrenFn
                on_click=Callback::new(move |()| set_open.update(|o| *o = !*o))
            />
        </div>
        <DropdownMenu
            trigger_ref=trigger_ref
            open=Signal::derive(move || open.get())
            on_close=Callback::new(move |()| set_open.set(false))
            search_placeholder="Filter status..."
        >
            <DropdownItem
                label="All statuses"
                selected=Signal::derive(move || value.get().is_empty())
                on_select=Callback::new({
                    let on_change = on_change.clone();
                    move |()| { on_change.run(String::new()); set_open.set(false); }
                })
            />
            {move || statuses.get().into_iter().map(|status| {
                let status_id = status.status_id.clone();
                let status_id_check = status.status_id.clone();
                let label = status.name.clone();
                let variant = IssueStatusVariant::parse(&status.category);
                view! {
                    <DropdownItem
                        label=label
                        selected=Signal::derive(move || value.get() == status_id_check)
                        on_select=Callback::new({
                            let on_change = on_change.clone();
                            let id = status_id.clone();
                            move |()| { on_change.run(id.clone()); set_open.set(false); }
                        })
                        icon=Arc::new(move || view! { <IssueStatusBadge status=variant size=14/> }.into_any()) as ChildrenFn
                    />
                }
            }).collect_view()}
        </DropdownMenu>
    }
}

#[component]
fn PriorityFilterDropdown(
    #[prop(into)] value: Signal<String>,
    on_change: Callback<String>,
) -> impl IntoView {
    let (open, set_open) = signal(false);
    let trigger_ref = NodeRef::<leptos::html::Div>::new();

    let priorities: Vec<(&str, &str, i32)> = vec![
        ("1", "Urgent", 1),
        ("2", "High", 2),
        ("3", "Medium", 3),
        ("4", "Low", 4),
    ];

    let display = Memo::new(move |_| {
        let v = value.get();
        if v.is_empty() { None } else {
            match v.as_str() {
                "1" => Some("Urgent".to_string()),
                "2" => Some("High".to_string()),
                "3" => Some("Medium".to_string()),
                "4" => Some("Low".to_string()),
                _ => None,
            }
        }
    });

    let current_priority = Memo::new(move |_| {
        value.get().parse::<i32>().ok()
    });

    view! {
        <div node_ref=trigger_ref>
            <DropdownTrigger
                label="All priorities"
                value=Signal::derive(move || display.get())
                icon=Arc::new(move || {
                    current_priority.get().map(|p| {
                        view! { <PriorityIndicator priority=p/> }.into_any()
                    }).unwrap_or_else(|| view! { <span/> }.into_any())
                }) as ChildrenFn
                on_click=Callback::new(move |()| set_open.update(|o| *o = !*o))
            />
        </div>
        <DropdownMenu
            trigger_ref=trigger_ref
            open=Signal::derive(move || open.get())
            on_close=Callback::new(move |()| set_open.set(false))
            search_placeholder="Filter priority..."
        >
            <DropdownItem
                label="All priorities"
                selected=Signal::derive(move || value.get().is_empty())
                on_select=Callback::new({
                    let on_change = on_change.clone();
                    move |()| { on_change.run(String::new()); set_open.set(false); }
                })
            />
            {priorities.iter().map(|(key, label, priority_val)| {
                let key_owned = key.to_string();
                let key_check = key.to_string();
                let label = label.to_string();
                let shortcut = key.to_string();
                let priority_val = *priority_val;
                view! {
                    <DropdownItem
                        label=label
                        selected=Signal::derive(move || value.get() == key_check)
                        on_select=Callback::new({
                            let on_change = on_change.clone();
                            let k = key_owned.clone();
                            move |()| { on_change.run(k.clone()); set_open.set(false); }
                        })
                        icon=Arc::new(move || view! { <PriorityIndicator priority=priority_val/> }.into_any()) as ChildrenFn
                        shortcut=shortcut
                    />
                }
            }).collect_view()}
        </DropdownMenu>
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
fn NewIssueModal(
    /// Whether the modal is visible.
    show: Signal<bool>,
    /// Called when the modal should close (cancel, escape, backdrop click).
    on_close: Callback<()>,
    /// Called after an issue is successfully created.
    on_created: Callback<()>,
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

        set_submitting.set(true);
        set_error_msg.set(None);

        leptos::task::spawn_local(async move {
            match create_issue(title_val, desc, prio, None, None, String::new(), None, None).await {
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
                {move || if submitting.get() { "Creating..." } else { "Create Issue" }}
            </Button>
        }.into_any()
    });

    view! {
        <Modal
            show=show
            on_close=on_close
            title="New Issue"
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
                            theme=Signal::stored({
                                let mut theme = super::issue_detail::trakkt_kode_theme();
                                theme.content_padding = Some("0.75rem 1rem");
                                theme
                            })
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
