// SPDX-License-Identifier: AGPL-3.0-or-later

//! Shared filter dropdown components for issue pages.
//!
//! Both `issue_list.rs` and `my_issues.rs` use the same status and priority
//! filter dropdowns. This module provides the canonical implementations to
//! eliminate duplication per CODING_STANDARDS.md ("No duplicated helper
//! functions across modules").

use std::sync::Arc;

use leptos::children::ChildrenFn;
use leptos::prelude::*;

use crate::components::{
    DropdownItem, DropdownMenu, DropdownTrigger, IssueStatusBadge, IssueStatusVariant,
    PriorityIndicator,
};
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
    pub fn from_str(s: &str) -> Option<Self> {
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
                ad.cmp(&bd)
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
                statuses_resource
                    .get()
                    .and_then(|r| r.ok())
                    .unwrap_or_default()
            }
        } else {
            statuses_resource
                .get()
                .and_then(|r| r.ok())
                .unwrap_or_default()
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
