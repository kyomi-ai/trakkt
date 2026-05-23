// SPDX-License-Identifier: AGPL-3.0-or-later

//! Archived Issues page — lists issues that have been auto-archived by the
//! server (completed/cancelled older than the archive threshold).
//!
//! Unlike the main issue list which reads from the SyncStore (client-side cache),
//! archived issues are NOT in the SyncStore. They are fetched from the server
//! via `get_archived_issues()`.
//!
//! Layout follows DESIGN.md patterns:
//! - Page header: back button + "Archived Issues" title
//! - Toolbar: search input for client-side filtering
//! - Content: issue rows with muted styling + "Unarchive" button per row
//!
//! Two entry points:
//! - `ArchivedIssuesPage` — workspace-scoped (all teams)
//! - `ArchivedIssuesForTeam` — team-scoped (reads `:key` from route params)

use std::sync::Arc;

use leptos::prelude::*;
use leptos_router::hooks::{use_location, use_navigate};
use leptos_router::location::State;
use leptos_router::NavigateOptions;
use phosphor_leptos::{Icon, IconWeight};

use crate::cache::store::SyncStore;
use crate::components::{
    Avatar, Button, ButtonSize, ButtonVariant, EmptyState, IssueStatusBadge, IssueStatusVariant,
    LabelBadge, PriorityIndicator, SearchInput,
};
use crate::server_fns::issues::{get_archived_issues, unarchive_issue};
use crate::types::IssueNavState;
use crate::utils::date::format_short_date;
use trakkt_types::models::IssueWithDetails;

/// Workspace-scoped archived issues page (all teams).
#[component]
pub fn ArchivedIssuesPage() -> impl IntoView {
    view! { <ArchivedIssuesInner/> }
}

/// Team-scoped archived issues page — reads `:key` from route params.
#[component]
pub fn ArchivedIssuesForTeam() -> impl IntoView {
    let params = leptos_router::hooks::use_params_map();
    let team_key = Signal::derive(move || params.read().get("key").unwrap_or_default());
    view! { <ArchivedIssuesInner team_key=team_key/> }
}

// ─────────────────────────────────────────────────────────────────────────────
// Shared inner component
// ─────────────────────────────────────────────────────────────────────────────

/// Resolve a team key to a team_id using the SyncStore.
fn resolve_team_id(store: Option<SyncStore>, team_key: &str) -> String {
    if team_key.is_empty() {
        return String::new();
    }
    store
        .and_then(|s| {
            s.teams()
                .get_untracked()
                .into_iter()
                .find(|t| t.key.eq_ignore_ascii_case(team_key))
                .map(|t| t.team_id)
        })
        .unwrap_or_default()
}

/// Page size for pagination.
const PAGE_SIZE: i64 = 50;

