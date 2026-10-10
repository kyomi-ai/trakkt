// SPDX-License-Identifier: AGPL-3.0-or-later

//! GitHub App installation callback page.
//!
//! Setup advances workspace-bound state to OAuth; the distinct OAuth callback
//! completes verified association. Query credentials are removed before API calls.

use leptos::prelude::*;
use phosphor_leptos::{Icon, IconWeight};

use crate::components::{
    Alert, AlertDescription, AlertVariant, Button, ButtonLink, ButtonVariant, Select,
    SelectVariant, Spinner,
};
#[cfg(target_arch = "wasm32")]
use crate::server_fns::github::{complete_github_authorization, process_github_callback};
use crate::server_fns::github::{get_github_connect_workspaces, start_direct_github_connection};

#[component]
pub fn GitHubCallbackPage() -> impl IntoView {
    let status = RwSignal::new(None::<Result<(), String>>);
    let direct_candidate = RwSignal::new(None::<i64>);
    let navigation_error = RwSignal::new(false);

    #[cfg(target_arch = "wasm32")]
    let params = web_sys::window()
        .and_then(|w| w.location().search().ok())
        .and_then(|s| web_sys::UrlSearchParams::new_with_str(&s).ok());
    #[cfg(target_arch = "wasm32")]
    let (installation_id_param, setup_action_param, state_param, code_param, is_oauth) = {
        let get = |name| params.as_ref().and_then(|p| p.get(name));
        let is_oauth = web_sys::window()
            .and_then(|w| w.location().pathname().ok())
            .is_some_and(|path| path.ends_with("/oauth/callback"));
        let direct = direct_install_candidate(
            is_oauth,
            get("installation_id").as_deref(),
            get("setup_action").as_deref(),
            get("state").as_deref(),
            get("code").as_deref(),
        );
        if let Some(id) = direct
            && let Some(history) = web_sys::window().and_then(|window| window.history().ok())
            && history
                .replace_state_with_url(
                    &wasm_bindgen::JsValue::NULL,
                    "",
                    Some(&format!(
                        "/integrations/github/callback?installation_id={id}&setup_action=install"
                    )),
                )
                .is_err()
        {
            tracing::warn!("Could not normalize direct GitHub installation URL");
        }
        if direct.is_none()
            && let Some(history) = web_sys::window().and_then(|window| window.history().ok())
        {
            let path = if is_oauth {
                "/integrations/github/oauth/callback"
            } else {
                "/integrations/github/callback"
            };
            if history
                .replace_state_with_url(&wasm_bindgen::JsValue::NULL, "", Some(path))
                .is_err()
            {
                tracing::warn!("Could not remove GitHub callback parameters from browser history");
            }
        }
        (
            get("installation_id"),
            get("setup_action"),
            get("state"),
            get("code"),
            is_oauth,
        )
    };
    #[cfg(target_arch = "wasm32")]
    leptos::task::spawn_local(async move {
        if let Some(id) = direct_install_candidate(
            is_oauth,
            installation_id_param.as_deref(),
            setup_action_param.as_deref(),
            state_param.as_deref(),
            code_param.as_deref(),
        ) {
            direct_candidate.set(Some(id));
            return;
        }
        let outcome = if is_oauth {
            match (state_param, code_param) {
                (Some(state), Some(code)) => complete_github_authorization(state, code).await,
                _ => Err(ServerFnError::new(
                    "Missing GitHub OAuth state or code. Start again from settings.",
                )),
            }
        } else {
            match (
                installation_id_param.and_then(|v| v.parse::<i64>().ok()),
                normalized_setup_action(setup_action_param),
                state_param,
            ) {
                (Some(id), action, Some(state)) => {
                    match process_github_callback(id, action, state).await {
                        Ok(url) => {
                            #[cfg(target_arch = "wasm32")]
                            if web_sys::window()
                                .is_some_and(|window| window.location().assign(&url).is_ok())
                            {
                                return;
                            }
                            #[cfg(not(target_arch = "wasm32"))]
                            drop(url);
                            Err(ServerFnError::new(
                                "Could not redirect to GitHub authorization",
                            ))
                        }
                        Err(error) => Err(error),
                    }
                }
                _ => Err(ServerFnError::new(
                    "Missing GitHub setup authorization. Start again from settings.",
                )),
            }
        };
        match outcome {
            Ok(workspace_id) => {
                status.set(Some(Ok(())));

                // A committed connection stays successful if updating the
                // user's workspace preference fails. Reload settings only
                // after selecting the verified state-bound destination.
                match crate::server_fns::sidebar::switch_workspace(workspace_id).await {
                    Ok(()) => {
                        gloo_timers::future::TimeoutFuture::new(800).await;
                        if !web_sys::window().is_some_and(|window| {
                            window.location().assign("/settings/integrations").is_ok()
                        }) {
                            navigation_error.set(true);
                        }
                    }
                    Err(_) => navigation_error.set(true),
                }
            }
            Err(e) => {
                let msg = e
                    .to_string()
                    .strip_prefix("error running server function: ")
                    .unwrap_or(&e.to_string())
                    .to_string();
                status.set(Some(Err(msg)));
            }
        }
    });

    view! {
        <div class="flex items-center justify-center min-h-[60vh]">
            <div class="text-center max-w-md space-y-4">
                {move || if let Some(id) = direct_candidate.get() {
                    view! { <DirectInstallLanding installation_id=id/> }.into_any()
                } else { match status.get() {
                    None => view! {
                        <div class="flex flex-col items-center gap-4">
                            <Spinner size="h-8 w-8".to_string() class="text-primary"/>
                            <p class="text-sm text-muted-foreground">
                                "Connecting your GitHub account..."
                            </p>
                        </div>
                    }.into_any(),

                    Some(Ok(())) => view! {
                        <div class="flex flex-col items-center gap-4">
                            <Icon
                                icon=phosphor_leptos::CHECK_CIRCLE
                                weight=IconWeight::Duotone
                                size="48px"
                                attr:class="text-success-foreground"
                            />
                            <p class="text-sm text-foreground font-medium">
                                "GitHub connected successfully!"
                            </p>
                            <p class="text-xs text-muted-foreground">
                                {move || if navigation_error.get() { "Connected. Choose the workspace in settings to view this connection." } else { "Redirecting to settings..." }}
                            </p>
                            <Show when=move || navigation_error.get()><ButtonLink href="/settings/integrations">"Go to settings"</ButtonLink></Show>
                        </div>
                    }.into_any(),

                    Some(Err(error_msg)) => view! {
                        <div class="flex flex-col items-center gap-4">
                            <Icon
                                icon=phosphor_leptos::WARNING
                                weight=IconWeight::Duotone
                                size="48px"
                                attr:class="text-error-foreground"
                            />
                            <p class="text-sm text-foreground font-medium">
                                "Failed to connect GitHub"
                            </p>
                            <Alert variant=AlertVariant::Error>
                                <AlertDescription>{error_msg}</AlertDescription>
                            </Alert>
                            <ButtonLink
                                href="/settings/integrations"
                                variant=ButtonVariant::Outline
                            >
                                "Back to Settings"
                            </ButtonLink>
                        </div>
                    }.into_any(),
                } }}
            </div>
        </div>
    }
}

