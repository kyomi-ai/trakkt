// SPDX-License-Identifier: AGPL-3.0-or-later

//! Team settings page — rename and manage a single team.
//!
//! Route: `/teams/:key/settings`
//!
//! Reads the `:key` route parameter, resolves the team from `SyncStore`,
//! and provides forms for updating name/key and a danger zone for deletion.

use leptos::prelude::*;
use leptos_router::hooks::{use_navigate, use_params_map};
use phosphor_leptos::{Icon, IconWeight};

use crate::cache::store::SyncStore;
use crate::components::{
    ActionStatus, Button, ButtonSize, ButtonVariant, Card, CardContent, CardDescription,
    CardHeader, CardTitle, ConfirmDialog, Skeleton, INPUT_CLASS,
};

// ─────────────────────────────────────────────────────────────────────────────
// Main page
// ─────────────────────────────────────────────────────────────────────────────

/// Team settings page — reads `:key` from route params.
#[component]
pub fn TeamSettingsPage() -> impl IntoView {
    let params = use_params_map();
    let team_key = Signal::derive(move || params.read().get("key").unwrap_or_default());

    let store = use_context::<SyncStore>();
    let initialized = Signal::derive(move || {
        store.is_some_and(|s| s.initialized().get())
    });

    // Resolve team reactively from SyncStore by key (case-insensitive).
    let team = Signal::derive(move || {
        let key = team_key.get();
        store.and_then(|s| {
            s.teams()
                .get()
                .into_iter()
                .find(|t| t.key.eq_ignore_ascii_case(&key))
        })
    });

    let back_href = Signal::derive(move || {
        format!("/teams/{}/issues", team_key.get().to_lowercase())
    });

    view! {
        <div class="flex flex-col h-full">
            // ── Header ─────────────────────────────────────────────────────
            <div class="page-header h-14 px-5 flex items-center gap-3 shrink-0">
                <Button
                    variant=ButtonVariant::GhostMuted
                    size=ButtonSize::IconSm
                    aria_label="Back to issues"
                    on:click=move |_| {
                        let nav = use_navigate();
                        nav(&back_href.get_untracked(), Default::default());
                    }
                >
                    <Icon icon=phosphor_leptos::ARROW_LEFT weight=IconWeight::Regular size="20px"/>
                </Button>
                <h1 class="text-2xl font-display text-foreground">"Team Settings"</h1>
            </div>

            // ── Content ────────────────────────────────────────────────────
            <div class="flex-1 overflow-y-auto p-4 md:p-6">
                {move || {
                    if !initialized.get() {
                        // Skeleton while SyncStore loads
                        view! {
                            <div class="space-y-6 max-w-2xl">
                                <Card>
                                    <CardHeader>
                                        <Skeleton class="h-5 w-1/3"/>
                                        <Skeleton class="h-4 w-2/3 mt-1"/>
                                    </CardHeader>
                                    <CardContent>
                                        <Skeleton class="h-10 w-full"/>
                                        <Skeleton class="h-10 w-full mt-4"/>
                                    </CardContent>
                                </Card>
                            </div>
                        }.into_any()
                    } else if let Some(t) = team.get() {
                        view! {
                            <div class="space-y-6 max-w-2xl">
                                <TeamGeneralCard team_id=t.team_id.clone() name=t.name.clone() team_key=t.key.clone()/>
                                <TeamDangerZone team_id=t.team_id.clone() team_name=t.name.clone()/>
                            </div>
                        }.into_any()
                    } else {
                        view! {
                            <div class="text-muted-foreground">"Team not found."</div>
                        }.into_any()
                    }
                }}
            </div>
        </div>
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// General settings card — name and key
// ─────────────────────────────────────────────────────────────────────────────

#[component]
fn TeamGeneralCard(team_id: String, name: String, team_key: String) -> impl IntoView {
    let (edit_name, set_edit_name) = signal(name);
    let (edit_key, set_edit_key) = signal(team_key);
    let (validation_error, set_validation_error) = signal(Option::<String>::None);

    let save_action = Action::new({
        let team_id = team_id.clone();
        move |(name, key): &(String, String)| {
            let team_id = team_id.clone();
            let name = name.clone();
            let key = key.clone();
            async move {
                crate::server_fns::teams::update_team(team_id, Some(name), Some(key)).await
            }
        }
    });

    // Navigate to updated URL when key changes successfully.
    let nav = use_navigate();
    Effect::new(move |_| {
        if let Some(Ok(())) = save_action.value().get() {
            let k = edit_key.get().to_lowercase();
            nav(&format!("/teams/{k}/settings"), Default::default());
        }
    });

    let handle_save = move |_| {
        let n = edit_name.get_untracked();
        let k = edit_key.get_untracked();

        if n.trim().is_empty() {
            set_validation_error.set(Some("Team name cannot be empty.".into()));
            return;
        }
        if k.len() < 2
            || k.len() > 5
            || !k.chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit())
        {
            set_validation_error
                .set(Some("Key must be 2\u{2013}5 uppercase alphanumeric characters.".into()));
            return;
        }

        set_validation_error.set(None);
        save_action.dispatch((n, k));
    };

    view! {
        <Card>
            <CardHeader>
                <div class="flex items-center justify-between">
                    <div>
                        <CardTitle>"General"</CardTitle>
                        <CardDescription>
                            "Update team name and key. The key is used as the prefix for issue identifiers."
                        </CardDescription>
                    </div>
                    <ActionStatus action=save_action/>
                </div>
            </CardHeader>
            <CardContent>
                <div class="space-y-4">
                    <div>
                        <label class="block text-sm font-medium text-foreground mb-2">"Team name"</label>
                        <input
                            type="text"
                            class=INPUT_CLASS
                            prop:value=edit_name
                            on:input=move |ev| set_edit_name.set(event_target_value(&ev))
                        />
                    </div>
                    <div>
                        <label class="block text-sm font-medium text-foreground mb-2">"Team key"</label>
                        <input
                            type="text"
                            class=INPUT_CLASS
                            maxlength="5"
                            prop:value=edit_key
                            on:input=move |ev| set_edit_key.set(event_target_value(&ev).to_uppercase())
                        />
                        <p class="text-xs text-muted-foreground mt-1">
                            "2\u{2013}5 uppercase letters or digits. Used as issue ID prefix (e.g. ENG-42)."
                        </p>
                    </div>

                    <Show when=move || validation_error.get().is_some()>
                        <p class="text-sm text-error-foreground">
                            {move || validation_error.get().unwrap_or_default()}
                        </p>
                    </Show>

                    <div class="flex justify-end">
                        <Button on:click=handle_save>"Save"</Button>
                    </div>
                </div>
            </CardContent>
        </Card>
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Danger zone — delete team
// ─────────────────────────────────────────────────────────────────────────────

#[component]
fn TeamDangerZone(team_id: String, team_name: String) -> impl IntoView {
    let (show_delete_confirm, set_show_delete_confirm) = signal(false);
    let nav = use_navigate();
    let store = use_context::<SyncStore>();

    let confirm_message = format!(
        "Are you sure you want to delete \"{}\"? All issues in this team will become unassigned. This action cannot be undone.",
        team_name,
    );

    view! {
        <Card>
            <CardHeader>
                <CardTitle>
                    <span class="text-error-foreground">"Danger Zone"</span>
                </CardTitle>
                <CardDescription>
                    "Destructive actions that cannot be undone."
                </CardDescription>
            </CardHeader>
            <CardContent>
                <div class="flex items-center justify-between">
                    <div>
                        <p class="text-sm font-medium text-foreground">"Delete this team"</p>
                        <p class="text-xs text-muted-foreground">
                            "Permanently delete this team and unassign all its issues."
                        </p>
                    </div>
                    <Button
                        variant=ButtonVariant::Destructive
                        on:click=move |_| set_show_delete_confirm.set(true)
                    >
                        "Delete team"
                    </Button>
                </div>
            </CardContent>
        </Card>

        {
            let team_id = team_id.clone();
            view! {
                <ConfirmDialog
                    open=Signal::derive(move || show_delete_confirm.get())
                    title="Delete team?"
                    message=confirm_message
                    confirm_text="Delete"
                    on_confirm=Callback::new(move |()| {
                        set_show_delete_confirm.set(false);
                        let team_id = team_id.clone();
                        let nav = nav.clone();
                        leptos::task::spawn_local(async move {
                            match crate::server_fns::teams::delete_team(team_id.clone(), None, None).await {
                                Ok(()) => {
                                    if let Some(store) = store {
                                        store.remove_team(&team_id);
                                    }
                                    nav("/my-issues", Default::default());
                                }
                                Err(e) => {
                                    web_sys::console::warn_1(&format!("delete_team failed: {e}").into());
                                }
                            }
                        });
                    })
                    on_cancel=Callback::new(move |()| set_show_delete_confirm.set(false))
                />
            }
        }
    }
}
