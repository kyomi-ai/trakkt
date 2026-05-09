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
    #[prop(into)] value: Signal<String>,
    on_change: Callback<String>,
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
        if let Some(team_id_signal) = team_id {
            if let Some(ref tid) = team_id_signal.get() {
                return all
                    .into_iter()
                    .filter(|s| {
                        s.team_id.is_none() || s.team_id.as_deref() == Some(tid.as_str())
                    })
                    .collect();
            }
        }
        all
    });

    // Display name for the current selection (looked up from loaded statuses).
    let display = Memo::new(move |_| {
        let v = value.get();
        if v.is_empty() {
            None
        } else {
            statuses
                .get()
                .iter()
                .find(|s| s.status_id == v)
                .map(|s| s.name.clone())
        }
    });

    // Icon variant for the current selection.
    let current_variant = Memo::new(move |_| {
        let v = value.get();
        if v.is_empty() {
            None
        } else {
            statuses
                .get()
                .iter()
                .find(|s| s.status_id == v)
                .map(|s| IssueStatusVariant::parse(&s.category))
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
            search_placeholder="Filter status..."
        >
            <DropdownItem
                label="All statuses"
                selected=Signal::derive(move || value.get().is_empty())
                on_select=Callback::new({
                    let on_change = on_change.clone();
                    move |()| { on_change.run(String::new()); set_open.set(false); }
                })
            />
            {move || statuses.get().into_iter().map(|status| {
                let status_id = status.status_id.clone();
                let status_id_check = status.status_id.clone();
                let label = status.name.clone();
                let variant = IssueStatusVariant::parse(&status.category);
                view! {
                    <DropdownItem
                        label=label
                        selected=Signal::derive(move || value.get() == status_id_check)
                        on_select=Callback::new({
                            let on_change = on_change.clone();
                            let id = status_id.clone();
                            move |()| { on_change.run(id.clone()); set_open.set(false); }
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
    #[prop(into)] value: Signal<String>,
    on_change: Callback<String>,
) -> impl IntoView {
    let (open, set_open) = signal(false);
    let trigger_ref = NodeRef::<leptos::html::Div>::new();

    let priorities: Vec<(&str, &str, i32)> = vec![
        ("1", "Urgent", 1),
        ("2", "High", 2),
        ("3", "Medium", 3),
        ("4", "Low", 4),
    ];

    let display = Memo::new(move |_| {
        let v = value.get();
        if v.is_empty() { None } else {
            match v.as_str() {
                "1" => Some("Urgent".to_string()),
                "2" => Some("High".to_string()),
                "3" => Some("Medium".to_string()),
                "4" => Some("Low".to_string()),
                _ => None,
            }
        }
    });

    let current_priority = Memo::new(move |_| {
        value.get().parse::<i32>().ok()
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
            search_placeholder="Filter priority..."
        >
            <DropdownItem
                label="All priorities"
                selected=Signal::derive(move || value.get().is_empty())
                on_select=Callback::new({
                    let on_change = on_change.clone();
                    move |()| { on_change.run(String::new()); set_open.set(false); }
                })
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
                        selected=Signal::derive(move || value.get() == key_check)
                        on_select=Callback::new({
                            let on_change = on_change.clone();
                            let k = key_owned.clone();
                            move |()| { on_change.run(k.clone()); set_open.set(false); }
                        })
                        icon=Arc::new(move || view! { <PriorityIndicator priority=priority_val/> }.into_any()) as ChildrenFn
                        shortcut=shortcut
                    />
                }
            }).collect_view()}
        </DropdownMenu>
    }
}
