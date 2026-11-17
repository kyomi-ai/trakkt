// SPDX-License-Identifier: AGPL-3.0-or-later

//! Integrations settings page — GitHub App installation self-service UI.
//!
//! Lists visible connections with per-account controls and authoritative
//! repository access. Automation rules are shared across the workspace; a team
//! can override each step, and the PR linkback comment can be turned off.

use leptos::prelude::*;
use phosphor_leptos::{Icon, IconWeight};
use trakkt_types::enums::{GitHubAutomationStep, StatusCategory};

use crate::components::{
    Alert, AlertDescription, AlertVariant, Button, ButtonSize, ButtonVariant, Card, CardContent,
    CardHeader, CardTitle, Select, Skeleton, Spinner, Switch,
};
use crate::server_fns::github::{
    AutomationSettingsDisplay, GitHubConnectionDisplay, GitHubIntegrationStatus,
    TeamAutomationDisplay, TeamStepSetting, TransitionRuleDisplay, clear_team_automation_rule,
    disconnect_github, get_automation_settings, get_github_integration_status,
    get_transition_rules, remove_github, set_github_linkback, set_team_automation_rule,
    start_github_connection, toggle_transition_rule,
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
                                        <Card><CardContent><TeamAutomationSection/></CardContent></Card>
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

                    <p
                        data-github-status=if active { "connected" } else { "inactive" }
                        class=if active { "text-sm font-medium text-success-foreground" } else { "text-sm font-medium text-error-foreground" }
                    >
                        {lifecycle}
                    </p>

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
                    {(!active).then(|| view! {
                        <Alert variant=AlertVariant::Warning>
                            <AlertDescription>
                                "Syncing is stopped. New pull requests and commits from this account are not being synced to Trakkt. "
                                {if uninstalled {
                                    "Reinstall the GitHub App to connect again."
                                } else if suspended {
                                    "Unsuspend the GitHub App on GitHub, then reconnect."
                                } else {
                                    "Reconnect this account to resume syncing."
                                }}
                            </AlertDescription>
                        </Alert>
                    })}
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

                    <div class="text-sm text-muted-foreground">
                        "Installed: " {display_date}
                    </div>

                    // Repository list
                    <div class="space-y-2">
                        <p class="text-sm font-medium text-foreground">
                            {if active { "Repositories:" } else { "Last configured repository access:" }}
                        </p>
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
                                        "A PR links to an issue through its branch name, its title, or a magic word before the issue key ("
                                        <span class="font-mono text-[11px]">"Fixes TRA-N"</span>
                                        ", "
                                        <span class="font-mono text-[11px]">"Refs TRA-N"</span>
                                        ", "
                                        <span class="font-mono text-[11px]">"Related to TRA-N"</span>
                                        "). Only closing words complete the issue on merge. "
                                        <span class="font-mono text-[11px]">"Skip TRA-N"</span>
                                        " prevents linking. A key mentioned in the description never changes status. A draft PR does nothing unless a team gives the draft step a rule, and issues are never moved backwards or out of Done or Cancelled."
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


// ─────────────────────────────────────────────────────────────────────────────
// Team automation section
// ─────────────────────────────────────────────────────────────────────────────

/// What a team's step is set to, as the select offers it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TeamStepChoice {
    /// No override: the workspace rule applies.
    WorkspaceDefault,
    /// The step does nothing for this team.
    NoAction,
    /// Move the issue to this status category.
    Target(StatusCategory),
}

impl TeamStepChoice {
    fn value(self) -> String {
        match self {
            Self::WorkspaceDefault => "default".to_string(),
            Self::NoAction => "none".to_string(),
            Self::Target(category) => category.as_str().to_string(),
        }
    }

    fn parse(value: &str) -> Option<Self> {
        match value {
            "default" => Some(Self::WorkspaceDefault),
            "none" => Some(Self::NoAction),
            other => StatusCategory::parse(other).map(Self::Target),
        }
    }

    fn from_override(setting: Option<&TeamStepSetting>) -> Self {
        match setting {
            None => Self::WorkspaceDefault,
            Some(TeamStepSetting { target: None, .. }) => Self::NoAction,
            Some(TeamStepSetting {
                target: Some(category),
                ..
            }) => Self::Target(*category),
        }
    }
}

