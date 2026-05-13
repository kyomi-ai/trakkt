// SPDX-License-Identifier: AGPL-3.0-or-later

use leptos::prelude::*;
use crate::components::{Button, ButtonVariant, ButtonSize, Card, CardContent, CardHeader, CardTitle, CardDescription};

#[component]
pub fn OnboardingPage() -> impl IntoView {
    view! {
        <div class="min-h-screen bg-background flex items-center justify-center p-8">
            <Card class="w-full max-w-lg".to_string()>
                <CardHeader>
                    <div class="flex flex-col items-center mb-4">
                        <svg viewBox="0 0 180 180" width="32" height="32" aria-label="Trakkt" class="dark:hidden">
                            <path d="M 18 18 L 78 18 L 78 44 L 52 44 L 52 136 L 78 136 L 78 162 L 18 162 Z" fill="#0D9488"/>
                            <path d="M 162 18 L 102 18 L 102 44 L 128 44 L 128 136 L 102 136 L 102 162 L 162 162 Z" fill="#0D9488"/>
                        </svg>
                        <svg viewBox="0 0 180 180" width="32" height="32" aria-label="Trakkt" class="hidden dark:block">
                            <path d="M 18 18 L 78 18 L 78 44 L 52 44 L 52 136 L 78 136 L 78 162 L 18 162 Z" fill="white"/>
                            <path d="M 162 18 L 102 18 L 102 44 L 128 44 L 128 136 L 102 136 L 102 162 L 162 162 Z" fill="white"/>
                        </svg>
                        <div class="text-sm font-bold text-foreground mt-1.5">"Trakkt"</div>
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