/// Shared implementation for both workspace and team-scoped archived pages.
#[component]
fn ArchivedIssuesInner(
    /// Optional reactive team key. When `Some`, filters to that team.
    #[prop(optional, into)]
    team_key: Option<Signal<String>>,
) -> impl IntoView {
    let (search, set_search) = signal(String::new());
    let (offset, set_offset) = signal(0i64);
    let (has_more, set_has_more) = signal(false);

    let store = use_context::<SyncStore>();

    // All fetched issues (accumulated across "Load more" clicks).
    let all_issues = RwSignal::new(Vec::<IssueWithDetails>::new());
    // Trigger signal to force re-fetch (e.g. after unarchive).
    let fetch_version = RwSignal::new(0u64);

    // Derive the team_id from SyncStore when team_key is provided.
    let team_id = Signal::derive(move || {
        match &team_key {
            Some(key_signal) => resolve_team_id(store, &key_signal.get()),
            None => String::new(),
        }
    });

    // Derive the page title.
    let page_title = Signal::derive(move || {
        match &team_key {
            Some(key_signal) => {
                let key = key_signal.get();
                if key.is_empty() {
                    "Archived Issues".to_string()
                } else {
                    format!("{} Archived Issues", key.to_uppercase())
                }
            }
            None => "Archived Issues".to_string(),
        }
    });

    // Back path depends on context.
    let back_path = match &team_key {
        Some(key_signal) => {
            let key_signal = *key_signal;
            Signal::derive(move || {
                let key = key_signal.get();
                if key.is_empty() {
                    "/my-issues".to_string()
                } else {
                    format!("/teams/{}/issues", key.to_lowercase())
                }
            })
        }
        None => Signal::derive(|| "/my-issues".to_string()),
    };

    // Fetch archived issues on mount and when offset/version changes.
    Effect::new(move |_| {
        let tid = team_id.get();
        let off = offset.get();
        let _ver = fetch_version.get();

        leptos::task::spawn_local(async move {
            match get_archived_issues(tid, Some(PAGE_SIZE), Some(off)).await {
                Ok(issues) => {
                    let fetched_count = issues.len() as i64;
                    if off == 0 {
                        all_issues.set(issues);
                    } else {
                        all_issues.update(|existing| existing.extend(issues));
                    }
                    set_has_more.set(fetched_count >= PAGE_SIZE);
                }
                Err(e) => {
                    tracing::warn!("Failed to fetch archived issues: {e}");
                    if off == 0 {
                        all_issues.set(Vec::new());
                    }
                    set_has_more.set(false);
                }
            }
        });
    });

    // Client-side search filtering on title and identifier.
    let filtered_issues = Signal::derive(move || {
        let issues = all_issues.get();
        let query = search.get().to_lowercase();
        if query.is_empty() {
            return issues;
        }
        issues
            .into_iter()
            .filter(|issue| {
                let identifier = format!("{}-{}", issue.team_key, issue.number).to_lowercase();
                issue.title.to_lowercase().contains(&query)
                    || identifier.contains(&query)
                    // Allow searching by just the number (e.g. "148")
                    || issue.number.to_string().contains(&query)
            })
            .collect()
    });

    let nav = use_navigate();

    view! {
        <div class="bg-background flex flex-col h-full">
            // ── Page header ─────────────────────────────────────────────────
            <div class="page-header h-14 px-5 flex items-center gap-3 shrink-0">
                <Button
                    variant=ButtonVariant::GhostMuted
                    size=ButtonSize::IconSm
                    aria_label="Go back"
                    on:click={
                        let nav = nav.clone();
                        move |_| {
                            let path = back_path.get_untracked();
                            nav(&path, Default::default());
                        }
                    }
                >
                    <Icon icon=phosphor_leptos::ARROW_LEFT size="20px"/>
                </Button>
                <h1 class="text-sm font-semibold text-foreground">{page_title}</h1>
            </div>

            // ── Toolbar ─────────────────────────────────────────────────────
            <div class="bg-background px-5 py-2 flex items-center gap-3 shrink-0">
                <SearchInput
                    value=Signal::derive(move || search.get())
                    on_input=Callback::new(move |v: String| set_search.set(v))
                    placeholder="Filter archived issues..."
                    class="flex-1 max-w-sm"
                />
            </div>

            // ── Content area ────────────────────────────────────────────────
            <div class="flex-1 overflow-y-auto">
                {move || {
                    let list = filtered_issues.get();

                    if list.is_empty() && all_issues.get().is_empty() {
                        let empty_icon: Arc<dyn Fn() -> AnyView + Send + Sync> = Arc::new(move || {
                            view! {
                                <Icon icon=phosphor_leptos::ARCHIVE weight=IconWeight::Duotone size="48px"/>
                            }.into_any()
                        });
                        return view! {
                            <div class="p-4 md:p-6">
                                <EmptyState
                                    icon=empty_icon
                                    title="No archived issues"
                                    description="Issues that have been completed or cancelled for more than 30 days will appear here"
                                />
                            </div>
                        }.into_any();
                    }

                    if list.is_empty() {
                        // Search returned no results, but issues exist.
                        let empty_icon: Arc<dyn Fn() -> AnyView + Send + Sync> = Arc::new(move || {
                            view! {
                                <Icon icon=phosphor_leptos::MAGNIFYING_GLASS weight=IconWeight::Duotone size="48px"/>
                            }.into_any()
                        });
                        return view! {
                            <div class="p-4 md:p-6">
                                <EmptyState
                                    icon=empty_icon
                                    title="No matching issues"
                                    description="Try adjusting your search query"
                                />
                            </div>
                        }.into_any();
                    }

                    view! {
                        <div role="list">
                            {list.iter().map(|issue| {
                                let issue = issue.clone();
                                let team_key_for_unarchive = issue.team_key.clone();
                                let number_for_unarchive = issue.number;
                                view! {
                                    <ArchivedIssueRow
                                        issue=issue
                                        on_unarchive=Callback::new(move |()| {
                                            let tk = team_key_for_unarchive.clone();
                                            let num = number_for_unarchive;
                                            leptos::task::spawn_local(async move {
                                                match unarchive_issue(tk, num).await {
                                                    Ok(()) => {
                                                        // Reset to first page and re-fetch.
                                                        set_offset.set(0);
                                                        fetch_version.update(|v| *v += 1);
                                                    }
                                                    Err(e) => {
                                                        tracing::warn!("Failed to unarchive issue: {e}");
                                                    }
                                                }
                                            });
                                        })
                                    />
                                }
                            }).collect_view()}
                        </div>

                        // ── Load more button ────────────────────────────────
                        <Show when=move || has_more.get()>
                            <div class="flex justify-center py-4">
                                <Button
                                    variant=ButtonVariant::Secondary
                                    size=ButtonSize::Sm
                                    on:click=move |_| {
                                        set_offset.update(|o| *o += PAGE_SIZE);
                                    }
                                >
                                    "Load more"
                                </Button>
                            </div>
                        </Show>
                    }.into_any()
                }}
            </div>
        </div>
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Archived Issue Row
// ─────────────────────────────────────────────────────────────────────────────

/// A single archived issue row.
///
/// Similar to `IssueRow` from `issue_row.rs` but simpler:
/// - Always rendered with muted opacity
/// - Includes an "Unarchive" button instead of keyboard navigation
/// - No drag/drop or selection support
#[component]
fn ArchivedIssueRow(
    issue: IssueWithDetails,
    on_unarchive: Callback<()>,
) -> impl IntoView {
    let issue_key = format!("{}-{}", issue.team_key, issue.number);
    let issue_href = format!("/issues/{issue_key}");
    let issue_href_click = issue_href.clone();
    let status = IssueStatusVariant::parse(&issue.status_category, &issue.status_name);
    let location = use_location();
    let nav = use_navigate();

    // Display the archived date (prefer archived_at, fall back to updated_at).
    let display_date = issue
        .archived_at
        .as_deref()
        .unwrap_or(&issue.updated_at)
        .to_string();

    view! {
        <div class="h-9 px-3 py-[6px] flex items-center gap-2.5 border-b border-border hover:bg-surface-alt transition-colors group">
            // Clickable issue content — navigates to issue detail
            <a
                href=issue_href
                class="flex-1 min-w-0 flex items-center gap-2.5 cursor-pointer no-underline text-inherit opacity-50 group-hover:opacity-75"
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

                // Issue ID (Geist Mono)
                <span class="font-mono text-xs text-muted-foreground shrink-0">
                    {issue_key}
                </span>

                // Title
                <span class="flex-1 min-w-0 text-sm font-medium text-foreground truncate">
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
                    {format_short_date(&display_date)}
                </span>

                // Assignee avatar (18px)
                {if issue.assignee_name.is_some() {
                    view! {
                        <Avatar name=issue.assignee_name.clone().unwrap_or_default()/>
                    }.into_any()
                } else {
                    view! {
                        <span class="w-[18px] h-[18px] shrink-0"></span>
                    }.into_any()
                }}
            </a>

            // Unarchive button — visible on hover
            <Button
                variant=ButtonVariant::Outline
                size=ButtonSize::Sm
                class="opacity-0 group-hover:opacity-100 shrink-0"
                on:click=move |ev: web_sys::MouseEvent| {
                    ev.stop_propagation();
                    on_unarchive.run(());
                }
            >
                "Unarchive"
            </Button>
        </div>
    }
}
