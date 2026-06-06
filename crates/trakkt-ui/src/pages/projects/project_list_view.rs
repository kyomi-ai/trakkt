// SPDX-License-Identifier: AGPL-3.0-or-later

//! Project-scoped list view — a sortable, groupable table of all issues
//! belonging to a project.
//!
//! Simpler than `IssueListInner`: no saved views, no custom filters, no
//! keyboard navigation. Focuses on displaying issue rows with grouping
//! and sorting controls.

use leptos::prelude::*;
use leptos_router::hooks::{use_location, use_navigate};
use leptos_router::location::State;
use leptos_router::NavigateOptions;
use phosphor_leptos::Icon;

use crate::components::{
    Avatar, DropdownItem, DropdownMenu, DropdownTrigger, EmptyState, IssueStatusBadge,
    IssueStatusVariant, LabelBadge, PriorityIndicator, SearchInput, TeamKeyBadge,
};
use crate::pages::issues::filters::{
    sort_issues, LabelFilterDropdown, PriorityFilterDropdown, SortDirection, SortDropdown,
    SortField,
};
use crate::types::IssueNavState;
use crate::utils::date::format_short_date;
use trakkt_types::models::IssueWithDetails;

// ─────────────────────────────────────────────────────────────────────────────
// Group-by options
// ─────────────────────────────────────────────────────────────────────────────

/// Grouping options for the project list view.
#[derive(Clone, Copy, PartialEq, Eq)]
enum GroupBy {
    None,
    StatusCategory,
    Assignee,
    Priority,
    Label,
    Team,
}

impl GroupBy {
    fn label(self) -> &'static str {
        match self {
            Self::None => "No grouping",
            Self::StatusCategory => "Status",
            Self::Assignee => "Assignee",
            Self::Priority => "Priority",
            Self::Label => "Label",
            Self::Team => "Team",
        }
    }

    const ALL: [GroupBy; 6] = [
        Self::None,
        Self::StatusCategory,
        Self::Assignee,
        Self::Priority,
        Self::Label,
        Self::Team,
    ];
}

/// Extract a group key from an issue for the given grouping.
fn group_key(issue: &IssueWithDetails, group_by: GroupBy) -> String {
    match group_by {
        GroupBy::None => String::new(),
        GroupBy::StatusCategory => issue.status_category.clone(),
        GroupBy::Assignee => issue
            .assignee_name
            .clone()
            .unwrap_or_else(|| "Unassigned".to_string()),
        GroupBy::Priority => match issue.priority {
            1 => "Urgent".to_string(),
            2 => "High".to_string(),
            3 => "Medium".to_string(),
            4 => "Low".to_string(),
            _ => "No priority".to_string(),
        },
        GroupBy::Label => {
            if issue.labels.is_empty() {
                "No label".to_string()
            } else {
                // Group by first label (issues with multiple labels appear
                // once, under their first label).
                issue.labels[0].name.clone()
            }
        }
        GroupBy::Team => issue.team_key.clone(),
    }
}

/// Pretty display label for a group key.
fn group_display_label(key: &str, group_by: GroupBy) -> String {
    match group_by {
        GroupBy::StatusCategory => match key {
            "backlog" => "Backlog".to_string(),
            "unstarted" => "Todo".to_string(),
            "started" => "In Progress".to_string(),
            "completed" => "Done".to_string(),
            "cancelled" => "Cancelled".to_string(),
            other => other.to_string(),
        },
        _ => key.to_string(),
    }
}

/// Ordering weight for status categories (so groups render in logical order).
fn status_category_order(cat: &str) -> u8 {
    match cat {
        "backlog" => 0,
        "unstarted" => 1,
        "started" => 2,
        "completed" => 3,
        "cancelled" => 4,
        _ => 5,
    }
}

/// Ordering weight for priority groups.
fn priority_order(label: &str) -> u8 {
    match label {
        "Urgent" => 0,
        "High" => 1,
        "Medium" => 2,
        "Low" => 3,
        _ => 4,
    }
}

