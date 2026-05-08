// SPDX-License-Identifier: AGPL-3.0-or-later

//! Shared layout component for all auth pages (login, signup, recovery).
//!
//! Matches `apps/frontend/src/pages/Login.jsx` layout structure and CSS classes.
//! Left panel: dark background with Trakkt branding (desktop only).
//! Right panel: centered content slot with title, subtitle, footer.

use leptos::prelude::*;

/// Shared layout for all authentication pages.
///
/// Renders the two-panel auth layout from React Login.jsx:
/// - Left: dark branded panel (hidden on mobile, `lg:flex lg:w-1/2`)
/// - Right: centered content with mobile logo, title/subtitle, children slot, and footer
///
/// Title and subtitle are reactive (`Signal<String>`) so the login page can
/// toggle between "Welcome back" / "Create your account" without remounting.
#[component]
pub fn AuthLayout(
    /// Title text (e.g., "Welcome back" or "Create your account").
    #[prop(into)]
    title: Signal<String>,
    /// Subtitle text (e.g., "Sign in to your account to continue").
    #[prop(into)]
    subtitle: Signal<String>,
    /// Main content slot (form fields, buttons, etc.).
    children: Children,
) -> impl IntoView {
    view! {
        // Outer container — theme-aware, respects user's localStorage preference
        <div class="min-h-screen bg-background flex">

            // ── Left side — Full Trakkt logo on warm-stone panel ─────────────
            // Hidden below 1024px (lg breakpoint); form fills the whole
            // viewport on mobile and tablets. Decorative — `aria-hidden`.
            <div
                class="hidden lg:flex lg:w-1/2 relative overflow-hidden auth-brand-panel items-center justify-center"
                aria-hidden="true"
            >
                <div class="flex flex-col items-center select-none">
                    <svg viewBox="0 0 180 180" width="48" height="48" aria-label="Trakkt">
                        <path d="M 18 18 L 78 18 L 78 44 L 52 44 L 52 136 L 78 136 L 78 162 L 18 162 Z" fill="white"/>
                        <path d="M 162 18 L 102 18 L 102 44 L 128 44 L 128 136 L 102 136 L 102 162 L 162 162 Z" fill="white"/>
                    </svg>
                    <div class="text-xl font-bold text-white mt-3">"Trakkt"</div>
                </div>

                // Bottom marginalia
                <div class="absolute bottom-10 left-12 right-12 z-10 flex items-center justify-between font-mono text-[10px] uppercase text-[color:rgba(245,243,239,0.30)]" style="letter-spacing:0.18em;">
                    <span>"TRAKKT"</span>
                    <span>"TRAKKT"</span>
                </div>
            </div>

            // ── Right side — Form content ───────────────────────────────────
            // React: className="w-full lg:w-1/2 flex items-center justify-center p-8"
            <div class="w-full lg:w-1/2 flex items-center justify-center p-8">
                // React: className="w-full max-w-md"
                <div class="w-full max-w-md">

                    // ── Mobile logo + title/subtitle ────────────────────────
                    // React: className="text-center mb-8"
                    <div class="text-center mb-8">
                        // Mobile logo — React: className="lg:hidden mb-6"
                        <div class="lg:hidden mb-6 flex flex-col items-center">
                            // Light mode: teal logo
                            <svg viewBox="0 0 180 180" width="36" height="36" aria-label="Trakkt" class="dark:hidden">
                                <path d="M 18 18 L 78 18 L 78 44 L 52 44 L 52 136 L 78 136 L 78 162 L 18 162 Z" fill="#0D9488"/>
                                <path d="M 162 18 L 102 18 L 102 44 L 128 44 L 128 136 L 102 136 L 102 162 L 162 162 Z" fill="#0D9488"/>
                            </svg>
                            // Dark mode: white logo
                            <svg viewBox="0 0 180 180" width="36" height="36" aria-label="Trakkt" class="hidden dark:block">
                                <path d="M 18 18 L 78 18 L 78 44 L 52 44 L 52 136 L 78 136 L 78 162 L 18 162 Z" fill="white"/>
                                <path d="M 162 18 L 102 18 L 102 44 L 128 44 L 128 136 L 102 136 L 102 162 L 162 162 Z" fill="white"/>
                            </svg>
                            <div class="text-base font-bold text-foreground mt-2">"Trakkt"</div>
                        </div>
                        // Title — page-level landmark, DESIGN.md: 2xl token = 30px = text-3xl, Instrument Serif
                        <h1 class="text-3xl font-display text-foreground mb-2">
                            {title}
                        </h1>
                        <p class="text-[18px] text-muted-foreground italic font-display leading-tight mb-8">
                            {subtitle}
                        </p>
                    </div>

                    // ── Main content slot ────────────────────────────────────
                    {children()}

                    // ── Footer ───────────────────────────────────────────────
                    // React: className="mt-8 pt-6 border-t border-border space-y-3"
                    <div class="mt-8 pt-6 border-t border-border space-y-3">
                        // Flex row — no `space-x-*`; each link carries its own
                        // `py-3 px-2` padding so the hit area reaches the WCAG
                        // 2.5.5 AAA minimum of 44x44px without changing visible
                        // text size. `py-3` = 12px top + 12px bottom + ~20px
                        // line-height on `text-sm` = 44px total.
                        <div class="flex justify-center items-center text-sm text-muted-foreground">
                            <a
                                href="https://trakkt.app/privacy"
                                target="_blank"
                                rel="noopener noreferrer"
                                class="inline-block py-3 px-2 hover:text-foreground transition-colors"
                            >
                                "Privacy"
                            </a>
                            <span aria-hidden="true">"·"</span>
                            <a
                                href="https://trakkt.app/terms"
                                target="_blank"
                                rel="noopener noreferrer"
                                class="inline-block py-3 px-2 hover:text-foreground transition-colors"
                            >
                                "Terms"
                            </a>
                            <span aria-hidden="true">"·"</span>
                            <a
                                href="https://trakkt.app"
                                target="_blank"
                                rel="noopener noreferrer"
                                class="inline-block py-3 px-2 hover:text-foreground transition-colors"
                            >
                                "About"
                            </a>
                            <span aria-hidden="true">"·"</span>
                            <a
                                href="https://status.trakkt.app"
                                target="_blank"
                                rel="noopener noreferrer"
                                class="inline-block py-3 px-2 hover:text-foreground transition-colors"
                            >
                                "Status"
                            </a>
                        </div>
                        // React: className="text-xs text-muted-foreground text-center"
                        <p class="text-xs text-muted-foreground text-center">
                            "All trademarks are property of their respective owners."
                        </p>
                    </div>
                </div>
            </div>
        </div>
    }
}
