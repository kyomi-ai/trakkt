// SPDX-License-Identifier: AGPL-3.0-or-later

//! GitHub App installation callback page.
//!
//! Route: `/integrations/github/callback?installation_id=xxx&setup_action=install`
//!
//! After a user installs the GitHub App on their organization, GitHub redirects
//! here. This page:
//! 1. Reads `installation_id` and `setup_action` from query params
//! 2. Calls `process_github_callback()` to verify and store the installation
//! 3. On success, navigates to `/settings/integrations`
//! 4. On error, shows an error message with a link back to settings

use leptos::prelude::*;
use phosphor_leptos::{Icon, IconWeight};

use crate::components::{Alert, AlertDescription, AlertVariant, ButtonLink, ButtonVariant, Spinner};
use crate::server_fns::github::process_github_callback;

#[component]
pub fn GitHubCallbackPage() -> impl IntoView {
    let (status, set_status) = signal(CallbackState::Processing);
    let (error_msg, set_error_msg) = signal(String::new());

    // Read query params and process the callback on mount (browser-only).
    #[cfg(target_arch = "wasm32")]
    let (installation_id_param, setup_action_param) = {
        let params = web_sys::window()
            .and_then(|w| w.location().search().ok())
            .and_then(|s| web_sys::UrlSearchParams::new_with_str(&s).ok());
        match params {
            Some(p) => (p.get("installation_id"), p.get("setup_action")),
            None => (None, None),
        }
    };
    #[cfg(not(target_arch = "wasm32"))]
    let (installation_id_param, setup_action_param): (Option<String>, Option<String>) =
        (None, None);

    #[cfg(target_arch = "wasm32")]
    let navigate = leptos_router::hooks::use_navigate();

    leptos::task::spawn_local(async move {
        // Parse installation_id
        let installation_id: i64 = match installation_id_param
            .as_deref()
            .and_then(|s| s.parse().ok())
        {
            Some(id) => id,
            None => {
                set_error_msg.set("Missing or invalid installation_id parameter.".to_string());
                set_status.set(CallbackState::Error);
                return;
            }
        };

        let setup_action = setup_action_param.unwrap_or_else(|| "install".to_string());

        match process_github_callback(installation_id, setup_action).await {
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
