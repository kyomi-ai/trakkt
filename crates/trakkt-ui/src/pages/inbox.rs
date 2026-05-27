// SPDX-License-Identifier: AGPL-3.0-or-later

//! Inbox page — notification feed for the current user.
//!
//! Shows all notifications grouped by time period (Today, Yesterday,
//! This Week, Older). Supports filtering between All and Unread,
//! mark-as-read on click, and bulk mark-all-as-read.

use std::collections::HashSet;

use leptos::prelude::*;
use leptos_router::hooks::use_navigate;
use leptos_router::location::State;
use leptos_router::NavigateOptions;
use phosphor_leptos::{Icon, IconWeight};
use wasm_bindgen::JsValue;

use crate::cache::store::SyncStore;
use crate::components::{
    Button, ButtonSize, ButtonVariant, Checkbox, ConfirmDialog, EmptyState, SearchInput, Select,
    SelectVariant,
};
use crate::server_fns::notifications::{
    bulk_delete_notifications, bulk_mark_notifications_read, bulk_mark_notifications_unread,
    list_notifications, mark_all_notifications_read, mark_notification_read,
};
use crate::server_fns::teams::list_teams;
use crate::types::IssueNavState;
use crate::utils::relative_time::relative_time;
use crate::utils::time_group::{classify_time_group, TimeGroup};
use trakkt_types::models::Notification;

fn notification_event_text(notification: &Notification) -> String {
    let actor = notification.actor_name.as_deref().unwrap_or("Someone");

    match notification.notification_type.as_str() {
        "commented" => format!("{actor} commented"),
        "status_changed" => "Status changed".to_string(),
        "assigned" => "You were assigned".to_string(),
        "priority_changed" => "Priority changed".to_string(),
        _ => format!("{actor} updated"),
    }
}