/// Build grouped issue lists from a flat list.
///
/// Returns `(group_label, issues)` pairs in a logical order for the group type.
fn build_groups(
    issues: Vec<IssueWithDetails>,
    group_by: GroupBy,
) -> Vec<(String, Vec<IssueWithDetails>)> {
    if group_by == GroupBy::None {
        return vec![(String::new(), issues)];
    }

    // Collect into an ordered map preserving insertion order.
    let mut groups: Vec<(String, Vec<IssueWithDetails>)> = Vec::new();
    for issue in issues {
        let key = group_key(&issue, group_by);
        if let Some(existing) = groups.iter_mut().find(|(k, _)| *k == key) {
            existing.1.push(issue);
        } else {
            groups.push((key, vec![issue]));
        }
    }

    // Sort groups by a sensible order for the group type.
    groups.sort_by(|(a, _), (b, _)| match group_by {
        GroupBy::StatusCategory => {
            status_category_order(a).cmp(&status_category_order(b))
        }
        GroupBy::Priority => priority_order(a).cmp(&priority_order(b)),
        _ => a.to_lowercase().cmp(&b.to_lowercase()),
    });

    groups
}

// ─────────────────────────────────────────────────────────────────────────────
// Project List View
// ─────────────────────────────────────────────────────────────────────────────

