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
use leptos_router::hooks::{use_location, use_navigate, use_params_map};
use leptos_router::location::State;
use leptos_router::NavigateOptions;
use phosphor_leptos::Icon;

use crate::components::{
    Button, ButtonSize, ButtonVariant, Card, CardContent, CardDescription,
    CardHeader, CardTitle, ConfirmDialog, DatePicker, EmptyState,
    IssueStatusBadge, IssueStatusVariant, INPUT_CLASS,
    PriorityIndicator, LabelBadge, Select, SelectVariant,
    TeamKeyBadge, ToggleButton,
};
use crate::pages::projects::project_board::ProjectBoardContent;
use crate::pages::projects::project_list_view::ProjectListView;
use crate::server_fns::projects::{
    archive_project, delete_project,
    get_project, get_project_progress, list_milestones,
    create_milestone, update_milestone, delete_milestone,
    list_project_updates, create_project_update,
    update_project,
};
use crate::server_fns::issues::list_issues;
use crate::server_fns::team::list_workspace_members;
use crate::types::IssueNavState;
use crate::utils::date::{format_date, format_short_date};
use trakkt_types::models::{IssueWithDetails, Project, ProjectMilestone, ProjectProgress, ProjectUpdate};

// ─────────────────────────────────────────────────────────────────────────────
// Project Detail Page
// ─────────────────────────────────────────────────────────────────────────────

