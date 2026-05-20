// SPDX-License-Identifier: AGPL-3.0-or-later

//! Workspace settings page — admin-only workspace configuration.
//!
//! Replaces `apps/frontend/src/components/settings/WorkspaceSettings.jsx`.
//! All data fetching uses server functions instead of REST API calls.

use leptos::prelude::*;

use trakkt_types::models::Team;

use crate::components::{
    ActionStatus, Badge, BadgeVariant, Button, ButtonSize, ButtonVariant, Card, CardContent,
    CardDescription, CardHeader, CardTitle, EmptyState, Skeleton, TeamCreationModal, TeamIcon,
    INPUT_CLASS,
};
use crate::server_fns::teams::{
    get_workspace_default_team_id, list_all_teams, set_workspace_default_team,
};
use crate::server_fns::workspace::*;
use crate::types::WorkspaceSettingsData;

// ─────────────────────────────────────────────────────────────────────────────
// Main page
// ─────────────────────────────────────────────────────────────────────────────

#[component]
pub fn WorkspacePage() -> impl IntoView {
    let settings = Resource::new(|| (), |_| get_workspace_settings());

    view! {
        <div class="p-4 sm:p-6">
            <h2 class="text-xl font-display text-foreground mb-4">"Workspace Settings"</h2>
            <p class="text-muted-foreground mb-6">
                "Configure workspace-wide preferences (admin only)."
            </p>

            <Transition fallback=move || view! {
                <div class="space-y-6">
                    <Card>
                        <CardHeader>
                            <Skeleton class="h-5 w-1/3"/>
                            <Skeleton class="h-4 w-2/3 mt-1"/>
                        </CardHeader>
                        <CardContent>
                            <Skeleton class="h-10 w-full"/>
                        </CardContent>
                    </Card>
                    <Card>
                        <CardHeader>
                            <Skeleton class="h-5 w-1/3"/>
                            <Skeleton class="h-4 w-2/3 mt-1"/>
                        </CardHeader>
                        <CardContent>
                            <Skeleton class="h-20 w-full"/>
                        </CardContent>
                    </Card>
                </div>
            }>
                {move || Suspend::new(async move {
                    match settings.await {
                        Ok(data) => {
                            view! {
                                <div class="space-y-6">
                                    <WorkspaceNameCard data=data/>
                                    <TeamsSection/>
                                </div>
                            }.into_any()
                        },
                        Err(e) => {
                            let msg = e.to_string();
                            view! {
                                <Card>
                                    <div class="p-6">
                                        <p class="text-error-foreground">"Failed to load workspace settings: " {msg}</p>
                                    </div>
                                </Card>
                            }.into_any()
                        },
                    }
                })}
            </Transition>
        </div>
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Workspace Name Card
// ─────────────────────────────────────────────────────────────────────────────

#[component]
fn WorkspaceNameCard(data: WorkspaceSettingsData) -> impl IntoView {
    let (name, set_name) = signal(data.workspace_name.clone());
    let save_action = Action::new(|name: &String| {
        let name = name.clone();
        async move { update_workspace_name(name).await }
    });

    let on_blur = move |_| {
        let current = name.get();
        if !current.trim().is_empty() {
            save_action.dispatch(current);
        }
    };

    view! {
        <Card>
            <CardHeader>
                <div class="flex items-center justify-between">
                    <div>
                        <CardTitle>"Workspace Name"</CardTitle>
                        <CardDescription>
                            "Give your workspace a meaningful name to help identify it."
                        </CardDescription>
                    </div>
                    <ActionStatus action=save_action/>
                </div>
            </CardHeader>
            <CardContent>
                <input
                    type="text"
                    class=INPUT_CLASS
                    placeholder="My Workspace"
                    prop:value=name
                    on:input=move |ev| set_name.set(event_target_value(&ev))
                    on:blur=on_blur
                />
            </CardContent>
        </Card>
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Teams Section
// ─────────────────────────────────────────────────────────────────────────────

#[component]
fn TeamsSection() -> impl IntoView {
    let (version, set_version) = signal(0u32);
    let teams = Resource::new(move || version.get(), |_| list_all_teams());
    let ws_default_id = Resource::new(move || version.get(), |_| get_workspace_default_team_id());
    let (show_create_modal, set_show_create_modal) = signal(false);

    let set_default_action = Action::new(move |id: &String| {
        let id = id.clone();
        async move { set_workspace_default_team(id).await }
    });

    // Refresh the team list when the set-default action succeeds.
    Effect::new(move || {
        if let Some(Ok(())) = set_default_action.value().get() {
            set_version.update(|v| *v += 1);
        }
    });

    view! {
        <Card>
            <CardHeader>
                <div class="flex items-center justify-between">
                    <div>
                        <CardTitle>"Teams"</CardTitle>
                        <CardDescription>
                            "Manage issue-tracker teams in this workspace."
                        </CardDescription>
                    </div>
                    <Button
                        variant=ButtonVariant::Default
                        on:click=move |_| set_show_create_modal.set(true)
                    >
                        "Create Team"
                    </Button>
                </div>
            </CardHeader>
            <CardContent>
                <Transition fallback=move || view! {
                    <div class="space-y-3">
                        <Skeleton class="h-12 w-full"/>
                        <Skeleton class="h-12 w-full"/>
                    </div>
                }>
                    {move || Suspend::new(async move {
                        let default_id = ws_default_id.await.ok().flatten();
                        match teams.await {
                            Ok(team_list) if team_list.is_empty() => {
                                view! {
                                    <EmptyState
                                        title="No teams yet"
                                        description="Create your first team to start tracking issues"
                                    />
                                }.into_any()
                            }
                            Ok(team_list) => {
                                view! { <div>{team_rows(team_list, default_id, set_default_action)}</div> }.into_any()
                            }
                            Err(e) => {
                                view! {
                                    <p class="text-sm text-error-foreground">{e.to_string()}</p>
                                }.into_any()
                            }
                        }
                    })}
                </Transition>
            </CardContent>
        </Card>
        <TeamCreationModal
            show=Signal::derive(move || show_create_modal.get())
            on_close=Callback::new(move |()| {
                set_show_create_modal.set(false);
                set_version.update(|v| *v += 1);
            })
        />
    }
}

/// Render each team as a linked row with icon, name, key badge, and default controls.
fn team_rows(
    team_list: Vec<Team>,
    default_id: Option<String>,
    set_default_action: Action<String, Result<(), ServerFnError>>,
) -> impl IntoView {
    team_list
        .into_iter()
        .map(|team| {
            let is_ws_default =
                default_id.as_deref() == Some(team.team_id.as_str());
            let href = format!("/teams/{}/settings", team.key.to_lowercase());
            let team_id_for_action = team.team_id.clone();

            view! {
                <a
                    href=href
                    class="flex items-center gap-3 py-2.5 px-2 border-b border-border last:border-b-0 hover:bg-muted/50 transition-colors rounded-sm group"
                >
                    <TeamIcon team=team.clone() size="28px"/>
                    <span class="text-sm font-medium text-foreground flex-1 min-w-0 truncate">
                        {team.name.clone()}
                    </span>
                    <Badge variant=BadgeVariant::Secondary>
                        {team.key.clone()}
                    </Badge>
                    {if is_ws_default {
                        view! {
                            <Badge variant=BadgeVariant::Default>
                                "Workspace default"
                            </Badge>
                        }.into_any()
                    } else {
                        view! {
                            <Button
                                variant=ButtonVariant::Ghost
                                size=ButtonSize::Sm
                                on:click=move |e: web_sys::MouseEvent| {
                                    e.prevent_default();
                                    e.stop_propagation();
                                    set_default_action.dispatch(team_id_for_action.clone());
                                }
                            >
                                "Set as default"
                            </Button>
                        }.into_any()
                    }}
                </a>
            }
        })
        .collect_view()
}

