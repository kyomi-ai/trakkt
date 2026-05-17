// SPDX-License-Identifier: AGPL-3.0-or-later

//! Shared filter dropdown components for issue pages.
//!
//! Both `issue_list.rs` and `my_issues.rs` use the same status and priority
//! filter dropdowns. This module provides the canonical implementations to
//! eliminate duplication per CODING_STANDARDS.md ("No duplicated helper
//! functions across modules").
//!
//! ## Composable Filter System (TRA-103)
//!
//! The composable filter system uses `FilterClause` triples (field, operator,
//! values) and renders them as chips in a `FilterBar`. Field definitions are
//! declared via `FilterFieldDef` structs; day-1 supports status and priority.

use std::sync::Arc;

use leptos::children::ChildrenFn;
use leptos::prelude::*;
use phosphor_leptos::Icon;

use crate::components::{
    Button, ButtonSize, ButtonVariant, Checkbox,
    DropdownItem, DropdownMenu, DropdownTrigger, IssueStatusBadge, IssueStatusVariant,
    PriorityIndicator,
};
use crate::pages::views::FilterClause;
use crate::server_fns::labels::list_labels;
use crate::server_fns::projects::list_projects;
use crate::server_fns::statuses::list_statuses;
use trakkt_types::models::IssueWithDetails;

// ─────────────────────────────────────────────────────────────────────────────
// Sort enums and helper
// ─────────────────────────────────────────────────────────────────────────────

/// Field to sort the issue list by.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortField {
    Priority,
    Status,
    CreatedDate,
    UpdatedDate,
    Assignee,
    DueDate,
}

impl SortField {
    /// Human-readable label for the dropdown.
    pub fn label(self) -> &'static str {
        match self {
            Self::Priority => "Priority",
            Self::Status => "Status",
            Self::CreatedDate => "Created date",
            Self::UpdatedDate => "Updated date",
            Self::Assignee => "Assignee",
            Self::DueDate => "Due date",
        }
    }

    /// The natural default direction for this sort field.
    pub fn default_direction(self) -> SortDirection {
        match self {
            Self::Priority => SortDirection::Asc,
            Self::Status => SortDirection::Asc,
            Self::CreatedDate => SortDirection::Desc,
            Self::UpdatedDate => SortDirection::Desc,
            Self::Assignee => SortDirection::Asc,
            Self::DueDate => SortDirection::Asc,
        }
    }

    /// All variants in display order.
    pub const ALL: [SortField; 6] = [
        Self::Priority,
        Self::Status,
        Self::CreatedDate,
        Self::UpdatedDate,
        Self::Assignee,
        Self::DueDate,
    ];
}

/// Sort direction — ascending or descending.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortDirection {
    Asc,
    Desc,
}

impl SortDirection {
    /// Toggle between ascending and descending.
    pub fn toggle(self) -> Self {
        match self {
            Self::Asc => Self::Desc,
            Self::Desc => Self::Asc,
        }
    }

    /// Arrow character for UI display.
    pub fn arrow(self) -> &'static str {
        match self {
            Self::Asc => "\u{2191}",  // ↑
            Self::Desc => "\u{2193}", // ↓
        }
    }

    /// Serialize to string for view persistence.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Asc => "asc",
            Self::Desc => "desc",
        }
    }

    /// Parse from string. Returns `None` for unrecognized input.
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "asc" => Some(Self::Asc),
            "desc" => Some(Self::Desc),
            _ => None,
        }
    }
}

/// Sort a slice of issues in-place by the given field and direction.
///
/// Shared by `issue_list.rs` and `my_issues.rs` to avoid duplicating sort
/// logic (per CODING_STANDARDS.md "No duplicated helper functions across
/// modules").
pub fn sort_issues(issues: &mut [IssueWithDetails], field: SortField, direction: SortDirection) {
    if field == SortField::Assignee {
        issues.sort_by_cached_key(|i| {
            i.assignee_name.as_deref().unwrap_or("\u{ffff}").to_lowercase()
        });
        if direction == SortDirection::Desc {
            issues.reverse();
        }
        return;
    }

    issues.sort_by(|a, b| {
        let cmp = match field {
            SortField::Priority => {
                let pa = if a.priority == 0 { 99 } else { a.priority };
                let pb = if b.priority == 0 { 99 } else { b.priority };
                pa.cmp(&pb)
            }
            SortField::Status => {
                let cat_order = |cat: &str| match cat {
                    "backlog" => 0,
                    "unstarted" => 1,
                    "started" => 2,
                    "completed" => 3,
                    "cancelled" => 4,
                    _ => 5,
                };
                cat_order(&a.status_category).cmp(&cat_order(&b.status_category))
            }
            SortField::CreatedDate => a.created_at.cmp(&b.created_at),
            SortField::UpdatedDate => a.updated_at.cmp(&b.updated_at),
            SortField::Assignee => unreachable!(),
            SortField::DueDate => {
                let ad = a.due_date.as_deref().unwrap_or("\u{ffff}");
                let bd = b.due_date.as_deref().unwrap_or("\u{ffff}");
                ad.cmp(bd)
            }
        };
        match direction {
            SortDirection::Asc => cmp,
            SortDirection::Desc => cmp.reverse(),
        }
    });
}