/// Parse the `view` query parameter from the URL search string.
///
/// Returns "overview", "board", or "list". Defaults to "overview" when absent.
fn parse_view_param(search: &str) -> String {
    for pair in search.split('&') {
        if pair.is_empty() {
            continue;
        }
        if let Some((key, value)) = pair.split_once('=')
            && key == "view"
        {
            return match value {
                "board" | "list" | "settings" => value.to_string(),
                _ => "overview".to_string(),
            };
        }
    }
    "overview".to_string()
}

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

    // Read view mode from URL query parameter.
    let location = use_location();
    let active_view = Memo::new(move |_| {
        let search = location.search.get();
        parse_view_param(&search)
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

    let server_updates = Resource::new(
        move || project_id.get(),
        move |id| async move { list_project_updates(id).await },
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
                // Pin/unpin toggle for workspace sidebar
                {move || {
                    let id = project_id.get();
                    if id.is_empty() {
                        None
                    } else {
                        Some(view! {
                            <crate::components::layout::FavoriteToggle target_type="project" target_id=id/>
                        })
                    }
                }}
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
                            let updates = match server_updates.get() {
                                Some(Ok(v)) => v,
                                Some(Err(e)) => {
                                    leptos::logging::warn!("Failed to load project updates: {e}");
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
                                    updates=updates
                                    server_updates=server_updates
                                    active_view=active_view
                                    project_id=project_id
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
///
/// Each metadata field is individually editable inline: click-to-edit name,
/// status dropdown, lead dropdown, date pickers, and click-to-edit description.
/// Mutations call `update_project` via `spawn_local` with optimistic local
/// signal updates; the SyncStore will reconcile via websocket.
#[component]
fn ProjectDetailContent(
    project: Project,
    issues: Vec<IssueWithDetails>,
    progress: Option<ProjectProgress>,
    milestones: Vec<ProjectMilestone>,
    server_milestones: Resource<Result<Vec<ProjectMilestone>, ServerFnError>>,
    updates: Vec<ProjectUpdate>,
    server_updates: Resource<Result<Vec<ProjectUpdate>, ServerFnError>>,
    /// The currently active view tab: "overview", "board", or "list".
    #[prop(into)]
    active_view: Memo<String>,
    /// The project ID (reactive memo).
    #[prop(into)]
    project_id: Memo<String>,
) -> impl IntoView {
    let issue_count = issues.len();
    let pid = StoredValue::new(project.project_id.clone());

    // Milestones signal — reactive via server_milestones Resource, falls back
    // to the initial snapshot when the Resource hasn't resolved yet.
    let initial_milestones = StoredValue::new(milestones.clone());
    let milestones_signal: Signal<Vec<ProjectMilestone>> = Signal::derive(move || {
        match server_milestones.get() {
            Some(Ok(v)) => v,
            _ => initial_milestones.get_value(),
        }
    });

    // Hoist use_navigate to component construction time (not inside closures).
    let nav_create_view = use_navigate();

    // ── Editable name (click-to-edit) ────────────────────────────────────
    let editing_name = RwSignal::new(false);
    let name_value = RwSignal::new(project.name.clone());
    let name_draft = RwSignal::new(project.name.clone());

    let save_name = move || {
        let new_name = name_draft.get_untracked();
        if new_name.trim().is_empty() {
            // Don't save empty names — revert.
            name_draft.set(name_value.get_untracked());
            editing_name.set(false);
            return;
        }
        if new_name == name_value.get_untracked() {
            editing_name.set(false);
            return;
        }
        name_value.set(new_name.clone());
        editing_name.set(false);
        let project_id = pid.get_value();
        leptos::task::spawn_local(async move {
            if let Err(e) = update_project(
                project_id, Some(new_name), None, None, None, None, None, None, None,
            ).await {
                leptos::logging::warn!("Failed to update project name: {e}");
            }
        });
    };

    let name_keydown = move |ev: leptos::ev::KeyboardEvent| {
        match ev.key().as_str() {
            "Enter" => {
                ev.prevent_default();
                save_name();
            }
            "Escape" => {
                ev.prevent_default();
                name_draft.set(name_value.get_untracked());
                editing_name.set(false);
            }
            _ => {}
        }
    };

    // ── Editable status ────────────────────────────────────────────────
    let status_value = RwSignal::new(project.status.clone());

    // ── Editable lead ──────────────────────────────────────────────────
    let lead_value = RwSignal::new(project.lead_id.clone().unwrap_or_default());

    let members_resource = Resource::new(
        || (),
        move |_| async move { list_workspace_members().await },
    );

    let member_options = Signal::derive(move || {
        let mut opts = vec![("".to_string(), "(Unassigned)".to_string())];
        if let Some(Ok(members)) = members_resource.get() {
            for m in members {
                let label = m.name.unwrap_or_else(|| m.email.clone());
                opts.push((m.user_id, label));
            }
        }
        opts
    });

    // ── Editable dates (DatePicker) ──────────────────────────────────────
    let start_date_value: RwSignal<Option<String>> = RwSignal::new(project.start_date.clone());
    let target_date_value: RwSignal<Option<String>> = RwSignal::new(project.target_date.clone());

    let start_date_signal = Signal::derive(move || start_date_value.get());
    let target_date_signal = Signal::derive(move || target_date_value.get());

    let on_start_date_change = {
        Callback::new(move |v: Option<String>| {
            start_date_value.set(v.clone());
            let project_id = pid.get_value();
            // Some("") = clear, Some(val) = set
            let param = Some(v.unwrap_or_default());
            leptos::task::spawn_local(async move {
                if let Err(e) = update_project(
                    project_id, None, None, None, None, None, None, param, None,
                ).await {
                    leptos::logging::warn!("Failed to update start date: {e}");
                }
            });
        })
    };

    let on_target_date_change = {
        Callback::new(move |v: Option<String>| {
            target_date_value.set(v.clone());
            let project_id = pid.get_value();
            let param = Some(v.unwrap_or_default());
            leptos::task::spawn_local(async move {
                if let Err(e) = update_project(
                    project_id, None, None, None, None, None, None, None, param,
                ).await {
                    leptos::logging::warn!("Failed to update target date: {e}");
                }
            });
        })
    };

    // ── Editable description (click-to-edit textarea) ────────────────────
    let editing_desc = RwSignal::new(false);
    let desc_value = RwSignal::new(project.description.clone().unwrap_or_default());
    let desc_draft = RwSignal::new(project.description.clone().unwrap_or_default());

    let save_desc = move || {
        let new_desc = desc_draft.get_untracked();
        if new_desc == desc_value.get_untracked() {
            editing_desc.set(false);
            return;
        }
        desc_value.set(new_desc.clone());
        editing_desc.set(false);
        let project_id = pid.get_value();
        leptos::task::spawn_local(async move {
            if let Err(e) = update_project(
                project_id, None, Some(new_desc), None, None, None, None, None, None,
            ).await {
                leptos::logging::warn!("Failed to update description: {e}");
            }
        });
    };

    view! {
        <div>
        <div class="max-w-[860px] mx-auto w-full">
            // ── Project name (click-to-edit) ─────────────────────────────
            <Show
                when=move || editing_name.get()
                fallback=move || view! {
                    <h1
                        class="text-2xl font-display text-foreground cursor-pointer hover:text-foreground/80 transition-colors duration-200 focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring rounded-sm"
                        tabindex="0"
                        role="button"
                        aria-label="Click to edit project name"
                        on:click=move |_| {
                            name_draft.set(name_value.get_untracked());
                            editing_name.set(true);
                        }
                        on:keydown=move |ev: leptos::ev::KeyboardEvent| {
                            if ev.key() == "Enter" {
                                name_draft.set(name_value.get_untracked());
                                editing_name.set(true);
                            }
                        }
                    >
                        {move || name_value.get()}
                    </h1>
                }
            >
                <input
                    type="text"
                    class=format!("{INPUT_CLASS} !text-2xl !font-display !h-auto !py-1")
                    prop:value=move || name_draft.get()
                    on:input=move |ev| name_draft.set(event_target_value(&ev))
                    on:keydown=name_keydown
                    on:blur=move |_| save_name()
                    autofocus=true
                />
            </Show>

            // ── Metadata bar ─────────────────────────────────────────────
            <div class="flex flex-wrap items-center gap-4 mt-4">
                // Status
                <div class="flex items-center gap-2">
                    <span class="text-xs text-muted-foreground font-medium uppercase tracking-wide">"Status"</span>
                    <div class="w-36">
                        <Select
                            value=status_value
                            options=Signal::derive(|| vec![
                                ("planned".to_string(), "Planned".to_string()),
                                ("in_progress".to_string(), "In Progress".to_string()),
                                ("paused".to_string(), "Paused".to_string()),
                                ("completed".to_string(), "Completed".to_string()),
                                ("cancelled".to_string(), "Cancelled".to_string()),
                            ])
                            on_change=Callback::new(move |new_status: String| {
                                status_value.set(new_status.clone());
                                let project_id = pid.get_value();
                                leptos::task::spawn_local(async move {
                                    if let Err(e) = update_project(
                                        project_id, None, None, None, None, Some(new_status), None, None, None,
                                    ).await {
                                        leptos::logging::warn!("Failed to update status: {e}");
                                    }
                                });
                            })
                            variant=SelectVariant::Form
                        />
                    </div>
                </div>

                // Lead
                <div class="flex items-center gap-2">
                    <span class="text-xs text-muted-foreground font-medium uppercase tracking-wide">"Lead"</span>
                    <div class="w-44">
                        <Select
                            value=lead_value
                            options=member_options
                            on_change=Callback::new(move |new_lead: String| {
                                lead_value.set(new_lead.clone());
                                let project_id = pid.get_value();
                                // Some("") = clear, Some(id) = set
                                let param = Some(new_lead);
                                leptos::task::spawn_local(async move {
                                    if let Err(e) = update_project(
                                        project_id, None, None, None, None, None, param, None, None,
                                    ).await {
                                        leptos::logging::warn!("Failed to update lead: {e}");
                                    }
                                });
                            })
                            variant=SelectVariant::Form
                            placeholder="Unassigned"
                        />
                    </div>
                </div>

                // Start date (DatePicker)
                <div class="flex items-center gap-2">
                    <span class="text-xs text-muted-foreground font-medium uppercase tracking-wide">"Start"</span>
                    <DatePicker
                        value=start_date_signal
                        on_change=on_start_date_change
                        placeholder="Set start date"
                    />
                </div>

                // Target date (DatePicker)
                <div class="flex items-center gap-2">
                    <span class="text-xs text-muted-foreground font-medium uppercase tracking-wide">"Target"</span>
                    <DatePicker
                        value=target_date_signal
                        on_change=on_target_date_change
                        placeholder="Set target date"
                    />
                </div>
            </div>

            // ── Description (click-to-edit textarea) ─────────────────────
            <div class="mt-4">
                <Show
                    when=move || editing_desc.get()
                    fallback=move || {
                        view! {
                            <p
                                class=move || {
                                    if !desc_value.get().is_empty() {
                                        "text-sm text-muted-foreground cursor-pointer hover:text-foreground/80 transition-colors duration-200 focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring rounded-sm"
                                    } else {
                                        "text-sm text-muted-foreground/60 italic cursor-pointer hover:text-muted-foreground transition-colors duration-200 focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring rounded-sm"
                                    }
                                }
                                tabindex="0"
                                role="button"
                                aria-label="Click to edit description"
                                on:click=move |_| {
                                    desc_draft.set(desc_value.get_untracked());
                                    editing_desc.set(true);
                                }
                                on:keydown=move |ev: leptos::ev::KeyboardEvent| {
                                    if ev.key() == "Enter" {
                                        desc_draft.set(desc_value.get_untracked());
                                        editing_desc.set(true);
                                    }
                                }
                            >
                                {move || {
                                    let v = desc_value.get();
                                    if v.is_empty() {
                                        "Add a description...".to_string()
                                    } else {
                                        v
                                    }
                                }}
                            </p>
                        }
                    }
                >
                    <textarea
                        class=format!("{INPUT_CLASS} !h-auto min-h-[80px] w-full resize-y")
                        prop:value=move || desc_draft.get()
                        on:input=move |ev| desc_draft.set(event_target_value(&ev))
                        autofocus=true
                    ></textarea>
                    <div class="flex items-center gap-2 mt-2">
                        <Button
                            variant=ButtonVariant::Secondary
                            size=ButtonSize::Sm
                            on:click=move |_| save_desc()
                        >
                            "Save"
                        </Button>
                        <Button
                            variant=ButtonVariant::GhostMuted
                            size=ButtonSize::Sm
                            on:click=move |_| {
                                desc_draft.set(desc_value.get_untracked());
                                editing_desc.set(false);
                            }
                        >
                            "Cancel"
                        </Button>
                    </div>
                </Show>
            </div>

            // ── Progress bar ─────────────────────────────────────────────
            {progress.filter(|p| p.total > 0).map(|p| {
                view! { <ProgressSection progress=p/> }
            })}

            // ── Milestones ──────────────────────────────────────────────
            <MilestoneSection
                project_id=project.project_id.clone()
                milestones=milestones.clone()
                issues=issues.clone()
                server_milestones=server_milestones
            />

            // ── Health Updates ───────────────────────────────────────────
            <HealthUpdateSection
                project_id=project.project_id.clone()
                updates=updates
                server_updates=server_updates
            />

            // ── Divider ───────────────────────────────────────────────────
            <div class="border-t border-border my-6"></div>

            // ── View tabs ────────────────────────────────────────────────
            <div class="flex items-center justify-between mb-4">
                <div class="flex items-center gap-1">
                    <ProjectViewTab
                        label="Overview"
                        value="overview"
                        active=active_view
                        project_id=pid
                    />
                    <ProjectViewTab
                        label="Board"
                        value="board"
                        active=active_view
                        project_id=pid
                    />
                    <ProjectViewTab
                        label="List"
                        value="list"
                        active=active_view
                        project_id=pid
                    />
                    <ProjectViewTab
                        label="Settings"
                        value="settings"
                        active=active_view
                        project_id=pid
                    />

                    // Issue count badge
                    <span class="text-xs text-muted-foreground ml-2">
                        {format!("{issue_count}")}
                    </span>
                </div>
                <Button
                    variant=ButtonVariant::Ghost
                    size=ButtonSize::Sm
                    on:click={
                        let nav = nav_create_view.clone();
                        move |_| {
                            let project_id = pid.get_value();
                            let filter = serde_json::json!([{
                                "field": "project",
                                "operator": "any_of",
                                "values": [project_id]
                            }]);
                            let filter_str = filter.to_string();
                            let encoded = percent_encoding::utf8_percent_encode(
                                &filter_str,
                                percent_encoding::NON_ALPHANUMERIC,
                            );
                            let url = format!("/workspace?filters={encoded}&new_view=1");
                            nav(&url, NavigateOptions {
                                resolve: false,
                                ..Default::default()
                            });
                        }
                    }
                >
                    <Icon icon=phosphor_leptos::FUNNEL size="16px"/>
                    "Create View"
                </Button>
            </div>
        </div>

        // ── View content ─────────────────────────────────────────────────
        // Board and list views break out of the max-w container to use full
        // width. Overview stays within the 860px constraint.
        {move || {
            let view = active_view.get();
            match view.as_str() {
                "board" => {
                    let pid_signal = Signal::derive(move || project_id.get());
                    view! {
                        <ProjectBoardContent project_id=pid_signal/>
                    }.into_any()
                }
                "list" => {
                    let pid_signal = Signal::derive(move || project_id.get());
                    view! {
                        <ProjectListView project_id=pid_signal milestones=milestones_signal/>
                    }.into_any()
                }
                "settings" => {
                    view! {
                        <div class="max-w-[860px] mx-auto w-full">
                            <ProjectSettingsContent
                                project=project.clone()
                            />
                        </div>
                    }.into_any()
                }
                _ => {
                    // Overview: milestone-grouped issue list.
                    view! {
                        <div class="max-w-[860px] mx-auto w-full">
                            <ProjectOverviewIssues
                                issues=issues.clone()
                                milestones=milestones.clone()
                            />
                        </div>
                    }.into_any()
                }
            }
        }}
        </div>
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// View Tab Button
// ─────────────────────────────────────────────────────────────────────────────

/// A single tab button in the project detail view switcher.
///
/// Renders as a pill-style toggle button. Active tab gets a highlighted background.
/// Navigates via query parameter: `/projects/:id?view=board`.
#[component]
fn ProjectViewTab(
    /// Display label (e.g. "Board").
    label: &'static str,
    /// Query param value (e.g. "board").
    value: &'static str,
    /// The currently active view.
    #[prop(into)]
    active: Memo<String>,
    /// Stored project ID for building the URL.
    project_id: StoredValue<String>,
) -> impl IntoView {
    let nav = use_navigate();
    let is_active = move || {
        active.get() == value
    };

    let variant = Signal::derive(move || {
        if is_active() { ButtonVariant::PillActive } else { ButtonVariant::Pill }
    });

    view! {
        <ToggleButton
            variant=variant
            size=ButtonSize::Xs
            on:click={
                let nav = nav.clone();
                move |_| {
                    if is_active() {
                        return;
                    }
                    let pid = project_id.get_value();
                    let url = if value == "overview" {
                        format!("/projects/{pid}")
                    } else {
                        format!("/projects/{pid}?view={value}")
                    };
                    nav(&url, NavigateOptions {
                        resolve: false,
                        ..Default::default()
                    });
                }
            }
        >
            {label}
        </ToggleButton>
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Overview Issues (extracted from ProjectDetailContent)
// ─────────────────────────────────────────────────────────────────────────────

/// The milestone-grouped issue list shown in the overview tab.
///
/// Extracted into its own component so the view-switching logic in
/// `ProjectDetailContent` stays clean.
#[component]
fn ProjectOverviewIssues(
    issues: Vec<IssueWithDetails>,
    milestones: Vec<ProjectMilestone>,
) -> impl IntoView {
    if issues.is_empty() {
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
    } else if milestones.is_empty() {
        // No milestones — flat list, no grouping.
        let rows = issues.iter().map(|issue| {
            view! { <ProjectIssueRow issue=issue.clone()/> }
        }).collect_view();
        view! {
            <div role="list">
                {rows}
            </div>
        }.into_any()
    } else {
        // Group issues by milestone.
        let mut groups: Vec<(Option<&str>, &str, Vec<&IssueWithDetails>)> = Vec::new();

        for ms in &milestones {
            let group_issues: Vec<&IssueWithDetails> = issues
                .iter()
                .filter(|i| i.milestone_id.as_deref() == Some(ms.milestone_id.as_str()))
                .collect();
            if !group_issues.is_empty() {
                groups.push((Some(ms.milestone_id.as_str()), ms.name.as_str(), group_issues));
            }
        }

        // Collect issues with no milestone AND issues whose milestone_id
        // doesn't match any known milestone (orphaned) into one group.
        let known_ids: Vec<&str> = milestones.iter().map(|ms| ms.milestone_id.as_str()).collect();
        let no_milestone: Vec<&IssueWithDetails> = issues
            .iter()
            .filter(|i| match &i.milestone_id {
                None => true,
                Some(mid) => !known_ids.contains(&mid.as_str()),
            })
            .collect();
        if !no_milestone.is_empty() {
            groups.push((None, "No milestone", no_milestone));
        }

        let group_views = groups.iter().map(|(_ms_id, label, group_issues)| {
            let total = group_issues.len();
            let done = group_issues.iter().filter(|i| {
                i.status_category == "completed" || i.status_category == "cancelled"
            }).count();
            let status_text = format!("{done}/{total} done");

            let is_no_milestone = *label == "No milestone";
            let header_class = if is_no_milestone {
                "text-sm font-medium text-muted-foreground"
            } else {
                "text-sm font-medium text-foreground"
            };

            let rows = group_issues.iter().map(|issue| {
                view! { <ProjectIssueRow issue=(*issue).clone()/> }
            }).collect_view();

            view! {
                <div class="mb-6">
                    <div class="flex items-center justify-between border-b border-border pb-2 mb-1">
                        <span class=header_class>{label.to_string()}</span>
                        <span class="text-xs text-muted-foreground">{status_text}</span>
                    </div>
                    <div role="list">
                        {rows}
                    </div>
                </div>
            }
        }).collect_view();

        view! {
            <div>
                {group_views}
            </div>
        }.into_any()
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
// Health Update Section
// ─────────────────────────────────────────────────────────────────────────────

/// Returns the CSS class for the health status dot.
fn health_dot_class(health: &str) -> &'static str {
    match health {
        "on_track" => "bg-primary",
        "at_risk" => "bg-[var(--color-warning-foreground)]",
        "off_track" => "bg-[var(--color-destructive)]",
        _ => "bg-muted-foreground",
    }
}

/// Returns the human-readable label for a health status value.
fn health_label(health: &str) -> &'static str {
    match health {
        "on_track" => "On Track",
        "at_risk" => "At Risk",
        "off_track" => "Off Track",
        _ => "Unknown",
    }
}

/// Health update timeline section — lists project status updates with an
/// inline form for posting new ones.
///
/// Each update card shows a colored health dot, label, formatted date,
/// and optional body text. Cards are stacked newest-first.
#[component]
fn HealthUpdateSection(
    project_id: String,
    updates: Vec<ProjectUpdate>,
    server_updates: Resource<Result<Vec<ProjectUpdate>, ServerFnError>>,
) -> impl IntoView {
    let posting_update: RwSignal<bool> = RwSignal::new(false);
    let post_health: RwSignal<String> = RwSignal::new("on_track".to_string());
    let post_body: RwSignal<String> = RwSignal::new(String::new());

    let pid = StoredValue::new(project_id.clone());
    let do_post = move || {
        let health_val = post_health.get_untracked();
        let body_val = post_body.get_untracked();
        let body_param = if body_val.trim().is_empty() {
            None
        } else {
            Some(body_val)
        };

        let pid_c = pid.get_value();
        leptos::task::spawn_local(async move {
            match create_project_update(pid_c, health_val, body_param).await {
                Ok(_) => {
                    posting_update.set(false);
                    post_health.set("on_track".to_string());
                    post_body.set(String::new());
                }
                Err(e) => {
                    leptos::logging::warn!("Failed to create project update: {e}");
                }
            }
            server_updates.refetch();
        });
    };

    let has_updates = !updates.is_empty();

    view! {
        <div class="mt-6">
            <h3 class="text-sm font-medium text-foreground mb-3">"Updates"</h3>

            // ── Update cards ──────────────────────────────────────────────
            {if has_updates {
                Some(view! {
                    <div class="flex flex-col gap-2">
                        {updates.iter().map(|update| {
                            let dot_class = format!(
                                "w-2 h-2 rounded-full shrink-0 {}",
                                health_dot_class(&update.health)
                            );
                            let label = health_label(&update.health);
                            let date = format_date(&update.created_at);
                            let body = update.body.clone();

                            view! {
                                <div class="border border-border rounded-md p-3">
                                    <div class="flex items-center gap-2">
                                        <span class=dot_class></span>
                                        <span class="text-sm font-medium text-foreground">{label}</span>
                                        <span class="text-muted-foreground">{"\u{2014}"}</span>
                                        <span class="font-mono text-xs text-muted-foreground">{date}</span>
                                    </div>
                                    {body.filter(|b| !b.trim().is_empty()).map(|b| {
                                        view! {
                                            <p class="text-sm text-foreground mt-1">{b}</p>
                                        }
                                    })}
                                </div>
                            }
                        }).collect_view()}
                    </div>
                })
            } else {
                None
            }}

            // ── Post update form / button ─────────────────────────────────
            <Show
                when=move || posting_update.get()
                fallback=move || {
                    view! {
                        <div class={if has_updates { "mt-2" } else { "" }}>
                            <Button
                                variant=ButtonVariant::GhostMuted
                                size=ButtonSize::Sm
                                on:click=move |_| posting_update.set(true)
                            >
                                <Icon icon=phosphor_leptos::PLUS size="14px"/>
                                "Post update"
                            </Button>
                        </div>
                    }
                }
            >
                <PostUpdateForm
                    posting_update=posting_update
                    post_health=post_health
                    post_body=post_body
                    on_post=Callback::new(move |()| do_post())
                />
            </Show>
        </div>
    }
}

/// Inline form for posting a new health update — health pill selector,
/// body textarea, and Post/Cancel action buttons.
#[component]
fn PostUpdateForm(
    posting_update: RwSignal<bool>,
    post_health: RwSignal<String>,
    post_body: RwSignal<String>,
    on_post: Callback<()>,
) -> impl IntoView {
    let cancel = move || {
        posting_update.set(false);
        post_health.set("on_track".to_string());
        post_body.set(String::new());
    };

    view! {
        <div class="mt-2 border border-border rounded-md p-3">
            // ── Health selector (pill buttons) ──────────────────────────
            <div class="flex items-center gap-1 mb-3">
                {["on_track", "at_risk", "off_track"].into_iter().map(|value| {
                    let label = health_label(value);
                    let dot_class = format!(
                        "w-2 h-2 rounded-full shrink-0 {}",
                        health_dot_class(value)
                    );
                    let value_owned = value.to_string();
                    let value_for_click = value.to_string();

                    view! {
                        <ToggleButton
                            variant=Signal::derive(move || {
                                if post_health.get() == value_owned {
                                    ButtonVariant::PillActive
                                } else {
                                    ButtonVariant::Pill
                                }
                            })
                            size=ButtonSize::Pill
                            on:click=move |_| post_health.set(value_for_click.clone())
                        >
                            <span class=dot_class.clone()></span>
                            {label}
                        </ToggleButton>
                    }
                }).collect_view()}
            </div>

            // ── Body textarea ───────────────────────────────────────────
            <textarea
                class=format!("{INPUT_CLASS} !h-auto min-h-[80px] w-full resize-y")
                placeholder="What\u{2019}s the latest?"
                prop:value=move || post_body.get()
                on:input=move |ev| post_body.set(event_target_value(&ev))
            ></textarea>

            // ── Action buttons ──────────────────────────────────────────
            <div class="flex items-center gap-2 mt-2">
                <Button
                    variant=ButtonVariant::Secondary
                    size=ButtonSize::Sm
                    on:click=move |_| on_post.run(())
                >
                    "Post"
                </Button>
                <Button
                    variant=ButtonVariant::GhostMuted
                    size=ButtonSize::Sm
                    on:click=move |_| cancel()
                >
                    "Cancel"
                </Button>
            </div>
        </div>
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
        .map(format_short_date)
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
    let issue_href = format!("/issues/{}-{}", issue.team_key, issue.number);
    let issue_href_click = issue_href.clone();
    let status = IssueStatusVariant::parse(&issue.status_category, &issue.status_name);
    let location = use_location();

    // Look up team color from SyncStore (same pattern as issue_row.rs)
    let team_color = {
        let sync_store = use_context::<crate::cache::store::SyncStore>();
        let team_id = issue.team_id.clone();
        sync_store.and_then(|store| {
            store.teams().get_untracked().into_iter()
                .find(|t| t.team_id == team_id)
                .and_then(|t| t.icon_color.clone())
        }).unwrap_or_else(|| crate::components::team_icon::DEFAULT_ICON_COLOR.to_string())
    };

    view! {
        <a
            href=issue_href
            class="h-9 px-3 py-[6px] flex items-center gap-2.5 border-b border-border hover:bg-surface-alt focus-visible:bg-surface-alt focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring transition-colors cursor-pointer no-underline text-inherit"
            role="listitem"
            tabindex="0"
            on:click={
                let issue_href = issue_href_click.clone();
                move |ev: web_sys::MouseEvent| {
                    if ev.meta_key() || ev.ctrl_key() || ev.shift_key() || ev.alt_key() || ev.button() != 0 {
                        return;
                    }
                    ev.prevent_default();
                    let path = location.pathname.get_untracked();
                    let search = location.search.get_untracked();
                    let nav_state = IssueNavState::from_current_path(&path, &search);
                    let json = nav_state.to_json();
                    let nav = use_navigate();
                    nav(&issue_href, NavigateOptions {
                        state: State::from(wasm_bindgen::JsValue::from_str(&json)),
                        ..Default::default()
                    });
                }
            }
        >
            // Priority icon
            <PriorityIndicator priority=issue.priority/>

            // Status icon
            <IssueStatusBadge status=status/>

            // Team badge (colored pill with team key)
            <TeamKeyBadge team_key=issue.team_key.clone() color=team_color.clone()/>
            // Issue number (Geist Mono)
            <span class="font-mono text-xs text-muted-foreground shrink-0">
                {issue.number}
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
// Project Settings Content
// ─────────────────────────────────────────────────────────────────────────────

/// Settings tab content for a project — archive/unarchive and delete.
///
/// Layout follows the same Card pattern used in `team_settings.rs`.
#[component]
fn ProjectSettingsContent(
    project: Project,
) -> impl IntoView {
    let project_id = StoredValue::new(project.project_id.clone());
    let project_name = StoredValue::new(project.name.clone());
    let is_archived = RwSignal::new(project.archived_at.is_some());

    let nav = use_navigate();
    let store = use_context::<crate::cache::store::SyncStore>();

    // ── Archive handler ────────────────────────────────────────────────────
    let on_archive_toggle = move |_| {
        let pid = project_id.get_value();
        let currently_archived = is_archived.get_untracked();
        let archive = !currently_archived;
        is_archived.set(archive);
        leptos::task::spawn_local(async move {
            if let Err(e) = archive_project(pid, archive).await {
                tracing::warn!(error = %e, "Failed to archive/unarchive project");
                is_archived.set(!archive);
            }
        });
    };

    // ── Delete confirmation ────────────────────────────────────────────────
    let (show_delete_confirm, set_show_delete_confirm) = signal(false);

    let confirm_message = format!(
        "Are you sure you want to delete \"{}\"? Issues linked to this project will be \
         unlinked. This action cannot be undone.",
        project_name.get_value(),
    );

    let on_confirm_delete = {
        let nav = nav.clone();
        Callback::new(move |()| {
            set_show_delete_confirm.set(false);
            let pid = project_id.get_value();
            let nav = nav.clone();
            leptos::task::spawn_local(async move {
                match delete_project(pid.clone()).await {
                    Ok(()) => {
                        if let Some(store) = store {
                            store.remove_project(&pid);
                        }
                        nav("/projects", Default::default());
                    }
                    Err(e) => {
                        tracing::warn!(error = %e, "Failed to delete project");
                    }
                }
            });
        })
    };

    let on_cancel_delete = Callback::new(move |()| {
        set_show_delete_confirm.set(false);
    });

    view! {
        <div class="space-y-6">
            // ── Archive card ───────────────────────────────────────────────
            <Card>
                <CardHeader>
                    <CardTitle>"Archive project"</CardTitle>
                    <CardDescription>
                        "Archived projects are hidden from the project list and sidebar. \
                         You can unarchive a project at any time."
                    </CardDescription>
                </CardHeader>
                <CardContent>
                    <div class="flex items-center justify-between">
                        <div>
                            <p class="text-sm font-medium text-foreground">
                                {move || if is_archived.get() {
                                    "This project is archived"
                                } else {
                                    "Archive this project"
                                }}
                            </p>
                            <p class="text-xs text-muted-foreground">
                                {move || if is_archived.get() {
                                    "Unarchive to restore the project to the active list."
                                } else {
                                    "Archived projects remain accessible via the filter toggle."
                                }}
                            </p>
                        </div>
                        <Button
                            variant={if project.archived_at.is_some() {
                                ButtonVariant::Secondary
                            } else {
                                ButtonVariant::Default
                            }}
                            on:click=on_archive_toggle
                        >
                            {move || if is_archived.get() {
                                "Unarchive"
                            } else {
                                "Archive"
                            }}
                        </Button>
                    </div>
                </CardContent>
            </Card>

            // ── Danger Zone card ───────────────────────────────────────────
            <Card>
                <CardHeader>
                    <CardTitle>
                        <span class="text-error-foreground">"Danger Zone"</span>
                    </CardTitle>
                    <CardDescription>
                        "Destructive actions that cannot be undone."
                    </CardDescription>
                </CardHeader>
                <CardContent>
                    <div class="flex items-center justify-between">
                        <div>
                            <p class="text-sm font-medium text-foreground">"Delete this project"</p>
                            <p class="text-xs text-muted-foreground">
                                "Permanently delete this project. Issues will be unlinked but not deleted."
                            </p>
                        </div>
                        <Button
                            variant=ButtonVariant::Destructive
                            on:click=move |_| set_show_delete_confirm.set(true)
                        >
                            "Delete project"
                        </Button>
                    </div>
                </CardContent>
            </Card>
        </div>

        // ── Delete confirmation dialog ─────────────────────────────────────
        <ConfirmDialog
            open=Signal::derive(move || show_delete_confirm.get())
            title="Delete project?"
            message=confirm_message
            confirm_text="Delete"
            destructive=true
            on_confirm=on_confirm_delete
            on_cancel=on_cancel_delete
        />
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
