// SPDX-License-Identifier: AGPL-3.0-or-later

//! App layout — sidebar + content area.

use leptos::prelude::*;
use leptos_router::components::Outlet;

use crate::components::Spinner;
use crate::server_fns::context::UserContext;
use crate::server_fns::sidebar::{get_sidebar_user, list_user_workspaces, switch_workspace, SidebarUser};

/// Main authenticated layout with sidebar and content area.
#[component]
pub fn Layout() -> impl IntoView {
    let user_ctx = expect_context::<LocalResource<Result<UserContext, ServerFnError>>>();
    let user_info = LocalResource::new(get_sidebar_user);
    let (user_menu_open, set_user_menu_open) = signal(false);

    let auth_confirmed = RwSignal::new(false);
    let nav = leptos_router::hooks::use_navigate();

    Effect::new(move || {
        match user_ctx.get() {
            Some(Ok(_)) => auth_confirmed.set(true),
            Some(Err(_)) => {
                nav("/login", Default::default());
            }
            None => {}
        }
    });

    view! {
        <Show
            when=move || auth_confirmed.get()
            fallback=|| view! {
                <div class="min-h-screen flex items-center justify-center">
                    <Spinner/>
                </div>
            }
        >
            <div class="h-dvh flex bg-background">
                <Sidebar user_info=user_info user_menu_open=user_menu_open set_user_menu_open=set_user_menu_open/>
                <main class="flex-1 overflow-y-auto">
                    <Outlet/>
                </main>
            </div>
        </Show>
    }
}

/// Sidebar component with navigation and user menu.
#[component]
fn Sidebar(
    user_info: LocalResource<Result<SidebarUser, ServerFnError>>,
    user_menu_open: ReadSignal<bool>,
    set_user_menu_open: WriteSignal<bool>,
) -> impl IntoView {

    view! {
        <div class="w-64 bg-[var(--color-sidebar)] border-r border-[var(--color-sidebar-border)] text-[var(--color-sidebar-foreground)] flex flex-col h-full">
            // Logo
            <div class="p-4 border-b border-[var(--color-sidebar-border)]">
                <a href="/">
                    <img src="/tane_full_logo_white.svg" alt="Tane" class="h-8"/>
                </a>
            </div>

            // Navigation
            <nav class="flex-1 p-3 space-y-1">
                <a href="/settings/profile" class="flex items-center gap-3 px-3 py-2 rounded-md text-sm text-[var(--color-sidebar-foreground-secondary)] hover:text-[var(--color-sidebar-foreground)] hover:bg-[var(--color-sidebar-hover)] transition-colors">
                    <svg class="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                        <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M10.325 4.317c.426-1.756 2.924-1.756 3.35 0a1.724 1.724 0 002.573 1.066c1.543-.94 3.31.826 2.37 2.37a1.724 1.724 0 001.066 2.573c1.756.426 1.756 2.924 0 3.35a1.724 1.724 0 00-1.066 2.573c.94 1.543-.826 3.31-2.37 2.37a1.724 1.724 0 00-2.573 1.066c-.426 1.756-2.924 1.756-3.35 0a1.724 1.724 0 00-2.573-1.066c-1.543.94-3.31-.826-2.37-2.37a1.724 1.724 0 00-1.066-2.573c-1.756-.426-1.756-2.924 0-3.35a1.724 1.724 0 001.066-2.573c-.94-1.543.826-3.31 2.37-2.37.996.608 2.296.07 2.572-1.065z"/>
                        <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M15 12a3 3 0 11-6 0 3 3 0 016 0z"/>
                    </svg>
                    "Settings"
                </a>
            </nav>

            // User menu at bottom
            <div class="border-t border-[var(--color-sidebar-border)] p-3 relative">
                <Suspense fallback=|| view! {
                    <div class="px-3 py-2 text-sm text-[var(--color-sidebar-foreground-muted)]">"Loading..."</div>
                }>
                    {move || user_info.get().map(|result| {
                        match result {
                            Ok(ref user) => {
                            let display_name = user.name.clone().unwrap_or_else(|| user.email.clone());
                            let avatar_char = user.name.as_ref().and_then(|n| n.chars().next()).unwrap_or('?').to_uppercase().to_string();
                            let ws_name = user.workspace_name.clone().unwrap_or_default();
                            view! {
                                <button
                                    class="w-full flex items-center gap-3 px-3 py-2 rounded-md text-sm hover:bg-[var(--color-sidebar-hover)] transition-colors text-left"
                                    on:click=move |_| set_user_menu_open.update(|v| *v = !*v)
                                >
                                    // Avatar
                                    <div class="w-8 h-8 rounded-full bg-primary flex items-center justify-center text-primary-foreground text-xs font-semibold flex-shrink-0">
                                        {avatar_char.clone()}
                                    </div>
                                    <div class="flex-1 min-w-0">
                                        <div class="text-[var(--color-sidebar-foreground)] font-medium truncate text-sm">
                                            {display_name.clone()}
                                        </div>
                                        <div class="text-[var(--color-sidebar-foreground-muted)] text-xs truncate">
                                            {ws_name.clone()}
                                        </div>
                                    </div>
                                    // Chevron
                                    <svg class="w-4 h-4 text-[var(--color-sidebar-foreground-muted)]" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                                        <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M19 9l-7 7-7-7"/>
                                    </svg>
                                </button>

                                // Dropdown menu
                                <Show when=move || user_menu_open.get()>
                                    <div class="absolute bottom-full left-3 right-3 mb-1 bg-popover border border-border rounded-lg shadow-lg py-1 z-50">
                                        // Workspace switcher (includes separator only when shown)
                                        <WorkspaceSwitcher set_user_menu_open=set_user_menu_open/>
                                        <a href="/settings/profile" class="block px-4 py-2 text-sm text-foreground hover:bg-secondary transition-colors">
                                            "Settings"
                                        </a>
                                        <button
                                            class="w-full text-left px-4 py-2 text-sm text-foreground hover:bg-secondary transition-colors"
                                            on:click=move |_| {
                                                set_user_menu_open.set(false);
                                                leptos::task::spawn_local(async move {
                                                    let _ = crate::server_fns::security::logout().await;
                                                    let _ = web_sys::window()
                                                        .and_then(|w| w.location().set_href("/login").ok());
                                                });
                                            }
                                        >
                                            "Sign Out"
                                        </button>
                                    </div>
                                </Show>
                            }.into_any()
                            },
                            Err(_) => view! {
                                <div class="px-3 py-2 text-sm text-[var(--color-sidebar-foreground-muted)]">"Not signed in"</div>
                            }.into_any(),
                        }
                    })}
                </Suspense>
            </div>
        </div>
    }
}

