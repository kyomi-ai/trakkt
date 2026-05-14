// SPDX-License-Identifier: AGPL-3.0-or-later

//! Project detail page — view a single project and its issues.
//!
//! Layout:
//! - Header: back button + project name
//! - Content: project metadata (status, lead, dates, description)
//! - Issue list: filtered view of all issues belonging to this project,
//!   using the same row format as IssueListPage.

use std::sync::Arc;

use leptos::prelude::*;
use leptos_router::hooks::{use_navigate, use_params_map};
use phosphor_leptos::Icon;

use crate::components::{
    Button, ButtonSize, ButtonVariant, ConfirmDialog, DatePicker, EmptyState,
    IssueStatusBadge, IssueStatusVariant, INPUT_CLASS,
    PriorityIndicator, LabelBadge,
    StatusBadge,
};
use crate::server_fns::projects::{
    get_project, get_project_progress, list_milestones,
    create_milestone, update_milestone, delete_milestone,
};
use crate::server_fns::issues::list_issues;
use crate::utils::date::{format_date, format_short_date};
use crate::utils::project::{status_label, status_variant};
use trakkt_types::models::{IssueWithDetails, Project, ProjectMilestone, ProjectProgress};

// ─────────────────────────────────────────────────────────────────────────────
// Project Detail Page
// ─────────────────────────────────────────────────────────────────────────────