/// Parse a sort field name back to the enum. Returns `None` for unknown input.
pub fn parse_sort_field(s: &str) -> Option<SortField> {
    match s {
        "priority" => Some(SortField::Priority),
        "status" => Some(SortField::Status),
        "created_date" => Some(SortField::CreatedDate),
        "updated_date" => Some(SortField::UpdatedDate),
        "assignee" => Some(SortField::Assignee),
        "due_date" => Some(SortField::DueDate),
        _ => None,
    }
}

/// Serialize a sort field to a stable string for view persistence.
pub fn sort_field_to_str(field: SortField) -> &'static str {
    match field {
        SortField::Priority => "priority",
        SortField::Status => "status",
        SortField::CreatedDate => "created_date",
        SortField::UpdatedDate => "updated_date",
        SortField::Assignee => "assignee",
        SortField::DueDate => "due_date",
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Status Filter Dropdown
// ─────────────────────────────────────────────────────────────────────────────

/// Dropdown filter for issue statuses.
///
/// Reads statuses from SyncStore (real-time) with a server function fallback.
/// When `team_id` is provided, filters to show only global statuses (team_id = None)
/// and statuses belonging to that team.
#[component]
pub fn StatusFilterDropdown(
    #[prop(into)] value: Signal<Vec<String>>,
    on_change: Callback<Vec<String>>,
    /// When filtering by team, only show statuses that are global (team_id = None)
    /// or belong to this team.
    #[prop(optional, into)]
    team_id: Option<Signal<Option<String>>>,
) -> impl IntoView {
    let (open, set_open) = signal(false);
    let trigger_ref = NodeRef::<leptos::html::Div>::new();

    // Use SyncStore for statuses when available (real-time), fall back to server.
    let sync_store = use_context::<crate::cache::store::SyncStore>();

    // Fetch statuses dynamically from the server.
    let statuses_resource = Resource::new(
        || (),
        move |_| async move { list_statuses(None).await },
    );

    // Resolved statuses — prefer SyncStore, fall back to server resource.
    // When team_id is provided, filter to global + team-specific statuses.
    let statuses = Memo::new(move |_| {
        let all = if let Some(store) = sync_store {
            let s = store.statuses().get();
            if !s.is_empty() || store.initialized().get() {
                s
            } else {
                match statuses_resource.get() {
                    Some(Ok(items)) => items,
                    Some(Err(e)) => {
                        tracing::warn!("Failed to load statuses for filter dropdown: {e}");
                        Vec::new()
                    }
                    None => Vec::new(),
                }
            }
        } else {
            match statuses_resource.get() {
                Some(Ok(items)) => items,
                Some(Err(e)) => {
                    tracing::warn!("Failed to load statuses for filter dropdown: {e}");
                    Vec::new()
                }
                None => Vec::new(),
            }
        };

        // Filter by team when a team_id signal is provided and has a value.
        if let Some(team_id_signal) = team_id
            && let Some(ref tid) = team_id_signal.get()
        {
            return all
                .into_iter()
                .filter(|s| {
                    s.team_id.is_none() || s.team_id.as_deref() == Some(tid.as_str())
                })
                .collect();
        }
        all
    });

    // Display name for the current selection (multi-select).
    // 0 selected → None (shows "All statuses" default label)
    // 1 selected → look up name from loaded statuses
    // 2+ selected → "Status (N)"
    let display = Memo::new(move |_| {
        let v = value.get();
        match v.len() {
            0 => None,
            1 => {
                let id = &v[0];
                statuses
                    .get()
                    .iter()
                    .find(|s| &s.status_id == id)
                    .map(|s| s.name.clone())
            }
            n => Some(format!("Status ({n})")),
        }
    });

    // Icon variant — only show when exactly 1 status is selected.
    let current_variant = Memo::new(move |_| {
        let v = value.get();
        if v.len() == 1 {
            statuses
                .get()
                .iter()
                .find(|s| s.status_id == v[0])
                .map(|s| IssueStatusVariant::parse(&s.category))
        } else {
            None
        }
    });

    view! {
        <div node_ref=trigger_ref>
            <DropdownTrigger
                label="All statuses"
                value=Signal::derive(move || display.get())
                icon=Arc::new(move || {
                    current_variant.get().map(|v| {
                        view! { <IssueStatusBadge status=v size=12/> }.into_any()
                    }).unwrap_or_else(|| view! { <span/> }.into_any())
                }) as ChildrenFn
                on_click=Callback::new(move |()| set_open.update(|o| *o = !*o))
            />
        </div>
        <DropdownMenu
            trigger_ref=trigger_ref
            open=Signal::derive(move || open.get())
            on_close=Callback::new(move |()| set_open.set(false))
        >
            <DropdownItem
                label="All statuses"
                selected=Signal::derive(move || value.get().is_empty())
                on_select=Callback::new(move |()| { on_change.run(Vec::new()); })
            />
            {move || statuses.get().into_iter().map(|status| {
                let status_id = status.status_id.clone();
                let status_id_check = status.status_id.clone();
                let label = status.name.clone();
                let variant = IssueStatusVariant::parse(&status.category);
                view! {
                    <DropdownItem
                        label=label
                        selected=Signal::derive(move || value.get().contains(&status_id_check))
                        on_select=Callback::new({
                            let id = status_id.clone();
                            move |()| {
                                let mut current = value.get_untracked();
                                if let Some(pos) = current.iter().position(|s| s == &id) {
                                    current.remove(pos);
                                } else {
                                    current.push(id.clone());
                                }
                                on_change.run(current);
                            }
                        })
                        icon=Arc::new(move || view! { <IssueStatusBadge status=variant size=14/> }.into_any()) as ChildrenFn
                    />
                }
            }).collect_view()}
        </DropdownMenu>
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Priority Filter Dropdown
// ─────────────────────────────────────────────────────────────────────────────

/// Dropdown filter for issue priorities.
///
/// Provides a fixed list of priority levels (Urgent, High, Medium, Low)
/// with their corresponding icons.
#[component]
pub fn PriorityFilterDropdown(
    #[prop(into)] value: Signal<Vec<String>>,
    on_change: Callback<Vec<String>>,
) -> impl IntoView {
    let (open, set_open) = signal(false);
    let trigger_ref = NodeRef::<leptos::html::Div>::new();

    let priorities: Vec<(&str, &str, i32)> = vec![
        ("1", "Urgent", 1),
        ("2", "High", 2),
        ("3", "Medium", 3),
        ("4", "Low", 4),
    ];

    // Display name for the current selection (multi-select).
    // 0 selected → None (shows "All priorities" default label)
    // 1 selected → look up name ("Urgent", "High", etc.)
    // 2+ selected → "Priority (N)"
    let display = Memo::new(move |_| {
        let v = value.get();
        match v.len() {
            0 => None,
            1 => match v[0].as_str() {
                "1" => Some("Urgent".to_string()),
                "2" => Some("High".to_string()),
                "3" => Some("Medium".to_string()),
                "4" => Some("Low".to_string()),
                _ => None,
            },
            n => Some(format!("Priority ({n})")),
        }
    });

    // Icon — only show when exactly 1 priority is selected.
    let current_priority = Memo::new(move |_| {
        let v = value.get();
        if v.len() == 1 {
            v[0].parse::<i32>().ok()
        } else {
            None
        }
    });

    view! {
        <div node_ref=trigger_ref>
            <DropdownTrigger
                label="All priorities"
                value=Signal::derive(move || display.get())
                icon=Arc::new(move || {
                    current_priority.get().map(|p| {
                        view! { <PriorityIndicator priority=p/> }.into_any()
                    }).unwrap_or_else(|| view! { <span/> }.into_any())
                }) as ChildrenFn
                on_click=Callback::new(move |()| set_open.update(|o| *o = !*o))
            />
        </div>
        <DropdownMenu
            trigger_ref=trigger_ref
            open=Signal::derive(move || open.get())
            on_close=Callback::new(move |()| set_open.set(false))
        >
            <DropdownItem
                label="All priorities"
                selected=Signal::derive(move || value.get().is_empty())
                on_select=Callback::new(move |()| { on_change.run(Vec::new()); })
            />
            {priorities.iter().map(|(key, label, priority_val)| {
                let key_owned = key.to_string();
                let key_check = key.to_string();
                let label = label.to_string();
                let shortcut = key.to_string();
                let priority_val = *priority_val;
                view! {
                    <DropdownItem
                        label=label
                        selected=Signal::derive(move || value.get().contains(&key_check))
                        on_select=Callback::new({
                            let k = key_owned.clone();
                            move |()| {
                                let mut current = value.get_untracked();
                                if let Some(pos) = current.iter().position(|s| s == &k) {
                                    current.remove(pos);
                                } else {
                                    current.push(k.clone());
                                }
                                on_change.run(current);
                            }
                        })
                        icon=Arc::new(move || view! { <PriorityIndicator priority=priority_val/> }.into_any()) as ChildrenFn
                        shortcut=shortcut
                    />
                }
            }).collect_view()}
        </DropdownMenu>
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Sort Dropdown
// ─────────────────────────────────────────────────────────────────────────────

/// Dropdown for selecting sort field and direction.
///
/// Clicking the currently active sort field toggles the direction (asc/desc).
/// Clicking a different field selects it with its default direction.
/// The active field shows a checkmark; the trigger shows the field name + an
/// arrow indicating current direction.
#[component]
pub fn SortDropdown(
    /// Current sort field.
    #[prop(into)]
    field: Signal<SortField>,
    /// Current sort direction.
    #[prop(into)]
    direction: Signal<SortDirection>,
    /// Called when the user changes sort field or direction.
    on_change: Callback<(SortField, SortDirection)>,
) -> impl IntoView {
    let (open, set_open) = signal(false);
    let trigger_ref = NodeRef::<leptos::html::Div>::new();

    // Display: "Priority ↑" etc.
    let display = Memo::new(move |_| {
        let f = field.get();
        let d = direction.get();
        Some(format!("{} {}", f.label(), d.arrow()))
    });

    view! {
        <div node_ref=trigger_ref>
            <DropdownTrigger
                label="Sort"
                value=Signal::derive(move || display.get())
                on_click=Callback::new(move |()| set_open.update(|o| *o = !*o))
            />
        </div>
        <DropdownMenu
            trigger_ref=trigger_ref
            open=Signal::derive(move || open.get())
            on_close=Callback::new(move |()| set_open.set(false))
        >
            {SortField::ALL.iter().map(|sort_field| {
                let sf = *sort_field;
                let label = sf.label().to_string();
                view! {
                    <DropdownItem
                        label=label
                        selected=Signal::derive(move || field.get() == sf)
                        on_select=Callback::new(move |()| {
                            if field.get_untracked() == sf {
                                // Same field: toggle direction.
                                on_change.run((sf, direction.get_untracked().toggle()));
                            } else {
                                // Different field: use its default direction.
                                on_change.run((sf, sf.default_direction()));
                            }
                            set_open.set(false);
                        })
                    />
                }
            }).collect_view()}
        </DropdownMenu>
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Label Filter Dropdown
// ─────────────────────────────────────────────────────────────────────────────

/// Dropdown filter for issue labels.
///
/// Reads labels from SyncStore (real-time) with a server function fallback.
/// When `team_id` is provided, filters to show only workspace-scoped labels
/// (team_id = None) and labels belonging to that team.
#[component]
pub fn LabelFilterDropdown(
    #[prop(into)] value: Signal<Vec<String>>,
    on_change: Callback<Vec<String>>,
    /// When filtering by team, only show labels that are workspace-scoped
    /// (team_id = None) or belong to this team.
    #[prop(optional, into)]
    team_id: Option<Signal<Option<String>>>,
) -> impl IntoView {
    let (open, set_open) = signal(false);
    let trigger_ref = NodeRef::<leptos::html::Div>::new();

    // Use SyncStore for labels when available (real-time), fall back to server.
    let sync_store = use_context::<crate::cache::store::SyncStore>();

    // Fetch labels dynamically from the server.
    let labels_resource = Resource::new(
        || (),
        move |_| async move { list_labels(None).await },
    );

    // Resolved labels — prefer SyncStore, fall back to server resource.
    // When team_id is provided, filter to workspace-scoped + team-specific labels.
    let labels = Memo::new(move |_| {
        let all = if let Some(store) = sync_store {
            let l = store.labels().get();
            if !l.is_empty() || store.initialized().get() {
                l
            } else {
                match labels_resource.get() {
                    Some(Ok(items)) => items,
                    Some(Err(e)) => {
                        tracing::warn!("Failed to load labels for filter dropdown: {e}");
                        Vec::new()
                    }
                    None => Vec::new(),
                }
            }
        } else {
            match labels_resource.get() {
                Some(Ok(items)) => items,
                Some(Err(e)) => {
                    tracing::warn!("Failed to load labels for filter dropdown: {e}");
                    Vec::new()
                }
                None => Vec::new(),
            }
        };

        // Filter by team when a team_id signal is provided and has a value.
        if let Some(team_id_signal) = team_id
            && let Some(ref tid) = team_id_signal.get()
        {
            return all
                .into_iter()
                .filter(|l| {
                    l.team_id.is_none() || l.team_id.as_deref() == Some(tid.as_str())
                })
                .collect();
        }
        all
    });

    // Display name for the current selection (multi-select).
    // 0 selected -> None (shows "All labels" default label)
    // 1 selected -> look up name from loaded labels
    // 2+ selected -> "Label (N)"
    let display = Memo::new(move |_| {
        let v = value.get();
        match v.len() {
            0 => None,
            1 => {
                let id = &v[0];
                labels
                    .get()
                    .iter()
                    .find(|l| &l.label_id == id)
                    .map(|l| l.name.clone())
            }
            n => Some(format!("Label ({n})")),
        }
    });

    // Icon — show a colored dot when exactly 1 label is selected.
    let current_label_color = Memo::new(move |_| {
        let v = value.get();
        if v.len() == 1 {
            labels
                .get()
                .iter()
                .find(|l| l.label_id == v[0])
                .map(|l| l.color.clone())
        } else {
            None
        }
    });

    view! {
        <div node_ref=trigger_ref>
            <DropdownTrigger
                label="All labels"
                value=Signal::derive(move || display.get())
                icon=Arc::new(move || {
                    current_label_color.get().map(|color| {
                        view! {
                            <span
                                class="inline-block w-2.5 h-2.5 rounded-full shrink-0"
                                style=format!("background-color: {color}")
                            />
                        }.into_any()
                    }).unwrap_or_else(|| view! { <span/> }.into_any())
                }) as ChildrenFn
                on_click=Callback::new(move |()| set_open.update(|o| *o = !*o))
            />
        </div>
        <DropdownMenu
            trigger_ref=trigger_ref
            open=Signal::derive(move || open.get())
            on_close=Callback::new(move |()| set_open.set(false))
        >
            <DropdownItem
                label="All labels"
                selected=Signal::derive(move || value.get().is_empty())
                on_select=Callback::new(move |()| { on_change.run(Vec::new()); })
            />
            {move || labels.get().into_iter().map(|label| {
                let label_id = label.label_id.clone();
                let label_id_check = label.label_id.clone();
                let label_name = label.name.clone();
                let label_color = label.color.clone();
                view! {
                    <DropdownItem
                        label=label_name
                        selected=Signal::derive(move || value.get().contains(&label_id_check))
                        on_select=Callback::new({
                            let id = label_id.clone();
                            move |()| {
                                let mut current = value.get_untracked();
                                if let Some(pos) = current.iter().position(|s| s == &id) {
                                    current.remove(pos);
                                } else {
                                    current.push(id.clone());
                                }
                                on_change.run(current);
                            }
                        })
                        icon=Arc::new(move || {
                            view! {
                                <span
                                    class="inline-block w-2.5 h-2.5 rounded-full shrink-0"
                                    style=format!("background-color: {}", label_color)
                                />
                            }.into_any()
                        }) as ChildrenFn
                    />
                }
            }).collect_view()}
        </DropdownMenu>
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Project Filter Dropdown
// ─────────────────────────────────────────────────────────────────────────────

/// Dropdown filter for projects.
///
/// Reads projects from SyncStore (real-time) with a server function fallback.
/// Multi-select: issues are matched by `project_id`.
#[component]
pub fn ProjectFilterDropdown(
    #[prop(into)] value: Signal<Vec<String>>,
    on_change: Callback<Vec<String>>,
) -> impl IntoView {
    let (open, set_open) = signal(false);
    let trigger_ref = NodeRef::<leptos::html::Div>::new();

    // Use SyncStore for projects when available (real-time), fall back to server.
    let sync_store = use_context::<crate::cache::store::SyncStore>();

    // Fetch projects dynamically from the server.
    let projects_resource = Resource::new(
        || (),
        move |_| async move { list_projects().await },
    );

    // Resolved projects — prefer SyncStore, fall back to server resource.
    let projects = Memo::new(move |_| {
        if let Some(store) = sync_store {
            let p = store.projects().get();
            if !p.is_empty() || store.initialized().get() {
                p
            } else {
                match projects_resource.get() {
                    Some(Ok(items)) => items,
                    Some(Err(e)) => {
                        tracing::warn!("Failed to load projects for filter dropdown: {e}");
                        Vec::new()
                    }
                    None => Vec::new(),
                }
            }
        } else {
            match projects_resource.get() {
                Some(Ok(items)) => items,
                Some(Err(e)) => {
                    tracing::warn!("Failed to load projects for filter dropdown: {e}");
                    Vec::new()
                }
                None => Vec::new(),
            }
        }
    });

    // Display name for the current selection (multi-select).
    // 0 selected -> None (shows "All projects" default label)
    // 1 selected -> look up name from loaded projects
    // 2+ selected -> "Project (N)"
    let display = Memo::new(move |_| {
        let v = value.get();
        match v.len() {
            0 => None,
            1 => {
                let id = &v[0];
                projects
                    .get()
                    .iter()
                    .find(|p| &p.project_id == id)
                    .map(|p| p.name.clone())
            }
            n => Some(format!("Project ({n})")),
        }
    });

    view! {
        <div node_ref=trigger_ref>
            <DropdownTrigger
                label="All projects"
                value=Signal::derive(move || display.get())
                on_click=Callback::new(move |()| set_open.update(|o| *o = !*o))
            />
        </div>
        <DropdownMenu
            trigger_ref=trigger_ref
            open=Signal::derive(move || open.get())
            on_close=Callback::new(move |()| set_open.set(false))
        >
            <DropdownItem
                label="All projects"
                selected=Signal::derive(move || value.get().is_empty())
                on_select=Callback::new(move |()| { on_change.run(Vec::new()); })
            />
            {move || projects.get().into_iter().map(|project| {
                let project_id = project.project_id.clone();
                let project_id_check = project.project_id.clone();
                let project_name = project.name.clone();
                view! {
                    <DropdownItem
                        label=project_name
                        selected=Signal::derive(move || value.get().contains(&project_id_check))
                        on_select=Callback::new({
                            let id = project_id.clone();
                            move |()| {
                                let mut current = value.get_untracked();
                                if let Some(pos) = current.iter().position(|s| s == &id) {
                                    current.remove(pos);
                                } else {
                                    current.push(id.clone());
                                }
                                on_change.run(current);
                            }
                        })
                    />
                }
            }).collect_view()}
        </DropdownMenu>
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Composable Filter System — Field Definitions (TRA-103)
// ─────────────────────────────────────────────────────────────────────────────

/// Definition of a filterable field — describes how to render filter chips
/// and what operators/values are available.
pub struct FilterFieldDef {
    pub key: &'static str,
    pub label: &'static str,
    pub icon: phosphor_leptos::IconData,
    pub operators: &'static [OperatorDef],
    pub value_kind: ValueKind,
}

/// Definition of an operator for a filter field.
pub struct OperatorDef {
    pub key: &'static str,
    pub label_singular: &'static str,
    pub label_plural: &'static str,
}

/// Describes what kind of value picker to render for a field.
#[derive(Clone, Copy, PartialEq)]
pub enum ValueKind {
    /// Fixed set of enum values (status categories, priority levels).
    EnumSelect,
}

// ── Day-1 field definitions ────────────────────────────────────────────────

pub const STATUS_FIELD: FilterFieldDef = FilterFieldDef {
    key: "status",
    label: "Status",
    icon: phosphor_leptos::CIRCLE_DASHED,
    operators: &[
        OperatorDef { key: "any_of", label_singular: "is", label_plural: "is any of" },
        OperatorDef { key: "none_of", label_singular: "is not", label_plural: "is not" },
    ],
    value_kind: ValueKind::EnumSelect,
};

pub const PRIORITY_FIELD: FilterFieldDef = FilterFieldDef {
    key: "priority",
    label: "Priority",
    icon: phosphor_leptos::CELL_SIGNAL_HIGH,
    operators: &[
        OperatorDef { key: "any_of", label_singular: "is", label_plural: "is any of" },
        OperatorDef { key: "none_of", label_singular: "is not", label_plural: "is not" },
    ],
    value_kind: ValueKind::EnumSelect,
};

/// Returns all available filter field definitions.
pub fn all_filter_fields() -> &'static [&'static FilterFieldDef] {
    &[&STATUS_FIELD, &PRIORITY_FIELD]
}

/// Look up a field definition by key.
pub fn find_field_def(key: &str) -> Option<&'static FilterFieldDef> {
    all_filter_fields().iter().find(|f| f.key == key).copied()
}

/// Look up an operator definition for a given field and operator key.
fn find_operator_def(field_def: &FilterFieldDef, operator_key: &str) -> Option<&'static OperatorDef> {
    field_def.operators.iter().find(|op| op.key == operator_key)
}

// ─────────────────────────────────────────────────────────────────────────────
// Composable Filter System — apply_clause helper
// ─────────────────────────────────────────────────────────────────────────────

/// Apply a single filter clause to an issue, returning `true` if the issue
/// passes the filter (should be included).
pub fn apply_clause(clause: &FilterClause, issue: &IssueWithDetails) -> bool {
    if clause.values.is_empty() {
        // A clause with no values selected passes everything.
        return true;
    }
    match (clause.field.as_str(), clause.operator.as_str()) {
        ("status", "any_of") => clause.values.contains(&issue.status_id),
        ("status", "none_of") => !clause.values.contains(&issue.status_id),
        ("priority", "any_of") => clause.values.contains(&issue.priority.to_string()),
        ("priority", "none_of") => !clause.values.contains(&issue.priority.to_string()),
        ("label", "any_of") => issue.labels.iter().any(|l| clause.values.contains(&l.label_id)),
        ("label", "none_of") => !issue.labels.iter().any(|l| clause.values.contains(&l.label_id)),
        ("project", "any_of") => issue.project_id.as_ref().is_some_and(|pid| clause.values.contains(pid)),
        ("project", "none_of") => !issue.project_id.as_ref().is_some_and(|pid| clause.values.contains(pid)),
        // Unknown field/operator — pass through (don't block issues).
        _ => true,
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Composable Filter System — FilterChip component
// ─────────────────────────────────────────────────────────────────────────────

/// Renders a single filter clause as a chip in the filter bar.
///
/// Layout: `[icon] Field` `operator` `value summary` `x`
///
/// Interactions:
/// - Click operator segment: dropdown with available operators for this field
/// - Click value segment: dropdown with value picker (checkboxes)
/// - Click x: remove this filter clause
#[component]
pub fn FilterChip(
    /// Index of this clause in the parent's clauses signal.
    index: usize,
    /// The shared clauses signal — chip modifies it in place.
    clauses: RwSignal<Vec<FilterClause>>,
    /// Team_id for filtering statuses by team. Pass `Signal::stored(None)` if not team-scoped.
    #[prop(into)]
    team_id: Signal<Option<String>>,
) -> impl IntoView {
    // Read the current clause reactively.
    let clause = Memo::new(move |_| {
        let all = clauses.get();
        all.get(index).cloned()
    });

    // Operator picker state.
    let (op_open, set_op_open) = signal(false);
    let op_trigger_ref = NodeRef::<leptos::html::Div>::new();

    // Value picker state.
    let (val_open, set_val_open) = signal(false);
    let val_trigger_ref = NodeRef::<leptos::html::Div>::new();

    // Value summary display text.
    let value_summary = Memo::new(move |_| {
        let Some(c) = clause.get() else { return String::new() };
        let count = c.values.len();
        if count == 0 {
            return "select...".to_string();
        }
        if count == 1 {
            // Try to resolve a human-readable label.
            let val = &c.values[0];
            match c.field.as_str() {
                "priority" => match val.as_str() {
                    "1" => return "Urgent".to_string(),
                    "2" => return "High".to_string(),
                    "3" => return "Medium".to_string(),
                    "4" => return "Low".to_string(),
                    _ => return val.clone(),
                },
                "status" => {
                    // Try to look up the status name from SyncStore.
                    if let Some(store) = use_context::<crate::cache::store::SyncStore>() {
                        let statuses = store.statuses().get();
                        if let Some(s) = statuses.iter().find(|s| s.status_id == *val) {
                            return s.name.clone();
                        }
                    }
                    return val.clone();
                }
                _ => return val.clone(),
            }
        }
        // 2+ values: show count.
        match c.field.as_str() {
            "status" => format!("{count} statuses"),
            "priority" => format!("{count} priorities"),
            _ => format!("{count} values"),
        }
    });

    // Operator label.
    let operator_label = Memo::new(move |_| {
        let Some(c) = clause.get() else { return String::new() };
        let field_def = find_field_def(&c.field);
        let op_def = field_def.and_then(|f| find_operator_def(f, &c.operator));
        match op_def {
            Some(od) => {
                if c.values.len() <= 1 {
                    od.label_singular.to_string()
                } else {
                    od.label_plural.to_string()
                }
            }
            None => c.operator.clone(),
        }
    });

    // Remove this clause.
    let remove = move |_: web_sys::MouseEvent| {
        clauses.update(|cs| {
            if index < cs.len() {
                cs.remove(index);
            }
        });
    };

    // Change operator for this clause.
    let set_operator = move |new_op: &'static str| {
        clauses.update(|cs| {
            if let Some(c) = cs.get_mut(index) {
                c.operator = new_op.to_string();
            }
        });
        set_op_open.set(false);
    };

    view! {
        {move || {
            let Some(c) = clause.get() else { return ().into_any() };
            let field_def = find_field_def(&c.field);
            let field_label = field_def.map(|f| f.label).unwrap_or(&c.field);
            let field_icon = field_def.map(|f| f.icon);

            view! {
                <div class="inline-flex items-center gap-1 h-7 px-2 rounded-md border border-border text-xs text-foreground bg-card transition-colors">
                    // Field icon + label
                    {field_icon.map(|icon| view! {
                        <Icon icon=icon size="12px"/>
                    })}
                    <span class="text-muted-foreground">{field_label.to_string()}</span>

                    // Operator segment (clickable)
                    <div node_ref=op_trigger_ref class="inline-flex">
                        <button
                            type="button"
                            class="cursor-pointer hover:text-primary transition-colors duration-200 text-muted-foreground focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
                            on:click=move |_| set_op_open.update(|o| *o = !*o)
                        >
                            {move || operator_label.get()}
                        </button>
                    </div>
                    <DropdownMenu
                        trigger_ref=op_trigger_ref
                        open=Signal::derive(move || op_open.get())
                        on_close=Callback::new(move |()| set_op_open.set(false))
                    >
                        {if let Some(fd) = field_def {
                            fd.operators.iter().map(|op| {
                                let op_key = op.key;
                                let label = op.label_singular.to_string();
                                let current_op = c.operator.clone();
                                view! {
                                    <DropdownItem
                                        label=label
                                        selected=Signal::derive(move || current_op == op_key)
                                        on_select=Callback::new(move |()| set_operator(op_key))
                                    />
                                }
                            }).collect_view().into_any()
                        } else {
                            ().into_any()
                        }}
                    </DropdownMenu>

                    // Value segment (clickable)
                    <div node_ref=val_trigger_ref class="inline-flex">
                        <button
                            type="button"
                            class="cursor-pointer hover:text-primary transition-colors duration-200 font-medium focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
                            on:click=move |_| set_val_open.update(|o| *o = !*o)
                        >
                            {move || value_summary.get()}
                        </button>
                    </div>
                    // Value picker dropdown
                    <DropdownMenu
                        trigger_ref=val_trigger_ref
                        open=Signal::derive(move || val_open.get())
                        on_close=Callback::new(move |()| set_val_open.set(false))
                    >
                        {match c.field.as_str() {
                            "status" => view! {
                                <StatusValuePicker
                                    clauses=clauses
                                    clause_index=index
                                    team_id=team_id
                                />
                            }.into_any(),
                            "priority" => view! {
                                <PriorityValuePicker
                                    clauses=clauses
                                    clause_index=index
                                />
                            }.into_any(),
                            _ => ().into_any(),
                        }}
                    </DropdownMenu>

                    // Remove button
                    <button
                        type="button"
                        class="ml-0.5 cursor-pointer text-muted-foreground hover:text-foreground transition-colors duration-200 focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
                        on:click=remove
                    >
                        "\u{00d7}"
                    </button>
                </div>
            }.into_any()
        }}
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Composable Filter System — Value Pickers
// ─────────────────────────────────────────────────────────────────────────────

/// Status value picker — renders checkboxes for all workspace statuses.
#[component]
fn StatusValuePicker(
    clauses: RwSignal<Vec<FilterClause>>,
    clause_index: usize,
    /// Team_id for filtering statuses. `None` means show all statuses.
    #[prop(into)]
    team_id: Signal<Option<String>>,
) -> impl IntoView {
    let sync_store = use_context::<crate::cache::store::SyncStore>();

    let statuses_resource = Resource::new(
        || (),
        move |_| async move { list_statuses(None).await },
    );

    let statuses = Memo::new(move |_| {
        let all = if let Some(store) = sync_store {
            let s = store.statuses().get();
            if !s.is_empty() || store.initialized().get() {
                s
            } else {
                match statuses_resource.get() {
                    Some(Ok(items)) => items,
                    Some(Err(e)) => {
                        tracing::warn!("Failed to load statuses for filter picker: {e}");
                        Vec::new()
                    }
                    None => Vec::new(),
                }
            }
        } else {
            match statuses_resource.get() {
                Some(Ok(items)) => items,
                Some(Err(e)) => {
                    tracing::warn!("Failed to load statuses for filter picker: {e}");
                    Vec::new()
                }
                None => Vec::new(),
            }
        };

        if let Some(ref tid) = team_id.get() {
            return all
                .into_iter()
                .filter(|s| {
                    s.team_id.is_none() || s.team_id.as_deref() == Some(tid.as_str())
                })
                .collect();
        }
        all
    });

    let selected_values = Memo::new(move |_| {
        let cs = clauses.get();
        cs.get(clause_index)
            .map(|c| c.values.clone())
            .unwrap_or_default()
    });

    view! {
        {move || statuses.get().into_iter().map(|status| {
            let status_id = status.status_id.clone();
            let status_id_for_check = status.status_id.clone();
            let label = status.name.clone();
            let variant = IssueStatusVariant::parse(&status.category);
            view! {
                <div
                    class="flex items-center gap-2 w-full cursor-default select-none text-[13px] px-2.5 py-[5px] mx-1 my-px rounded-[3px] transition-colors duration-100 hover:bg-secondary"
                >
                    <Checkbox
                        checked=Signal::derive(move || selected_values.get().contains(&status_id_for_check))
                        on_change=Callback::new(move |_: bool| {
                            let val = status_id.clone();
                            clauses.update(|cs| {
                                if let Some(c) = cs.get_mut(clause_index) {
                                    if let Some(pos) = c.values.iter().position(|v| v == &val) {
                                        c.values.remove(pos);
                                    } else {
                                        c.values.push(val);
                                    }
                                }
                            });
                        })
                    />
                    <IssueStatusBadge status=variant size=14/>
                    <span class="truncate">{label}</span>
                </div>
            }
        }).collect_view()}
    }
}

/// Priority value picker — renders checkboxes for Urgent(1), High(2), Medium(3), Low(4).
#[component]
fn PriorityValuePicker(
    clauses: RwSignal<Vec<FilterClause>>,
    clause_index: usize,
) -> impl IntoView {
    let priorities: &'static [(&str, &str, i32)] = &[
        ("1", "Urgent", 1),
        ("2", "High", 2),
        ("3", "Medium", 3),
        ("4", "Low", 4),
    ];

    let selected_values = Memo::new(move |_| {
        let cs = clauses.get();
        cs.get(clause_index)
            .map(|c| c.values.clone())
            .unwrap_or_default()
    });

    view! {
        {priorities.iter().map(|(key, label, priority_val)| {
            let key_for_check = key.to_string();
            let key_for_cb = key.to_string();
            let label = label.to_string();
            let priority_val = *priority_val;
            view! {
                <div
                    class="flex items-center gap-2 w-full cursor-default select-none text-[13px] px-2.5 py-[5px] mx-1 my-px rounded-[3px] transition-colors duration-100 hover:bg-secondary"
                >
                    <Checkbox
                        checked=Signal::derive(move || selected_values.get().contains(&key_for_check))
                        on_change=Callback::new(move |_: bool| {
                            let val = key_for_cb.clone();
                            clauses.update(|cs| {
                                if let Some(c) = cs.get_mut(clause_index) {
                                    if let Some(pos) = c.values.iter().position(|v| v == &val) {
                                        c.values.remove(pos);
                                    } else {
                                        c.values.push(val);
                                    }
                                }
                            });
                        })
                    />
                    <PriorityIndicator priority=priority_val/>
                    <span class="truncate">{label}</span>
                </div>
            }
        }).collect_view()}
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Composable Filter System — AddFilterMenu component
// ─────────────────────────────────────────────────────────────────────────────

/// A "+ Add Filter" button that opens a dropdown listing available fields.
/// Selecting a field adds a new FilterClause with default operator and empty values,
/// then the chip's value picker opens automatically.
#[component]
pub fn AddFilterMenu(
    clauses: RwSignal<Vec<FilterClause>>,
) -> impl IntoView {
    let (open, set_open) = signal(false);
    let trigger_ref = NodeRef::<leptos::html::Div>::new();

    view! {
        <div node_ref=trigger_ref class="inline-flex">
            <Button
                variant=ButtonVariant::Ghost
                size=ButtonSize::Sm
                on:click=move |_| set_open.update(|o| *o = !*o)
            >
                <Icon icon=phosphor_leptos::PLUS size="12px"/>
                "Filter"
            </Button>
        </div>
        <DropdownMenu
            trigger_ref=trigger_ref
            open=Signal::derive(move || open.get())
            on_close=Callback::new(move |()| set_open.set(false))
        >
            {all_filter_fields().iter().map(|field_def| {
                let key = field_def.key;
                let label = field_def.label.to_string();
                let icon_data = field_def.icon;
                view! {
                    <DropdownItem
                        label=label
                        on_select=Callback::new(move |()| {
                            clauses.update(|cs| {
                                cs.push(FilterClause {
                                    field: key.to_string(),
                                    operator: "any_of".to_string(),
                                    values: Vec::new(),
                                });
                            });
                            set_open.set(false);
                        })
                        icon=Arc::new(move || view! { <Icon icon=icon_data size="14px"/> }.into_any()) as ChildrenFn
                    />
                }
            }).collect_view()}
        </DropdownMenu>
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Composable Filter System — FilterBar component
// ─────────────────────────────────────────────────────────────────────────────

/// Wraps filter chips + the AddFilterMenu. Renders the composable filter bar.
#[component]
pub fn FilterBar(
    /// The shared signal of all active filter clauses.
    clauses: RwSignal<Vec<FilterClause>>,
    /// Team_id for filtering status values by team. Pass a signal returning None
    /// for workspace-level (no team) views.
    #[prop(into)]
    team_id: Signal<Option<String>>,
) -> impl IntoView {
    view! {
        <div class="flex items-center gap-2 flex-wrap">
            {move || {
                let current = clauses.get();
                current.iter().enumerate().map(|(idx, _)| {
                    view! {
                        <FilterChip
                            index=idx
                            clauses=clauses
                            team_id=team_id
                        />
                    }
                }).collect_view()
            }}
            <AddFilterMenu clauses=clauses/>
        </div>
    }
}
