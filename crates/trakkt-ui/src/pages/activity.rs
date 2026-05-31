// SPDX-License-Identifier: AGPL-3.0-or-later

//! Activity feed — chronological workspace-wide activity stream.
//!
//! Shows all issue activities across all teams, grouped by time period
//! (Today, Yesterday, This Week, Older). Supports filtering by team
//! and activity type, with "Load more" pagination.

use leptos::prelude::*;
use phosphor_leptos::{Icon, IconWeight};

use crate::components::{Button, ButtonVariant, Select, SelectVariant, Spinner};
use crate::server_fns::activities::list_workspace_activities;
use crate::server_fns::teams::list_teams;
use crate::utils::relative_time::relative_time;
use crate::utils::time_group::{classify_time_group, TimeGroup};
use trakkt_types::models::WorkspaceActivity;

// ─── Activity helpers ────────────────────────────────────────────────────────

/// Map action_type to a phosphor icon view.
///
/// Matches the icons used in `issue_detail.rs` for visual consistency,
/// with the addition of `comment_added` (which issue detail filters out).
fn activity_icon(action_type: &str) -> AnyView {
    match action_type {
        "created" => view! { <Icon icon=phosphor_leptos::PLUS_CIRCLE size="16px" attr:class="text-muted-foreground"/> }.into_any(),
        "status_changed" => view! { <Icon icon=phosphor_leptos::CIRCLE_DASHED size="16px" attr:class="text-muted-foreground"/> }.into_any(),
        "comment_added" => view! { <Icon icon=phosphor_leptos::CHAT_TEXT size="16px" attr:class="text-muted-foreground"/> }.into_any(),
        "assignee_changed" => view! { <Icon icon=phosphor_leptos::USER size="16px" attr:class="text-muted-foreground"/> }.into_any(),
        "priority_changed" => view! { <Icon icon=phosphor_leptos::CELL_SIGNAL_FULL size="16px" attr:class="text-muted-foreground"/> }.into_any(),
        "label_added" | "label_removed" => view! { <Icon icon=phosphor_leptos::TAG size="16px" attr:class="text-muted-foreground"/> }.into_any(),
        "title_changed" | "description_changed" => view! { <Icon icon=phosphor_leptos::PENCIL_SIMPLE size="16px" attr:class="text-muted-foreground"/> }.into_any(),
        "relation_added" => view! { <Icon icon=phosphor_leptos::LINK size="16px" attr:class="text-muted-foreground"/> }.into_any(),
        "relation_removed" => view! { <Icon icon=phosphor_leptos::LINK_BREAK size="16px" attr:class="text-muted-foreground"/> }.into_any(),
        "project_changed" => view! { <Icon icon=phosphor_leptos::BRIEFCASE size="16px" attr:class="text-muted-foreground"/> }.into_any(),
        "milestone_changed" => view! { <Icon icon=phosphor_leptos::FLAG size="16px" attr:class="text-muted-foreground"/> }.into_any(),
        "due_date_changed" => view! { <Icon icon=phosphor_leptos::CALENDAR_BLANK size="16px" attr:class="text-muted-foreground"/> }.into_any(),
        "parent_changed" => view! { <Icon icon=phosphor_leptos::TREE_STRUCTURE size="16px" attr:class="text-muted-foreground"/> }.into_any(),
        "moved_to_team" => view! { <Icon icon=phosphor_leptos::ARROWS_LEFT_RIGHT size="16px" attr:class="text-muted-foreground"/> }.into_any(),
        "estimate_changed" => view! { <Icon icon=phosphor_leptos::GAUGE size="16px" attr:class="text-muted-foreground"/> }.into_any(),
        _ => view! { <Icon icon=phosphor_leptos::LIGHTNING size="16px" attr:class="text-muted-foreground"/> }.into_any(),
    }
}

