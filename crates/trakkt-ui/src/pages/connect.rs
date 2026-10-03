// SPDX-License-Identifier: AGPL-3.0-or-later

//! Session-owned terminals backed by the user's Connect agent.

use leptos::prelude::*;
use phosphor_leptos::{Icon, IconWeight};

use crate::components::alert::{Alert, AlertDescription, AlertVariant};
use crate::components::button::{Button, ButtonSize, ButtonVariant, ToggleButton};
use crate::components::terminal::tab_manager::{SessionTab, TabBar};
use crate::components::terminal::{Grid, TerminalRenderer};

#[component]
pub fn ConnectPage() -> impl IntoView {
    let tabs = RwSignal::<Vec<SessionTab>>::new(Vec::new());
    let active = RwSignal::<Option<String>>::new(None);
    let grid = RwSignal::new(Grid::new(80, 24));
    let browser_connected = RwSignal::new(false);
    let agent_connected = RwSignal::new(false);
    let error = RwSignal::<Option<String>>::new(None);
    let expanded = RwSignal::new(false);
    let terminal_ref = NodeRef::<leptos::html::Div>::new();

    #[cfg(target_arch = "wasm32")]
    let actions = crate::components::terminal::client::start(
        tabs,
        active,
        grid,
        browser_connected,
        agent_connected,
        error,
        terminal_ref,
    );
    #[cfg(not(target_arch = "wasm32"))]
    let (new_shell, new_claude, select, close) = (
        Callback::new(|()| {}),
        Callback::new(|()| {}),
        Callback::new(|_: String| {}),
        Callback::new(|_: String| {}),
    );
    #[cfg(target_arch = "wasm32")]
    let (new_shell, new_claude, select, close) = (
        actions.new_shell,
        actions.new_claude,
        actions.select,
        actions.close,
    );

    view! {
        <div class=move || if expanded.get() {
            "fixed inset-0 z-50 flex flex-col bg-background"
        } else { "flex flex-col h-full min-h-0 bg-background" }>
            <div class="page-header flex items-center justify-between gap-3 px-5 shrink-0">
                <h1 class="text-sm font-semibold text-foreground">"Connect"</h1>
                <div class="flex items-center gap-3">
                    <span data-testid="connect-status" class="text-sm text-muted-foreground" aria-live="polite">
                        {move || if agent_connected.get() { "Agent connected" } else { "Agent disconnected" }}
                    </span>
                    <ToggleButton variant=Signal::derive(move || if expanded.get() { ButtonVariant::Active } else { ButtonVariant::GhostMuted })
                        size=ButtonSize::IconSm
                        aria_label=Signal::derive(move || if expanded.get() { "Exit fullscreen".to_string() } else { "Enter fullscreen".to_string() })
                        on:click=move |_| expanded.update(|expanded| *expanded = !*expanded)>
                        <Icon icon=phosphor_leptos::CORNERS_OUT weight=IconWeight::Regular size="16px"/>
                    </ToggleButton>
                </div>
            </div>
            <Show when=move || !agent_connected.get()>
                <div class="px-5 pb-3">
                    <Alert variant=AlertVariant::Info>
                        <AlertDescription>
                            {move || if browser_connected.get() {
                                "Start trakkt-connect on your computer and connect it to this workspace. Your existing terminal output stays available while the agent is disconnected."
                            } else { "Connecting to the terminal service… Your existing terminal output stays available." }}
                            " " <a href="/docs/self-hosting/connect.html" class="underline">"Connect setup"</a>
                        </AlertDescription>
                    </Alert>
                </div>
            </Show>
            <Show when=move || error.get().is_some()>
                <div class="px-5 pb-3" data-testid="connect-error">
                    <Alert variant=AlertVariant::Error>
                        <AlertDescription>{move || error.get()}</AlertDescription>
                    </Alert>
                </div>
            </Show>
            <div class="flex items-center gap-2 px-5 pb-3 shrink-0">
                <Button variant=ButtonVariant::Secondary size=ButtonSize::Sm
                    disabled=Signal::derive(move || !browser_connected.get() || !agent_connected.get())
                    on:click=move |_| new_shell.run(())>"New shell session"</Button>
                <Button variant=ButtonVariant::Secondary size=ButtonSize::Sm
                    disabled=Signal::derive(move || !browser_connected.get() || !agent_connected.get())
                    on:click=move |_| new_claude.run(())>"New Claude session"</Button>
            </div>
            <TabBar tabs=Signal::derive(move || tabs.get()) active_session=Signal::derive(move || active.get())
                on_select=select on_close=close/>
            <div node_ref=terminal_ref data-testid="connect-terminal" tabindex="0"
                aria-label="Terminal" role="region"
                class="flex-1 min-h-0 overflow-auto bg-[#1e1e1e] focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring">
                <Show when=move || active.get().is_some() fallback=|| view! {
                    <div class="flex items-center justify-center h-full text-muted-foreground text-sm">
                        "No active sessions. Start a shell or Claude session when your agent is connected."
                    </div>
                }>
                    <TerminalRenderer grid=grid/>
                </Show>
            </div>
        </div>
    }
}
