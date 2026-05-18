// SPDX-License-Identifier: AGPL-3.0-or-later

//! Integrations settings page — GitHub App installation self-service UI.
//!
//! Displays the current GitHub integration status and allows workspace admins
//! to connect/disconnect their GitHub organization. Three states:
//!
//! - **NotConfigured**: GitHub App not set up (self-hosted, no env vars).
//!   Shows a setup guide.
//! - **NotConnected**: App exists but workspace not connected. Shows a
//!   "Connect GitHub" button linking to the GitHub App installation flow.
//! - **Connected**: Active installation. Shows connection details, repo list,
//!   and a disconnect button with inline confirmation.

use leptos::prelude::*;
use phosphor_leptos::{Icon, IconWeight};

use crate::components::{
    Alert, AlertDescription, AlertVariant, Button, ButtonLink, ButtonSize, ButtonVariant, Card,
    CardContent, CardHeader, CardTitle, Skeleton, Spinner, Switch,
};
use crate::server_fns::github::{
    disconnect_github, get_github_integration_status, get_transition_rules,
    toggle_transition_rule, GitHubIntegrationStatus, TransitionRuleDisplay,
};

// ─────────────────────────────────────────────────────────────────────────────
// Main page
// ─────────────────────────────────────────────────────────────────────────────