// GitHub guarantees the candidate ID, but may omit setup_action. Existing
// stateful setup still validates the server-held state and rejects any explicit
// action other than install; this default does not enter direct onboarding.
#[cfg(any(target_arch = "wasm32", test))]
fn normalized_setup_action(action: Option<String>) -> String {
    action.unwrap_or_else(|| "install".into())
}

/// Only an absent state on a setup redirect can enter direct onboarding.
#[cfg(any(target_arch = "wasm32", test))]
fn direct_install_candidate(
    is_oauth: bool,
    installation_id: Option<&str>,
    action: Option<&str>,
    state: Option<&str>,
    code: Option<&str>,
) -> Option<i64> {
    if is_oauth
        || state.is_some()
        || code.is_some()
        || !matches!(action, None | Some("install" | "update"))
    {
        return None;
    }
    installation_id?.parse::<i64>().ok().filter(|id| *id > 0)
}

#[component]
fn DirectInstallLanding(installation_id: i64) -> impl IntoView {
    let user_context = expect_context::<
        LocalResource<Result<crate::server_fns::context::UserContext, ServerFnError>>,
    >();
    let workspaces = LocalResource::new(get_github_connect_workspaces);
    let selected = RwSignal::new(String::new());
    let error = RwSignal::new(None::<String>);
    let pending = RwSignal::new(false);
    Effect::new(move || {
        if selected.get_untracked().is_empty()
            && let Some(Ok(rows)) = workspaces.get()
            && let Some(row) = rows
                .iter()
                .find(|row| row.is_current)
                .or_else(|| rows.first())
        {
            selected.set(row.workspace_id.clone());
        }
    });
    let options = Signal::derive(move || {
        workspaces
            .get()
            .and_then(Result::ok)
            .unwrap_or_default()
            .into_iter()
            .map(|row| (row.workspace_id, row.name))
            .collect::<Vec<_>>()
    });
    let can_continue = move || {
        workspaces.get().and_then(Result::ok).is_some_and(|rows| {
            rows.iter()
                .any(|row| row.workspace_id == selected.get() && row.is_admin)
        })
    };
    let login_url = format!(
        "/login?redirect=%2Fintegrations%2Fgithub%2Fcallback%3Finstallation_id%3D{installation_id}%26setup_action%3Dinstall"
    );
    let on_continue = move |_| {
        if pending.get_untracked() || !can_continue() {
            return;
        }
        let workspace_id = selected.get_untracked();
        pending.set(true);
        leptos::task::spawn_local(async move {
            match start_direct_github_connection(installation_id, workspace_id).await {
                Ok(url) => {
                    #[cfg(target_arch = "wasm32")]
                    if web_sys::window()
                        .is_some_and(|window| window.location().assign(&url).is_ok())
                    {
                        return;
                    }
                    drop(url);
                    error.set(Some("Could not redirect to GitHub authorization".into()));
                }
                Err(err) => error.set(Some(err.to_string())),
            }
            pending.set(false);
        });
    };
    view! {
        <div class="space-y-4">
            <h1 class="text-xl font-medium">"Connect GitHub to Trakkt"</h1>
            <p class="text-sm text-muted-foreground">"Choose the workspace for this GitHub installation."</p>
            {move || {
                match user_context.get() {
                    None => return view! { <Spinner/> }.into_any(),
                    Some(Err(_)) => return view! { <ButtonLink href=login_url.clone()>"Sign in to continue"</ButtonLink> }.into_any(),
                    Some(Ok(_)) => {}
                }
                match workspaces.get() {
                None => view! { <Spinner/> }.into_any(),
                Some(Err(_)) => view! {
                    <p>"Could not load your workspaces. Try again."</p>
                    <Button on:click=move |_| workspaces.refetch()>"Try again"</Button>
                }.into_any(),
                Some(Ok(rows)) if rows.is_empty() => view! {
                    <p>"Create or join a workspace before connecting GitHub."</p>
                    <ButtonLink href="/onboarding">"Set up a workspace"</ButtonLink>
                }.into_any(),
                Some(Ok(_)) => view! {
                    <Select value=selected options=options on_change=Callback::new(move |id| selected.set(id)) variant=SelectVariant::Form placeholder="Choose a workspace"/>
                    <Show when=move || !can_continue()><p class="text-sm text-muted-foreground">"A workspace administrator must connect GitHub. Choose a workspace you administer."</p></Show>
                    <Button disabled=Signal::derive(move || pending.get() || !can_continue()) on:click=on_continue>"Continue with GitHub"</Button>
                }.into_any(),
            } }}
            <Show when=move || error.get().is_some()><Alert variant=AlertVariant::Error><AlertDescription>{move || error.get()}</AlertDescription></Alert></Show>
        </div>
    }
}