/// What a team's step can be set to.
fn team_step_options() -> Vec<(String, String)> {
    let mut options = vec![
        (TeamStepChoice::WorkspaceDefault, "Workspace default"),
        (TeamStepChoice::NoAction, "No action"),
    ];
    options.extend(
        StatusCategory::all()
            .iter()
            .map(|c| (TeamStepChoice::Target(*c), format_target_status(c.as_str()))),
    );
    options
        .into_iter()
        .map(|(choice, label)| (choice.value(), label.to_string()))
        .collect()
}

/// The PR linkback comment switch and each team's overrides of the shared rules.
#[component]
fn TeamAutomationSection() -> impl IntoView {
    let (version, set_version) = signal(0u32);
    let (save_error, set_save_error) = signal(None::<String>);
    let settings_version = use_context::<crate::cache::store::SyncStore>()
        .map(|store| store.integration_settings_version());
    let settings = Resource::new(
        move || {
            (
                version.get(),
                settings_version.as_ref().map(|v| v.get()).unwrap_or(0),
            )
        },
        |_| get_automation_settings(),
    );

    view! {
        <div class="space-y-3" data-github-team-automation>
            <p class="text-sm font-medium text-foreground">"Team automation"</p>
            {move || save_error.get().map(|message| view! {
                <Alert variant=AlertVariant::Error>
                    <AlertDescription>{message}</AlertDescription>
                </Alert>
            })}
            <Transition fallback=move || view! { <Skeleton class="h-4 w-full"/> }>
                {move || Suspend::new(async move {
                    match settings.await {
                        Ok(settings) => view! {
                            <TeamAutomationBody
                                settings=settings
                                // Reloading rebuilds every select from what the
                                // server holds, so a failed save never leaves a
                                // choice on screen that was not saved.
                                on_changed=Callback::new(move |()| set_version.update(|v| *v += 1))
                                on_error=Callback::new(move |message: Option<String>| set_save_error.set(message))
                            />
                        }.into_any(),
                        Err(e) => view! {
                            <Alert variant=AlertVariant::Error>
                                <AlertDescription>
                                    {format!("Failed to load team automation: {e}")}
                                </AlertDescription>
                            </Alert>
                        }.into_any(),
                    }
                })}
            </Transition>
        </div>
    }
}

#[component]
fn TeamAutomationBody(
    settings: AutomationSettingsDisplay,
    on_changed: Callback<()>,
    on_error: Callback<Option<String>>,
) -> impl IntoView {
    let (linkback, set_linkback) = signal(settings.linkback_enabled);
    let linkback_action = Action::new(move |enabled: &bool| {
        let enabled = *enabled;
        async move { set_github_linkback(enabled).await }
    });
    Effect::new(move || {
        if let Some(Err(e)) = linkback_action.value().get() {
            tracing::warn!("Failed to change linkback comment setting: {e}");
            set_linkback.update(|v| *v = !*v);
            on_error.run(Some(format!("Could not save the PR comment setting: {e}")));
        }
    });
    let on_linkback = Callback::new(move |enabled: bool| {
        on_error.run(None);
        set_linkback.set(enabled);
        linkback_action.dispatch(enabled);
    });

    view! {
        <div class="flex items-center justify-between py-1.5">
            <p class="text-sm text-foreground">"Comment on the PR when automation changes an issue's status"</p>
            <Switch checked=Signal::derive(move || linkback.get()) on_change=on_linkback/>
        </div>
        <p class="text-xs text-muted-foreground">
            "Each team can replace a shared rule above for its own issues, or turn a step off."
        </p>
        <div class="space-y-4">
            {settings.teams.into_iter().map(|team| {
                view! { <TeamAutomationRow team=team on_changed=on_changed on_error=on_error/> }
            }).collect_view()}
        </div>
    }
}

