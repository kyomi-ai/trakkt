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
    Button, ButtonSize, ButtonVariant, EmptyState,
    IssueStatusBadge, IssueStatusVariant,
    PriorityIndicator, LabelBadge,
    StatusBadge,
};
use crate::server_fns::projects::get_project;
use crate::server_fns::issues::list_issues;
use crate::utils::date::{format_date, format_short_date};
use crate::utils::project::{status_label, status_variant};
use trakkt_types::models::{IssueWithDetails, Project};

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
            list_issues(None, None, None, None, None, None, None).await
        },
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
                            view! {
                                <ProjectDetailContent project=project issues=issues/>
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
// Project Issue Row (mirrors IssueRow from issue_list.rs)
// ─────────────────────────────────────────────────────────────────────────────

/// An issue row within the project detail — same layout as the main issue list.
///
/// Order: Priority | Status | Issue ID (team-key + number) | Title | Labels | Date
#[component]
fn ProjectIssueRow(issue: IssueWithDetails) -> impl IntoView {
    let number = issue.number;
    let issue_key = format!("{}-{}", issue.team_key, issue.number);
    let issue_href = format!("/issues/{number}");
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