/// Workspace switcher — shows list of workspaces, allows switching.
#[component]
fn WorkspaceSwitcher(set_user_menu_open: WriteSignal<bool>) -> impl IntoView {
    let workspaces = LocalResource::new(list_user_workspaces);

    view! {
        <Suspense fallback=|| ()>
            {move || workspaces.get().map(|result| {
                match result {
                    Ok(ref ws_list) if ws_list.len() > 1 => {
                        let items = ws_list.clone();
                        view! {
                            <div class="px-3 py-1 text-xs text-muted-foreground font-medium uppercase tracking-wider">
                                "Workspaces"
                            </div>
                            {items.into_iter().map(|ws| {
                                let ws_id = ws.workspace_id.clone();
                                let name = ws.name.clone();
                                let is_current = ws.is_current;
                                view! {
                                    <button
                                        class="w-full text-left px-4 py-2 text-sm hover:bg-secondary transition-colors flex items-center gap-2"
                                        disabled=is_current
                                        on:click=move |_| {
                                            let ws_id = ws_id.clone();
                                            set_user_menu_open.set(false);
                                            leptos::task::spawn_local(async move {
                                                let _ = switch_workspace(ws_id).await;
                                                let _ = web_sys::window()
                                                    .and_then(|w| w.location().reload().ok());
                                            });
                                        }
                                    >
                                        {if is_current {
                                            view! { <span class="w-2 h-2 rounded-full bg-primary flex-shrink-0"/> }.into_any()
                                        } else {
                                            view! { <span class="w-2 h-2 flex-shrink-0"/> }.into_any()
                                        }}
                                        <span class=if is_current { "font-medium text-foreground" } else { "text-foreground" }>
                                            {name}
                                        </span>
                                    </button>
                                }
                            }).collect_view()}
                            <div class="border-t border-border my-1"/>
                        }.into_any()
                    }
                    _ => view! { <span/> }.into_any(),
                }
            })}
        </Suspense>
    }
}