#[component]
pub fn InboxPage() -> impl IntoView {
    let (unread_only, set_unread_only) = signal(false);
    let (refetch_version, set_refetch_version) = signal(0u32);

    // Filter state
    let (team_filter, set_team_filter) = signal(String::new());
    let (type_filter, set_type_filter) = signal(String::new());
    let (search_text, set_search_text) = signal(String::new());

    // Selection state for bulk actions
    let selected = RwSignal::new(HashSet::<String>::new());
    let bulk_pending = RwSignal::new(false);
    let confirm_delete_open = RwSignal::new(false);
    let pending_delete_ids = RwSignal::new(Vec::<String>::new());

    // Clear selection when any filter changes
    Effect::new(move |_| {
        unread_only.get();
        team_filter.get();
        type_filter.get();
        search_text.get();
        selected.set(HashSet::new());
    });

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
            ("commented".to_string(), "Comments".to_string()),
            ("status_changed".to_string(), "Status changes".to_string()),
            ("assigned".to_string(), "Assignments".to_string()),
            ("priority_changed".to_string(), "Priority changes".to_string()),
        ]
    });

    let notifications_resource = Resource::new(
        move || (
            unread_only.get(),
            refetch_version.get(),
            team_filter.get(),
            type_filter.get(),
            search_text.get(),
        ),
        |(uo, _, tk, tf, search)| async move {
            let team_key = if tk.is_empty() { None } else { Some(tk) };
            let notification_type = if tf.is_empty() { None } else { Some(tf) };
            let search = if search.is_empty() { None } else { Some(search) };
            list_notifications(uo, notification_type, team_key, search).await
        },
    );

    let sync_store = use_context::<SyncStore>();

    let marking_all = RwSignal::new(false);
    let handle_mark_all_read = move |_| {
        if marking_all.get_untracked() {
            return;
        }
        marking_all.set(true);
        if let Some(store) = sync_store {
            for mut n in store.notifications().get_untracked() {
                if !n.read {
                    n.read = true;
                    store.upsert_notification(n);
                }
            }
        }
        leptos::task::spawn_local(async move {
            let _ = mark_all_notifications_read().await;
            marking_all.set(false);
            set_refetch_version.update(|v| *v += 1);
        });
    };

    let has_active_filters = move || {
        !team_filter.get().is_empty()
            || !type_filter.get().is_empty()
            || !search_text.get().is_empty()
    };

    // ── Bulk action handlers ────────────────────────────────────────────
    let handle_bulk_mark_read = move |_: web_sys::MouseEvent| {
        if bulk_pending.get_untracked() {
            return;
        }
        let ids: Vec<String> = selected.get_untracked().into_iter().collect();
        if ids.is_empty() {
            return;
        }
        bulk_pending.set(true);

        // Optimistic SyncStore update
        if let Some(store) = sync_store {
            for id in &ids {
                if let Some(mut n) = store
                    .notifications()
                    .get_untracked()
                    .into_iter()
                    .find(|n| &n.notification_id == id)
                {
                    n.read = true;
                    store.upsert_notification(n);
                }
            }
        }

        let csv = ids.join(",");
        selected.set(HashSet::new());
        leptos::task::spawn_local(async move {
            let _ = bulk_mark_notifications_read(csv).await;
            bulk_pending.set(false);
            set_refetch_version.update(|v| *v += 1);
        });
    };

    let handle_bulk_mark_unread = move |_: web_sys::MouseEvent| {
        if bulk_pending.get_untracked() {
            return;
        }
        let ids: Vec<String> = selected.get_untracked().into_iter().collect();
        if ids.is_empty() {
            return;
        }
        bulk_pending.set(true);

        // Optimistic SyncStore update
        if let Some(store) = sync_store {
            for id in &ids {
                if let Some(mut n) = store
                    .notifications()
                    .get_untracked()
                    .into_iter()
                    .find(|n| &n.notification_id == id)
                {
                    n.read = false;
                    store.upsert_notification(n);
                }
            }
        }

        let csv = ids.join(",");
        selected.set(HashSet::new());
        leptos::task::spawn_local(async move {
            let _ = bulk_mark_notifications_unread(csv).await;
            bulk_pending.set(false);
            set_refetch_version.update(|v| *v += 1);
        });
    };

    let handle_bulk_delete = move |_: web_sys::MouseEvent| {
        if bulk_pending.get_untracked() {
            return;
        }
        let ids: Vec<String> = selected.get_untracked().into_iter().collect();
        if ids.is_empty() {
            return;
        }
        pending_delete_ids.set(ids);
        confirm_delete_open.set(true);
    };

    let on_delete_confirmed = Callback::new(move |()| {
        confirm_delete_open.set(false);
        let ids = pending_delete_ids.get_untracked();
        pending_delete_ids.set(Vec::new());
        if ids.is_empty() {
            return;
        }
        bulk_pending.set(true);
        let csv = ids.join(",");
        selected.set(HashSet::new());
        leptos::task::spawn_local(async move {
            let _ = bulk_delete_notifications(csv).await;
            bulk_pending.set(false);
            set_refetch_version.update(|v| *v += 1);
        });
    });

    let handle_clear_selection = move |_: web_sys::MouseEvent| {
        selected.set(HashSet::new());
    };

    // ── Derived signals for toolbar ─────────────────────────────────────
    let has_selection = Signal::derive(move || !selected.get().is_empty());
    let selection_count = Signal::derive(move || selected.get().len());

    let all_notification_ids = Signal::derive(move || {
        notifications_resource
            .get()
            .and_then(|r| r.ok())
            .map(|notifications| {
                notifications
                    .iter()
                    .map(|n| n.notification_id.clone())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default()
    });

    const TAB_ACTIVE: &str = "px-3 py-1.5 text-sm rounded-md transition-colors bg-secondary text-foreground font-medium";
    const TAB_INACTIVE: &str = "px-3 py-1.5 text-sm rounded-md transition-colors text-muted-foreground hover:text-foreground hover:bg-secondary/50";

    view! {
        <div class="h-full flex flex-col">
            <div class="page-header h-14 px-5 flex items-center justify-between shrink-0">
                <h1 class="text-sm font-semibold text-foreground">"Inbox"</h1>
                <button
                    class="text-sm text-muted-foreground hover:text-foreground transition-colors"
                    on:click=handle_mark_all_read
                    prop:disabled=move || marking_all.get()
                >
                    {move || if marking_all.get() { "Marking..." } else { "Mark all read" }}
                </button>
            </div>

            <div class="flex items-center gap-1 px-5 py-3 border-b border-border">
                <button
                    class=move || if !unread_only.get() { TAB_ACTIVE } else { TAB_INACTIVE }
                    on:click=move |_| set_unread_only.set(false)
                >
                    "All"
                </button>
                <button
                    class=move || if unread_only.get() { TAB_ACTIVE } else { TAB_INACTIVE }
                    on:click=move |_| set_unread_only.set(true)
                >
                    "Unread"
                </button>
            </div>

            // Filter bar / bulk action toolbar
            <div class="flex items-center gap-2 px-5 py-3 border-b border-border min-h-[52px]">
                // Select-all checkbox — visible when notifications exist
                {move || {
                    let ids = all_notification_ids.get();
                    (!ids.is_empty()).then(|| {
                        view! {
                            <Checkbox
                                checked=Signal::derive(move || {
                                    let sel = selected.get();
                                    let ids = all_notification_ids.get();
                                    !ids.is_empty() && sel.len() == ids.len()
                                })
                                indeterminate=Signal::derive(move || {
                                    let sel = selected.get();
                                    let ids = all_notification_ids.get();
                                    !sel.is_empty() && sel.len() < ids.len()
                                })
                                on_change=Callback::new(move |_checked: bool| {
                                    let ids = all_notification_ids.get_untracked();
                                    selected.update(|set| {
                                        if set.len() == ids.len() && !ids.is_empty() {
                                            set.clear();
                                        } else {
                                            *set = ids.into_iter().collect();
                                        }
                                    });
                                })
                            />
                        }
                    })
                }}

                {move || {
                    if has_selection.get() {
                        let count = selection_count.get();
                        view! {
                            <span class="text-sm font-medium text-foreground whitespace-nowrap">
                                {format!("{count} selected")}
                            </span>
                            <Button
                                variant=ButtonVariant::Ghost
                                size=ButtonSize::Sm
                                disabled=Signal::derive(move || bulk_pending.get())
                                on:click=handle_bulk_mark_read
                            >
                                <Icon icon=phosphor_leptos::ENVELOPE_OPEN attr:class="h-4 w-4 mr-1.5" />
                                "Mark Read"
                            </Button>
                            <Button
                                variant=ButtonVariant::Ghost
                                size=ButtonSize::Sm
                                disabled=Signal::derive(move || bulk_pending.get())
                                on:click=handle_bulk_mark_unread
                            >
                                <Icon icon=phosphor_leptos::ENVELOPE attr:class="h-4 w-4 mr-1.5" />
                                "Mark Unread"
                            </Button>
                            <Button
                                variant=ButtonVariant::GhostDestructive
                                size=ButtonSize::Sm
                                disabled=Signal::derive(move || bulk_pending.get())
                                on:click=handle_bulk_delete
                            >
                                <Icon icon=phosphor_leptos::TRASH attr:class="h-4 w-4 mr-1.5" />
                                "Delete"
                            </Button>
                            <Button
                                variant=ButtonVariant::GhostMuted
                                size=ButtonSize::Sm
                                class="ml-auto"
                                on:click=handle_clear_selection
                            >
                                "Cancel"
                            </Button>
                        }.into_any()
                    } else {
                        view! {
                            <SearchInput
                                value=Signal::derive(move || search_text.get())
                                on_input=Callback::new(move |v: String| set_search_text.set(v))
                                placeholder="Search by title or identifier..."
                                class="max-w-xs".to_string()
                            />
                            <Select
                                value=Signal::derive(move || type_filter.get())
                                options=type_options
                                on_change=Callback::new(move |v: String| set_type_filter.set(v))
                                variant=SelectVariant::Compact
                            />
                            <Select
                                value=Signal::derive(move || team_filter.get())
                                options=team_options
                                on_change=Callback::new(move |v: String| set_team_filter.set(v))
                                variant=SelectVariant::Compact
                            />
                        }.into_any()
                    }
                }}
            </div>

            <div class="flex-1 overflow-y-auto">
                <Suspense fallback=move || view! {
                    <div class="flex items-center justify-center py-12">
                        <crate::components::Spinner/>
                    </div>
                }>
                    {move || {
                        notifications_resource.get().map(|result| {
                            match result {
                                Ok(ref notifications) if notifications.is_empty() => {
                                    if has_active_filters() {
                                        view! {
                                            <EmptyState
                                                title="No matches"
                                                description="No notifications match this filter. Try adjusting your search or filters."
                                            />
                                        }.into_any()
                                    } else {
                                        view! {
                                            <div class="flex flex-col items-center justify-center py-16 text-muted-foreground">
                                                <Icon icon=phosphor_leptos::CHECK_CIRCLE weight=IconWeight::Light size="48px" attr:class="mb-4 text-muted-foreground/50"/>
                                                <p class="text-lg font-medium">"All caught up"</p>
                                            </div>
                                        }.into_any()
                                    }
                                }
                                Ok(ref notifications) => {
                                    let grouped = group_notifications(notifications);
                                    let groups: Vec<(TimeGroup, Vec<Notification>)> = vec![
                                        (TimeGroup::Today, grouped.today),
                                        (TimeGroup::Yesterday, grouped.yesterday),
                                        (TimeGroup::ThisWeek, grouped.this_week),
                                        (TimeGroup::Older, grouped.older),
                                    ];

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
                                                        {items.into_iter().map(|notification| {
                                                            view! {
                                                                <NotificationRow
                                                                    notification=notification
                                                                    sync_store=sync_store
                                                                    selected=selected
                                                                    on_read=Callback::new(move |()| {
                                                                        set_refetch_version.update(|v| *v += 1);
                                                                    })
                                                                />
                                                            }
                                                        }).collect_view()}
                                                    </div>
                                                }
                                            }).collect_view()}
                                        </div>
                                    }.into_any()
                                }
                                Err(ref e) => {
                                    let msg = format!("{e}");
                                    view! {
                                        <div class="flex items-center justify-center py-12">
                                            <p class="text-sm text-destructive">{msg}</p>
                                        </div>
                                    }.into_any()
                                }
                            }
                        })
                    }}
                </Suspense>
            </div>

            <ConfirmDialog
                open=Signal::derive(move || confirm_delete_open.get())
                title=Signal::derive(move || {
                    let count = pending_delete_ids.get().len();
                    format!("Delete {count} notification{}?", if count == 1 { "" } else { "s" })
                })
                message="Deleted notifications cannot be recovered."
                confirm_text="Delete"
                destructive=true
                on_confirm=on_delete_confirmed
                on_cancel=Callback::new(move |()| confirm_delete_open.set(false))
            />
        </div>
    }
}

