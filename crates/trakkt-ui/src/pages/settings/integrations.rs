// SPDX-License-Identifier: AGPL-3.0-or-later

//! Integrations settings page — GitHub App installation self-service UI.
//!
//! Lists visible connections with per-account controls and authoritative
//! repository access. Automation rules remain shared across the workspace.

use leptos::prelude::*;
use phosphor_leptos::{Icon, IconWeight};

use crate::components::{
    Alert, AlertDescription, AlertVariant, Button, ButtonSize, ButtonVariant, Card, CardContent,
    CardHeader, CardTitle, Skeleton, Spinner, Switch,
};
use crate::server_fns::github::{
    GitHubConnectionDisplay, GitHubIntegrationStatus, TransitionRuleDisplay, disconnect_github,
    get_github_integration_status, get_transition_rules, start_github_connection,
    remove_github, toggle_transition_rule,
};

// ─────────────────────────────────────────────────────────────────────────────
// Main page
// ─────────────────────────────────────────────────────────────────────────────

#[component]
pub fn IntegrationsPage() -> impl IntoView {
    let (version, set_version) = signal(0u32);
    let settings_version = use_context::<crate::cache::store::SyncStore>()
        .map(|store| store.integration_settings_version());
    let status_resource = Resource::new(
        move || {
            (
                version.get(),
                settings_version.as_ref().map(|v| v.get()).unwrap_or(0),
            )
        },
        |_| get_github_integration_status(),
    );

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
                            GitHubIntegrationStatus::Connections { connections } => {
                                view! {
                                    <div class="space-y-4">
                                        <NotConnectedCard/>
                                        {connections.into_iter().map(|connection| view! {
                                            <ConnectedCard connection=connection on_disconnected=Callback::new(move |()| set_version.update(|v| *v += 1))/>
                                        }).collect_view()}
                                        <Card><CardContent><TransitionRulesSection/></CardContent></Card>
                                    </div>
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

/// Adding an account stays available regardless of the existing connections.
#[component]
fn NotConnectedCard() -> impl IntoView {
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
                        "Connect a GitHub personal account or organization to automatically link pull requests, commits, and branches to Trakkt issues."
                    </p>

                    <GitHubConnectButton label="Add account"/>
                </div>
            </CardContent>
        </Card>
    }
}

/// Add and per-account reconnect both begin with a server-created admin state.
#[component]
fn GitHubConnectButton(
    label: &'static str,
    #[prop(optional)] connection_id: Option<String>,
    #[prop(default = false)] reinstall: bool,
) -> impl IntoView {
    let action = Action::new(move |_: &()| {
        let connection_id = connection_id.clone();
        async move { start_github_connection(connection_id, reinstall).await }
    });
    let (error, set_error) = signal(Option::<String>::None);
    Effect::new(move || {
        if let Some(result) = action.value().get() {
            match result {
                Ok(url) => {
                    #[cfg(target_arch = "wasm32")]
                    if web_sys::window()
                        .is_none_or(|window| window.location().assign(&url).is_err())
                    {
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

/// A retained connection, including inactive states that require authorization.
#[component]
fn ConnectedCard(
    connection: GitHubConnectionDisplay,
    on_disconnected: Callback<()>,
) -> impl IntoView {
    let GitHubConnectionDisplay {
        connection_id,
        account_login,
        account_type,
        repos,
        repository_selection,
        connected_at,
        github_installation_id,
        active,
        disconnected,
        suspended,
        uninstalled,
        verified,
        scope_pending,
    } = connection;
    let card_id = connection_id.clone();
    let disconnect_id = connection_id.clone();
    let (show_confirm, set_show_confirm) = signal(false);
    let (disconnect_error, set_disconnect_error) = signal(Option::<String>::None);

    let disconnect_action = Action::new(move |_: &()| {
        let id = disconnect_id.clone();
        async move {
            if active {
                disconnect_github(id).await
            } else {
                remove_github(id).await
            }
        }
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

    let manage_url = if account_type == "Organization" {
        format!(
            "https://github.com/organizations/{account_login}/settings/installations/{github_installation_id}"
        )
    } else {
        format!("https://github.com/settings/installations/{github_installation_id}")
    };

    let all_repos = repository_selection == "all";
    let lifecycle = [
        disconnected.then_some("Disconnected"),
        suspended.then_some("Suspended by GitHub"),
        uninstalled.then_some("Uninstalled on GitHub"),
        (!verified).then_some("Authorization required"),
        scope_pending
            .then_some("Repository access needs refresh. Reconnect to refresh permissions."),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>()
    .join(" · ");
    let lifecycle = if active {
        "Connected".to_string()
    } else {
        lifecycle
    };
    let action_label = if active { "Disconnect" } else { "Remove integration" };
    let confirmation = if active {
        "Stop syncing this account? History is retained and the GitHub App stays installed."
    } else {
        "Remove this integration from settings? Ticket and PR history is retained. The GitHub App stays installed."
    };
    let repos_clone = repos.clone();

    view! {
        <div data-github-connection=card_id>
        <Card>
            <CardHeader>
                <div class="flex flex-wrap items-center justify-between gap-2">
                    <div class="flex items-center gap-2">
                        <Icon icon=phosphor_leptos::GITHUB_LOGO weight=IconWeight::Regular size="24px" attr:class="text-muted-foreground"/>
                        <CardTitle>"GitHub Integration"</CardTitle>
                    </div>

                    // Disconnect button / inline confirmation
                    <div class="flex items-center gap-2">
                        {move || {
                            if show_confirm.get() {
                                view! {
                                    <div class="flex flex-wrap items-center gap-2">
                                        <span class="text-xs text-muted-foreground">
                                            {confirmation}
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
                                                        {if active { "Disconnecting..." } else { "Removing..." }}
                                                    }.into_any()
                                                } else {
                                                    view! { {if active { "Yes, disconnect" } else { "Yes, remove integration" }} }.into_any()
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
                                        {action_label}
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

                    {(!uninstalled).then(|| view! {
                        <GitHubConnectButton label="Reconnect GitHub" connection_id=connection_id.clone()/>
                    })}
                    <GitHubConnectButton label="Reinstall GitHub App" connection_id=connection_id.clone() reinstall=true/>

                    // Connection details
                    <div class="flex items-center gap-2 text-sm">
                        <Icon icon=if active { phosphor_leptos::CHECK_CIRCLE } else { phosphor_leptos::X_CIRCLE } weight=IconWeight::Regular size="16px" attr:class="text-muted-foreground"/>
                        <span class="text-foreground font-medium">
                            "Account: "
                            <span class="font-mono text-xs">"@"{account_login.clone()}</span>
                        </span>
                        <span class="text-muted-foreground">
                            "("{account_type_label}")"
                        </span>
                    </div>

                    <p class="text-sm text-secondary-foreground">{lifecycle}</p>
                    <div class="text-sm text-muted-foreground">
                        "Installed: " {display_date}
                    </div>

                    // Repository list
                    <div class="space-y-2">
                        <p class="text-sm font-medium text-foreground">"Repositories:"</p>
                        {if scope_pending {
                            view! { <p class="text-sm text-secondary-foreground ml-2">"Repository access needs refresh"</p> }.into_any()
                        } else if all_repos {
                            view! {
                                <p class="text-sm text-secondary-foreground ml-2">
                                    "All repositories"
                                </p>
                            }.into_any()
                        } else if repos_clone.is_empty() {
                            view! { <p class="text-sm text-secondary-foreground ml-2">"No repositories selected"</p> }.into_any()
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


                </div>
            </CardContent>
        </Card>
        </div>
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Transition rules section
// ─────────────────────────────────────────────────────────────────────────────

/// Displays all transition rules for the workspace with toggle switches.
#[component]
fn TransitionRulesSection() -> impl IntoView {
    let settings_version = use_context::<crate::cache::store::SyncStore>()
        .map(|store| store.integration_settings_version());
    let rules_resource = Resource::new(
        move || settings_version.as_ref().map(|v| v.get()).unwrap_or(0),
        |_| get_transition_rules(),
    );

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
mod tests {
    use super::*;

    fn boot_executor() {
        static EXECUTOR: std::sync::Once = std::sync::Once::new();
        EXECUTOR.call_once(|| {
            any_spawner::Executor::init_tokio()
                .expect("native connection rendering initializes the Tokio executor");
        });
    }

    fn connection(id: &str, repository_selection: &str) -> GitHubConnectionDisplay {
        GitHubConnectionDisplay {
            connection_id: id.into(),
            account_login: id.into(),
            account_type: "User".into(),
            repos: Vec::new(),
            repository_selection: repository_selection.into(),
            connected_at: "2026-10-10T00:00:00Z".into(),
            github_installation_id: 123,
            active: false,
            disconnected: true,
            suspended: false,
            uninstalled: false,
            verified: true,
            scope_pending: false,
        }
    }

    #[tokio::test]
    async fn retained_connections_distinguish_all_and_selected_empty_and_offer_reconnect() {
        boot_executor();
        tokio::task::LocalSet::new().run_until(async {
            let owner = Owner::new();
            let mut pending = connection("pending", "selected");
            pending.disconnected = false;
            pending.scope_pending = true;
            let html = owner.with(|| view! {
                <NotConnectedCard/>
                <ConnectedCard connection=connection("personal", "all") on_disconnected=Callback::new(|()| {})/>
                <ConnectedCard connection=connection("organization", "selected") on_disconnected=Callback::new(|()| {})/>
                <ConnectedCard connection=pending on_disconnected=Callback::new(|()| {})/>
            }.to_html());
            assert_eq!(html.matches("data-github-connection=").count(), 3);
            assert_eq!(html.matches("All repositories").count(), 1);
            assert_eq!(html.matches("No repositories selected").count(), 1);
            assert_eq!(html.matches("Reconnect GitHub").count(), 3);
            assert_eq!(html.matches("Reinstall GitHub App").count(), 3);
            assert!(html.contains("Add account"));
            assert!(html.contains("Repository access needs refresh. Reconnect to refresh permissions."));
            assert_eq!(html.matches("Disconnected").count(), 2);
            assert_eq!(html.matches("Remove integration").count(), 3);
            assert!(!html.contains("Status Transitions"), "rules belong to the workspace, not the account cards");
        }).await;
    }

    #[tokio::test]
    async fn uninstalled_card_offers_removal_and_reinstall_without_dead_reconnect() {
        boot_executor();
        tokio::task::LocalSet::new().run_until(async {
            let owner = Owner::new();
            let mut old = connection("old", "all");
            old.uninstalled = true;
            let html = owner.with(|| view! {
                <ConnectedCard connection=old on_disconnected=Callback::new(|()| {})/>
            }.to_html());
            assert!(html.contains("Remove integration"));
            assert!(html.contains("Reinstall GitHub App"));
            assert!(!html.contains("Reconnect GitHub"));
        }).await;
    }
}