/// Human-readable description for an activity entry.
fn activity_description(a: &WorkspaceActivity) -> String {
    let actor = a.actor_name.as_deref().unwrap_or("Someone");
    match a.action_type.as_str() {
        "created" => format!("{actor} created this issue"),
        "status_changed" => {
            match (a.old_value.as_deref(), a.new_value.as_deref()) {
                (Some(old), Some(new)) => format!("{actor} changed status from {old} to {new}"),
                (_, Some(new)) => format!("{actor} set status to {new}"),
                _ => format!("{actor} changed status"),
            }
        }
        "comment_added" => format!("{actor} commented"),
        "assignee_changed" => {
            match (a.old_value.as_deref(), a.new_value.as_deref()) {
                (_, Some(new)) => format!("{actor} assigned to {new}"),
                (Some(old), None) => format!("{actor} unassigned {old}"),
                _ => format!("{actor} changed assignee"),
            }
        }
        "priority_changed" => {
            match (a.old_value.as_deref(), a.new_value.as_deref()) {
                (Some(old), Some(new)) => format!("{actor} changed priority from {old} to {new}"),
                _ => format!("{actor} changed priority"),
            }
        }
        "label_added" => {
            match a.new_value.as_deref() {
                Some(label) => format!("{actor} added label {label}"),
                None => format!("{actor} added a label"),
            }
        }
        "label_removed" => {
            match a.old_value.as_deref() {
                Some(label) => format!("{actor} removed label {label}"),
                None => format!("{actor} removed a label"),
            }
        }
        "title_changed" => format!("{actor} changed the title"),
        "description_changed" => format!("{actor} updated the description"),
        "estimate_changed" => format!("{actor} changed the estimate"),
        "due_date_changed" => format!("{actor} changed the due date"),
        "project_changed" => format!("{actor} moved to a different project"),
        "milestone_changed" => format!("{actor} changed the milestone"),
        "parent_changed" => format!("{actor} changed the parent issue"),
        _ => format!("{actor} updated the issue"),
    }
}

const PAGE_SIZE: i64 = 50;

// ─── Grouped activities ──────────────────────────────────────────────────────

struct GroupedActivities {
    today: Vec<WorkspaceActivity>,
    yesterday: Vec<WorkspaceActivity>,
    this_week: Vec<WorkspaceActivity>,
    older: Vec<WorkspaceActivity>,
}

fn group_activities(activities: &[WorkspaceActivity]) -> GroupedActivities {
    let mut today = Vec::new();
    let mut yesterday = Vec::new();
    let mut this_week = Vec::new();
    let mut older = Vec::new();

    for a in activities {
        match classify_time_group(&a.created_at) {
            TimeGroup::Today => today.push(a.clone()),
            TimeGroup::Yesterday => yesterday.push(a.clone()),
            TimeGroup::ThisWeek => this_week.push(a.clone()),
            TimeGroup::Older => older.push(a.clone()),
        }
    }

    GroupedActivities {
        today,
        yesterday,
        this_week,
        older,
    }
}

// ─── Page component ──────────────────────────────────────────────────────────

