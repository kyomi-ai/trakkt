// SPDX-License-Identifier: AGPL-3.0-or-later

//! Inbox page — notification feed for the current user.
//!
//! Shows all notifications grouped by time period (Today, Yesterday,
//! This Week, Older). Supports filtering between All and Unread,
//! mark-as-read on click, and bulk mark-all-as-read.

use leptos::prelude::*;
use leptos_router::hooks::use_navigate;
use leptos_router::location::State;
use leptos_router::NavigateOptions;
use phosphor_leptos::{Icon, IconWeight};
use wasm_bindgen::JsValue;

use crate::cache::store::SyncStore;
use crate::server_fns::notifications::{
    list_notifications, mark_all_notifications_read, mark_notification_read,
};
use crate::types::IssueNavState;
use crate::utils::relative_time::relative_time;
use trakkt_types::models::Notification;

#[derive(Clone, Copy, PartialEq)]
enum TimeGroup {
    Today,
    Yesterday,
    ThisWeek,
    Older,
}

impl TimeGroup {
    fn label(self) -> &'static str {
        match self {
            Self::Today => "Today",
            Self::Yesterday => "Yesterday",
            Self::ThisWeek => "This Week",
            Self::Older => "Older",
        }
    }
}

fn classify_time_group(created_at: &str) -> TimeGroup {
    use chrono::NaiveDateTime;

    let parsed = created_at
        .parse::<chrono::DateTime<chrono::Utc>>()
        .ok()
        .or_else(|| {
            chrono::DateTime::parse_from_str(created_at, "%Y-%m-%d %H:%M:%S%.f%#z")
                .ok()
                .map(|dt| dt.to_utc())
        })
        .or_else(|| {
            NaiveDateTime::parse_from_str(created_at, "%Y-%m-%dT%H:%M:%S%.f")
                .ok()
                .or_else(|| {
                    NaiveDateTime::parse_from_str(created_at, "%Y-%m-%d %H:%M:%S").ok()
                })
                .map(|naive| naive.and_utc())
        });

    let ts = match parsed {
        Some(dt) => dt,
        None => return TimeGroup::Older,
    };

    let ts_ms = ts.timestamp_millis() as f64;

    let now = js_sys::Date::new_0();
    let now_day = now.get_day() as i32;

    let today_start = js_sys::Date::new_0();
    today_start.set_hours(0);
    today_start.set_minutes(0);
    today_start.set_seconds(0);
    today_start.set_milliseconds(0);
    let today_start_ms = today_start.get_time();

    if ts_ms >= today_start_ms {
        return TimeGroup::Today;
    }

    let yesterday_start_ms = today_start_ms - 86_400_000.0;
    if ts_ms >= yesterday_start_ms {
        return TimeGroup::Yesterday;
    }

    let monday_offset = if now_day == 0 { 6 } else { now_day - 1 };
    let week_start_ms = today_start_ms - (monday_offset as f64 * 86_400_000.0);
    if ts_ms >= week_start_ms {
        return TimeGroup::ThisWeek;
    }

    TimeGroup::Older
}

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

    let notifications_resource = Resource::new(
        move || (unread_only.get(), refetch_version.get()),
        |(uo, _)| async move { list_notifications(uo).await },
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
                                    view! {
                                        <div class="flex flex-col items-center justify-center py-16 text-muted-foreground">
                                            <Icon icon=phosphor_leptos::CHECK_CIRCLE weight=IconWeight::Light size="48px" attr:class="mb-4 text-muted-foreground/50"/>
                                            <p class="text-lg font-medium">"All caught up"</p>
                                        </div>
                                    }.into_any()
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
    on_read: Callback<()>,
) -> impl IntoView {
    let nav = use_navigate();
    let is_unread = !notification.read;
    let notification_id = notification.notification_id.clone();
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
