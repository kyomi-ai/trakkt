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
    Alert, AlertDescription, AlertVariant, Button, ButtonSize, ButtonVariant, Card,
    CardContent, CardHeader, CardTitle, Skeleton, Spinner, Switch,
};
use crate::server_fns::github::{
    GitHubIntegrationStatus, TransitionRuleDisplay, disconnect_github,
    get_github_integration_status, get_transition_rules, start_github_connection,
    toggle_transition_rule,
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
                            GitHubIntegrationStatus::AuthorizationNotConfigured => {
                                view! { <Alert variant=AlertVariant::Warning><AlertDescription>"GitHub user authorization is not configured. Ask your server administrator to set GITHUB_OAUTH_CLIENT_ID, GITHUB_OAUTH_CLIENT_SECRET and GITHUB_OAUTH_CALLBACK_URL, and register /integrations/github/oauth/callback as the GitHub App Callback URL. Existing automation remains available."</AlertDescription></Alert> }.into_any()
                            }
                            GitHubIntegrationStatus::NotConnected { retained_installation, .. } => {
                                view! { <NotConnectedCard retained_installation=retained_installation/> }.into_any()
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
                            <li>"Set " <code class="text-xs bg-muted px-1 py-0.5 rounded">"GITHUB_APP_ID"</code> ", " <code class="text-xs bg-muted px-1 py-0.5 rounded">"GITHUB_APP_NAME"</code> ", " <code class="text-xs bg-muted px-1 py-0.5 rounded">"GITHUB_APP_PRIVATE_KEY_PATH"</code> ", and " <code class="text-xs bg-muted px-1 py-0.5 rounded">"GITHUB_WEBHOOK_SECRET"</code></li>
                            <li>"Restart Trakkt"</li>
                            <li>"Connect GitHub from this page and select the repositories to integrate"</li>
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
fn NotConnectedCard(retained_installation: bool) -> impl IntoView {
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

                    <GitHubConnectButton label=if retained_installation { "Reconnect GitHub" } else { "Connect GitHub" }/>
                    {retained_installation.then(|| view! { <GitHubConnectButton label="Reinstall GitHub App" reinstall=true/> })}
                </div>
            </CardContent>
        </Card>
    }
}

/// Both connect and reconnect must begin with a server-created admin state.
#[component]
fn GitHubConnectButton(
    label: &'static str,
    #[prop(default = false)] reinstall: bool,
) -> impl IntoView {
    let action = Action::new(move |_: &()| async move { start_github_connection(reinstall).await });
    let (error, set_error) = signal(Option::<String>::None);
    Effect::new(move || {
        if let Some(result) = action.value().get() {
            match result {
                Ok(url) => {
                    #[cfg(target_arch = "wasm32")]
                    if web_sys::window().is_none_or(|window| window.location().assign(&url).is_err()) {
                        set_error.set(Some("Could not open GitHub authorization".into()));
                    }
                    #[cfg(not(target_arch = "wasm32"))]
                    drop(url);
                }
                Err(e) => set_error.set(Some(e.to_string())),
            }
        }
    });
    view! {
        <Button disabled=Signal::derive(move || action.pending().get()) on:click=move |_| { set_error.set(None); action.dispatch(()); }>{label}</Button>
        {move || error.get().map(|message| view! { <Alert variant=AlertVariant::Error><AlertDescription>{message}</AlertDescription></Alert> })}
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

    let disconnect_action = Action::new(move |_: &()| async move { disconnect_github().await });

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

                    <GitHubConnectButton label="Reconnect GitHub"/>
                    <GitHubConnectButton label="Reinstall GitHub App" reinstall=true/>

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

#[cfg(all(test, feature = "ssr"))]
mod authorization_ui_tests {
    use super::*;

    #[tokio::test]
    async fn disconnected_connection_offers_reinstall_without_hiding_reconnect() {
        static EXECUTOR: std::sync::Once = std::sync::Once::new();
        EXECUTOR.call_once(|| {
            any_spawner::Executor::init_tokio().expect("native UI tests initialize the Tokio executor once");
        });
        tokio::task::LocalSet::new().run_until(async {
        let owner = Owner::new();
        let retained = owner.with(|| view! { <NotConnectedCard retained_installation=true/> }.to_html());
        assert!(retained.contains("Reconnect GitHub"));
        assert!(retained.contains("Reinstall GitHub App"), "a disconnected account whose old installation was deleted needs the setup route");
        let fresh = owner.with(|| view! { <NotConnectedCard retained_installation=false/> }.to_html());
        assert!(fresh.contains("Connect GitHub"));
        assert!(!fresh.contains("Reinstall GitHub App"));
        }).await;
    }
}