/// Project-scoped sortable/groupable list view.
#[component]
pub fn ProjectListView(
    /// The project ID to filter issues by.
    project_id: Signal<String>,
) -> impl IntoView {
    let sync_store = use_context::<crate::cache::store::SyncStore>();

    // ── Data: issues for this project ────────────────────────────────────
    let project_issues = Memo::new(move |_| {
        let pid = project_id.get();
        if pid.is_empty() {
            return Vec::new();
        }
        let Some(store) = sync_store else {
            return Vec::new();
        };
        store
            .issues()
            .get()
            .into_iter()
            .filter(|issue| issue.project_id.as_deref() == Some(&pid))
            .collect::<Vec<_>>()
    });

    // ── Filter state ─────────────────────────────────────────────────────
    let (search, set_search) = signal(String::new());
    let (priority_filter, set_priority_filter) = signal(Vec::<String>::new());
    let (label_filter, set_label_filter) = signal(Vec::<String>::new());

    // ── Sort state ───────────────────────────────────────────────────────
    let (sort_field, set_sort_field) = signal(SortField::Priority);
    let (sort_direction, set_sort_direction) = signal(SortDirection::Asc);

    // ── Group-by state ───────────────────────────────────────────────────
    let (group_by, set_group_by) = signal(GroupBy::None);

    // ── Computed: filtered + sorted issues ───────────────────────────────
    let display_issues = Memo::new(move |_| {
        let raw = project_issues.get();
        let search_val = search.get().to_lowercase();
        let priority_val = priority_filter.get();
        let label_val = label_filter.get();

        let mut filtered: Vec<IssueWithDetails> = raw
            .into_iter()
            .filter(|issue| {
                if !priority_val.is_empty() {
                    let p_str = issue.priority.to_string();
                    if !priority_val.contains(&p_str) {
                        return false;
                    }
                }
                if !label_val.is_empty() {
                    let has_match = issue.labels.iter().any(|l| label_val.contains(&l.label_id));
                    if !has_match {
                        return false;
                    }
                }
                if !search_val.is_empty() && !issue.title.to_lowercase().contains(&search_val) {
                    return false;
                }
                true
            })
            .collect();

        sort_issues(&mut filtered, sort_field.get(), sort_direction.get());
        filtered
    });

    // ── Render ──────────────────────────────────────────────────────────
    view! {
        <div class="flex flex-col h-full">
            // ── Toolbar ─────────────────────────────────────────────────
            <div class="bg-background px-5 py-2 flex items-center gap-3 shrink-0 flex-wrap">
                <SearchInput
                    value=Signal::derive(move || search.get())
                    on_input=Callback::new(move |v: String| set_search.set(v))
                    placeholder="Filter issues..."
                    class="flex-1 max-w-sm"
                />
                <PriorityFilterDropdown
                    value=priority_filter
                    on_change=Callback::new(move |v: Vec<String>| set_priority_filter.set(v))
                />
                <LabelFilterDropdown
                    value=label_filter
                    on_change=Callback::new(move |v: Vec<String>| set_label_filter.set(v))
                />
                <SortDropdown
                    field=sort_field
                    direction=sort_direction
                    on_change=Callback::new(move |(f, d): (SortField, SortDirection)| {
                        set_sort_field.set(f);
                        set_sort_direction.set(d);
                    })
                />
                <GroupByDropdown
                    value=group_by
                    on_change=Callback::new(move |v: GroupBy| set_group_by.set(v))
                />
            </div>

            // ── Content area ────────────────────────────────────────────
            <div class="flex-1 overflow-y-auto">
                {move || {
                    let issues = display_issues.get();
                    let current_group_by = group_by.get();

                    if issues.is_empty() {
                        let empty_icon: std::sync::Arc<dyn Fn() -> leptos::prelude::AnyView + Send + Sync> = std::sync::Arc::new(move || {
                            view! {
                                <Icon icon=phosphor_leptos::CLIPBOARD_TEXT weight=phosphor_leptos::IconWeight::Duotone size="48px"/>
                            }.into_any()
                        });
                        return view! {
                            <EmptyState
                                icon=empty_icon
                                title="No matching issues"
                                description="Try adjusting your filters."
                            />
                        }.into_any();
                    }

                    let groups = build_groups(issues, current_group_by);

                    if current_group_by == GroupBy::None {
                        // Flat list — no group headers.
                        let rows = groups
                            .into_iter()
                            .flat_map(|(_, issues)| issues)
                            .map(|issue| {
                                view! { <ProjectListRow issue=issue/> }
                            })
                            .collect_view();
                        view! {
                            <div role="list">
                                {rows}
                            </div>
                        }.into_any()
                    } else {
                        // Grouped with headers.
                        let group_views = groups
                            .into_iter()
                            .map(|(key, group_issues)| {
                                let label = group_display_label(&key, current_group_by);
                                let count = group_issues.len();
                                let rows = group_issues
                                    .into_iter()
                                    .map(|issue| {
                                        view! { <ProjectListRow issue=issue/> }
                                    })
                                    .collect_view();

                                view! {
                                    <div class="mb-2">
                                        <div class="flex items-center justify-between px-3 py-2 border-b border-border bg-surface-alt/50">
                                            <span class="text-sm font-medium text-foreground">{label}</span>
                                            <span class="text-xs text-muted-foreground">{count}</span>
                                        </div>
                                        <div role="list">
                                            {rows}
                                        </div>
                                    </div>
                                }
                            })
                            .collect_view();

                        view! {
                            <div>
                                {group_views}
                            </div>
                        }.into_any()
                    }
                }}
            </div>
        </div>
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Group-by Dropdown
// ─────────────────────────────────────────────────────────────────────────────

/// Dropdown for selecting the group-by dimension.
#[component]
fn GroupByDropdown(
    #[prop(into)] value: Signal<GroupBy>,
    on_change: Callback<GroupBy>,
) -> impl IntoView {
    let (open, set_open) = signal(false);
    let trigger_ref = NodeRef::<leptos::html::Div>::new();

    let display = Memo::new(move |_| {
        let v = value.get();
        if v == GroupBy::None {
            None
        } else {
            Some(v.label().to_string())
        }
    });

    view! {
        <div node_ref=trigger_ref>
            <DropdownTrigger
                label="Group"
                value=Signal::derive(move || display.get())
                on_click=Callback::new(move |()| set_open.update(|o| *o = !*o))
            />
        </div>
        <DropdownMenu
            trigger_ref=trigger_ref
            open=Signal::derive(move || open.get())
            on_close=Callback::new(move |()| set_open.set(false))
        >
            {GroupBy::ALL.iter().map(|gb| {
                let g = *gb;
                let label = g.label().to_string();
                view! {
                    <DropdownItem
                        label=label
                        selected=Signal::derive(move || value.get() == g)
                        on_select=Callback::new(move |()| {
                            on_change.run(g);
                            set_open.set(false);
                        })
                    />
                }
            }).collect_view()}
        </DropdownMenu>
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Project List Row
// ─────────────────────────────────────────────────────────────────────────────

/// A single issue row in the project list view.
///
/// Follows DESIGN.md Issue Row Pattern:
/// `px-3 py-[6px] h-9 flex items-center gap-2.5 border-b border-border`
///
/// Order: Priority | Status | Team badge + number | Title | Labels | Date | Assignee
#[component]
fn ProjectListRow(issue: IssueWithDetails) -> impl IntoView {
    let issue_href = format!("/issues/{}-{}", issue.team_key, issue.number);
    let issue_href_click = issue_href.clone();
    let status = IssueStatusVariant::parse(&issue.status_category, &issue.status_name);
    let location = use_location();

    // Look up team color from SyncStore.
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

            // Team badge + issue number
            <TeamKeyBadge team_key=issue.team_key.clone() color=team_color/>
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

            // Date
            <span class="font-mono text-xs text-muted-foreground shrink-0 hidden sm:inline">
                {format_short_date(&issue.created_at)}
            </span>

            // Assignee
            {if let Some(ref name) = issue.assignee_name {
                view! {
                    <Avatar name=name.clone()/>
                }.into_any()
            } else {
                view! {
                    <span class="w-5 h-5 shrink-0"></span>
                }.into_any()
            }}
        </a>
    }
}