struct GroupedNotifications {
    today: Vec<Notification>,
    yesterday: Vec<Notification>,
    this_week: Vec<Notification>,
    older: Vec<Notification>,
}

fn group_notifications(notifications: &[Notification]) -> GroupedNotifications {
    let mut today = Vec::new();
    let mut yesterday = Vec::new();
    let mut this_week = Vec::new();
    let mut older = Vec::new();

    for n in notifications {
        match classify_time_group(&n.created_at) {
            TimeGroup::Today => today.push(n.clone()),
            TimeGroup::Yesterday => yesterday.push(n.clone()),
            TimeGroup::ThisWeek => this_week.push(n.clone()),
            TimeGroup::Older => older.push(n.clone()),
        }
    }

    GroupedNotifications {
        today,
        yesterday,
        this_week,
        older,
    }
}

#[component]
fn NotificationRow(
    notification: Notification,
    sync_store: Option<SyncStore>,
    selected: RwSignal<HashSet<String>>,
    on_read: Callback<()>,
) -> impl IntoView {
    let nav = use_navigate();
    let is_unread = !notification.read;
    let notification_id = notification.notification_id.clone();
    let nid_for_checked = notification.notification_id.clone();
    let nid_for_toggle = notification.notification_id.clone();
    let event_text = notification_event_text(&notification);
    let via_suffix = crate::components::attribution::render_via_suffix(
        notification.action_source,
        notification.action_source_label.clone(),
    );
    let issue_title = notification.issue_title.clone().unwrap_or_default();
    let timestamp = relative_time(&notification.created_at);

    let issue_id_for_lookup = notification.issue_id.clone();
    let issue_id_for_label = notification.issue_id.clone();
    let team_key_for_label = notification.team_key.clone();
    let issue_number_for_label = notification.issue_number;
    let team_key_for_click = notification.team_key.clone();
    let issue_number_for_click = notification.issue_number;

    let issue_label = Signal::derive(move || {
        // Prefer data from the notification itself
        if let (Some(tk), Some(num)) = (&team_key_for_label, issue_number_for_label) {
            return Some(format!("{tk}-{num}"));
        }
        // Fallback to SyncStore for older notifications that may not have team_key
        sync_store.and_then(|store| {
            store.issues().get()
                .iter()
                .find(|i| i.issue_id == issue_id_for_label)
                .map(|issue| format!("{}-{}", issue.team_key, issue.number))
        })
    });

    let on_click = move |_: web_sys::MouseEvent| {
        if is_unread {
            let nid = notification_id.clone();
            if let Some(store) = sync_store {
                let mut n = notification.clone();
                n.read = true;
                store.upsert_notification(n);
            }
            leptos::task::spawn_local(async move {
                let _ = mark_notification_read(nid).await;
                on_read.run(());
            });
        }
        let href = {
            // Prefer data from the notification itself
            let from_notification = team_key_for_click.as_ref().and_then(|tk| {
                issue_number_for_click.map(|num| format!("/issues/{tk}-{num}"))
            });
            from_notification.or_else(|| {
                sync_store.and_then(|store| {
                    store.issues().get_untracked()
                        .iter()
                        .find(|i| i.issue_id == issue_id_for_lookup)
                        .map(|issue| format!("/issues/{}-{}", issue.team_key, issue.number))
                })
            })
        };
        if let Some(h) = href {
            let nav_state = IssueNavState::from_current_path("/inbox", "");
            let json = nav_state.to_json();
            nav(&h, NavigateOptions {
                state: State::from(JsValue::from_str(&json)),
                ..Default::default()
            });
        }
    };

    view! {
        <div
            class="flex items-start gap-3 px-6 py-3 hover:bg-accent transition-colors cursor-pointer"
            on:click=on_click
        >
            // Per-row checkbox — stopPropagation to prevent row navigation
            <div class="flex-shrink-0 pt-0.5" on:click=|e: web_sys::MouseEvent| e.stop_propagation()>
                <Checkbox
                    checked=Signal::derive(move || selected.get().contains(&nid_for_checked))
                    on_change=Callback::new(move |_checked: bool| {
                        selected.update(|set| {
                            if !set.remove(&nid_for_toggle) {
                                set.insert(nid_for_toggle.clone());
                            }
                        });
                    })
                />
            </div>

            <div class="flex-shrink-0 pt-1.5">
                {if is_unread {
                    view! { <div class="w-2 h-2 rounded-full bg-primary"/> }.into_any()
                } else {
                    view! { <div class="w-2 h-2 rounded-full border border-muted-foreground/30"/> }.into_any()
                }}
            </div>

            <div class="flex-1 min-w-0">
                <div class="flex items-baseline gap-2">
                    <span class=if is_unread { "text-sm font-medium text-foreground" } else { "text-sm text-muted-foreground" }>
                        {event_text}{via_suffix}
                    </span>
                    {move || issue_label.get().map(|label| view! {
                        <span class="text-xs text-muted-foreground font-mono">{label}</span>
                    })}
                </div>
                {(!issue_title.is_empty()).then(|| view! {
                    <p class="text-sm text-muted-foreground truncate mt-0.5">{issue_title}</p>
                })}
            </div>

            <span class="flex-shrink-0 text-xs text-muted-foreground pt-0.5">
                {timestamp}
            </span>
        </div>
    }
}