#[cfg(test)]
mod direct_install_tests {
    use super::*;

    #[test]
    fn only_absent_state_and_valid_setup_candidate_enter_onboarding() {
        assert_eq!(normalized_setup_action(None), "install");
        for action in ["", "update", "unknown"] {
            assert_eq!(normalized_setup_action(Some(action.into())), action);
        }
        assert_eq!(
            direct_install_candidate(false, Some("123"), None, None, None),
            Some(123)
        );
        for action in ["install", "update"] {
            assert_eq!(
                direct_install_candidate(false, Some("123"), Some(action), None, None),
                Some(123)
            );
        }
        for state in ["", "malformed", "previously-consumed-state"] {
            assert_eq!(
                direct_install_candidate(false, Some("123"), Some("install"), Some(state), None),
                None
            );
        }
        for candidate in ["", "0", "-1", "bad-id"] {
            assert_eq!(
                direct_install_candidate(false, Some(candidate), Some("install"), None, None),
                None
            );
        }
        assert_eq!(
            direct_install_candidate(true, Some("123"), Some("install"), None, None),
            None
        );
        assert_eq!(
            direct_install_candidate(false, Some("123"), Some("install"), None, Some("code")),
            None
        );
        assert_eq!(
            direct_install_candidate(false, Some("123"), Some("unknown"), None, None),
            None
        );
    }
}