#[component]
pub fn IntegrationsPage() -> impl IntoView {
    let (version, set_version) = signal(0u32);
    let status_resource = Resource::new(move || version.get(), |_| get_github_integration_status());

    view! {
        <div class="p-4 sm:p-6">
            <h2 class="text-xl font-display text-foreground mb-4">"Integrations"</h2>
            <p class="text-muted-foreground mb-6">
                "Connect external services to your workspace."
            </p>

            <Transition fallback=move || view! {
                <Card>
                    <CardHeader>
                        <Skeleton class="h-5 w-1/3"/>
                    </CardHeader>
                    <CardContent>
                        <div class="space-y-3">
                            <Skeleton class="h-4 w-2/3"/>
                            <Skeleton class="h-10 w-40"/>
                        </div>
                    </CardContent>
                </Card>
            }>
                {move || Suspend::new(async move {
                    match status_resource.await {
                        Ok(status) => match status {
                            GitHubIntegrationStatus::NotConfigured => {
                                view! { <NotConfiguredCard/> }.into_any()
                            }
                            GitHubIntegrationStatus::NotConnected { app_slug } => {
                                view! { <NotConnectedCard app_slug=app_slug/> }.into_any()
                            }
                            GitHubIntegrationStatus::Connected {
                                account_login,
                                account_type,
                                repos,
                                connected_at,
                                github_installation_id,
                            } => {
                                view! {
                                    <ConnectedCard
                                        account_login=account_login
                                        account_type=account_type
                                        repos=repos
                                        connected_at=connected_at
                                        github_installation_id=github_installation_id
                                        on_disconnected=Callback::new(move |()| {
                                            set_version.update(|v| *v += 1);
                                        })
                                    />
                                }.into_any()
                            }
                        },
                        Err(e) => {
                            let msg = e.to_string();
                            view! {
                                <Card>
                                    <div class="p-6">
                                        <Alert variant=AlertVariant::Error>
                                            <AlertDescription>
                                                "Failed to load integration status: " {msg}
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
// State 1: Not Configured
// ─────────────────────────────────────────────────────────────────────────────

/// GitHub App not configured — show setup guide for self-hosted users.
#[component]
fn NotConfiguredCard() -> impl IntoView {
    view! {
        <Card>
            <CardHeader>
                <div class="flex items-center gap-2">
                    <Icon icon=phosphor_leptos::GITHUB_LOGO weight=IconWeight::Regular size="24px" attr:class="text-muted-foreground"/>
                    <CardTitle>"GitHub Integration"</CardTitle>
                </div>
            </CardHeader>
            <CardContent>
                <div class="space-y-4">
                    <Alert variant=AlertVariant::Info>
                        <AlertDescription>
                            <div class="flex items-start gap-2">
                                <Icon icon=phosphor_leptos::WARNING weight=IconWeight::Bold size="16px" attr:class="mt-0.5 flex-shrink-0"/>
                                <span>"GitHub App is not configured for this instance."</span>
                            </div>
                        </AlertDescription>
                    </Alert>

                    <div class="text-sm text-foreground space-y-2">
                        <p class="font-medium">"To enable GitHub integration:"</p>
                        <ol class="list-decimal list-inside space-y-1.5 text-secondary-foreground ml-1">
                            <li>"Register a GitHub App for your domain"</li>
                            <li>"Set the required environment variables " <code class="text-xs bg-muted px-1 py-0.5 rounded">"GITHUB_APP_ID"</code> ", " <code class="text-xs bg-muted px-1 py-0.5 rounded">"GITHUB_APP_NAME"</code> ", and " <code class="text-xs bg-muted px-1 py-0.5 rounded">"GITHUB_PRIVATE_KEY"</code></li>
                            <li>"Restart Trakkt"</li>
                        </ol>
                    </div>

                    <p class="text-sm text-muted-foreground">
                        "See the deployment documentation for detailed setup instructions."
                    </p>
                </div>
            </CardContent>
        </Card>
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// State 2: Not Connected
// ─────────────────────────────────────────────────────────────────────────────

/// GitHub App exists but workspace not connected — show connect button.
#[component]
fn NotConnectedCard(app_slug: String) -> impl IntoView {
    let install_url = format!("https://github.com/apps/{app_slug}/installations/new");

    view! {
        <Card>
            <CardHeader>
                <div class="flex items-center gap-2">
                    <Icon icon=phosphor_leptos::GITHUB_LOGO weight=IconWeight::Regular size="24px" attr:class="text-muted-foreground"/>
                    <CardTitle>"GitHub Integration"</CardTitle>
                </div>
            </CardHeader>
            <CardContent>
                <div class="space-y-4">
                    <p class="text-sm text-secondary-foreground">
                        "Connect your GitHub organization to automatically link pull requests, commits, and branches to Trakkt issues."
                    </p>

                    <ButtonLink
                        href=install_url
                        target="_blank"
                        rel="noopener noreferrer"
                        variant=ButtonVariant::Default
                    >
                        <Icon icon=phosphor_leptos::GITHUB_LOGO weight=IconWeight::Bold size="16px"/>
                        "Connect GitHub"
                    </ButtonLink>
                </div>
            </CardContent>
        </Card>
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// State 3: Connected
// ─────────────────────────────────────────────────────────────────────────────

/// Active GitHub installation — show connection details and disconnect option.
#[component]
fn ConnectedCard(
    account_login: String,
    account_type: String,
    repos: Vec<String>,
    connected_at: String,
    github_installation_id: i64,
    on_disconnected: Callback<()>,
) -> impl IntoView {
    let (show_confirm, set_show_confirm) = signal(false);
    let (disconnect_error, set_disconnect_error) = signal(Option::<String>::None);

    let disconnect_action = Action::new(move |_: &()| async move {
        disconnect_github().await
    });

    // React to disconnect result
    Effect::new(move || {
        if let Some(result) = disconnect_action.value().get() {
            match result {
                Ok(()) => {
                    set_show_confirm.set(false);
                    set_disconnect_error.set(None);
                    on_disconnected.run(());
                }
                Err(e) => {
                    set_disconnect_error.set(Some(e.to_string()));
                }
            }
        }
    });

    let is_disconnecting = Signal::derive(move || disconnect_action.pending().get());

    // Format connected_at for display — just show the date portion
    let display_date = connected_at
        .split('T')
        .next()
        .unwrap_or(&connected_at)
        .to_string();

    let account_type_label = match account_type.as_str() {
        "Organization" => "Organization".to_string(),
        "User" => "User account".to_string(),
        other => other.to_string(),
    };

    let manage_url = format!(
        "https://github.com/settings/installations/{}",
        github_installation_id
    );

    // Empty Vec means "all repositories" — server returns empty when repository_selection="all"
    let all_repos = repos.is_empty();
    let repos_clone = repos.clone();

    view! {
        <Card>
            <CardHeader>
                <div class="flex items-center justify-between">
                    <div class="flex items-center gap-2">
                        <Icon icon=phosphor_leptos::GITHUB_LOGO weight=IconWeight::Regular size="24px" attr:class="text-muted-foreground"/>
                        <CardTitle>"GitHub Integration"</CardTitle>
                    </div>

                    // Disconnect button / inline confirmation
                    <div class="flex items-center gap-2">
                        {move || {
                            if show_confirm.get() {
                                view! {
                                    <div class="flex items-center gap-2">
                                        <span class="text-xs text-muted-foreground">
                                            "This will stop all GitHub syncing."
                                        </span>
                                        <Button
                                            variant=ButtonVariant::Outline
                                            size=ButtonSize::Sm
                                            on:click=move |_| set_show_confirm.set(false)
                                            disabled=MaybeProp::from(is_disconnecting)
                                        >
                                            "Cancel"
                                        </Button>
                                        <Button
                                            variant=ButtonVariant::Destructive
                                            size=ButtonSize::Sm
                                            disabled=MaybeProp::from(is_disconnecting)
                                            on:click=move |_| { disconnect_action.dispatch(()); }
                                        >
                                            {move || {
                                                if is_disconnecting.get() {
                                                    view! {
                                                        <Spinner class="text-white"/>
                                                        "Disconnecting..."
                                                    }.into_any()
                                                } else {
                                                    view! { "Yes, disconnect" }.into_any()
                                                }
                                            }}
                                        </Button>
                                    </div>
                                }.into_any()
                            } else {
                                view! {
                                    <Button
                                        variant=ButtonVariant::Outline
                                        size=ButtonSize::Sm
                                        on:click=move |_| {
                                            set_disconnect_error.set(None);
                                            set_show_confirm.set(true);
                                        }
                                    >
                                        "Disconnect"
                                    </Button>
                                }.into_any()
                            }
                        }}
                    </div>
                </div>
            </CardHeader>
            <CardContent>
                <div class="space-y-4">
                    // Disconnect error
                    {move || disconnect_error.get().map(|e| view! {
                        <Alert variant=AlertVariant::Error>
                            <AlertDescription>{e}</AlertDescription>
                        </Alert>
                    })}

                    // Connection details
                    <div class="flex items-center gap-2 text-sm">
                        <Icon icon=phosphor_leptos::CHECK_CIRCLE weight=IconWeight::Fill size="16px" attr:class="text-success-foreground"/>
                        <span class="text-foreground font-medium">
                            "Connected to: "
                            <span class="font-mono text-xs">"@"{account_login.clone()}</span>
                        </span>
                        <span class="text-muted-foreground">
                            "("{account_type_label}")"
                        </span>
                    </div>

                    <div class="text-sm text-muted-foreground">
                        "Installed: " {display_date}
                    </div>

                    // Repository list
                    <div class="space-y-2">
                        <p class="text-sm font-medium text-foreground">"Repositories:"</p>
                        {if all_repos {
                            view! {
                                <p class="text-sm text-secondary-foreground ml-2">
                                    "All repositories"
                                </p>
                            }.into_any()
                        } else {
                            let items = repos_clone.into_iter().map(|repo| {
                                view! {
                                    <li class="text-sm text-secondary-foreground font-mono text-xs">
                                        {repo}
                                    </li>
                                }
                            }).collect_view();
                            view! {
                                <ul class="list-disc list-inside ml-2 space-y-0.5">
                                    {items}
                                </ul>
                            }.into_any()
                        }}
                    </div>

                    // Manage link
                    <a
                        href=manage_url
                        target="_blank"
                        rel="noopener noreferrer"
                        class="inline-flex items-center gap-1.5 text-sm text-primary hover:text-primary/80 transition-colors duration-200 rounded focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
                    >
                        "Manage repositories on GitHub"
                        <Icon icon=phosphor_leptos::ARROW_SQUARE_OUT weight=IconWeight::Light size="14px"/>
                    </a>

                    // Status transition rules
                    <div class="border-t border-border pt-3 mt-3">
                        <TransitionRulesSection/>
                    </div>
                </div>
            </CardContent>
        </Card>
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Transition rules section
// ─────────────────────────────────────────────────────────────────────────────

/// Displays all transition rules for the workspace with toggle switches.
#[component]
fn TransitionRulesSection() -> impl IntoView {
    let rules_resource = Resource::new(|| (), |_| get_transition_rules());

    view! {
        <div class="space-y-3">
            <div class="flex items-center justify-between">
                <p class="text-sm font-medium text-foreground">"Status Transitions"</p>
            </div>
            <Transition fallback=move || view! {
                <div class="space-y-2">
                    <Skeleton class="h-4 w-full"/>
                    <Skeleton class="h-4 w-full"/>
                    <Skeleton class="h-4 w-full"/>
                </div>
            }>
                {move || Suspend::new(async move {
                    match rules_resource.await {
                        Ok(rules) => {
                            if rules.is_empty() {
                                view! {
                                    <p class="text-xs text-muted-foreground">
                                        "No transition rules configured."
                                    </p>
                                }.into_any()
                            } else {
                                view! {
                                    <div class="space-y-2">
                                        {rules.into_iter().map(|rule| {
                                            view! { <TransitionRuleRow rule=rule/> }
                                        }).collect_view()}
                                    </div>
                                    <p class="text-xs text-muted-foreground mt-3">
                                        "Rules with "
                                        <span class="font-medium">"close intent"</span>
                                        " only fire when the PR description contains "
                                        <span class="font-mono text-[11px]">"\"Closes TRA-N\""</span>
                                        " or similar keywords."
                                    </p>
                                }.into_any()
                            }
                        }
                        Err(e) => {
                            view! {
                                <Alert variant=AlertVariant::Error>
                                    <AlertDescription>
                                        {format!("Failed to load transition rules: {e}")}
                                    </AlertDescription>
                                </Alert>
                            }.into_any()
                        }
                    }
                })}
            </Transition>
        </div>
    }
}

/// A single transition rule row with a description and toggle switch.
#[component]
fn TransitionRuleRow(rule: TransitionRuleDisplay) -> impl IntoView {
    let rule_id = rule.rule_id.clone();
    let (enabled, set_enabled) = signal(rule.enabled);

    let description = format_rule_description(&rule.trigger_event, rule.close_intent_required);
    let target = format_target_status(&rule.target_status_category);

    let toggle_action = Action::new(move |new_val: &bool| {
        let rid = rule_id.clone();
        let val = *new_val;
        async move { toggle_transition_rule(rid, val).await }
    });

    let on_change = Callback::new(move |new_val: bool| {
        set_enabled.set(new_val);
        toggle_action.dispatch(new_val);
    });

    // Log toggle errors and revert the optimistic update on failure
    Effect::new(move || {
        if let Some(Err(e)) = toggle_action.value().get() {
            tracing::warn!("Failed to toggle transition rule: {e}");
            set_enabled.update(|v| *v = !*v);
        }
    });

    view! {
        <div class="flex items-center justify-between py-1.5">
            <div class="flex-1 min-w-0">
                <p class="text-sm text-foreground">
                    {description}
                    " \u{2192} "
                    <span class="font-medium">{target}</span>
                </p>
            </div>
            <Switch
                checked=Signal::derive(move || enabled.get())
                on_change=on_change
            />
        </div>
    }
}

fn format_rule_description(trigger_event: &str, close_intent: bool) -> String {
    match (trigger_event, close_intent) {
        ("pr_opened", _) => "When a PR is opened".to_string(),
        ("pr_merged", true) => "When a PR is merged (with close intent)".to_string(),
        ("pr_merged", false) => "When any PR is merged".to_string(),
        ("pr_closed", true) => "When a PR is closed without merge (with close intent)".to_string(),
        ("pr_closed", false) => "When any PR is closed without merge".to_string(),
        (other, _) => other.to_string(),
    }
}

fn format_target_status(category: &str) -> &'static str {
    match category {
        "started" => "In Progress",
        "completed" => "Done",
        "cancelled" => "Cancelled",
        "backlog" => "Backlog",
        "unstarted" => "Todo",
        _ => "Unknown",
    }
}
