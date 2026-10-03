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

use crate::components::{Alert, AlertVariant, Button, ButtonSize, ButtonVariant, EmptyState, ProjectCreationModal, SearchInput, StatusBadge, ToggleButton};
use crate::server_fns::projects::list_projects;
use crate::utils::date::format_date;
use crate::utils::project::{status_label, status_variant};

// ─────────────────────────────────────────────────────────────────────────────
// Project List Page
// ─────────────────────────────────────────────────────────────────────────────

/// Main project list page — displays all projects in the workspace.
#[component]
pub fn ProjectListPage() -> impl IntoView {
    // ── Create-project modal ──────────────────────────────────────────────
    let (show_create, set_show_create) = signal(false);

    // ── Search state ─────────────────────────────────────────────────────
    let (search_text, set_search_text) = signal(String::new());

    // ── Show archived toggle ─────────────────────────────────────────────
    let (show_archived, set_show_archived) = signal(false);

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
                <Button
                    variant=ButtonVariant::Default
                    on:click=move |_| set_show_create.set(true)
                >
                    "New Project"
                </Button>
            </div>

            <ProjectCreationModal
                show=Signal::derive(move || show_create.get())
                on_close=Callback::new(move |()| set_show_create.set(false))
            />

            // ── Search bar ─────────────────────────────────────────────────
            <div class="px-5 py-2 border-b border-border shrink-0 flex items-center gap-3">
                <SearchInput
                    value=Signal::derive(move || search_text.get())
                    on_input=Callback::new(move |v: String| set_search_text.set(v))
                    placeholder="Search projects..."
                    class="max-w-sm".to_string()
                />
                <ToggleButton
                    variant=Signal::derive(move || {
                        if show_archived.get() {
                            ButtonVariant::Secondary
                        } else {
                            ButtonVariant::GhostMuted
                        }
                    })
                    size=ButtonSize::Sm
                    on:click=move |_| set_show_archived.update(|v| *v = !*v)
                >
                    <Icon icon=phosphor_leptos::ARCHIVE size="14px"/>
                    "Archived"
                </ToggleButton>
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
                    let all = projects.get();
                    let search = search_text.get().to_lowercase();
                    let include_archived = show_archived.get();
                    let list: Vec<_> = all.iter().filter(|p| {
                        // Filter out archived projects unless show_archived is enabled.
                        if !include_archived && p.archived_at.is_some() {
                            return false;
                        }
                        search.is_empty() || p.name.to_lowercase().contains(&search)
                    }).cloned().collect();

                    if list.is_empty() {
                        let (title, description) = if all.is_empty() {
                            (
                                "No projects yet",
                                "Projects are cross-team initiatives that group related issues together.",
                            )
                        } else {
                            (
                                "No matching projects",
                                "Try a different search term.",
                            )
                        };
                        let empty_icon: Arc<dyn Fn() -> AnyView + Send + Sync> = Arc::new(move || {
                            view! {
                                <Icon icon=phosphor_leptos::FOLDER weight=phosphor_leptos::IconWeight::Duotone size="48px"/>
                            }.into_any()
                        });
                        view! {
                            <div class="p-4 md:p-6">
                                <EmptyState
                                    icon=empty_icon
                                    title=title
                                    description=description
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
    let is_archived = project.archived_at.is_some();

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

    let row_class = if is_archived {
        "h-9 px-3 py-[6px] flex items-center gap-2.5 border-b border-border hover:bg-surface-alt focus-visible:bg-surface-alt focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring transition-colors cursor-pointer no-underline text-inherit opacity-60"
    } else {
        "h-9 px-3 py-[6px] flex items-center gap-2.5 border-b border-border hover:bg-surface-alt focus-visible:bg-surface-alt focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring transition-colors cursor-pointer no-underline text-inherit"
    };

    view! {
        <a
            href=href
            class=row_class
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

            // Archived badge
            {is_archived.then(|| view! {
                <span class="text-[10px] font-medium uppercase tracking-wide text-muted-foreground bg-muted px-1.5 py-0.5 rounded">
                    "Archived"
                </span>
            })}

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
