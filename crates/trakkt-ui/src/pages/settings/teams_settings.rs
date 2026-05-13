// SPDX-License-Identifier: AGPL-3.0-or-later

//! Teams management page — issue-tracker team CRUD.
//!
//! Provides a list of existing teams (key badge + name + actions) and an
//! "Add Team" form with auto-derived key from name. These are issue-tracker
//! teams (e.g. "Engineering" / ENG), not workspace membership.

use leptos::prelude::*;

use crate::components::{
    Alert, AlertDescription, AlertVariant, Badge, BadgeVariant, Button, ButtonSize, ButtonVariant,
    Card, CardContent, CardDescription, CardHeader, CardTitle, ConfirmDialog, EmptyState, Skeleton,
    INPUT_CLASS,
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
    let (version, set_version) = signal(0u32);
    let teams = Resource::new(move || version.get(), |_| list_teams());
    let default_team = Resource::new(move || version.get(), |_| get_default_team());

    // Create team form state
    let (new_name, set_new_name) = signal(String::new());
    let (new_key, set_new_key) = signal(String::new());
    let (key_manually_edited, set_key_manually_edited) = signal(false);
    let (create_error, set_create_error) = signal(Option::<String>::None);

    // Delete state
    let delete_dialog_open = RwSignal::new(false);
    let (delete_team_id, set_delete_team_id) = signal(String::new());
    let (delete_team_name, set_delete_team_name) = signal(String::new());
    let (delete_error, set_delete_error) = signal(Option::<String>::None);

    // Actions
    let create_action = Action::new(move |(name, key): &(String, String)| {
        let name = name.clone();
        let key = key.clone();
        async move { create_team(name, key, None, None).await }
    });

    let delete_action = Action::new(move |id: &String| {
        let id = id.clone();
        async move { delete_team(id, None, None).await }
    });

    let set_default_action = Action::new(move |id: &String| {
        let id = id.clone();
        async move { set_my_default_team(id).await }
    });

    // React to create action
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

    // React to delete action
    Effect::new(move || {
        if let Some(result) = delete_action.value().get() {
            match result {
                Ok(_) => {
                    set_delete_error.set(None);
                    set_version.update(|v| *v += 1);
                }
                Err(e) => {
                    set_delete_error.set(Some(e.to_string()));
                }
            }
        }
    });

    // React to set-default action
    Effect::new(move || {
        if let Some(result) = set_default_action.value().get()
            && result.is_ok()
        {
            set_version.update(|v| *v += 1);
        }
    });

    // Form handlers
    let on_name_input = move |ev: leptos::ev::Event| {
        let name = event_target_value(&ev);
        set_new_name.set(name.clone());
        if !key_manually_edited.get_untracked() {
            set_new_key.set(derive_key_from_name(&name));
        }
    };

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

    let key_format_ok = Memo::new(move |_| {
        let key = new_key.get();
        key.is_empty() || is_valid_key(&key)
    });

    let can_submit = Memo::new(move |_| {
        let name = new_name.get();
        let key = new_key.get();
        !name.trim().is_empty() && is_valid_key(&key)
    });

    // Delete handlers
    let request_delete = move |team: &Team| {
        set_delete_team_id.set(team.team_id.clone());
        set_delete_team_name.set(team.name.clone());
        set_delete_error.set(None);
        delete_dialog_open.set(true);
    };

    let on_confirm_delete = Callback::new(move |()| {
        delete_dialog_open.set(false);
        let id = delete_team_id.get_untracked();
        if !id.is_empty() {
            delete_action.dispatch(id);
        }
    });

    let on_cancel_delete = Callback::new(move |()| {
        delete_dialog_open.set(false);
    });

    // Set-default handler
    let on_set_default = move |team: &Team| {
        set_default_action.dispatch(team.team_id.clone());
    };

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
                            let default_id = default_team.await
                                .ok()
                                .map(|t| t.team_id);

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
                                        let team_count = team_list.len();
                                        let default_id_cloned = default_id.clone();
                                        let rows = team_list.into_iter().map(move |team| {
                                            let is_default = default_id_cloned.as_deref() == Some(team.team_id.as_str());
                                            let can_delete = team_count > 1;
                                            let team_for_delete = team.clone();
                                            let team_for_default = team.clone();
                                            view! {
                                                <div class="flex items-center gap-3 py-2.5 px-1 border-b border-border last:border-b-0 hover:bg-muted/50 transition-colors rounded-sm">
                                                    <span class="font-mono text-xs bg-muted px-2 py-0.5 rounded text-foreground flex-shrink-0">
                                                        {team.key.clone()}
                                                    </span>
                                                    <span class="text-sm font-medium text-foreground flex-1 min-w-0 truncate">
                                                        {team.name.clone()}
                                                    </span>
                                                    {is_default.then(|| view! {
                                                        <Badge variant=BadgeVariant::Default class="flex-shrink-0">
                                                            "Default"
                                                        </Badge>
                                                    })}
                                                    {(!is_default).then(|| {
                                                        let t = team_for_default.clone();
                                                        view! {
                                                            <Button
                                                                variant=ButtonVariant::Ghost
                                                                size=ButtonSize::Sm
                                                                on:click=move |_| on_set_default(&t)
                                                            >
                                                                "Set default"
                                                            </Button>
                                                        }
                                                    })}
                                                    {can_delete.then(|| {
                                                        let t = team_for_delete.clone();
                                                        view! {
                                                            <Button
                                                                variant=ButtonVariant::Ghost
                                                                size=ButtonSize::Sm
                                                                on:click=move |_| request_delete(&t)
                                                                class="text-destructive hover:text-destructive"
                                                            >
                                                                "Delete"
                                                            </Button>
                                                        }
                                                    })}
                                                </div>
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

                    // Delete error
                    {move || delete_error.get().map(|e| view! {
                        <Alert variant=AlertVariant::Error class="mb-4">
                            <AlertDescription>{e}</AlertDescription>
                        </Alert>
                    })}

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

            // Delete confirmation dialog
            <ConfirmDialog
                open=Signal::from(delete_dialog_open)
                title=Signal::derive(move || format!("Delete {}?", delete_team_name.get()))
                message=Signal::derive(move || format!(
                    "Permanently delete team \u{201c}{}\u{201d}? Issues in this team will need to be moved first. This cannot be undone.",
                    delete_team_name.get()
                ))
                confirm_text="Delete"
                on_confirm=on_confirm_delete
                on_cancel=on_cancel_delete
            />
        </div>
    }
}