/// Full project detail page — metadata + filtered issue list.
#[component]
pub fn ProjectDetailPage() -> impl IntoView {
    let params = use_params_map();
    let project_id = Memo::new(move |_| {
        params
            .get()
            .get("id")
            .unwrap_or_default()
    });

    // ── Data source: SyncStore (real-time) with server function fallback ───
    let sync_store = use_context::<crate::cache::store::SyncStore>();

    // Server function fallback for the project itself.
    let server_project = Resource::new(
        move || project_id.get(),
        move |id| async move { get_project(id).await },
    );

    // Server function fallback for issues (used on SSR before SyncStore is ready).
    let server_issues = Resource::new(
        || (),
        move |_| async move {
            list_issues(None, None, None, None, None, None, None, None).await
        },
    );

    let server_progress = Resource::new(
        move || project_id.get(),
        move |id| async move { get_project_progress(id).await },
    );

    let server_milestones = Resource::new(
        move || project_id.get(),
        move |id| async move { list_milestones(id).await },
    );

    // Resolve the project from SyncStore or server function.
    let project_data = Signal::derive(move || {
        let id = project_id.get();
        if id.is_empty() {
            return Some(Ok(None));
        }
        if let Some(store) = sync_store {
            let items = store.projects().get();
            if let Some(project) = items.iter().find(|p| p.project_id == id) {
                return Some(Ok(Some(project.clone())));
            }
            if store.initialized().get() {
                return Some(Ok(None));
            }
        }
        server_project.get()
    });

    // Resolve issues for this project from SyncStore or server function.
    let project_issues = Memo::new(move |_| {
        let id = project_id.get();
        if id.is_empty() {
            return Vec::new();
        }

        let raw = if let Some(store) = sync_store {
            let issues = store.issues().get();
            if !issues.is_empty() || store.initialized().get() {
                issues
            } else {
                server_issues
                    .get()
                    .and_then(|r| r.ok())
                    .unwrap_or_default()
            }
        } else {
            server_issues
                .get()
                .and_then(|r| r.ok())
                .unwrap_or_default()
        };

        raw.into_iter()
            .filter(|issue| issue.project_id.as_deref() == Some(&id))
            .collect::<Vec<_>>()
    });

    // Hoist use_navigate to component construction time (not inside closures).
    let nav = use_navigate();

    // ── Render ──────────────────────────────────────────────────────────────
    view! {
        <div class="bg-background flex flex-col h-full">
            // ── Header ────────────────────────────────────────────────────
            <div class="page-header h-14 px-5 flex items-center gap-3 shrink-0">
                <Button
                    variant=ButtonVariant::GhostMuted
                    size=ButtonSize::IconSm
                    aria_label="Back to projects"
                    on:click=move |_| {
                        nav("/projects", Default::default());
                    }
                >
                    <Icon icon=phosphor_leptos::ARROW_LEFT size="20px"/>
                </Button>
                <span class="text-muted-foreground">
                    <Icon icon=phosphor_leptos::FOLDER weight=phosphor_leptos::IconWeight::Duotone size="16px"/>
                </span>
                <span class="text-sm font-semibold text-foreground truncate">
                    {move || {
                        match project_data.get() {
                            Some(Ok(Some(p))) => p.name.clone(),
                            _ => "Project".to_string(),
                        }
                    }}
                </span>
            </div>

            // ── Content ───────────────────────────────────────────────────
            <div class="flex-1 overflow-y-auto p-4 md:p-6">
                {move || {
                    match project_data.get() {
                        Some(Ok(Some(project))) => {
                            let issues = project_issues.get();
                            let progress = server_progress.get().and_then(|r| r.ok());
                            let milestones = match server_milestones.get() {
                                Some(Ok(v)) => v,
                                Some(Err(e)) => {
                                    leptos::logging::warn!("Failed to load milestones: {e}");
                                    Vec::new()
                                }
                                None => Vec::new(),
                            };
                            view! {
                                <ProjectDetailContent
                                    project=project
                                    issues=issues
                                    progress=progress
                                    milestones=milestones
                                    server_milestones=server_milestones
                                />
                            }.into_any()
                        }
                        Some(Ok(None)) => {
                            view! { <ProjectNotFound/> }.into_any()
                        }
                        Some(Err(_)) => {
                            view! {
                                <div class="max-w-[860px] mx-auto w-full text-center py-16">
                                    <p class="text-muted-foreground">"Failed to load project. Please try again."</p>
                                </div>
                            }.into_any()
                        }
                        None => {
                            view! {
                                <div class="max-w-[860px] mx-auto w-full text-center py-16">
                                    <p class="text-muted-foreground">"Loading..."</p>
                                </div>
                            }.into_any()
                        }
                    }
                }}
            </div>
        </div>
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Project Detail Content
// ─────────────────────────────────────────────────────────────────────────────

/// The main content of the project detail page — metadata + issue list.
#[component]
fn ProjectDetailContent(
    project: Project,
    issues: Vec<IssueWithDetails>,
    progress: Option<ProjectProgress>,
    milestones: Vec<ProjectMilestone>,
    server_milestones: Resource<Result<Vec<ProjectMilestone>, ServerFnError>>,
) -> impl IntoView {

    let variant = status_variant(&project.status);
    let label = status_label(&project.status);
    let issue_count = issues.len();

    view! {
        <div class="max-w-[860px] mx-auto w-full">
            // ── Project name ──────────────────────────────────────────────
            <h1 class="text-2xl font-display text-foreground">
                {project.name.clone()}
            </h1>

            // ── Metadata bar ──────────────────────────────────────────────
            <div class="flex flex-wrap items-center gap-4 mt-4">
                // Status
                <div class="flex items-center gap-2">
                    <span class="text-xs text-muted-foreground font-medium uppercase tracking-wide">"Status"</span>
                    <StatusBadge variant=variant>
                        {label}
                    </StatusBadge>
                </div>

                // Lead
                {if project.lead_name.is_some() || project.lead_id.is_some() {
                    let display = project.lead_name.clone()
                        .unwrap_or_else(|| "Unknown".to_string());
                    Some(view! {
                        <div class="flex items-center gap-2">
                            <span class="text-xs text-muted-foreground font-medium uppercase tracking-wide">"Lead"</span>
                            <span class="text-sm text-foreground">{display}</span>
                        </div>
                    })
                } else {
                    None
                }}

                // Start date
                {project.start_date.as_deref().map(|d| {
                    let formatted = format_date(d);
                    view! {
                        <div class="flex items-center gap-2">
                            <span class="text-xs text-muted-foreground font-medium uppercase tracking-wide">"Start"</span>
                            <span class="font-mono text-sm text-foreground">{formatted}</span>
                        </div>
                    }
                })}

                // Target date
                {project.target_date.as_deref().map(|d| {
                    let formatted = format_date(d);
                    view! {
                        <div class="flex items-center gap-2">
                            <span class="text-xs text-muted-foreground font-medium uppercase tracking-wide">"Target"</span>
                            <span class="font-mono text-sm text-foreground">{formatted}</span>
                        </div>
                    }
                })}
            </div>

            // ── Description ───────────────────────────────────────────────
            {project.description.as_ref().map(|desc| {
                view! {
                    <div class="mt-4">
                        <p class="text-sm text-muted-foreground">{desc.clone()}</p>
                    </div>
                }
            })}

            // ── Progress bar ─────────────────────────────────────────────
            {progress.filter(|p| p.total > 0).map(|p| {
                view! { <ProgressSection progress=p/> }
            })}

            // ── Milestones ──────────────────────────────────────────────
            <MilestoneSection
                project_id=project.project_id.clone()
                milestones=milestones
                issues=issues.clone()
                server_milestones=server_milestones
            />

            // ── Divider ───────────────────────────────────────────────────
            <div class="border-t border-border my-6"></div>

            // ── Issues section ────────────────────────────────────────────
            <h2 class="text-sm font-medium text-foreground mb-4">
                {format!("Issues ({issue_count})")}
            </h2>

            {if issues.is_empty() {
                let empty_icon: Arc<dyn Fn() -> AnyView + Send + Sync> = Arc::new(move || {
                    view! {
                        <Icon icon=phosphor_leptos::CLIPBOARD_TEXT weight=phosphor_leptos::IconWeight::Duotone size="48px"/>
                    }.into_any()
                });
                view! {
                    <EmptyState
                        icon=empty_icon
                        title="No issues in this project"
                        description="Assign issues to this project to track them here."
                    />
                }.into_any()
            } else {
                let rows = issues.iter().map(|issue| {
                    view! { <ProjectIssueRow issue=issue.clone()/> }
                }).collect_view();
                view! {
                    <div role="list">
                        {rows}
                    </div>
                }.into_any()
            }}
        </div>
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Progress Section
// ─────────────────────────────────────────────────────────────────────────────

/// Issue completion progress bar — teal fill over muted background.
///
/// Shows the fraction and percentage right-aligned: "67% (8 of 12)".
/// Only rendered when `progress.total > 0` (guard is in the caller).
#[component]
fn ProgressSection(progress: ProjectProgress) -> impl IntoView {
    let pct = progress.percent_done.round() as i64;
    let done = progress.completed + progress.cancelled;

    view! {
        <div class="mt-6">
            <div class="flex items-center justify-between mb-2">
                <span class="text-sm font-medium text-foreground">"Progress"</span>
                <span class="font-mono text-sm text-muted-foreground">
                    {format!("{}% ({} of {})", pct, done, progress.total)}
                </span>
            </div>
            <div class="h-1.5 bg-muted rounded-full overflow-hidden">
                <div
                    class="h-full bg-primary rounded-full transition-all duration-200"
                    style=format!("width: {}%", progress.percent_done.clamp(0.0, 100.0))
                ></div>
            </div>
        </div>
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Milestone Section
// ─────────────────────────────────────────────────────────────────────────────

/// Milestone timeline section — lists project milestones with inline editing,
/// creation, and deletion.
///
/// Each milestone row shows a status dot (filled if all assigned issues are
/// done, empty ring otherwise), name, target date, issue count, and
/// edit/delete actions.
#[component]
fn MilestoneSection(
    project_id: String,
    milestones: Vec<ProjectMilestone>,
    issues: Vec<IssueWithDetails>,
    server_milestones: Resource<Result<Vec<ProjectMilestone>, ServerFnError>>,
) -> impl IntoView {
    // ── State signals ──────────────────────────────────────────────────────
    let editing_milestone_id: RwSignal<Option<String>> = RwSignal::new(None);
    let adding_milestone: RwSignal<bool> = RwSignal::new(false);
    let deleting_milestone_id: RwSignal<Option<String>> = RwSignal::new(None);

    // Refetch callback used after mutations.
    let refetch = move || server_milestones.refetch();

    // ── Delete confirmation ────────────────────────────────────────────────
    let delete_dialog_open = Signal::derive(move || deleting_milestone_id.get().is_some());

    let on_confirm_delete = {
        let refetch = refetch.clone();
        Callback::new(move |()| {
            if let Some(mid) = deleting_milestone_id.get_untracked() {
                deleting_milestone_id.set(None);
                leptos::task::spawn_local(async move {
                    if let Err(e) = delete_milestone(mid).await {
                        leptos::logging::warn!("Failed to delete milestone: {e}");
                    }
                    refetch();
                });
            }
        })
    };

    let on_cancel_delete = Callback::new(move |()| {
        deleting_milestone_id.set(None);
    });

    view! {
        <div class="mt-6">
            <h3 class="text-sm font-medium text-foreground mb-3">"Milestones"</h3>

            // ── Milestone rows ──────────────────────────────────────────
            <div class="flex flex-col gap-1">
                {milestones.iter().map(|ms| {
                    let ms_id = ms.milestone_id.clone();
                    let ms_name = ms.name.clone();
                    let ms_target_date = ms.target_date.clone();

                    // Count issues assigned to this milestone.
                    let assigned: Vec<&IssueWithDetails> = issues
                        .iter()
                        .filter(|i| i.milestone_id.as_deref() == Some(&ms_id))
                        .collect();
                    let total = assigned.len();
                    let done = assigned
                        .iter()
                        .filter(|i| {
                            i.status_category == "completed" || i.status_category == "cancelled"
                        })
                        .count();
                    let all_done = total > 0 && done == total;

                    let ms_id_edit = ms_id.clone();
                    let ms_id_delete = ms_id.clone();

                    view! {
                        <MilestoneRow
                            milestone_id=ms_id
                            name=ms_name
                            target_date=ms_target_date
                            done_count=done
                            total_count=total
                            all_done=all_done
                            editing_milestone_id=editing_milestone_id
                            on_edit=Callback::new(move |()| {
                                editing_milestone_id.set(Some(ms_id_edit.clone()));
                            })
                            on_delete=Callback::new(move |()| {
                                deleting_milestone_id.set(Some(ms_id_delete.clone()));
                            })
                            server_milestones=server_milestones
                        />
                    }
                }).collect_view()}
            </div>

            // ── Add milestone form / button ─────────────────────────────
            <Show
                when=move || adding_milestone.get()
                fallback=move || {
                    view! {
                        <div class="mt-2">
                            <Button
                                variant=ButtonVariant::GhostMuted
                                size=ButtonSize::Sm
                                on:click=move |_| adding_milestone.set(true)
                            >
                                <Icon icon=phosphor_leptos::PLUS size="14px"/>
                                "Add milestone"
                            </Button>
                        </div>
                    }
                }
            >
                <AddMilestoneForm
                    project_id=project_id.clone()
                    adding_milestone=adding_milestone
                    server_milestones=server_milestones
                />
            </Show>
        </div>

        // ── Delete confirmation dialog ──────────────────────────────────
        <ConfirmDialog
            open=delete_dialog_open
            title="Delete milestone?"
            message="This will remove the milestone. Issues assigned to it will become unassigned."
            confirm_text="Delete"
            destructive=true
            on_confirm=on_confirm_delete
            on_cancel=on_cancel_delete
        />
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Milestone Row
// ─────────────────────────────────────────────────────────────────────────────

/// A single milestone row — supports inline editing of name and target date.
///
/// Display mode: status dot + name + date + issue count + edit/delete buttons.
/// Edit mode: name input + date picker + save on Enter/blur, cancel on Escape.
#[component]
fn MilestoneRow(
    milestone_id: String,
    name: String,
    target_date: Option<String>,
    done_count: usize,
    total_count: usize,
    all_done: bool,
    editing_milestone_id: RwSignal<Option<String>>,
    on_edit: Callback<()>,
    on_delete: Callback<()>,
    server_milestones: Resource<Result<Vec<ProjectMilestone>, ServerFnError>>,
) -> impl IntoView {
    let mid = milestone_id.clone();
    let is_editing = Signal::derive(move || {
        editing_milestone_id.get().as_deref() == Some(&mid)
    });

    // ── Edit state signals ──
    let edit_name = RwSignal::new(name.clone());
    let edit_date = RwSignal::new(target_date.clone());

    // Reset edit fields when entering edit mode.
    let name_for_reset = name.clone();
    let date_for_reset = target_date.clone();
    Effect::new(move || {
        if is_editing.get() {
            edit_name.set(name_for_reset.clone());
            edit_date.set(date_for_reset.clone());
        }
    });

    // ── Save handler ──
    // Store originals so the save closure can be called multiple times (Fn, not FnOnce).
    let original_name = StoredValue::new(name.clone());
    let original_date = StoredValue::new(target_date.clone());
    let mid_save = StoredValue::new(milestone_id.clone());

    let do_save = move || {
        let new_name = edit_name.get_untracked();
        let new_date = edit_date.get_untracked();
        editing_milestone_id.set(None);

        // Only call server if something changed.
        let name_changed = new_name != original_name.get_value();
        let date_changed = new_date != original_date.get_value();
        if !name_changed && !date_changed {
            return;
        }

        let mid_c = mid_save.get_value();
        let name_param = if name_changed { Some(new_name) } else { None };
        // For target_date: None = no change, Some("") = clear, Some(val) = set.
        let date_param = if date_changed {
            Some(new_date.unwrap_or_default())
        } else {
            None
        };
        leptos::task::spawn_local(async move {
            if let Err(e) = update_milestone(mid_c, name_param, None, date_param).await {
                leptos::logging::warn!("Failed to update milestone: {e}");
            }
            server_milestones.refetch();
        });
    };

    let save_on_keydown = move |ev: leptos::ev::KeyboardEvent| {
        match ev.key().as_str() {
            "Enter" => {
                ev.prevent_default();
                do_save();
            }
            "Escape" => {
                ev.prevent_default();
                editing_milestone_id.set(None);
            }
            _ => {}
        }
    };

    // ── Date formatting ──
    let date_display = target_date
        .as_deref()
        .map(|d| format_short_date(d))
        .unwrap_or_else(|| "No date".to_string());

    // ── Issue count display ──
    let count_display = format!("{done_count}/{total_count} done");

    // ── Status dot class ──
    let dot_class = if all_done {
        "w-2 h-2 rounded-full bg-primary shrink-0"
    } else {
        "w-2 h-2 rounded-full border-2 border-muted-foreground shrink-0"
    };

    view! {
        <div class="flex items-center gap-2 py-1 group">
            // Status dot
            <span class=dot_class></span>

            <Show
                when=move || is_editing.get()
                fallback={
                    let name_display = name.clone();
                    let date_display = date_display.clone();
                    let count_display = count_display.clone();
                    move || view! {
                        // Name (click to edit)
                        <span
                            class="text-sm text-foreground cursor-pointer hover:text-primary transition-colors duration-200"
                            on:click=move |_| on_edit.run(())
                        >
                            {name_display.clone()}
                        </span>

                        // Target date
                        <span class="font-mono text-xs text-muted-foreground">
                            {date_display.clone()}
                        </span>

                        // Issue count
                        <span class="text-xs text-muted-foreground">
                            {count_display.clone()}
                        </span>

                        // Spacer to push buttons right
                        <span class="flex-1"></span>

                        // Edit button
                        <span class="opacity-0 group-hover:opacity-100 transition-opacity duration-200">
                            <Button
                                variant=ButtonVariant::GhostMuted
                                size=ButtonSize::IconXs
                                aria_label="Edit milestone"
                                on:click=move |_| on_edit.run(())
                            >
                                <Icon icon=phosphor_leptos::PENCIL_SIMPLE size="14px"/>
                            </Button>
                        </span>

                        // Delete button
                        <span class="opacity-0 group-hover:opacity-100 transition-opacity duration-200">
                            <Button
                                variant=ButtonVariant::GhostMuted
                                size=ButtonSize::IconXs
                                aria_label="Delete milestone"
                                on:click=move |_| on_delete.run(())
                            >
                                <Icon icon=phosphor_leptos::X size="14px"/>
                            </Button>
                        </span>
                    }
                }
            >
                {
                    let edit_date_signal = Signal::derive(move || edit_date.get());
                    let on_date_change = Callback::new(move |v: Option<String>| {
                        edit_date.set(v);
                    });
                    view! {
                        // Name input
                        <input
                            type="text"
                            class=format!("{INPUT_CLASS} !h-7 !py-0 !w-48")
                            prop:value=move || edit_name.get()
                            on:input=move |ev| {
                                edit_name.set(event_target_value(&ev));
                            }
                            on:keydown=save_on_keydown
                            autofocus=true
                        />

                        // Date picker
                        <DatePicker
                            value=edit_date_signal
                            on_change=on_date_change
                            placeholder="No date"
                        />

                        // Save / Cancel buttons
                        <Button
                            variant=ButtonVariant::Secondary
                            size=ButtonSize::Xs
                            on:click=move |_| do_save()
                        >
                            "Save"
                        </Button>
                        <Button
                            variant=ButtonVariant::GhostMuted
                            size=ButtonSize::Xs
                            on:click=move |_| editing_milestone_id.set(None)
                        >
                            "Cancel"
                        </Button>
                    }
                }
            </Show>
        </div>
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Add Milestone Form
// ─────────────────────────────────────────────────────────────────────────────

/// Inline form for creating a new milestone — name input + optional date
/// picker + Add/Cancel buttons.
#[component]
fn AddMilestoneForm(
    project_id: String,
    adding_milestone: RwSignal<bool>,
    server_milestones: Resource<Result<Vec<ProjectMilestone>, ServerFnError>>,
) -> impl IntoView {
    let new_name = RwSignal::new(String::new());
    let new_date: RwSignal<Option<String>> = RwSignal::new(None);

    let pid = project_id.clone();
    let do_create = move || {
        let name_val = new_name.get_untracked();
        if name_val.trim().is_empty() {
            return;
        }
        let date_val = new_date.get_untracked();
        adding_milestone.set(false);

        let pid_c = pid.clone();
        leptos::task::spawn_local(async move {
            if let Err(e) = create_milestone(pid_c, name_val, None, date_val).await {
                leptos::logging::warn!("Failed to create milestone: {e}");
            }
            server_milestones.refetch();
        });
    };

    let do_create_enter = do_create.clone();
    let on_keydown = move |ev: leptos::ev::KeyboardEvent| {
        match ev.key().as_str() {
            "Enter" => {
                ev.prevent_default();
                do_create_enter();
            }
            "Escape" => {
                ev.prevent_default();
                adding_milestone.set(false);
            }
            _ => {}
        }
    };

    let date_signal = Signal::derive(move || new_date.get());
    let on_date_change = Callback::new(move |v: Option<String>| {
        new_date.set(v);
    });

    view! {
        <div class="mt-2 flex items-center gap-2">
            <input
                type="text"
                class=format!("{INPUT_CLASS} !h-7 !py-0 !w-48")
                placeholder="Milestone name"
                prop:value=move || new_name.get()
                on:input=move |ev| new_name.set(event_target_value(&ev))
                on:keydown=on_keydown
                autofocus=true
            />
            <DatePicker
                value=date_signal
                on_change=on_date_change
                placeholder="No date"
            />
            <Button
                variant=ButtonVariant::Secondary
                size=ButtonSize::Sm
                on:click=move |_| do_create()
            >
                "Add"
            </Button>
            <Button
                variant=ButtonVariant::GhostMuted
                size=ButtonSize::Sm
                on:click=move |_| adding_milestone.set(false)
            >
                "Cancel"
            </Button>
        </div>
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Project Issue Row (mirrors IssueRow from issue_list.rs)
// ─────────────────────────────────────────────────────────────────────────────

/// An issue row within the project detail — same layout as the main issue list.
///
/// Order: Priority | Status | Issue ID (team-key + number) | Title | Labels | Date
#[component]
fn ProjectIssueRow(issue: IssueWithDetails) -> impl IntoView {
    let issue_key = format!("{}-{}", issue.team_key, issue.number);
    let issue_href = format!("/issues/{issue_key}");
    let status = IssueStatusVariant::parse(&issue.status_category);

    view! {
        <a
            href=issue_href
            class="h-9 px-3 py-[6px] flex items-center gap-2.5 border-b border-border hover:bg-surface-alt focus-visible:bg-surface-alt focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring transition-colors cursor-pointer no-underline text-inherit"
            role="listitem"
            tabindex="0"
        >
            // Priority icon
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
        </a>
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Not Found State
// ─────────────────────────────────────────────────────────────────────────────

/// Displayed when the project ID does not resolve to a project.
#[component]
fn ProjectNotFound() -> impl IntoView {
    let nav = use_navigate();

    view! {
        <div class="max-w-[860px] mx-auto w-full text-center py-16">
            <h2 class="text-xl font-semibold text-foreground mb-2">
                "Project not found"
            </h2>
            <p class="text-muted-foreground mb-6">
                "This project may have been deleted or you don\u{2019}t have access."
            </p>
            <Button
                variant=ButtonVariant::Secondary
                on:click=move |_| {
                    nav("/projects", Default::default());
                }
            >
                <Icon icon=phosphor_leptos::ARROW_LEFT size="14px"/>
                "Back to Projects"
            </Button>
        </div>
    }
}
