// SPDX-License-Identifier: AGPL-3.0-or-later

//! App layout — sidebar + content area.

use leptos::prelude::*;
use leptos_router::components::Outlet;

use wasm_bindgen::closure::Closure;
use wasm_bindgen::JsCast;

use phosphor_leptos::{Icon, IconWeight};

use std::sync::Arc;

use crate::cache::store::SyncStore;
use crate::components::{Button, ButtonVariant, CommandPalette, Modal, ModalSize, Spinner, INPUT_CLASS};
use crate::components::popover::{Popover, Placement};
use crate::server_fns::sidebar::{get_sidebar_user, list_user_workspaces, switch_workspace, SidebarUser};

/// Main authenticated layout with sidebar and content area.
#[component]
pub fn Layout() -> impl IntoView {
    let user_info = LocalResource::new(get_sidebar_user);
    let (user_menu_open, set_user_menu_open) = signal(false);
    let (mobile_sidebar_open, set_mobile_sidebar_open) = signal(false);
    let (show_palette, set_show_palette) = signal(false);

    // Provide SyncStore on all targets so page components can reference it.
    // On SSR it remains empty; on WASM the sync engine populates it.
    let sync_store = SyncStore::new();
    provide_context(sync_store);

    let auth_confirmed = RwSignal::new(false);
    let nav = leptos_router::hooks::use_navigate();

    Effect::new(move || {
        match user_info.get() {
            Some(Ok(_)) => auth_confirmed.set(true),
            Some(Err(_)) => {
                nav("/login", Default::default());
            }
            None => {}
        }
    });

    // ── Sync engine wiring (WASM only) ────────────────────────────────────
    // Once auth is confirmed and user context is available:
    // 1. Hydrate the store from IndexedDB for instant UI
    // 2. Connect the WebSocket
    // 3. Start the sync engine to keep data current
    #[cfg(target_arch = "wasm32")]
    {
        use crate::cache::websocket;
        use crate::cache::sync_engine;
        use crate::server_fns::context::UserContext;

        let user_ctx = expect_context::<LocalResource<Result<UserContext, ServerFnError>>>();

        // Track whether we've already started the sync engine to avoid
        // re-connecting on every reactive re-fire.
        let sync_started = std::rc::Rc::new(std::cell::Cell::new(false));

        Effect::new(move |_| {
            web_sys::console::log_1(&"[trakkt-sync] Effect fired, checking user_ctx".into());
            // Wait for user context to resolve successfully.
            let Some(Ok(ctx)) = user_ctx.get() else {
                web_sys::console::log_1(&"[trakkt-sync] user_ctx not ready yet".into());
                return;
            };

            if sync_started.get() {
                web_sys::console::log_1(&"[trakkt-sync] already started, skipping".into());
                return;
            }
            sync_started.set(true);

            let user_id = ctx.user_id.clone();
            let workspace_id = ctx
                .workspace_id
                .clone()
                .unwrap_or_else(|| "workspace-local".to_string());
            web_sys::console::log_1(&format!("[trakkt-sync] starting sync for {user_id} / {workspace_id}").into());

            // 1. Hydrate from IDB (instant cached data)
            let wid_hydrate = workspace_id.clone();
            leptos::task::spawn_local(async move {
                match crate::cache::db::init_cache_db(&wid_hydrate).await {
                    Ok(cache_db) => {
                        sync_engine::hydrate_store_from_db(&cache_db, &wid_hydrate, &sync_store)
                            .await;
                    }
                    Err(e) => {
                        web_sys::console::warn_1(&format!("Failed to open IDB: {e}").into());
                    }
                }
            });

            // 2. Connect WebSocket — start with empty token (connects immediately
            //    so provide_context works in the reactive scope). Then fetch a
            //    JWT asynchronously and reconnect with it for multi-user mode.
            let ws_client = websocket::connect(&user_id, &workspace_id, "");

            sync_engine::start_sync_engine(&ws_client, &sync_store, &workspace_id);

            let ws_for_cleanup = ws_client.clone();
            provide_context(ws_client.clone());

            on_cleanup(move || {
                websocket::disconnect(&ws_for_cleanup);
            });

            // Fetch JWT and reconnect with auth (multi-user mode only).
            let ws_for_reconnect = ws_client;
            let uid_reconnect = user_id.clone();
            let wid_reconnect = workspace_id.clone();
            leptos::task::spawn_local(async move {
                if let Ok(token) = crate::server_fns::auth::get_ws_token().await {
                    if !token.is_empty() {
                        ws_for_reconnect.reconnect(&uid_reconnect, &wid_reconnect, &token);
                    }
                }
            });
        });
    }

    // ── Global Cmd+K / Ctrl+K listener ─────────────────────────────────────
    Effect::new(move |_| {
        let Some(window) = web_sys::window() else { return };
        let cb = Closure::<dyn Fn(web_sys::KeyboardEvent)>::new(move |ev: web_sys::KeyboardEvent| {
            // Cmd+K (macOS) or Ctrl+K (Linux/Windows) toggles the command palette.
            if ev.key() == "k" && (ev.meta_key() || ev.ctrl_key()) {
                ev.prevent_default();
                set_show_palette.update(|v| *v = !*v);
            }
        });
        let _ = window.add_event_listener_with_callback(
            "keydown",
            cb.as_ref().unchecked_ref(),
        );
        let cb_cleanup = send_wrapper::SendWrapper::new(cb);
        on_cleanup(move || {
            let Some(window) = web_sys::window() else { return };
            let cb = cb_cleanup.take();
            let _ = window.remove_event_listener_with_callback(
                "keydown",
                cb.as_ref().unchecked_ref(),
            );
        });
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
                // Desktop sidebar
                <div class="hidden md:block">
                    <Sidebar user_info=user_info user_menu_open=user_menu_open set_user_menu_open=set_user_menu_open/>
                </div>

                // Mobile sidebar overlay
                <Show when=move || mobile_sidebar_open.get()>
                    <div class="fixed inset-0 z-40 md:hidden">
                        <div
                            class="fixed inset-0 bg-black/50"
                            on:click=move |_| set_mobile_sidebar_open.set(false)
                        />
                        <div class="fixed inset-y-0 left-0 z-50 w-[220px]">
                            <Sidebar user_info=user_info user_menu_open=user_menu_open set_user_menu_open=set_user_menu_open/>
                        </div>
                    </div>
                </Show>

                <div class="flex-1 flex flex-col overflow-hidden">
                    // Mobile header bar
                    <div class="md:hidden flex items-center gap-3 px-4 py-3 border-b border-border bg-background">
                        <button
                            class="p-2 rounded-md hover:bg-secondary transition-colors"
                            on:click=move |_| set_mobile_sidebar_open.update(|v| *v = !*v)
                            aria-label="Toggle menu"
                        >
                            <svg class="w-5 h-5 text-foreground" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                                <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M4 6h16M4 12h16M4 18h16"/>
                            </svg>
                        </button>
                        <a href="/" class="flex items-center gap-2">
                            // Light mode: teal logo
                            <svg viewBox="0 0 180 180" width="18" height="18" aria-label="Trakkt" class="dark:hidden">
                                <path d="M 18 18 L 78 18 L 78 44 L 52 44 L 52 136 L 78 136 L 78 162 L 18 162 Z" fill="#0D9488"/>
                                <path d="M 162 18 L 102 18 L 102 44 L 128 44 L 128 136 L 102 136 L 102 162 L 162 162 Z" fill="#0D9488"/>
                            </svg>
                            // Dark mode: white logo
                            <svg viewBox="0 0 180 180" width="18" height="18" aria-label="Trakkt" class="hidden dark:block">
                                <path d="M 18 18 L 78 18 L 78 44 L 52 44 L 52 136 L 78 136 L 78 162 L 18 162 Z" fill="white"/>
                                <path d="M 162 18 L 102 18 L 102 44 L 128 44 L 128 136 L 102 136 L 102 162 L 162 162 Z" fill="white"/>
                            </svg>
                            <span class="text-sm font-bold text-foreground">"Trakkt"</span>
                        </a>
                    </div>
                    <main class="flex-1 overflow-y-auto">
                        <Outlet/>
                    </main>
                </div>
            </div>

            // Command palette — rendered at the app level so it's available on all pages.
            <CommandPalette
                show=Signal::derive(move || show_palette.get())
                on_close=Callback::new(move |()| set_show_palette.set(false))
            />
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
    let user_menu_trigger_ref = NodeRef::<leptos::html::Div>::new();

    view! {
        <div class="w-[220px] bg-[var(--color-sidebar)] border-r border-[var(--color-sidebar-border)] text-[var(--color-sidebar-foreground)] flex flex-col h-full">
            // Logo
            <div class="p-4 border-b border-[var(--color-sidebar-border)]">
                <a href="/" class="flex items-center gap-2">
                    <svg viewBox="0 0 180 180" width="18" height="18" aria-label="Trakkt">
                        <path d="M 18 18 L 78 18 L 78 44 L 52 44 L 52 136 L 78 136 L 78 162 L 18 162 Z" fill="#F5F3EF"/>
                        <path d="M 162 18 L 102 18 L 102 44 L 128 44 L 128 136 L 102 136 L 102 162 L 162 162 Z" fill="#F5F3EF"/>
                    </svg>
                    <span class="text-sm font-bold text-[var(--color-sidebar-foreground)]">"Trakkt"</span>
                </a>
            </div>

            // Navigation
            <nav class="flex-1 p-3 space-y-1 overflow-y-auto">
                <SidebarNavItem href="/my-issues" icon=phosphor_leptos::LIST_CHECKS label="My Issues"/>

                <SidebarFavoritesSection/>

                <SidebarTeamsSection/>
                <SidebarProjectsSection/>
            </nav>

            // User menu at bottom
            <div class="border-t border-[var(--color-sidebar-border)] p-3">
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
                                <div node_ref=user_menu_trigger_ref>
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
                                </div>

                                // Dropdown menu — portalled via Popover for click-outside, Escape, and viewport-aware positioning
                                <Popover
                                    trigger_ref=user_menu_trigger_ref
                                    open=Signal::derive(move || user_menu_open.get())
                                    on_close=Callback::new(move |()| set_user_menu_open.set(false))
                                    placement=Placement::TOP_START
                                    match_width=true
                                    class="bg-popover border border-border rounded-lg shadow-lg py-1"
                                >
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
                                </Popover>
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

/// Section header for "Teams" with dynamic team list from SyncStore.
/// Teams are collapsible and the section includes create/join actions.
#[component]
fn SidebarTeamsSection() -> impl IntoView {
    let store = use_context::<SyncStore>();
    let (show_create, set_show_create) = signal(false);

    view! {
        {move || {
            let Some(store) = store else { return view! { <span/> }.into_any() };
            let teams = store.teams().get();
            view! {
                <div class="group flex items-center justify-between px-3 pt-4 pb-1">
                    <span class="text-[10px] font-semibold uppercase tracking-wider text-[var(--color-sidebar-foreground-muted)]">
                        "Teams"
                    </span>
                    <button
                        class="p-0.5 rounded text-[var(--color-sidebar-foreground-muted)] hover:text-[var(--color-sidebar-foreground)] hover:bg-[var(--color-sidebar-hover)] transition-colors opacity-0 group-hover:opacity-100"
                        on:click=move |_| set_show_create.update(|v| *v = !*v)
                        title="Create or join a team"
                    >
                        <Icon icon=phosphor_leptos::PLUS weight=IconWeight::Bold size="12px"/>
                    </button>
                </div>

                <Show when=move || show_create.get()>
                    <SidebarCreateTeam on_done=Callback::new(move |()| set_show_create.set(false))/>
                </Show>

                <div class="space-y-0.5">
                    {teams.into_iter().map(|team| {
                        let key_lower = team.key.to_lowercase();
                        let name = team.name.clone();
                        let team_id = team.team_id.clone();
                        let team_key = team.key.clone();
                        let issues_href = format!("/teams/{key_lower}/issues");
                        view! {
                            <SidebarTeamSubNav team_id=team_id name=name team_key=team_key issues_href=issues_href/>
                        }
                    }).collect_view()}
                </div>
            }.into_any()
        }}
    }
}

/// Section for "Favorites" — shows user-pinned teams, projects, and views.
///
/// Only renders when favorites exist. Each favorite resolves its name, icon,
/// and link from the corresponding entity in SyncStore.
#[component]
fn SidebarFavoritesSection() -> impl IntoView {
    let store = use_context::<SyncStore>();

    view! {
        {move || {
            let Some(store) = store else { return view! { <span/> }.into_any() };
            let favorites = store.favorites().get();
            if favorites.is_empty() {
                return view! { <span/> }.into_any();
            }
            let teams = store.teams().get();
            let projects = store.projects().get();
            // Limit to 10 items.
            let items: Vec<_> = favorites.into_iter().take(10).collect();
            view! {
                <SidebarSectionHeader label="Favorites"/>
                <div class="space-y-0.5">
                    {items.into_iter().filter_map(|fav| {
                        match fav.target_type.as_str() {
                            "team" => {
                                let team = teams.iter().find(|t| t.team_id == fav.target_id)?;
                                let key = team.key.to_lowercase();
                                let href = format!("/teams/{key}/issues");
                                let name = team.name.clone();
                                Some(view! {
                                    <SidebarEntityItem href=href name=name icon=phosphor_leptos::USERS_THREE/>
                                }.into_any())
                            }
                            "project" => {
                                let project = projects.iter().find(|p| p.project_id == fav.target_id)?;
                                let href = format!("/projects/{}", project.project_id);
                                let name = project.name.clone();
                                Some(view! {
                                    <SidebarEntityItem href=href name=name icon=phosphor_leptos::FOLDER/>
                                }.into_any())
                            }
                            // View favorites are skipped for now — the view model
                            // doesn't include the team key needed to build the URL
                            // to the team issues page where the view tab lives.
                            "view" => None,
                            _ => None,
                        }
                    }).collect_view()}
                </div>
            }.into_any()
        }}
    }
}

/// Small star button that toggles favorite state for a given target.
///
/// Shows a filled star when favorited, outline when not. On click, calls
/// `add_favorite` or `remove_favorite` server function.
#[component]
pub fn FavoriteToggle(
    target_type: &'static str,
    target_id: String,
) -> impl IntoView {
    let store = use_context::<SyncStore>();
    let tid = target_id.clone();
    let is_fav = Signal::derive(move || {
        store
            .map(|s| {
                s.favorites()
                    .get()
                    .iter()
                    .any(|f| f.target_type == target_type && f.target_id == tid)
            })
            .unwrap_or(false)
    });

    let toggling = RwSignal::new(false);
    let target_id_click = target_id.clone();

    let on_click = move |ev: web_sys::MouseEvent| {
        ev.prevent_default();
        ev.stop_propagation();
        if toggling.get_untracked() {
            return;
        }
        toggling.set(true);
        let tt = target_type.to_string();
        let ti = target_id_click.clone();
        let currently_fav = is_fav.get_untracked();
        leptos::task::spawn_local(async move {
            if currently_fav {
                let _ = crate::server_fns::favorites::remove_favorite(tt, ti).await;
            } else {
                let _ = crate::server_fns::favorites::add_favorite(tt, ti).await;
            }
            toggling.set(false);
        });
    };

    let weight = Signal::derive(move || {
        if is_fav.get() { IconWeight::Fill } else { IconWeight::Light }
    });
    let star_class = move || {
        let base = "p-0.5 rounded transition-colors flex-shrink-0";
        if is_fav.get() {
            format!("{base} text-amber-400 hover:text-amber-300")
        } else {
            format!("{base} text-[var(--color-sidebar-foreground-muted)] hover:text-amber-400")
        }
    };

    view! {
        <button
            class=star_class
            on:click=on_click
            title=move || if is_fav.get() { "Remove from favorites" } else { "Add to favorites" }
        >
            <Icon icon=phosphor_leptos::STAR weight=weight size="14px"/>
        </button>
    }
}

/// A sidebar entity link with active-state tracking. Used for views, projects, etc.
///
/// When `favorite_type` and `favorite_id` are provided, a star toggle button
/// appears on hover to let users pin/unpin the item from their favorites.
#[component]
fn SidebarEntityItem(
    href: String,
    name: String,
    icon: phosphor_leptos::IconData,
    #[prop(optional)] favorite_type: Option<&'static str>,
    #[prop(optional)] favorite_id: Option<String>,
) -> impl IntoView {
    let path = leptos_router::hooks::use_location().pathname;
    let href_match = href.clone();
    let is_active = Signal::derive(move || {
        path.get().starts_with(&href_match)
    });
    let weight = Signal::derive(move || {
        if is_active.get() { IconWeight::Fill } else { IconWeight::Light }
    });
    let wrapper_class = move || {
        let base = "group flex items-center rounded-md transition-colors";
        if is_active.get() {
            format!("{base} bg-[var(--color-sidebar-active)]")
        } else {
            format!("{base} hover:bg-[var(--color-sidebar-hover)]")
        }
    };
    let link_class = move || {
        let base = "flex-1 min-w-0 flex items-center gap-3 px-3 py-1.5 text-sm";
        if is_active.get() {
            format!("{base} text-[var(--color-sidebar-foreground)] font-medium")
        } else {
            format!("{base} text-[var(--color-sidebar-foreground-secondary)] hover:text-[var(--color-sidebar-foreground)]")
        }
    };
    view! {
        <div class=wrapper_class>
            <a href=href class=link_class>
                <Icon icon=icon weight=weight size="16px"/>
                <span class="truncate">{name}</span>
            </a>
            {favorite_type.zip(favorite_id.as_ref()).map(|(ft, fi)| {
                let fi = fi.clone();
                view! {
                    <div class="flex items-center gap-1 pr-2 opacity-0 group-hover:opacity-100 transition-opacity">
                        <FavoriteToggle target_type=ft target_id=fi/>
                    </div>
                }
            })}
        </div>
    }
}

/// Section header for "Projects" with dynamic project list from SyncStore.
#[component]
fn SidebarProjectsSection() -> impl IntoView {
    let store = use_context::<SyncStore>();

    view! {
        {move || {
            let Some(store) = store else { return view! { <span/> }.into_any() };
            let projects = store.projects().get();
            if projects.is_empty() {
                return view! { <span/> }.into_any();
            }
            view! {
                <SidebarSectionHeader label="Projects"/>
                <div class="space-y-0.5">
                    {projects.into_iter().map(|project| {
                        let fav_id = project.project_id.clone();
                        let href = format!("/projects/{}", project.project_id);
                        let name = project.name.clone();
                        view! {
                            <SidebarEntityItem href=href name=name icon=phosphor_leptos::FOLDER favorite_type="project" favorite_id=fav_id/>
                        }
                    }).collect_view()}
                </div>
            }.into_any()
        }}
    }
}


/// Small, uppercase, muted section header (Linear-style).
#[component]
fn SidebarSectionHeader(label: &'static str) -> impl IntoView {
    view! {
        <div class="px-3 pt-4 pb-1 text-[10px] font-semibold uppercase tracking-wider text-[var(--color-sidebar-foreground-muted)]">
            {label}
        </div>
    }
}

/// Inline form for creating a new team or joining an existing one from the sidebar.
#[component]
fn SidebarCreateTeam(on_done: Callback<()>) -> impl IntoView {
    let (name, set_name) = signal(String::new());
    let (error, set_error) = signal(Option::<String>::None);
    let (submitting, set_submitting) = signal(false);
    let input_ref = NodeRef::<leptos::html::Input>::new();

    Effect::new(move || {
        if let Some(input) = input_ref.get() {
            let _ = input.focus();
        }
    });

    let on_submit = move |ev: web_sys::SubmitEvent| {
        ev.prevent_default();
        let name_val = name.get_untracked().trim().to_string();
        if name_val.is_empty() { return; }

        // Auto-derive a 3-char uppercase key from the name.
        let key: String = name_val
            .chars()
            .filter(|c| c.is_alphanumeric())
            .take(3)
            .collect::<String>()
            .to_uppercase();

        if key.len() < 2 {
            set_error.set(Some("Name too short for a team key".into()));
            return;
        }

        set_submitting.set(true);
        set_error.set(None);

        leptos::task::spawn_local(async move {
            match crate::server_fns::teams::create_team(name_val, key, None, None).await {
                Ok(_) => {
                    on_done.run(());
                }
                Err(e) => {
                    set_error.set(Some(format!("{e}")));
                    set_submitting.set(false);
                }
            }
        });
    };

    view! {
        <form class="px-2 pb-2" on:submit=on_submit>
            <input
                node_ref=input_ref
                type="text"
                placeholder="New team name..."
                class="w-full px-2 py-1.5 text-sm bg-[var(--color-sidebar-hover)] text-[var(--color-sidebar-foreground)] rounded-md border border-transparent focus:border-primary focus:outline-none placeholder:text-[var(--color-sidebar-foreground-muted)]"
                prop:value=move || name.get()
                on:input=move |ev| set_name.set(event_target_value(&ev))
                prop:disabled=move || submitting.get()
            />
            <Show when=move || error.get().is_some()>
                <p class="mt-1 text-[11px] text-red-400 px-1">
                    {move || error.get().unwrap_or_default()}
                </p>
            </Show>
        </form>
    }
}

/// A team's sub-navigation: clickable team name toggles expanded state,
/// showing/hiding the Issues sub-link. Right-click the team name to open
/// a context menu with "Rename" and "Leave team" options.
#[component]
fn SidebarTeamSubNav(
    team_id: String,
    name: String,
    team_key: String,
    issues_href: String,
) -> impl IntoView {
    let path = leptos_router::hooks::use_location().pathname;

    let issues_href_match = issues_href.clone();

    let issues_active = Signal::derive(move || path.get().starts_with(&issues_href_match));

    // Auto-expand if the current path is within this team.
    let (expanded, set_expanded) = signal(false);
    let (menu_open, set_menu_open) = signal(false);

    // Rename modal state
    let (show_rename, set_show_rename) = signal(false);
    let (rename_name, set_rename_name) = signal(name.clone());
    let (rename_key, set_rename_key) = signal(team_key.clone());
    let (rename_error, set_rename_error) = signal(Option::<String>::None);
    let (rename_submitting, set_rename_submitting) = signal(false);

    // Expand when navigating into a team's pages.
    Effect::new(move |_| {
        if issues_active.get() {
            set_expanded.set(true);
        }
    });

    let chevron_class = move || {
        if expanded.get() { "transition-transform duration-150" } else { "transition-transform duration-150 -rotate-90" }
    };

    // Close context menu on click-outside — register once, check state inside.
    let outer_ref = NodeRef::<leptos::html::Div>::new();
    Effect::new(move |_| {
        let Some(window) = web_sys::window() else { return };
        let outer = outer_ref.get();
        let cb = Closure::<dyn Fn(web_sys::MouseEvent)>::new(move |ev: web_sys::MouseEvent| {
            if !menu_open.get_untracked() { return; }
            if let Some(ref el) = outer
                && let Some(target) = ev.target()
            {
                let target_node: web_sys::Node = target.unchecked_into();
                if !el.contains(Some(&target_node)) {
                    set_menu_open.set(false);
                }
            }
        });
        let _ = window.add_event_listener_with_callback(
            "click",
            cb.as_ref().unchecked_ref(),
        );
        let cb_cleanup = send_wrapper::SendWrapper::new(cb);
        on_cleanup(move || {
            let Some(window) = web_sys::window() else { return };
            let cb = cb_cleanup.take();
            let _ = window.remove_event_listener_with_callback(
                "click",
                cb.as_ref().unchecked_ref(),
            );
        });
    });

    let display_name = name.clone();
    let name_for_menu = name;
    let team_id_for_menu = team_id.clone();
    let team_key_for_menu = team_key;
    let team_id_for_rename = team_id.clone();

    view! {
        <div class="mt-0.5 relative" node_ref=outer_ref>
            // Row wrapper — owns group hover for the entire row
            <div class="group flex items-center rounded-md hover:bg-[var(--color-sidebar-hover)] transition-colors">
                // Left zone: expand/collapse + right-click context menu
                <button
                    class="flex-1 min-w-0 flex items-center gap-2 px-3 py-1.5 text-sm font-medium text-[var(--color-sidebar-foreground)] text-left"
                    on:click=move |_| set_expanded.update(|v| *v = !*v)
                    on:contextmenu=move |ev| {
                        ev.prevent_default();
                        set_menu_open.set(true);
                    }
                >
                    <Icon icon=phosphor_leptos::CARET_DOWN weight=IconWeight::Bold size="12px" attr:class=chevron_class/>
                    <Icon icon=phosphor_leptos::USERS_THREE weight=IconWeight::Light size="16px"/>
                    <span class="flex-1 truncate">{display_name}</span>
                </button>
                // Right zone: actions (hover-reveal)
                <div class="flex items-center gap-1 pr-2 opacity-0 group-hover:opacity-100 transition-opacity">
                    <FavoriteToggle target_type="team" target_id=team_id.clone()/>
                    <button
                        class="p-0.5 rounded text-[var(--color-sidebar-foreground-muted)] hover:text-[var(--color-sidebar-foreground)] hover:bg-[var(--color-sidebar-hover)] transition-colors"
                        on:click=move |_| set_menu_open.set(true)
                        title="More actions"
                    >
                        <Icon icon=phosphor_leptos::DOTS_THREE weight=IconWeight::Bold size="14px"/>
                    </button>
                </div>
            </div>

            // Context menu dropdown
            <Show when=move || menu_open.get()>
                <div class="absolute left-0 right-0 top-full mt-1 bg-popover border border-border rounded-lg shadow-lg py-1 z-50">
                    <button
                        class="w-full text-left px-4 py-2 text-sm text-foreground hover:bg-secondary transition-colors"
                        on:click={
                            let name = name_for_menu.clone();
                            let team_key = team_key_for_menu.clone();
                            move |_| {
                                set_menu_open.set(false);
                                set_rename_name.set(name.clone());
                                set_rename_key.set(team_key.clone());
                                set_rename_error.set(None);
                                set_show_rename.set(true);
                            }
                        }
                    >
                        "Rename..."
                    </button>
                    <button
                        class="w-full text-left px-4 py-2 text-sm text-foreground hover:bg-secondary transition-colors"
                        on:click={
                            let team_id = team_id_for_menu.clone();
                            move |_| {
                                let team_id = team_id.clone();
                                set_menu_open.set(false);
                                leptos::task::spawn_local(async move {
                                    if let Err(e) = crate::server_fns::teams::leave_team(team_id).await {
                                        web_sys::console::warn_1(&format!("leave_team failed: {e}").into());
                                    }
                                });
                            }
                        }
                    >
                        "Leave team"
                    </button>
                </div>
            </Show>

            // Indented sub-items — shown when expanded
            {move || expanded.get().then(|| {
                let ih = issues_href.clone();
                view! {
                    <div class="ml-4">
                        <SidebarSubNavItem href=ih icon=phosphor_leptos::LIST_BULLETS label="Issues" is_active=issues_active/>
                    </div>
                }
            })}

            // Rename team modal
            {
                let on_close_rename = Callback::new(move |()| set_show_rename.set(false));
                let team_id_for_footer = team_id_for_rename.clone();
                let modal_footer: Arc<dyn Fn() -> AnyView + Send + Sync> = Arc::new(move || {
                    let team_id = team_id_for_footer.clone();
                    let handle_save = move |_| {
                        let n = rename_name.get_untracked();
                        let k = rename_key.get_untracked();
                        if n.trim().is_empty() {
                            set_rename_error.set(Some("Team name cannot be empty".into()));
                            return;
                        }
                        if k.len() < 2 || k.len() > 5 || !k.chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit()) {
                            set_rename_error.set(Some("Key must be 2-5 uppercase alphanumeric characters".into()));
                            return;
                        }
                        set_rename_submitting.set(true);
                        set_rename_error.set(None);
                        let team_id = team_id.clone();
                        leptos::task::spawn_local(async move {
                            match crate::server_fns::teams::update_team(team_id, Some(n), Some(k)).await {
                                Ok(()) => {
                                    set_show_rename.set(false);
                                }
                                Err(e) => {
                                    set_rename_error.set(Some(format!("{e}")));
                                }
                            }
                            set_rename_submitting.set(false);
                        });
                    };
                    view! {
                        <Button
                            variant=ButtonVariant::Ghost
                            disabled=Signal::derive(move || rename_submitting.get())
                            on:click=move |_| set_show_rename.set(false)
                        >
                            "Cancel"
                        </Button>
                        <Button
                            disabled=Signal::derive(move || rename_submitting.get())
                            on:click=handle_save
                        >
                            {move || if rename_submitting.get() { "Saving..." } else { "Save" }}
                        </Button>
                    }.into_any()
                });

                view! {
                    <Modal
                        show=Signal::derive(move || show_rename.get())
                        on_close=on_close_rename
                        title="Rename team"
                        size=ModalSize::Sm
                        footer=modal_footer
                    >
                        <div class="space-y-4">
                            <div>
                                <label class="block text-sm font-medium text-foreground mb-2">"Team name"</label>
                                <input
                                    type="text"
                                    class=INPUT_CLASS
                                    prop:value=move || rename_name.get()
                                    on:input=move |ev| set_rename_name.set(event_target_value(&ev))
                                    prop:disabled=move || rename_submitting.get()
                                />
                            </div>
                            <div>
                                <label class="block text-sm font-medium text-foreground mb-2">"Team key"</label>
                                <input
                                    type="text"
                                    class=INPUT_CLASS
                                    maxlength="5"
                                    prop:value=move || rename_key.get()
                                    on:input=move |ev| set_rename_key.set(event_target_value(&ev).to_uppercase())
                                    prop:disabled=move || rename_submitting.get()
                                />
                            </div>
                            <Show when=move || rename_error.get().is_some()>
                                <p class="text-sm text-red-400">
                                    {move || rename_error.get().unwrap_or_default()}
                                </p>
                            </Show>
                        </div>
                    </Modal>
                }
            }
        </div>
    }
}

/// Indented sub-nav item used within team sections.
#[component]
fn SidebarSubNavItem(
    href: String,
    icon: phosphor_leptos::IconData,
    label: &'static str,
    is_active: Signal<bool>,
) -> impl IntoView {
    let weight = Signal::derive(move || {
        if is_active.get() { IconWeight::Fill } else { IconWeight::Light }
    });
    let class = move || {
        let base = "flex items-center gap-2.5 px-3 py-1.5 rounded-md text-[13px] transition-colors";
        if is_active.get() {
            format!("{base} bg-[var(--color-sidebar-active)] text-[var(--color-sidebar-foreground)] font-medium")
        } else {
            format!("{base} text-[var(--color-sidebar-foreground-secondary)] hover:text-[var(--color-sidebar-foreground)] hover:bg-[var(--color-sidebar-hover)]")
        }
    };
    view! {
        <a href=href class=class>
            <Icon icon=icon weight=weight size="14px"/>
            {label}
        </a>
    }
}

#[component]
fn SidebarNavItem(
    href: &'static str,
    icon: phosphor_leptos::IconData,
    label: &'static str,
) -> impl IntoView {
    let path = leptos_router::hooks::use_location().pathname;
    let is_active = Memo::new(move |_| {
        let p = path.get();
        if href == "/settings" {
            p.starts_with("/settings")
        } else {
            p == href || p.starts_with(&format!("{href}/"))
        }
    });
    let weight = Memo::new(move |_| {
        if is_active.get() { IconWeight::Fill } else { IconWeight::Light }
    });
    let class = move || {
        let base = "flex items-center gap-3 px-3 py-3 rounded-md text-sm transition-colors";
        if is_active.get() {
            format!("{base} bg-[var(--color-sidebar-active)] text-[var(--color-sidebar-foreground)] font-medium")
        } else {
            format!("{base} text-[var(--color-sidebar-foreground-secondary)] hover:text-[var(--color-sidebar-foreground)] hover:bg-[var(--color-sidebar-hover)]")
        }
    };
    view! {
        <a href=href class=class>
            <Icon icon=icon weight=weight size="18px"/>
            {label}
        </a>
    }
}