#[component]
fn TeamAutomationRow(
    team: TeamAutomationDisplay,
    on_changed: Callback<()>,
    on_error: Callback<Option<String>>,
) -> impl IntoView {
    let team_id = team.team_id.clone();
    let save = Action::new(move |(step, choice): &(GitHubAutomationStep, TeamStepChoice)| {
        let team_id = team_id.clone();
        let (step, choice) = (*step, *choice);
        async move {
            let result = match choice {
                TeamStepChoice::WorkspaceDefault => clear_team_automation_rule(team_id, step).await,
                TeamStepChoice::NoAction => set_team_automation_rule(team_id, step, None).await,
                TeamStepChoice::Target(category) => {
                    set_team_automation_rule(team_id, step, Some(category)).await
                }
            };
            match &result {
                Ok(()) => on_error.run(None),
                Err(e) => on_error.run(Some(format!("Could not save the team rule: {e}"))),
            }
            // Success or not, show what the server now holds.
            on_changed.run(());
            result
        }
    });

    view! {
        <div class="space-y-1" data-github-team=team.key.clone()>
            <p class="text-sm font-medium text-foreground">{format!("{} ({})", team.name, team.key)}</p>
            {GitHubAutomationStep::all().iter().map(|step| {
                let step = *step;
                let current = TeamStepChoice::from_override(
                    team.overrides.iter().find(|o| o.step == step),
                )
                .value();
                view! {
                    <div class="flex items-center justify-between gap-3 py-0.5">
                        <span class="text-sm text-muted-foreground">{step.label()}</span>
                        <Select
                            value=Signal::derive(move || current.clone())
                            options=Signal::derive(team_step_options)
                            on_change=Callback::new(move |value: String| {
                                if let Some(choice) = TeamStepChoice::parse(&value) {
                                    save.dispatch((step, choice));
                                }
                            })
                        />
                    </div>
                }
            }).collect_view()}
        </div>
    }
}

/// A single transition rule row with a description and toggle switch.
#[component]
fn TransitionRuleRow(rule: TransitionRuleDisplay) -> impl IntoView {
    let rule_id = rule.rule_id.clone();
    let (enabled, set_enabled) = signal(rule.enabled);

    let description = format_rule_description(rule.step);
    let target = format_target_status(rule.target.as_str());

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

fn format_rule_description(step: GitHubAutomationStep) -> String {
    match step {
        GitHubAutomationStep::PrDraft => "When a PR is opened as a draft".to_string(),
        GitHubAutomationStep::PrOpened => {
            "When a PR linked to the issue is opened or marked ready for review".to_string()
        }
        GitHubAutomationStep::PrMerged => {
            "When the last open PR linked to the issue is merged".to_string()
        }
        GitHubAutomationStep::PrClosed => {
            "When a PR that closes the issue is closed without merge, with no other PR open or merged on it".to_string()
        }
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
    async fn connected_and_inactive_cards_distinguish_live_access_from_saved_configuration() {
        boot_executor();
        tokio::task::LocalSet::new().run_until(async {
            for state in ["connected", "disconnected", "suspended", "unverified", "pending", "uninstalled"] {
                let owner = Owner::new();
                let mut row = connection(state, "all");
                row.active = state == "connected";
                row.disconnected = state == "disconnected";
                row.suspended = state == "suspended";
                row.verified = state != "unverified";
                row.scope_pending = state == "pending";
                row.uninstalled = state == "uninstalled";
                let html = owner.with(|| view! {
                    <ConnectedCard connection=row on_disconnected=Callback::new(|()| {})/>
                }.to_html());
                if state == "connected" {
                    assert!(html.contains("data-github-status=\"connected\""));
                    assert!(!html.contains("Syncing is stopped"));
                    assert!(!html.contains("Last configured repository access:"));
                } else {
                    assert!(html.contains("data-github-status=\"inactive\""), "{state}");
                    assert!(html.contains("Syncing is stopped"), "{state}");
                    assert!(html.contains("Last configured repository access:"), "{state}");
                    assert!(!html.contains("data-github-status=\"connected\""), "{state}");
                    if state == "suspended" {
                        assert!(html.contains("Unsuspend the GitHub App on GitHub, then reconnect."));
                        assert!(!html.contains("Reconnect this account to resume syncing."));
                    }
                }
            }
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
