// SPDX-License-Identifier: AGPL-3.0-or-later

//! Board page — Kanban view of issues grouped by status.
//!
//! Layout follows DESIGN.md "Board Page (Kanban)" spec:
//! - Page header: "Board" title (Instrument Serif)
//! - Content: 5 status columns (Backlog, Todo, In Progress, Done, Cancelled)
//! - Each column: header with status name + count, scrollable card list
//! - Cards: issue number, title, labels, priority + assignee footer
//!
//! Drag and drop:
//! - HTML5 drag API (dragstart, dragover, dragenter, dragleave, drop)
//! - Optimistic UI: card moves immediately, reverts on server error
//! - Same-column drops are no-ops

use leptos::prelude::*;
use leptos_router::hooks::use_navigate;
use wasm_bindgen::JsCast;

use crate::components::{Avatar, IssueStatusBadge, IssueStatusVariant, LabelBadge, PriorityIndicator, Skeleton};
use crate::server_fns::issues::{list_issues, update_issue};
use crate::server_fns::statuses::list_statuses;
use trakkt_types::models::{IssueWithDetails, Status};

// ─────────────────────────────────────────────────────────────────────────────
// Helpers
// ─────────────────────────────────────────────────────────────────────────────

/// Sort issues within a column: priority ASC (urgent=1 first, none=0 last),
/// then created_at DESC (newest first).
fn sort_column_issues(issues: &mut [IssueWithDetails]) {
    issues.sort_by(|a, b| {
        // Priority 0 (none) should sort last, so map 0 → i32::MAX.
        let pa = if a.priority == 0 { i32::MAX } else { a.priority };
        let pb = if b.priority == 0 { i32::MAX } else { b.priority };
        pa.cmp(&pb).then_with(|| b.created_at.cmp(&a.created_at))
    });
}

/// Group a flat list of issues into per-status columns, sorted.
///
/// Columns are ordered by the `statuses` list (server returns them ordered by
/// category then position). Issues are grouped by `status_id`.
fn group_by_status(statuses: &[Status], all: &[IssueWithDetails]) -> Vec<(Status, Vec<IssueWithDetails>)> {
    statuses
        .iter()
        .map(|status| {
            let mut col_issues: Vec<IssueWithDetails> = all
                .iter()
                .filter(|i| i.status_id == status.status_id)
                .cloned()
                .collect();
            sort_column_issues(&mut col_issues);
            (status.clone(), col_issues)
        })
        .collect()
}

// ─────────────────────────────────────────────────────────────────────────────
// Board Page
// ─────────────────────────────────────────────────────────────────────────────

