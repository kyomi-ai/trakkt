// SPDX-License-Identifier: AGPL-3.0-or-later

//! Notification preferences settings page.
//!
//! Lets users configure which event types generate notifications and whether
//! to receive self-notifications for agent/API actions.

use leptos::prelude::*;
use phosphor_leptos::{Icon, IconWeight};

use crate::components::{
    Alert, AlertDescription, AlertVariant, Card, CardContent, CardHeader, CardTitle,
    SettingsPageSkeleton, Switch,
};
use crate::server_fns::notifications::{get_notification_preferences, update_notification_preference};
use trakkt_types::models::NotificationPreferences;

// ─────────────────────────────────────────────────────────────────────────────
// Main page
// ─────────────────────────────────────────────────────────────────────────────

#[component]
pub fn NotificationsPage() -> impl IntoView {
    let prefs_resource = LocalResource::new(get_notification_preferences);

    view! {
        <div class="p-4 sm:p-6">
            <h2 class="text-xl font-display text-foreground mb-4">"Notification Preferences"</h2>
            <p class="text-muted-foreground mb-6">
                "Control which events notify you and how."
            </p>

            <Transition fallback=move || view! { <SettingsPageSkeleton /> }>
                {move || Suspend::new(async move {
                    match prefs_resource.await {
                        Ok(prefs) => {
                            view! { <PreferencesContent prefs=prefs /> }.into_any()
                        }
                        Err(e) => {
                            let msg = e.to_string();
                            view! {
                                <Card>
                                    <div class="p-6">
                                        <Alert variant=AlertVariant::Error>
                                            <AlertDescription>
                                                "Failed to load notification preferences: " {msg}
                                            </AlertDescription>
                                        </Alert>
                                    </div>
                                </Card>
                            }.into_any()
                        }
                    }
                })}
            </Transition>
        </div>
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Loaded content
// ─────────────────────────────────────────────────────────────────────────────

#[component]
fn PreferencesContent(prefs: NotificationPreferences) -> impl IntoView {
    view! {
        <div class="space-y-6">
            <EventTypesCard prefs=prefs.clone() />
            <AgentApiCard prefs=prefs.clone() />
            <DeliveryCard />
        </div>
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Event types card
// ─────────────────────────────────────────────────────────────────────────────

#[component]
fn EventTypesCard(prefs: NotificationPreferences) -> impl IntoView {
    view! {
        <Card>
            <CardHeader>
                <div class="flex items-center gap-2">
                    <Icon icon=phosphor_leptos::FUNNEL weight=IconWeight::Regular size="20px" attr:class="text-muted-foreground"/>
                    <CardTitle>"Event Types"</CardTitle>
                </div>
            </CardHeader>
            <CardContent>
                <p class="text-sm text-muted-foreground mb-4">
                    "Choose which types of issue events generate notifications."
                </p>
                <div class="space-y-1">
                    <PreferenceToggle
                        field="notify_status_changes"
                        label="Status changes"
                        description="When an issue's status is updated"
                        initial=prefs.notify_status_changes
                    />
                    <PreferenceToggle
                        field="notify_comments"
                        label="Comments"
                        description="When a comment is added to a watched issue"
                        initial=prefs.notify_comments
                    />
                    <PreferenceToggle
                        field="notify_assignments"
                        label="Assignments"
                        description="When an issue is assigned to someone"
                        initial=prefs.notify_assignments
                    />
                    <PreferenceToggle
                        field="notify_priority_changes"
                        label="Priority changes"
                        description="When an issue's priority is updated"
                        initial=prefs.notify_priority_changes
                    />
                    <PreferenceToggle
                        field="notify_label_changes"
                        label="Label changes"
                        description="When labels are added or removed from an issue"
                        initial=prefs.notify_label_changes
                    />
                    <PreferenceToggle
                        field="notify_due_date_changes"
                        label="Due date changes"
                        description="When an issue's due date is set, changed, or cleared"
                        initial=prefs.notify_due_date_changes
                    />
                    <PreferenceToggle
                        field="notify_estimate_changes"
                        label="Estimate changes"
                        description="When an issue's estimate is changed"
                        initial=prefs.notify_estimate_changes
                    />
                    <PreferenceToggle
                        field="notify_milestone_changes"
                        label="Milestone changes"
                        description="When an issue's milestone is set, changed, or cleared"
                        initial=prefs.notify_milestone_changes
                    />
                    <PreferenceToggle
                        field="notify_project_changes"
                        label="Project changes"
                        description="When an issue's project is set, changed, or cleared"
                        initial=prefs.notify_project_changes
                    />
                    <PreferenceToggle
                        field="notify_team_changes"
                        label="Team changes"
                        description="When an issue is moved between teams"
                        initial=prefs.notify_team_changes
                    />
                    <PreferenceToggle
                        field="notify_relation_changes"
                        label="Relation changes"
                        description="When a relation is added to an issue"
                        initial=prefs.notify_relation_changes
                    />
                </div>
            </CardContent>
        </Card>
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Agent & API card
// ─────────────────────────────────────────────────────────────────────────────

#[component]
fn AgentApiCard(prefs: NotificationPreferences) -> impl IntoView {
    view! {
        <Card>
            <CardHeader>
                <div class="flex items-center gap-2">
                    <Icon icon=phosphor_leptos::ROBOT weight=IconWeight::Regular size="20px" attr:class="text-muted-foreground"/>
                    <CardTitle>"Agent & API"</CardTitle>
                </div>
            </CardHeader>
            <CardContent>
                <p class="text-sm text-muted-foreground mb-4">
                    "By default, actions you trigger through agents or the API do not generate "
                    "self-notifications. Enable these to be notified of your own automated actions."
                </p>
                <div class="space-y-1">
                    <PreferenceToggle
                        field="notify_own_agent_actions"
                        label="Notify me of actions by agents on my behalf"
                        description="MCP agents, automation bots, and similar integrations"
                        initial=prefs.notify_own_agent_actions
                    />
                    <PreferenceToggle
                        field="notify_own_api_actions"
                        label="Notify me of actions by API integrations on my behalf"
                        description="API token-based integrations and scripts"
                        initial=prefs.notify_own_api_actions
                    />
                </div>
            </CardContent>
        </Card>
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Delivery card
// ─────────────────────────────────────────────────────────────────────────────

#[component]
fn DeliveryCard() -> impl IntoView {
    view! {
        <Card>
            <CardHeader>
                <div class="flex items-center gap-2">
                    <Icon icon=phosphor_leptos::PAPER_PLANE_TILT weight=IconWeight::Regular size="20px" attr:class="text-muted-foreground"/>
                    <CardTitle>"Delivery"</CardTitle>
                </div>
            </CardHeader>
            <CardContent>
                <p class="text-sm text-muted-foreground mb-4">
                    "How you receive notifications."
                </p>
                <div class="space-y-3">
                    // In-app — active
                    <div class="flex items-center justify-between py-2">
                        <div class="flex items-center gap-3">
                            <Icon icon=phosphor_leptos::BELL weight=IconWeight::Regular size="18px" attr:class="text-foreground"/>
                            <div>
                                <p class="text-sm font-medium text-foreground">"In-app"</p>
                                <p class="text-xs text-muted-foreground">"Notifications appear in your inbox"</p>
                            </div>
                        </div>
                        <span class="text-xs font-medium text-primary bg-primary/10 px-2 py-1 rounded">"Active"</span>
                    </div>

                    // Email — coming soon
                    <div class="flex items-center justify-between py-2 opacity-50">
                        <div class="flex items-center gap-3">
                            <Icon icon=phosphor_leptos::ENVELOPE weight=IconWeight::Regular size="18px" attr:class="text-muted-foreground"/>
                            <div>
                                <p class="text-sm font-medium text-muted-foreground">"Email"</p>
                                <p class="text-xs text-muted-foreground">"Receive notifications via email"</p>
                            </div>
                        </div>
                        <span class="text-xs font-medium text-muted-foreground bg-muted px-2 py-1 rounded">"Coming soon"</span>
                    </div>
                </div>
            </CardContent>
        </Card>
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Toggle row
// ─────────────────────────────────────────────────────────────────────────────

/// A single preference toggle row with optimistic update and error revert.
///
/// Follows the same pattern as `TransitionRuleRow` in `integrations.rs`.
#[component]
fn PreferenceToggle(
    /// The database field name (e.g. "notify_status_changes").
    field: &'static str,
    /// Display label for the toggle.
    label: &'static str,
    /// Description text shown below the label.
    description: &'static str,
    /// Initial value from loaded preferences.
    initial: bool,
) -> impl IntoView {
    let (enabled, set_enabled) = signal(initial);

    let toggle_action = Action::new(move |new_val: &bool| {
        let val = *new_val;
        async move { update_notification_preference(field.to_string(), val).await }
    });

    let on_change = Callback::new(move |new_val: bool| {
        set_enabled.set(new_val);
        toggle_action.dispatch(new_val);
    });

    // Revert optimistic update on failure.
    Effect::new(move || {
        if let Some(Err(e)) = toggle_action.value().get() {
            tracing::warn!("Failed to update notification preference: {e}");
            set_enabled.update(|v| *v = !*v);
        }
    });

    view! {
        <div class="flex items-center justify-between py-2">
            <div class="flex-1 min-w-0 pr-4">
                <p class="text-sm font-medium text-foreground">{label}</p>
                <p class="text-xs text-muted-foreground">{description}</p>
            </div>
            <Switch
                checked=Signal::derive(move || enabled.get())
                on_change=on_change
            />
        </div>
    }
}
