// SPDX-License-Identifier: AGPL-3.0-or-later

//! Root application component and router.

use leptos::prelude::*;
use leptos_meta::provide_meta_context;
use leptos_router::{
    components::{ParentRoute, Redirect, Route, Router, Routes},
    path,
};

use crate::pages::auth::{
    account_recovery::AccountRecoveryPage,
    account_recovery_complete::AccountRecoveryCompletePage,
    google_callback::GoogleCallbackPage,
    login::LoginPage,
    oauth_complete::OAuthCompletePage,
    passkey_recovery::PasskeyRecoveryPage,
    passkey_recovery_complete::PasskeyRecoveryCompletePage,
    passkey_signup_complete::PasskeySignupCompletePage,
    signup_complete::SignupCompletePage,
};
use crate::pages::accept_ownership::AcceptOwnershipPage;
use crate::pages::board::BoardPage;
use crate::pages::issues::issue_detail::IssueDetailPage;
use crate::pages::issues::issue_list::IssueListPage;
use crate::pages::onboarding::OnboardingPage;
use crate::pages::settings::{
    labels::LabelsPage,
    profile::ProfilePage,
    security::security_tab::SecurityTab,
    settings_shell::SettingsShell,
    team::TeamPage,
    teams_settings::TeamsSettingsPage,
    workspace::WorkspacePage,
};

/// Shell HTML page that loads the WASM bundle.
#[component]
pub fn Shell(#[prop(optional)] children: Option<Children>) -> impl IntoView {
    view! {
        <!DOCTYPE html>
        <html lang="en">
            <head>
                <meta charset="utf-8"/>
                <meta name="viewport" content="width=device-width, initial-scale=1, viewport-fit=cover"/>
                <title>"Trakkt"</title>
                <leptos_meta::MetaTags/>
            </head>
            <body class="min-h-screen bg-background text-foreground antialiased">
                {children.map(|c| c())}
            </body>
        </html>
    }
}

use crate::components::layout::Layout;

/// Root Leptos application component.
#[component]
pub fn App() -> impl IntoView {
    provide_meta_context();

    let auth_version = RwSignal::new(0u64);
    provide_context(auth_version);
    let user_ctx = LocalResource::new(move || {
        auth_version.get();
        crate::server_fns::context::get_user_context()
    });
    provide_context(user_ctx);

    view! {
        <crate::components::theme::ThemeProvider initial_preference="system".to_string()>
        <Router>
            <Routes fallback=|| view! {
                <div class="min-h-screen bg-background flex items-center justify-center p-8">
                    <div class="text-center max-w-md">
                        <h1 class="text-6xl font-display text-foreground mb-4">"404"</h1>
                        <p class="text-lg text-muted-foreground mb-8">"This page doesn\u{2019}t exist."</p>
                        <a href="/" class="inline-flex items-center justify-center px-5 py-3 rounded-md bg-primary text-primary-foreground text-sm font-medium hover:bg-primary/90 transition-colors">
                            "Go home"
                        </a>
                    </div>
                </div>
            }>
                // ── Public routes (no auth required) ─────────────────────────
                <Route path=path!("/login") view=|| view! { <LoginPage/> }/>
                <Route path=path!("/signup") view=|| view! { <LoginPage signup_mode=true/> }/>
                <Route path=path!("/signup/complete") view=SignupCompletePage/>
                <Route path=path!("/auth/google/callback") view=GoogleCallbackPage/>
                <Route path=path!("/account/recover") view=AccountRecoveryPage/>
                <Route path=path!("/account/recover/complete") view=AccountRecoveryCompletePage/>
                <Route path=path!("/auth/passkey-signup") view=PasskeySignupCompletePage/>
                <Route path=path!("/auth/recover-passkey") view=PasskeyRecoveryPage/>
                <Route path=path!("/auth/recover-passkey/complete") view=PasskeyRecoveryCompletePage/>
                <Route path=path!("/oauth-complete") view=OAuthCompletePage/>

                // ── Authenticated routes (Layout provides sidebar + auth guard) ────
                <ParentRoute path=path!("") view=Layout>
                    <Route path=path!("/") view=|| view! { <Redirect path="/issues"/> }/>
                    <Route path=path!("/onboarding") view=OnboardingPage/>
                    <Route path=path!("/accept-ownership/:transfer_id") view=AcceptOwnershipPage/>

                    // Issue tracker
                    <Route path=path!("/issues") view=IssueListPage/>
                    <Route path=path!("/issues/:number") view=IssueDetailPage/>
                    <Route path=path!("/board") view=BoardPage/>

                    // Settings
                    <ParentRoute path=path!("/settings") view=|| view! {
                        <div class="flex flex-col h-full bg-muted overflow-x-hidden" style:flex-direction="column">
                            <div class="flex-1 overflow-y-auto p-4 md:p-6 relative">
                                <div class="absolute top-4 right-4 md:top-6 md:right-6 flex items-center gap-2 z-10">
                                    <button
                                        class="px-3 py-2 text-sm text-muted-foreground hover:text-foreground hover:bg-secondary rounded-lg transition-colors"
                                        on:click=move |_| {
                                            leptos::task::spawn_local(async move {
                                                let _ = crate::server_fns::security::logout().await;
                                                let _ = web_sys::window()
                                                    .and_then(|w| w.location().set_href("/login").ok());
                                            });
                                        }
                                    >
                                        "Sign Out"
                                    </button>
                                    <a
                                        href="/"
                                        class="p-2 text-muted-foreground hover:text-foreground hover:bg-secondary rounded-lg transition-colors"
                                        aria-label="Close settings"
                                    >
                                        <svg class="w-6 h-6" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                                            <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M6 18L18 6M6 6l12 12"/>
                                        </svg>
                                    </a>
                                </div>
                                <div class="w-full">
                                    <SettingsShell/>
                                </div>
                            </div>
                        </div>
                    }>
                        <Route path=path!("") view=|| view! { <Redirect path="/settings/profile"/> }/>
                        <Route path=path!("/profile") view=ProfilePage/>
                        <Route path=path!("/security") view=SecurityTab/>
                        <Route path=path!("/workspace") view=WorkspacePage/>
                        <Route path=path!("/team") view=TeamPage/>
                        <Route path=path!("/labels") view=LabelsPage/>
                        <Route path=path!("/teams") view=TeamsSettingsPage/>
                    </ParentRoute>
                </ParentRoute>
            </Routes>
        </Router>
        </crate::components::theme::ThemeProvider>
    }
}
