// SPDX-License-Identifier: AGPL-3.0-or-later

//! Project list page — lists all projects in the workspace.
//!
//! Layout follows the same dual-source pattern as IssueListPage:
//! SyncStore (real-time) with server function fallback for SSR.
//!
//! Each project shows: name, status badge, start date, target date.
//! Clicking a project navigates to `/projects/{project_id}`.

use std::sync::Arc;

use leptos::prelude::*;
use phosphor_leptos::Icon;

use crate::components::{Alert, AlertVariant, EmptyState, StatusBadge};
use crate::server_fns::projects::list_projects;
use crate::utils::date::format_date;
use crate::utils::project::{status_label, status_variant};

// ─────────────────────────────────────────────────────────────────────────────
// Project List Page
// ─────────────────────────────────────────────────────────────────────────────

/// Main project list page — displays all projects in the workspace.
#[component]
pub fn ProjectListPage() -> impl IntoView {
    // ── Data source: SyncStore (real-time) with server function fallback ───
    let sync_store = use_context::<crate::cache::store::SyncStore>();

    // ── Error state for server function failures ──────────────────────────
    let error_msg = RwSignal::new(Option::<String>::None);

    let server_projects = Resource::new(
        || (),
        move |_| async move { list_projects().await },
    );

    let projects = Memo::new(move |_| {
        if let Some(store) = sync_store {
            let items = store.projects().get();
            if !items.is_empty() || store.initialized().get() {
                return items;
            }
        }
        // SSR or store not yet initialized — use server function
        match server_projects.get() {
            Some(Ok(items)) => {
                error_msg.set(None);
                items
            }
            Some(Err(e)) => {
                error_msg.set(Some(format!("Failed to load projects: {e}")));
                Vec::new()
            }
            None => Vec::new(),
        }
    });

    // ── Render ──────────────────────────────────────────────────────────────
    view! {
        <div class="bg-background flex flex-col h-full">
            // ── Page header ─────────────────────────────────────────────────
            <div class="page-header h-14 px-5 flex items-center justify-between shrink-0">
                <div class="flex items-center gap-2">
                    <span class="text-muted-foreground">
                        <Icon icon=phosphor_leptos::FOLDER weight=phosphor_leptos::IconWeight::Duotone size="18px"/>
                    </span>
                    <h1 class="text-sm font-semibold text-foreground">"Projects"</h1>
                </div>
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
                    let list = projects.get();

                    if list.is_empty() {
                        let empty_icon: Arc<dyn Fn() -> AnyView + Send + Sync> = Arc::new(move || {
                            view! {
                                <Icon icon=phosphor_leptos::FOLDER weight=phosphor_leptos::IconWeight::Duotone size="48px"/>
                            }.into_any()
                        });
                        view! {
                            <div class="p-4 md:p-6">
                                <EmptyState
                                    icon=empty_icon
                                    title="No projects yet"
                                    description="Projects are cross-team initiatives that group related issues together."
                                />
                            </div>
                        }.into_any()
                    } else {
                        let rows = list.iter().map(|project| {
                            view! { <ProjectRow project=project.clone()/> }
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
// Project Row
// ─────────────────────────────────────────────────────────────────────────────

/// A single project row in the list.
///
/// Layout: [folder icon] [name] [status badge] [dates]
///
/// Row height and padding match the issue row pattern for visual consistency.
#[component]
fn ProjectRow(project: trakkt_types::models::Project) -> impl IntoView {
    let href = format!("/projects/{}", project.project_id);
    let variant = status_variant(&project.status);
    let label = status_label(&project.status);

    let dates_view = {
        let start = project.start_date.as_deref().map(format_date);
        let target = project.target_date.as_deref().map(format_date);
        match (start, target) {
            (Some(s), Some(t)) => Some(format!("{s} \u{2192} {t}")),
            (Some(s), None) => Some(format!("Started {s}")),
            (None, Some(t)) => Some(format!("Target {t}")),
            (None, None) => None,
        }
    };

    view! {
        <a
            href=href
            class="h-9 px-3 py-[6px] flex items-center gap-2.5 border-b border-border hover:bg-surface-alt focus-visible:bg-surface-alt focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring transition-colors cursor-pointer no-underline text-inherit"
            role="listitem"
            tabindex="0"
        >
            // Folder icon
            <span class="text-muted-foreground shrink-0">
                <Icon icon=phosphor_leptos::FOLDER weight=phosphor_leptos::IconWeight::Duotone size="14px"/>
            </span>

            // Project name
            <span class="text-sm font-medium text-foreground flex-1 truncate">
                {project.name.clone()}
            </span>

            // Status badge
            <StatusBadge variant=variant>
                {label}
            </StatusBadge>

            // Dates
            {dates_view.map(|d| {
                view! {
                    <span class="font-mono text-xs text-muted-foreground shrink-0 hidden sm:inline">
                        {d}
                    </span>
                }
            })}
        </a>
    }
}