#[component]
pub fn ActivityPage() -> impl IntoView {
    // Filter state
    let (team_filter, set_team_filter) = signal(String::new());
    let (type_filter, set_type_filter) = signal(String::new());

    // Pagination state
    let loaded_activities: RwSignal<Vec<WorkspaceActivity>> = RwSignal::new(Vec::new());
    let has_more = RwSignal::new(false);
    let loading = RwSignal::new(false);
    let initial_loaded = RwSignal::new(false);

    // Load teams for the team filter dropdown
    let teams_resource = Resource::new(|| (), |_| async move { list_teams().await });

    let team_options = Signal::derive(move || {
        let mut opts = vec![("".to_string(), "All teams".to_string())];
        if let Some(Ok(ref teams)) = teams_resource.get() {
            for team in teams {
                opts.push((team.key.clone(), team.name.clone()));
            }
        }
        opts
    });

    let type_options = Signal::derive(|| {
        vec![
            ("".to_string(), "All types".to_string()),
            ("status_changed".to_string(), "Status changes".to_string()),
            ("comment_added".to_string(), "Comments".to_string()),
            ("assignee_changed".to_string(), "Assignments".to_string()),
            ("priority_changed".to_string(), "Priority".to_string()),
            ("label_added".to_string(), "Labels".to_string()),
        ]
    });

    // Fetch function — loads a page of activities and updates state
    let fetch_activities = move |reset: bool| {
        if loading.get_untracked() {
            return;
        }
        loading.set(true);

        let offset = if reset {
            0
        } else {
            loaded_activities.get_untracked().len() as i64
        };

        let tk = team_filter.get_untracked();
        let team_key = if tk.is_empty() { None } else { Some(tk) };

        let action_type = {
            let tf = type_filter.get_untracked();
            if tf.is_empty() { None } else { Some(tf) }
        };

        leptos::task::spawn_local(async move {
            match list_workspace_activities(team_key, action_type, Some(PAGE_SIZE), Some(offset)).await {
                Ok(activities) => {
                    let count = activities.len() as i64;
                    if reset {
                        loaded_activities.set(activities);
                    } else {
                        loaded_activities.update(|list| list.extend(activities));
                    }
                    has_more.set(count == PAGE_SIZE);
                    initial_loaded.set(true);
                }
                Err(e) => {
                    tracing::warn!("Failed to load activities: {e}");
                    initial_loaded.set(true);
                }
            }
            loading.set(false);
        });
    };

    // Initial load
    let fetch_initial = fetch_activities;
    Effect::new(move |_| {
        fetch_initial(true);
    });

    // Re-fetch on filter change (track both signals)
    let fetch_on_filter = fetch_activities;
    Effect::new(move |prev: Option<(String, String)>| {
        let tk = team_filter.get();
        let tf = type_filter.get();
        let current = (tk.clone(), tf.clone());

        // Skip the first fire — the initial load Effect handles that
        if prev.is_some_and(|prev_val| prev_val != current) {
            fetch_on_filter(true);
        }

        current
    });

    let fetch_more = fetch_activities;

    view! {
        <div class="h-full flex flex-col">
            // Page header
            <div class="page-header h-14 px-5 flex items-center justify-between shrink-0">
                <h1 class="text-sm font-semibold text-foreground">"Activity"</h1>
            </div>

            // Filter bar
            <div class="flex items-center gap-2 px-5 py-3 border-b border-border">
                <Select
                    value=Signal::derive(move || team_filter.get())
                    options=team_options
                    on_change=Callback::new(move |v: String| set_team_filter.set(v))
                    variant=SelectVariant::Compact
                />
                <Select
                    value=Signal::derive(move || type_filter.get())
                    options=type_options
                    on_change=Callback::new(move |v: String| set_type_filter.set(v))
                    variant=SelectVariant::Compact
                />
            </div>

            // Content area
            <div class="flex-1 overflow-y-auto">
                {move || {
                    if !initial_loaded.get() {
                        // Initial loading state
                        return view! {
                            <div class="flex items-center justify-center py-12">
                                <Spinner/>
                            </div>
                        }.into_any();
                    }

                    let activities = loaded_activities.get();

                    if activities.is_empty() {
                        // Empty state
                        return view! {
                            <div class="flex flex-col items-center justify-center py-16 text-muted-foreground">
                                <Icon icon=phosphor_leptos::CLOCK_COUNTER_CLOCKWISE weight=IconWeight::Light size="48px" attr:class="mb-4 text-muted-foreground/50"/>
                                <p class="text-lg font-medium">"No activity yet"</p>
                            </div>
                        }.into_any();
                    }

                    let grouped = group_activities(&activities);
                    let groups: Vec<(TimeGroup, Vec<WorkspaceActivity>)> = vec![
                        (TimeGroup::Today, grouped.today),
                        (TimeGroup::Yesterday, grouped.yesterday),
                        (TimeGroup::ThisWeek, grouped.this_week),
                        (TimeGroup::Older, grouped.older),
                    ];

                    let show_load_more = has_more.get();
                    let is_loading = loading.get();

                    view! {
                        <div class="divide-y divide-border">
                            {groups.into_iter().filter(|(_, items)| !items.is_empty()).map(|(group, items)| {
                                view! {
                                    <div>
                                        <div class="px-6 py-2">
                                            <span class="text-xs text-muted-foreground font-medium uppercase tracking-wide">
                                                {group.label()}
                                            </span>
                                        </div>
                                        {items.into_iter().map(|activity| {
                                            view! { <ActivityRow activity=activity/> }
                                        }).collect_view()}
                                    </div>
                                }
                            }).collect_view()}
                        </div>

                        // Load more button
                        {show_load_more.then(|| {
                            let fetch = fetch_more;
                            view! {
                                <div class="flex justify-center py-4">
                                    <Button
                                        variant=ButtonVariant::GhostMuted
                                        on:click=move |_| fetch(false)
                                    >
                                        {if is_loading { "Loading..." } else { "Load more" }}
                                    </Button>
                                </div>
                            }
                        })}
                    }.into_any()
                }}
            </div>
        </div>
    }
}

// ─── Activity row ────────────────────────────────────────────────────────────

#[component]
fn ActivityRow(activity: WorkspaceActivity) -> impl IntoView {
    let icon = activity_icon(&activity.action_type);
    let description = activity_description(&activity);
    let via_suffix = crate::components::attribution::render_via_suffix(
        activity.action_source,
        activity.action_source_label.clone(),
    );
    let identifier = format!("{}-{}", activity.team_key, activity.issue_number);
    let href = format!("/issues/{}-{}", activity.team_key, activity.issue_number);
    let issue_title = activity.issue_title.clone();
    let timestamp = relative_time(&activity.created_at);

    view! {
        <a href=href class="flex items-start gap-3 px-6 py-2.5 hover:bg-accent transition-colors">
            <div class="flex-shrink-0 pt-0.5">
                {icon}
            </div>
            <div class="flex-1 min-w-0">
                <div class="flex items-baseline gap-2">
                    <span class="text-sm text-foreground">{description}{via_suffix}</span>
                    <span class="text-xs text-muted-foreground font-mono">{identifier}</span>
                </div>
                <p class="text-sm text-muted-foreground truncate mt-0.5">{issue_title}</p>
            </div>
            <span class="flex-shrink-0 text-xs text-muted-foreground pt-0.5">
                {timestamp}
            </span>
        </a>
    }
}
