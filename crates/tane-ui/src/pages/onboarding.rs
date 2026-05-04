// SPDX-License-Identifier: AGPL-3.0-or-later

use leptos::prelude::*;
use crate::components::{Button, ButtonVariant, ButtonSize, Card, CardContent, CardHeader, CardTitle, CardDescription};

#[component]
pub fn OnboardingPage() -> impl IntoView {
    view! {
        <div class="min-h-screen bg-background flex items-center justify-center p-8">
            <Card class="w-full max-w-lg".to_string()>
                <CardHeader>
                    <div class="flex justify-center mb-4">
                        <img src="/tane_full_logo.svg" alt="Tane" class="h-10 dark:hidden"/>
                        <img src="/tane_full_logo_white.svg" alt="Tane" class="h-10 hidden dark:block"/>
                    </div>
                    <CardTitle class="text-center".to_string()>"Welcome aboard!"</CardTitle>
                    <CardDescription class="text-center".to_string()>
                        "Your account is ready. This is where you'd configure your workspace — add team members, set preferences, and get started."
                    </CardDescription>
                </CardHeader>
                <CardContent>
                    <div class="space-y-4">
                        <div class="rounded-lg border border-border p-4 text-sm text-muted-foreground">
                            "This onboarding flow is a placeholder. Replace it with your app's setup steps."
                        </div>
                        <Button
                            variant=ButtonVariant::Default
                            size=ButtonSize::Lg
                            class="w-full".to_string()
                            on:click=move |_| {
                                let _ = web_sys::window()
                                    .and_then(|w| w.location().set_href("/settings/profile").ok());
                            }
                        >
                            "Go to Settings"
                        </Button>
                    </div>
                </CardContent>
            </Card>
        </div>
    }
}
