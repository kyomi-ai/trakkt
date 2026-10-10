// SPDX-License-Identifier: AGPL-3.0-or-later

//! GitHub App installation callback page.
//!
//! Setup advances workspace-bound state to OAuth; the distinct OAuth callback
//! completes verified association. Query credentials are removed before API calls.

use leptos::prelude::*;
use phosphor_leptos::{Icon, IconWeight};

use crate::components::{
    Alert, AlertDescription, AlertVariant, ButtonLink, ButtonVariant, Spinner,
};
use crate::server_fns::github::{complete_github_authorization, process_github_callback};

#[component]
pub fn GitHubCallbackPage() -> impl IntoView {
    let (status, set_status) = signal(CallbackState::Processing);
    let (error_msg, set_error_msg) = signal(String::new());

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
        if let Some(history) = web_sys::window().and_then(|window| window.history().ok()) {
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
    #[cfg(not(target_arch = "wasm32"))]
    let (installation_id_param, setup_action_param, state_param, code_param, is_oauth): (
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
        bool,
    ) = (None, None, None, None, false);

    #[cfg(target_arch = "wasm32")]
    let navigate = leptos_router::hooks::use_navigate();

    leptos::task::spawn_local(async move {
        let result = if is_oauth {
            match (state_param, code_param) {
                (Some(state), Some(code)) => complete_github_authorization(state, code).await,
                _ => Err(ServerFnError::new(
                    "Missing GitHub OAuth state or code. Start again from settings.",
                )),
            }
        } else {
            match (
                installation_id_param.and_then(|v| v.parse::<i64>().ok()),
                setup_action_param,
                state_param,
            ) {
                (Some(id), Some(action), Some(state)) => {
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
        match result {
            Ok(()) => {
                set_status.set(CallbackState::Success);

                // Navigate to integrations settings after a brief pause so the
                // user sees the success state.
                #[cfg(target_arch = "wasm32")]
                {
                    let nav = navigate.clone();
                    gloo_timers::future::TimeoutFuture::new(800).await;
                    nav("/settings/integrations", Default::default());
                }
            }
            Err(e) => {
                let msg = e
                    .to_string()
                    .strip_prefix("error running server function: ")
                    .unwrap_or(&e.to_string())
                    .to_string();
                set_error_msg.set(msg);
                set_status.set(CallbackState::Error);
            }
        }
    });

    view! {
        <div class="flex items-center justify-center min-h-[60vh]">
            <div class="text-center max-w-md space-y-4">
                {move || match status.get() {
                    CallbackState::Processing => view! {
                        <div class="flex flex-col items-center gap-4">
                            <Spinner size="h-8 w-8".to_string() class="text-primary"/>
                            <p class="text-sm text-muted-foreground">
                                "Connecting your GitHub account..."
                            </p>
                        </div>
                    }.into_any(),

                    CallbackState::Success => view! {
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
                                "Redirecting to settings..."
                            </p>
                        </div>
                    }.into_any(),

                    CallbackState::Error => view! {
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
                                <AlertDescription>{error_msg.get()}</AlertDescription>
                            </Alert>
                            <ButtonLink
                                href="/settings/integrations"
                                variant=ButtonVariant::Outline
                            >
                                "Back to Settings"
                            </ButtonLink>
                        </div>
                    }.into_any(),
                }}
            </div>
        </div>
    }
}

#[derive(Clone, Copy, PartialEq)]
enum CallbackState {
    Processing,
    Success,
    Error,
}
