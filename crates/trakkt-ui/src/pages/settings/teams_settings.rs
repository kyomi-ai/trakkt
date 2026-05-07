// SPDX-License-Identifier: AGPL-3.0-or-later

//! Teams management page — issue-tracker team CRUD.
//!
//! Provides a list of existing teams (key badge + name) and a "Add Team"
//! form with auto-derived key from name. These are issue-tracker teams
//! (e.g. "Engineering" / ENG), not workspace membership.

use leptos::prelude::*;

use crate::components::{
    Alert, AlertDescription, AlertVariant, Badge, BadgeVariant, Button, ButtonSize, ButtonVariant,
    Card, CardContent, CardDescription, CardHeader, CardTitle, EmptyState, Skeleton, INPUT_CLASS,
};
use crate::server_fns::teams::*;
use trakkt_types::models::Team;

// ─── Key derivation helper ────────────────────────────────────────────────

/// Derive a team key from a name: uppercase first 3 alphabetic characters.
fn derive_key_from_name(name: &str) -> String {
    name.chars()
        .filter(|c| c.is_ascii_alphabetic())
        .take(3)
        .collect::<String>()
        .to_uppercase()
}

/// Validate a team key: 2-5 uppercase ASCII letters.
fn is_valid_key(key: &str) -> bool {
    let len = key.len();
    (2..=5).contains(&len) && key.chars().all(|c| c.is_ascii_uppercase())
}

// ─── Main page ────────────────────────────────────────────────────────────

