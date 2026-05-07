// SPDX-License-Identifier: AGPL-3.0-or-later

//! Issue list page — the first thing users see after login.
//!
//! Layout follows DESIGN.md "Issue List Page" spec:
//! - Page header: title + "New Issue" button
//! - Toolbar: search + filter dropdowns
//! - Content: issue rows, loading skeletons, or empty state
//!
//! Issue Row follows DESIGN.md "Issue Row Pattern":
//! `px-4 md:px-6 py-3 flex items-center gap-3 border-b border-border`
//! hover:bg-surface-alt transition-colors cursor-pointer

use std::sync::Arc;

use leptos::prelude::*;
use leptos_router::hooks::use_navigate;
use phosphor_leptos::Icon;

use crate::components::{
    Avatar, Button, ButtonSize, ButtonVariant, EmptyState, IssueStatusBadge, IssueStatusVariant,
    LabelBadge, Modal, ModalSize, PriorityIndicator, SearchInput, Skeleton, StyledSelect,
    INPUT_CLASS,
};
use crate::server_fns::issues::{create_issue, list_issues};
use trakkt_types::models::IssueWithDetails;

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

    // ── Data fetching ───────────────────────────────────────────────────────
    let (version, set_version) = signal(0u32);

    let issues = Resource::new(
        move || (version.get(), search.get(), status_filter.get(), priority_filter.get()),
        move |(_, search_val, status_val, priority_val)| {
            let status = if status_val.is_empty() { None } else { Some(status_val) };
            let priority = if priority_val.is_empty() {
                None
            } else {
                priority_val.parse::<i32>().ok()
            };
            let search = if search_val.is_empty() { None } else { Some(search_val) };
            async move {
                list_issues(status, priority, None, None, search, None, None).await
            }
        },
    );

    // ── New Issue modal state ───────────────────────────────────────────────
    let (show_new_issue, set_show_new_issue) = signal(false);

    let on_issue_created = Callback::new(move |()| {
        set_show_new_issue.set(false);
        set_version.update(|v| *v += 1);
    });

    // ── Render ──────────────────────────────────────────────────────────────
    view! {
        <div class="bg-background flex flex-col h-full">
            // ── Page header ─────────────────────────────────────────────────
            <div class="page-header h-16 px-4 md:px-6 flex items-center justify-between shrink-0">
                <h1 class="text-3xl font-display text-foreground">"Issues"</h1>
                <Button
                    size=ButtonSize::Sm
                    on:click=move |_| set_show_new_issue.set(true)
                >
                    <Icon icon=phosphor_leptos::PLUS size="14px"/>
                    "New Issue"
                </Button>
            </div>

            // ── Toolbar ─────────────────────────────────────────────────────
            <div class="bg-background px-4 md:px-6 py-3 flex items-center gap-3 shrink-0">
                <SearchInput
                    value=Signal::derive(move || search.get())
                    on_input=Callback::new(move |v: String| set_search.set(v))
                    placeholder="Search issues..."
                    class="flex-1 max-w-sm"
                />
                <div class="w-40">
                    <StyledSelect
                        value=status_filter.get_untracked()
                        options=vec![
                            ("", "All statuses"),
                            ("backlog", "Backlog"),
                            ("todo", "Todo"),
                            ("in_progress", "In Progress"),
                            ("done", "Done"),
                            ("cancelled", "Cancelled"),
                        ]
                        on_change=move |v: String| set_status_filter.set(v)
                    />
                </div>
                <div class="w-36">
                    <StyledSelect
                        value=priority_filter.get_untracked()
                        options=vec![
                            ("", "All priorities"),
                            ("1", "Urgent"),
                            ("2", "High"),
                            ("3", "Medium"),
                            ("4", "Low"),
                        ]
                        on_change=move |v: String| set_priority_filter.set(v)
                    />
                </div>
            </div>

            // ── Content area ────────────────────────────────────────────────
            <div class="flex-1 overflow-y-auto">
                <Suspense fallback=move || view! { <IssueListSkeleton/> }>
                    {move || Suspend::new(async move {
                        match issues.await {
                            Ok(ref list) if list.is_empty() => {
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
                            }
                            Ok(ref list) => {
                                let rows = list.iter().map(|issue| {
                                    view! { <IssueRow issue=issue.clone()/> }
                                }).collect_view();
                                view! {
                                    <div role="list">
                                        {rows}
                                    </div>
                                }.into_any()
                            }
                            Err(_) => {
                                view! {
                                    <div class="p-4 md:p-6">
                                        <EmptyState
                                            title="Failed to load issues"
                                            description="Something went wrong. Please try again."
                                        />
                                    </div>
                                }.into_any()
                            }
                        }
                    })}
                </Suspense>
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
/// [status_dot] TRK-42  Fix login redirect loop    [bug] [auth]  priority  @assignee
/// ```
#[component]
fn IssueRow(issue: IssueWithDetails) -> impl IntoView {
    let number = issue.number;
    let issue_key = format!("{}-{}", issue.team_key, issue.number);
    let status = IssueStatusVariant::parse(&issue.status);
    let go_to_issue = move || {
        let nav = use_navigate();
        nav(&format!("/issues/{number}"), Default::default());
    };

    view! {
        <div
            class="px-4 md:px-6 py-3 flex items-center gap-3 border-b border-border hover:bg-surface-alt focus-visible:bg-surface-alt focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring transition-colors cursor-pointer"
            role="listitem"
            tabindex="0"
            on:click=move |_| go_to_issue()
            on:keydown=move |ev: web_sys::KeyboardEvent| {
                if ev.key() == "Enter" {
                    go_to_issue();
                }
            }
        >
            // Status dot
            <IssueStatusBadge status=status/>

            // Issue number (monospace)
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

            // Priority
            <PriorityIndicator priority=issue.priority/>

            // Assignee
            {if issue.assignee_name.is_some() {
                view! {
                    <Avatar name=issue.assignee_name.clone().unwrap_or_default()/>
                }.into_any()
            } else {
                // Empty placeholder to keep alignment
                view! {
                    <span class="w-5 h-5 shrink-0"></span>
                }.into_any()
            }}
        </div>
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Loading Skeleton
// ─────────────────────────────────────────────────────────────────────────────

/// Six skeleton rows matching the issue row shape for the loading state.
///
/// DESIGN.md Loading State Pattern: "Content-shaped Skeleton rectangles".
#[component]
fn IssueListSkeleton() -> impl IntoView {
    let rows = (0..6).map(|_| {
        view! {
            <div class="px-4 md:px-6 py-3 flex items-center gap-3 border-b border-border">
                // Status dot placeholder
                <Skeleton class="w-2 h-2 rounded-full"/>
                // Issue number placeholder
                <Skeleton class="w-14 h-4"/>
                // Title placeholder
                <Skeleton class="flex-1 h-4 max-w-md"/>
                // Label placeholder
                <Skeleton class="hidden sm:block w-12 h-5 rounded-sm"/>
                // Priority placeholder
                <Skeleton class="w-2.5 h-2.5 rounded-[2px]"/>
                // Avatar placeholder
                <Skeleton class="w-5 h-5 rounded-full"/>
            </div>
        }
    }).collect_view();

    view! { <div>{rows}</div> }
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
            match create_issue(title_val, desc, prio, None, None, String::new()).await {
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

                // Description
                <div class="space-y-2">
                    <label for="issue-description" class="text-sm font-medium text-foreground">
                        "Description"
                    </label>
                    <textarea
                        id="issue-description"
                        rows="4"
                        placeholder="Add a description..."
                        class=format!("{INPUT_CLASS} min-h-[100px] resize-y")
                        prop:value=move || description.get()
                        on:input=move |ev| set_description.set(event_target_value(&ev))
                    />
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