/// Kanban board page — issues arranged in status columns with drag-and-drop.
#[component]
pub fn BoardPage() -> impl IntoView {
    // ── Data fetching ───────────────────────────────────────────────────────
    let (version, set_version) = signal(0u32);

    let issues_resource = Resource::new(
        move || version.get(),
        move |_| async move { list_issues(None, None, None, None, None, None, None).await },
    );

    // Fetch statuses once (no team filter — show all workspace statuses).
    let statuses_resource = Resource::new(
        || (),
        move |_| async move { list_statuses(None).await },
    );

    // ── Drag state ──────────────────────────────────────────────────────────
    // The issue_id of the card currently being dragged.
    let (dragging, set_dragging) = signal(Option::<String>::None);
    // The status_id of the column currently being dragged over.
    let (drag_target, set_drag_target) = signal(Option::<String>::None);

    // ── Local issues for optimistic updates ─────────────────────────────────
    // When the resource resolves, we copy its data into a local signal.
    // This lets us move cards optimistically before the server responds.
    let (local_issues, set_local_issues) = signal(Vec::<IssueWithDetails>::new());

    // Sync resource data into local_issues whenever the resource resolves.
    Effect::new(move || {
        if let Some(Ok(ref list)) = issues_resource.get() {
            set_local_issues.set(list.clone());
        }
    });

    // ── Resolved statuses (for grouping) ────────────────────────────────────
    let (local_statuses, set_local_statuses) = signal(Vec::<Status>::new());

    Effect::new(move || {
        if let Some(Ok(ref list)) = statuses_resource.get() {
            set_local_statuses.set(list.clone());
        }
    });

    // ── Drop handler ────────────────────────────────────────────────────────
    let handle_drop = move |issue_id: String, target_status_id: String| {
        // Find the issue and its current status_id.
        let current_issues = local_issues.get_untracked();
        let issue = current_issues.iter().find(|i| i.issue_id == issue_id);
        let issue = match issue {
            Some(i) => i.clone(),
            None => return,
        };

        // Same-column drop is a no-op.
        if issue.status_id == target_status_id {
            return;
        }

        let old_status_id = issue.status_id.clone();
        let old_status_name = issue.status_name.clone();
        let old_status_category = issue.status_category.clone();
        let issue_number = issue.number;

        // Look up the target status to update display fields optimistically.
        let statuses = local_statuses.get_untracked();
        let target_status = statuses.iter().find(|s| s.status_id == target_status_id);
        let target_name = target_status.map(|s| s.name.clone()).unwrap_or_default();
        let target_category = target_status.map(|s| s.category.clone()).unwrap_or_default();

        // Optimistic update: move card to new column immediately.
        set_local_issues.update(|issues| {
            if let Some(i) = issues.iter_mut().find(|i| i.issue_id == issue_id) {
                i.status_id = target_status_id.clone();
                i.status_name = target_name;
                i.status_category = target_category;
            }
        });

        // Server update.
        let target_id_for_server = target_status_id.clone();
        leptos::task::spawn_local(async move {
            match update_issue(
                issue_number,
                None, // title
                None, // description
                Some(target_id_for_server),
                None, // priority
                None, // assignee_id
                None, // due_date
                None, // project_id
                None, // milestone_id
            )
            .await
            {
                Ok(_) => {
                    // Bump version to refetch and get canonical server state.
                    set_version.update(|v| *v += 1);
                }
                Err(e) => {
                    // Revert optimistic update on failure.
                    tracing::warn!("Failed to update issue status: {e}");
                    set_local_issues.update(|issues| {
                        if let Some(i) = issues.iter_mut().find(|i| i.issue_id == issue_id) {
                            i.status_id = old_status_id;
                            i.status_name = old_status_name;
                            i.status_category = old_status_category;
                        }
                    });
                }
            }
        });
    };

    // ── Render ──────────────────────────────────────────────────────────────
    view! {
        <div class="bg-background flex flex-col h-full">
            // ── Page header ─────────────────────────────────────────────────
            <div class="page-header h-14 px-5 flex items-center justify-between shrink-0">
                <h1 class="text-sm font-semibold text-foreground">"Board"</h1>
            </div>

            // ── Content area ────────────────────────────────────────────────
            <div class="flex-1 overflow-x-auto px-4 md:px-6 py-4" style="scrollbar-width: thin;">
                <Transition fallback=move || view! { <BoardSkeleton/> }>
                    {move || {
                        let issues_state = issues_resource.get();
                        let statuses_state = statuses_resource.get();
                        match (issues_state, statuses_state) {
                            (None, _) | (_, None) => {
                                // Still loading — show skeleton.
                                view! { <BoardSkeleton/> }.into_any()
                            }
                            (Some(Err(_)), _) | (_, Some(Err(_))) => {
                                view! {
                                    <div class="flex items-center justify-center h-full text-muted-foreground">
                                        "Failed to load board. Please try again."
                                    </div>
                                }.into_any()
                            }
                            (Some(Ok(_)), Some(Ok(_))) => {
                                // Use local signals for rendering (supports optimistic updates).
                                let grouped = move || group_by_status(&local_statuses.get(), &local_issues.get());

                                view! {
                                    <div class="flex gap-4 h-full">
                                        {move || grouped().into_iter().map(|(status, issues)| {
                                            let status_id = status.status_id.clone();
                                            let status_name = status.name.clone();
                                            let status_variant = IssueStatusVariant::parse(&status.category);
                                            let count = issues.len();

                                            view! {
                                                <BoardColumn
                                                    status_id=status_id
                                                    label=status_name
                                                    status_variant=status_variant
                                                    count=count
                                                    issues=issues
                                                    dragging=dragging
                                                    set_dragging=set_dragging
                                                    drag_target=drag_target
                                                    set_drag_target=set_drag_target
                                                    on_drop=handle_drop
                                                />
                                            }
                                        }).collect_view()}
                                    </div>
                                }.into_any()
                            }
                        }
                    }}
                </Transition>
            </div>
        </div>
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Board Column
// ─────────────────────────────────────────────────────────────────────────────

/// A single Kanban column for one status.
///
/// DESIGN.md: `min-w-[280px] max-w-[320px] flex flex-col`
/// - Column header: sticky top, status name + issue count
/// - Cards container: scrollable, flex-col gap-2
#[component]
fn BoardColumn(
    /// The status_id for this column (used for drag-drop targeting).
    status_id: String,
    /// Human-readable label (e.g. "In Progress").
    label: String,
    /// Status variant for the SVG icon in the column header.
    status_variant: IssueStatusVariant,
    /// Number of issues in this column.
    count: usize,
    /// Issues to render in this column.
    issues: Vec<IssueWithDetails>,
    /// Signal: which issue_id is currently being dragged.
    dragging: ReadSignal<Option<String>>,
    /// Setter for the dragging signal.
    set_dragging: WriteSignal<Option<String>>,
    /// Signal: which status_id is the current drag target.
    drag_target: ReadSignal<Option<String>>,
    /// Setter for the drag target signal.
    set_drag_target: WriteSignal<Option<String>>,
    /// Callback when an issue is dropped on this column.
    #[prop(into)]
    on_drop: Callback<(String, String)>,
) -> impl IntoView {
    let status_id_for_over = status_id.clone();
    let status_id_for_drop = status_id.clone();
    let status_id_for_class = status_id.clone();

    // Determine if this column is the active drop target.
    let is_drop_target = move || {
        drag_target.get().as_deref() == Some(status_id_for_class.as_str())
    };

    let column_class = move || {
        let base = "min-w-[280px] max-w-[320px] flex flex-col rounded-lg transition-colors duration-200";
        if is_drop_target() {
            format!("{base} border-2 border-primary bg-primary/5")
        } else {
            format!("{base} border-2 border-transparent")
        }
    };

    view! {
        <div
            class=column_class
            on:dragover=move |ev: web_sys::DragEvent| {
                ev.prevent_default();
                set_drag_target.set(Some(status_id_for_over.clone()));
            }
            on:dragleave=move |ev: web_sys::DragEvent| {
                let should_clear = match (ev.current_target(), ev.related_target()) {
                    (Some(target), Some(related)) => {
                        match (
                            target.dyn_into::<web_sys::Node>(),
                            related.dyn_into::<web_sys::Node>(),
                        ) {
                            (Ok(container), Ok(rel_node)) => !container.contains(Some(&rel_node)),
                            _ => true,
                        }
                    }
                    _ => true,
                };
                if should_clear {
                    set_drag_target.set(None);
                }
            }
            on:drop={
                let status_id_drop = status_id_for_drop.clone();
                move |ev: web_sys::DragEvent| {
                    ev.prevent_default();
                    if let Some(dt) = ev.data_transfer()
                        && let Ok(issue_id) = dt.get_data("text/plain")
                        && !issue_id.is_empty()
                    {
                        on_drop.run((issue_id, status_id_drop.clone()));
                    }
                    set_drag_target.set(None);
                }
            }
        >
            // ── Column header ───────────────────────────────────────────────
            <div class="sticky top-0 bg-background pb-3 pt-1 px-2">
                <div class="flex items-center gap-2">
                    <IssueStatusBadge status=status_variant/>
                    <span class="font-medium text-sm text-foreground">{label}</span>
                    <span class="text-muted-foreground text-xs">{count}</span>
                </div>
            </div>

            // ── Cards container ─────────────────────────────────────────────
            <div class="flex flex-col gap-2 flex-1 overflow-y-auto px-1 pb-2" style="scrollbar-width: thin;">
                {if issues.is_empty() {
                    view! {
                        <div class="flex items-center justify-center py-8 text-muted-foreground text-sm">
                            "No issues"
                        </div>
                    }.into_any()
                } else {
                    issues.into_iter().map(|issue| {
                        view! {
                            <BoardCard
                                issue=issue
                                dragging=dragging
                                set_dragging=set_dragging
                            />
                        }
                    }).collect_view().into_any()
                }}
            </div>
        </div>
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Board Card
// ─────────────────────────────────────────────────────────────────────────────

/// A single issue card on the Kanban board.
///
/// DESIGN.md Kanban Card Pattern:
/// ```text
/// ┌─────────────────────┐
/// │ TRK-42              │  ← Geist Mono, text-xs, text-muted
/// │ Fix login redirect  │  ← DM Sans, text-sm, font-medium
/// │ loop                │
/// │                     │
/// │ [bug] [auth]        │  ← label pills
/// │ ■ Urgent    @jason  │  ← priority + assignee
/// └─────────────────────┘
/// ```
///
/// - `bg-card border border-border rounded-md p-4 shadow-sm`
/// - Hover: `shadow-md` with `transition-shadow`
/// - Dragging: reduced opacity
/// - Click: navigate to `/issues/:number`
#[component]
fn BoardCard(
    issue: IssueWithDetails,
    /// Signal: which issue_id is currently being dragged.
    dragging: ReadSignal<Option<String>>,
    /// Setter for the dragging signal.
    set_dragging: WriteSignal<Option<String>>,
) -> impl IntoView {
    let issue_id = issue.issue_id.clone();
    let issue_id_for_drag = issue_id.clone();
    let issue_id_for_opacity = issue_id.clone();
    let issue_key = format!("{}-{}", issue.team_key, issue.number);
    let number = issue.number;
    let title = issue.title.clone();
    let labels = issue.labels.clone();
    let priority = issue.priority;
    let assignee_name = issue.assignee_name.clone();

    let (did_drag, set_did_drag) = signal(false);

    // Track if THIS card is being dragged for opacity.
    let is_dragging = move || dragging.get().as_deref() == Some(issue_id_for_opacity.as_str());

    let card_class = move || {
        let base = "bg-card border border-border rounded-md p-4 shadow-sm hover:shadow-md transition-shadow cursor-grab focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring";
        if is_dragging() {
            format!("{base} opacity-50")
        } else {
            base.to_string()
        }
    };

    let navigate_to_issue = move |_: web_sys::MouseEvent| {
        if did_drag.get_untracked() {
            set_did_drag.set(false);
            return;
        }
        let nav = use_navigate();
        nav(&format!("/issues/{number}"), Default::default());
    };

    view! {
        <div
            class=card_class
            draggable="true"
            tabindex="0"
            on:dragstart={
                let issue_id_ds = issue_id_for_drag.clone();
                move |ev: web_sys::DragEvent| {
                    if let Some(dt) = ev.data_transfer() {
                        let _ = dt.set_data("text/plain", &issue_id_ds);
                        // Set drag effect.
                        dt.set_effect_allowed("move");
                    }
                    set_dragging.set(Some(issue_id_ds.clone()));
                }
            }
            on:dragend=move |_: web_sys::DragEvent| {
                set_dragging.set(None);
                set_did_drag.set(true);
            }
            on:click=navigate_to_issue
            on:keydown=move |ev: web_sys::KeyboardEvent| {
                if ev.key() == "Enter" {
                    let nav = use_navigate();
                    nav(&format!("/issues/{number}"), Default::default());
                }
            }
        >
            // Issue number
            <div class="font-mono text-xs text-muted-foreground mb-1">
                {issue_key}
            </div>

            // Title (2-line clamp)
            <div class="text-sm font-medium text-foreground line-clamp-2">
                {title}
            </div>

            // Labels
            {if !labels.is_empty() {
                view! {
                    <div class="flex gap-1 flex-wrap mt-2">
                        {labels.iter().map(|label| {
                            view! {
                                <LabelBadge
                                    name=label.name.clone()
                                    color=label.color.clone()
                                />
                            }
                        }).collect_view()}
                    </div>
                }.into_any()
            } else {
                ().into_any()
            }}

            // Footer: priority + assignee
            <div class="flex items-center justify-between mt-3">
                <PriorityIndicator priority=priority/>
                {if let Some(ref name) = assignee_name {
                    view! {
                        <Avatar name=name.clone()/>
                    }.into_any()
                } else {
                    view! {
                        <span class="w-5 h-5 shrink-0"></span>
                    }.into_any()
                }}
            </div>
        </div>
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Loading Skeleton
// ─────────────────────────────────────────────────────────────────────────────

/// Five skeleton columns with placeholder card shapes for the loading state.
///
/// DESIGN.md Loading State Pattern: "Content-shaped Skeleton rectangles".
#[component]
fn BoardSkeleton() -> impl IntoView {
    let columns = (0..5).map(|_| {
        let cards = (0..3).map(|_| {
            view! {
                <div class="bg-card border border-border rounded-md p-4">
                    // Issue number placeholder
                    <Skeleton class="w-14 h-3 mb-2"/>
                    // Title placeholder (2 lines)
                    <Skeleton class="w-full h-4 mb-1"/>
                    <Skeleton class="w-3/4 h-4 mb-3"/>
                    // Labels placeholder
                    <div class="flex gap-1 mb-3">
                        <Skeleton class="w-10 h-5 rounded-sm"/>
                        <Skeleton class="w-12 h-5 rounded-sm"/>
                    </div>
                    // Footer: priority + avatar
                    <div class="flex items-center justify-between">
                        <Skeleton class="w-3.5 h-3.5 rounded-[2px]"/>
                        <Skeleton class="w-5 h-5 rounded-full"/>
                    </div>
                </div>
            }
        }).collect_view();

        view! {
            <div class="min-w-[280px] max-w-[320px] flex flex-col">
                // Column header skeleton
                <div class="pb-3 pt-1 px-2">
                    <div class="flex items-center gap-2">
                        <Skeleton class="w-3.5 h-3.5 rounded-full"/>
                        <Skeleton class="w-20 h-4"/>
                        <Skeleton class="w-4 h-3"/>
                    </div>
                </div>
                // Card skeletons
                <div class="flex flex-col gap-2 px-1">
                    {cards}
                </div>
            </div>
        }
    }).collect_view();

    view! {
        <div class="flex gap-4 h-full">
            {columns}
        </div>
    }
}