#[component]
pub fn TeamsSettingsPage() -> impl IntoView {
    // Data fetching with version-based refresh
    let (version, set_version) = signal(0u32);
    let teams = Resource::new(move || version.get(), |_| list_teams());

    // Create team form state
    let (new_name, set_new_name) = signal(String::new());
    let (new_key, set_new_key) = signal(String::new());
    let (key_manually_edited, set_key_manually_edited) = signal(false);
    let (create_error, set_create_error) = signal(Option::<String>::None);

    // Action
    let create_action = Action::new(move |(name, key): &(String, String)| {
        let name = name.clone();
        let key = key.clone();
        async move { create_team(name, key).await }
    });

    // React to action completion
    Effect::new(move || {
        if let Some(result) = create_action.value().get() {
            match result {
                Ok(_) => {
                    set_new_name.set(String::new());
                    set_new_key.set(String::new());
                    set_key_manually_edited.set(false);
                    set_create_error.set(None);
                    set_version.update(|v| *v += 1);
                }
                Err(e) => {
                    set_create_error.set(Some(e.to_string()));
                }
            }
        }
    });

    // Auto-derive key from name (unless manually edited)
    let on_name_input = move |ev: leptos::ev::Event| {
        let name = event_target_value(&ev);
        set_new_name.set(name.clone());
        if !key_manually_edited.get_untracked() {
            set_new_key.set(derive_key_from_name(&name));
        }
    };

    // Key input: filter to uppercase A-Z, max 5 chars
    let on_key_input = move |ev: leptos::ev::Event| {
        let raw = event_target_value(&ev);
        let filtered: String = raw
            .chars()
            .filter(|c| c.is_ascii_alphabetic())
            .take(5)
            .collect::<String>()
            .to_uppercase();
        set_new_key.set(filtered);
        set_key_manually_edited.set(true);
    };

    // Validation memo
    let key_format_ok = Memo::new(move |_| {
        let key = new_key.get();
        key.is_empty() || is_valid_key(&key)
    });

    let can_submit = Memo::new(move |_| {
        let name = new_name.get();
        let key = new_key.get();
        !name.trim().is_empty() && is_valid_key(&key)
    });

    view! {
        <div class="p-4 sm:p-6">
            <h2 class="text-xl font-display text-foreground mb-4">"Teams"</h2>
            <p class="text-muted-foreground mb-6">
                "Manage issue-tracker teams. Each team has a unique prefix for issue numbers."
            </p>

            <Card>
                <CardHeader>
                    <CardTitle>"Teams"</CardTitle>
                    <CardDescription>"Teams organize issues with unique prefixes"</CardDescription>
                </CardHeader>
                <CardContent>
                    // Team list
                    <Transition fallback=move || view! {
                        <div class="space-y-3">
                            <Skeleton class="h-10 w-full"/>
                            <Skeleton class="h-10 w-full"/>
                        </div>
                    }>
                        {move || Suspend::new(async move {
                            match teams.await {
                                Ok(team_list) => {
                                    if team_list.is_empty() {
                                        view! {
                                            <EmptyState
                                                title="No teams yet"
                                                description="Create your first team to start tracking issues"
                                                class="mb-6"
                                            />
                                        }.into_any()
                                    } else {
                                        let rows = team_list.into_iter().enumerate().map(|(idx, team)| {
                                            let is_default = idx == 0;
                                            view! {
                                                <TeamRow team=team is_default=is_default />
                                            }
                                        }).collect_view();

                                        view! {
                                            <div class="mb-6">
                                                {rows}
                                            </div>
                                        }.into_any()
                                    }
                                },
                                Err(e) => {
                                    let msg = e.to_string();
                                    view! {
                                        <Alert variant=AlertVariant::Error class="mb-6">
                                            <AlertDescription>{msg}</AlertDescription>
                                        </Alert>
                                    }.into_any()
                                },
                            }
                        })}
                    </Transition>

                    // Add team form
                    <div class="border-t border-border pt-4 mt-2">
                        <h3 class="text-sm font-semibold text-foreground mb-3">"Add Team"</h3>
                        <div class="space-y-3">
                            <div>
                                <label class="block text-sm font-medium text-foreground mb-1">
                                    "Name"
                                </label>
                                <input
                                    type="text"
                                    class=INPUT_CLASS
                                    placeholder="e.g. Engineering, Design, Marketing"
                                    prop:value=new_name
                                    on:input=on_name_input
                                />
                            </div>
                            <div>
                                <label class="block text-sm font-medium text-foreground mb-1">
                                    "Key"
                                </label>
                                <input
                                    type="text"
                                    class=INPUT_CLASS
                                    placeholder="e.g. ENG"
                                    maxlength="5"
                                    prop:value=new_key
                                    on:input=on_key_input
                                />
                                <p class="text-xs text-muted-foreground mt-1">
                                    "2\u{2013}5 uppercase letters. Used as issue prefix (e.g. ENG-42)."
                                </p>
                                {move || {
                                    let key = new_key.get();
                                    if !key.is_empty() && !key_format_ok.get() {
                                        Some(view! {
                                            <p class="text-xs text-error-foreground mt-1">
                                                "Key must be 2\u{2013}5 uppercase letters (A\u{2013}Z only)."
                                            </p>
                                        })
                                    } else {
                                        None
                                    }
                                }}
                            </div>

                            // Create error
                            {move || create_error.get().map(|e| view! {
                                <Alert variant=AlertVariant::Error>
                                    <AlertDescription>{e}</AlertDescription>
                                </Alert>
                            })}

                            <div class="flex justify-end">
                                <Button
                                    variant=ButtonVariant::Default
                                    size=ButtonSize::Sm
                                    disabled=MaybeProp::from(Signal::derive(move || !can_submit.get()))
                                    on:click=move |_| {
                                        let name = new_name.get_untracked();
                                        let key = new_key.get_untracked();
                                        if !name.trim().is_empty() && is_valid_key(&key) {
                                            create_action.dispatch((name, key));
                                        }
                                    }
                                >
                                    "Add"
                                </Button>
                            </div>
                        </div>
                    </div>
                </CardContent>
            </Card>
        </div>
    }
}

// ─── Team Row ─────────────────────────────────────────────────────────────

#[component]
fn TeamRow(
    team: Team,
    is_default: bool,
) -> impl IntoView {
    view! {
        <div class="flex items-center gap-3 py-2.5 px-1 border-b border-border last:border-b-0 hover:bg-muted/50 transition-colors rounded-sm">
            // Team key badge
            <span class="font-mono text-xs bg-muted px-2 py-0.5 rounded text-foreground flex-shrink-0">
                {team.key}
            </span>
            // Team name
            <span class="text-sm font-medium text-foreground flex-1 min-w-0 truncate">
                {team.name}
            </span>
            // Default badge
            {is_default.then(|| view! {
                <Badge variant=BadgeVariant::Default class="flex-shrink-0">
                    "Default"
                </Badge>
            })}
        </div>
    }
}
