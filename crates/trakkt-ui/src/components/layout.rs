// SPDX-License-Identifier: AGPL-3.0-or-later

//! App layout — sidebar + content area.

use leptos::prelude::*;
use leptos_router::components::Outlet;

use wasm_bindgen::closure::Closure;
use wasm_bindgen::JsCast;

use phosphor_leptos::{Icon, IconWeight};

use std::collections::HashMap;
use crate::cache::store::SyncStore;
use crate::components::issue_status_badge::{IssueStatusVariant, view_status_icon};
use crate::components::{Avatar, AvatarSize, Button, ButtonSize, ButtonVariant, CommandPalette, ConfirmDialog, CreateIssueTrigger, FeedbackModal, ProjectCreationModal, Spinner, TeamCreationModal, TeamIcon};
use crate::components::popover::{Popover, Placement};
use crate::server_fns::context::UserContext;
use crate::server_fns::sidebar::{get_sidebar_user, list_user_workspaces, switch_workspace, SidebarUser};

/// Sidebar expand/collapse state shared across all `SidebarTeamSubNav` instances.
/// Keyed by team key string. Persists across SPA navigation within a session.
#[derive(Clone)]
struct SidebarExpandState(RwSignal<HashMap<String, bool>>);

/// Main authenticated layout with sidebar and content area.
#[component]
pub fn Layout() -> impl IntoView {
    let user_info = LocalResource::new(get_sidebar_user);
    let (user_menu_open, set_user_menu_open) = signal(false);
    let (mobile_sidebar_open, set_mobile_sidebar_open) = signal(false);
    let (show_palette, set_show_palette) = signal(false);
    let (show_feedback, set_show_feedback) = signal(false);

    // Provide SyncStore on all targets so page components can reference it.
    // On SSR it remains empty; on WASM the sync engine populates it.
    let sync_store = SyncStore::new();
    provide_context(sync_store);

    // Initialize console.error interceptor for feedback context collection.
    #[cfg(target_arch = "wasm32")]
    crate::utils::feedback_context::init();
    provide_context(CreateIssueTrigger(RwSignal::new(false)));
    let initial_expand_state: HashMap<String, bool> = {
        #[cfg(target_arch = "wasm32")]
        {
            web_sys::window()
                .and_then(|w| w.local_storage().ok().flatten())
                .and_then(|storage| storage.get_item("trakkt:sidebar:teams").ok().flatten())
                .and_then(|json| match serde_json::from_str::<HashMap<String, bool>>(&json) {
                    Ok(map) => Some(map),
                    Err(e) => {
                        tracing::warn!("Failed to parse sidebar expand state from localStorage: {e}");
                        None
                    }
                })
                .unwrap_or_default()
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            HashMap::new()
        }
    };
    provide_context(SidebarExpandState(RwSignal::new(initial_expand_state)));

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
    // Every tab of a browser shares one IndexedDB cache, so only one of them —
    // the tab holding the sync leadership lock — may run a sync engine against
    // it. Two tabs writing entities and the shared cursor concurrently is what
    // let a throttled tab's stale writes land on top of a live tab's newer
    // ones. See `cache::tab_leader`.
    //
    // Every tab:      set workspace, hydrate from IndexedDB, subscribe to the
    //                 leader's broadcast, request leadership.
    // The leader tab: additionally connect the WebSocket and start the engine —
    //                 immediately if it wins the lock, or later on promotion
    //                 when the previous leader's tab closes.
    #[cfg(target_arch = "wasm32")]
    {
        use crate::cache::sync_engine;
        use crate::cache::tab_leader::{self, Leadership, SyncBroadcast};
        use crate::cache::websocket;
        use crate::server_fns::context::UserContext;

        let user_ctx = expect_context::<LocalResource<Result<UserContext, ServerFnError>>>();

        // Track what has already been done so neither half re-runs when the
        // effect re-fires (it re-fires on promotion, by design).
        let sync_started = std::rc::Rc::new(std::cell::Cell::new(false));
        let leader_started = std::rc::Rc::new(std::cell::Cell::new(false));

        // Set once the leadership lock is granted. A plain signal is all the
        // promotion machinery needs: the grant callback sets it, this effect
        // re-runs and starts the engine — no polling, and the work happens
        // under the reactive owner rather than inside a bare JS callback.
        let is_leader = RwSignal::new(false);

        // Non-Send browser handles that must outlive the effect run that
        // created them. Hoisted to component setup so they are created once.
        let broadcast: StoredValue<send_wrapper::SendWrapper<Option<SyncBroadcast>>> =
            StoredValue::new(send_wrapper::SendWrapper::new(None));
        let leadership: StoredValue<send_wrapper::SendWrapper<Option<tab_leader::LeadershipRequest>>> =
            StoredValue::new(send_wrapper::SendWrapper::new(None));

        Effect::new(move |_| {
            // Re-runs when leadership is granted; both halves below are guarded.
            let leader_now = is_leader.get();

            // Wait for user context to resolve successfully.
            let Some(Ok(ctx)) = user_ctx.get() else {
                return;
            };

            let user_id = ctx.user_id.clone();
            let workspace_id = ctx
                .workspace_id
                .clone()
                .unwrap_or_else(|| "workspace-local".to_string());

            // ── Every tab ───────────────────────────────────────────────────
            if !sync_started.get() {
                sync_started.set(true);
                sync_store.set_workspace_id(workspace_id.clone());

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
                            // Mark initialized even on IDB failure — an empty store is
                            // valid state (the sync engine bootstrap will populate it).
                            // Without this, the sidebar stays in skeleton state forever.
                            sync_store.set_initialized(true);
                        }
                    }
                });

                // 2. Subscribe to the leader's broadcast. A follower's entire
                //    live-update path runs through here; the leader opens the
                //    same channel to publish on (it never receives its own
                //    messages back).
                match SyncBroadcast::open(&workspace_id) {
                    Ok(channel) => {
                        channel.set_on_message(move |message| {
                            crate::cache::apply::apply_broadcast_to_memory(&sync_store, &message);
                        });
                        *broadcast.write_value() =
                            send_wrapper::SendWrapper::new(Some(channel));
                    }
                    Err(e) => tracing::warn!(
                        "sync: no BroadcastChannel ({e:?}) — this tab will not see the \
                         leader's updates until it reloads"
                    ),
                }

                // 3. Until this tab is the leader it has no WebSocket, but
                //    pages still resolve the client from context.
                provide_context(websocket::disconnected());

                // 4. Stand for election. The callback fires immediately if no
                //    other tab holds the lock, or when the leader's tab closes.
                match tab_leader::acquire_leadership(&workspace_id, move || {
                    is_leader.set(true);
                }) {
                    Leadership::Requested(request) => {
                        *leadership.write_value() =
                            send_wrapper::SendWrapper::new(Some(request));
                    }
                    Leadership::Unsupported => {
                        // Documented capability fallback: a browser with no Web
                        // Locks cannot elect anyone, so every tab syncs as it
                        // did before this change.
                        tracing::info!(
                            "sync: no Web Locks in this browser — running without a tab \
                             leader, as every tab did previously"
                        );
                        is_leader.set(true);
                    }
                }
            }

            // ── Leader tab only ─────────────────────────────────────────────
            if !leader_now || leader_started.get() {
                return;
            }
            leader_started.set(true);
            tracing::info!(%workspace_id, "sync: this tab is the sync leader");

            // Connect WebSocket — start with empty token (connects immediately
            // so provide_context works in the reactive scope). Then fetch a
            // JWT asynchronously and reconnect with it for multi-user mode.
            let ws_client = websocket::connect(&user_id, &workspace_id, "");

            sync_engine::start_sync_engine(
                &ws_client,
                &sync_store,
                &workspace_id,
                broadcast.with_value(|channel| (**channel).clone()),
            );

            let ws_for_cleanup = ws_client.clone();
            // Replaces the disconnected handle provided above — but only for
            // consumers created after this point. Leptos context is a
            // setup-time snapshot, not a reactive value, so a page already
            // mounted when this tab is promoted keeps the disconnected handle.
            // Nothing reads `WebSocketClient` from context today (the sync
            // engine is handed it directly), so this costs nothing now. A page
            // that starts reading it — a connection indicator, say — needs the
            // handle wrapped in a signal instead.
            provide_context(ws_client.clone());

            on_cleanup(move || {
                websocket::disconnect(&ws_for_cleanup);
            });

            // Fetch JWT and reconnect with auth (multi-user mode only).
            let ws_for_reconnect = ws_client;
            let uid_reconnect = user_id.clone();
            let wid_reconnect = workspace_id.clone();
            leptos::task::spawn_local(async move {
                if let Ok(token) = crate::server_fns::auth::get_ws_token().await && !token.is_empty() {
                    ws_for_reconnect.reconnect(&uid_reconnect, &wid_reconnect, &token);
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
                    <Sidebar user_info=user_info user_menu_open=user_menu_open set_user_menu_open=set_user_menu_open set_show_feedback=set_show_feedback/>
                </div>

                // Mobile sidebar overlay
                <Show when=move || mobile_sidebar_open.get()>
                    <div class="fixed inset-0 z-40 md:hidden">
                        <div
                            class="fixed inset-0 bg-black/50"
                            on:click=move |_| set_mobile_sidebar_open.set(false)
                        />
                        <div class="fixed inset-y-0 left-0 z-50 w-[220px]">
                            <Sidebar user_info=user_info user_menu_open=user_menu_open set_user_menu_open=set_user_menu_open set_show_feedback=set_show_feedback/>
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
                    <BillingBanner/>
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

            // Feedback modal — rendered at the app level, triggered from user menu.
            <FeedbackModal
                show=Signal::derive(move || show_feedback.get())
                on_open_change=Callback::new(move |open: bool| set_show_feedback.set(open))
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
    set_show_feedback: WriteSignal<bool>,
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
                <SidebarInboxNavItem/>
                <SidebarNavItem href="/activity" icon=phosphor_leptos::LIGHTNING label="Activity"/>
                <SidebarNavItem href="/my-issues" icon=phosphor_leptos::LIST_CHECKS label="My Issues"/>

                <SidebarWorkspaceSection/>

                <SidebarProjectsSection/>

                <SidebarTeamsSection/>
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
                            let ws_name = user.workspace_name.clone().unwrap_or_default();
                            let email = user.email.clone();
                            let header_name = display_name.clone();
                            let header_email = email.clone();
                            let is_personal = user.is_personal_mode;
                            view! {
                                <div node_ref=user_menu_trigger_ref>
                                    <button
                                        class="w-full flex items-center gap-3 pl-2 pr-3 py-1 min-h-[44px] rounded-lg text-sm hover:bg-[var(--color-sidebar-hover)] transition-colors text-left focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
                                        on:click=move |_| set_user_menu_open.update(|v| *v = !*v)
                                    >
                                        // Avatar
                                        <Avatar name=display_name.clone() size=AvatarSize::Md/>
                                        <div class="flex-1 min-w-0">
                                            <div class="text-sm font-medium text-[var(--color-sidebar-foreground)] truncate">
                                                {display_name.clone()}
                                            </div>
                                            <div class="text-xs text-[var(--color-sidebar-foreground-muted)] truncate">
                                                {ws_name.clone()}
                                            </div>
                                        </div>
                                        // Animated chevron — rotates 180deg when menu is open
                                        <span
                                            class="text-[var(--color-sidebar-foreground-muted)] flex-shrink-0 transition-transform duration-200"
                                            style=move || if user_menu_open.get() { "transform: rotate(180deg)" } else { "transform: rotate(0deg)" }
                                        >
                                            <Icon icon=phosphor_leptos::CARET_DOWN weight=IconWeight::Light size="16px"/>
                                        </span>
                                    </button>
                                </div>

                                // Dropdown menu — portalled via Popover for click-outside, Escape, and viewport-aware positioning
                                <Popover
                                    trigger_ref=user_menu_trigger_ref
                                    open=Signal::derive(move || user_menu_open.get())
                                    on_close=Callback::new(move |()| set_user_menu_open.set(false))
                                    placement=Placement::TOP_START
                                    match_width=true
                                    class="bg-[var(--color-sidebar)] border border-[var(--color-sidebar-border)] rounded-lg shadow-[0_8px_24px_rgba(0,0,0,0.3)] py-1"
                                >
                                    // User info header
                                    <div class="px-3 py-2 border-b border-[var(--color-sidebar-border)]">
                                        <div class="text-sm font-medium text-[var(--color-sidebar-foreground)]">{header_name.clone()}</div>
                                        <div class="text-xs text-[var(--color-sidebar-foreground-muted)] truncate">{header_email.clone()}</div>
                                    </div>
                                    // Workspace switcher (includes separator only when shown)
                                    <WorkspaceSwitcher set_user_menu_open=set_user_menu_open/>
                                    <a
                                        href="/settings/profile"
                                        on:click=move |_| set_user_menu_open.set(false)
                                        class="w-full text-left px-4 py-2 text-sm text-[var(--color-sidebar-foreground)] hover:bg-[var(--color-sidebar-hover)] transition-colors flex items-center space-x-3"
                                    >
                                        <Icon icon=phosphor_leptos::GEAR weight=IconWeight::Light size="16px"/>
                                        <span>"Settings"</span>
                                    </a>
                                    {(!is_personal).then(|| view! {
                                        <button
                                            class="w-full text-left px-4 py-2 text-sm text-[var(--color-sidebar-foreground)] hover:bg-[var(--color-sidebar-hover)] transition-colors flex items-center space-x-3"
                                            on:click=move |_| {
                                                set_user_menu_open.set(false);
                                                set_show_feedback.set(true);
                                            }
                                        >
                                            <Icon icon=phosphor_leptos::CHAT_CIRCLE weight=IconWeight::Light size="16px"/>
                                            <span>"Send Feedback"</span>
                                        </button>
                                    })}
                                    <button
                                        class="w-full text-left px-4 py-2 text-sm text-error-foreground hover:bg-error/10 transition-colors flex items-center space-x-3"
                                        on:click=move |_| {
                                            set_user_menu_open.set(false);
                                            leptos::task::spawn_local(async move {
                                                let _ = crate::server_fns::security::logout().await;
                                                let _ = web_sys::window()
                                                    .and_then(|w| w.location().set_href("/login").ok());
                                            });
                                        }
                                    >
                                        <Icon icon=phosphor_leptos::SIGN_OUT weight=IconWeight::Light size="16px"/>
                                        <span>"Sign Out"</span>
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

/// Section header for "Projects" with dynamic project list from SyncStore.
///
/// Lists all workspace projects as `SidebarEntityItem` entries with a "+"
/// button to create new projects. The section expand/collapse state is
/// persisted to localStorage under `trakkt:sidebar:projects`.
#[component]
fn SidebarProjectsSection() -> impl IntoView {
    let store = use_context::<SyncStore>();
    let (show_create, set_show_create) = signal(false);

    // ── Expand/collapse state persisted to localStorage ──────────────────
    let initial_expanded: bool = {
        #[cfg(target_arch = "wasm32")]
        {
            web_sys::window()
                .and_then(|w| w.local_storage().ok().flatten())
                .and_then(|storage| storage.get_item("trakkt:sidebar:projects").ok().flatten())
                .map(|v| v == "true")
                .unwrap_or(true)
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            true
        }
    };
    let (expanded, set_expanded) = signal(initial_expanded);

    let persist_expanded = move |value: bool| {
        set_expanded.set(value);
        #[cfg(target_arch = "wasm32")]
        {
            if let Some(storage) = web_sys::window()
                .and_then(|w| w.local_storage().ok().flatten())
                && let Err(e) = storage.set_item("trakkt:sidebar:projects", if value { "true" } else { "false" })
            {
                tracing::warn!("Failed to persist sidebar projects state to localStorage: {e:?}");
            }
        }
    };

    view! {
        {move || {
            let Some(store) = store else { return view! { <span/> }.into_any() };

            // Show skeleton placeholders while the sync store is hydrating.
            if !store.initialized().get() {
                return view! {
                    <div class="px-3 pt-4 pb-1">
                        <span class="text-[10px] font-semibold uppercase tracking-wider text-[var(--color-sidebar-foreground-muted)]">
                            "Projects"
                        </span>
                    </div>
                    <div class="space-y-1 px-2">
                        <div class="h-7 bg-[var(--color-sidebar-hover)] rounded animate-pulse"/>
                        <div class="h-7 bg-[var(--color-sidebar-hover)] rounded animate-pulse w-3/4"/>
                    </div>
                }.into_any();
            }

            let projects: Vec<_> = store.projects().get()
                .into_iter()
                .filter(|p| p.archived_at.is_none())
                .collect();

            view! {
                <div class="group flex items-center justify-between px-3 pt-4 pb-1">
                    <button
                        class="flex items-center gap-1 text-[10px] font-semibold uppercase tracking-wider text-[var(--color-sidebar-foreground-muted)] hover:text-[var(--color-sidebar-foreground)] transition-colors duration-200 focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring rounded"
                        on:click=move |_| persist_expanded(!expanded.get_untracked())
                    >
                        <Icon
                            icon=phosphor_leptos::CARET_DOWN
                            weight=IconWeight::Bold
                            size="10px"
                            attr:class=move || {
                                if expanded.get() {
                                    "transition-transform duration-150"
                                } else {
                                    "transition-transform duration-150 -rotate-90"
                                }
                            }
                        />
                        "Projects"
                    </button>
                    <button
                        class="p-0.5 rounded text-[var(--color-sidebar-foreground-muted)] hover:text-[var(--color-sidebar-foreground)] hover:bg-[var(--color-sidebar-hover)] transition-colors duration-200 opacity-0 group-hover:opacity-100 focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
                        on:click=move |_| set_show_create.set(true)
                        title="Create a project"
                    >
                        <Icon icon=phosphor_leptos::PLUS weight=IconWeight::Bold size="12px"/>
                    </button>
                </div>

                <ProjectCreationModal
                    show=Signal::derive(move || show_create.get())
                    on_close=Callback::new(move |()| set_show_create.set(false))
                />

                {expanded.get().then(|| view! {
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
                })}
            }.into_any()
        }}
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

            // Show skeleton placeholders while the sync store is hydrating.
            // This prevents the sidebar from showing an empty teams list
            // after client-side login navigation (before bootstrap completes).
            if !store.initialized().get() {
                return view! {
                    <div class="px-3 pt-4 pb-1">
                        <span class="text-[10px] font-semibold uppercase tracking-wider text-[var(--color-sidebar-foreground-muted)]">
                            "Teams"
                        </span>
                    </div>
                    <div class="space-y-1 px-2">
                        <div class="h-7 bg-[var(--color-sidebar-hover)] rounded animate-pulse"/>
                        <div class="h-7 bg-[var(--color-sidebar-hover)] rounded animate-pulse w-3/4"/>
                    </div>
                }.into_any();
            }

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

                <TeamCreationModal
                    show=Signal::derive(move || show_create.get())
                    on_close=Callback::new(move |()| set_show_create.set(false))
                />

                <div class="space-y-0.5">
                    {teams.into_iter().map(|team| {
                        let issues_href = format!("/teams/{}/issues", team.key.to_lowercase());
                        view! {
                            <SidebarTeamSubNav team=team issues_href=issues_href/>
                        }
                    }).collect_view()}
                </div>
            }.into_any()
        }}
    }
}

/// Section for "Workspace" — shows preset views (Issues, Active, Backlog)
/// and user-saved workspace-scoped views.
///
/// Workspace-scoped views are those with `team_id == None`.
#[component]
fn SidebarWorkspaceSection() -> impl IntoView {
    let store = use_context::<SyncStore>();
    let location = leptos_router::hooks::use_location();
    let path = location.pathname;
    let search = location.search;

    // Active state for the three workspace presets — match pathname AND query param.
    let issues_active = Signal::derive(move || {
        let p = path.get();
        let s = search.get();
        p == "/workspace"
            && (s.split('&').any(|seg| seg == "view=issues")
                || !s.split('&').any(|seg| seg.starts_with("view=")))
    });
    let active_active = Signal::derive(move || {
        path.get() == "/workspace"
            && search.get().split('&').any(|p| p == "view=active")
    });
    let backlog_active = Signal::derive(move || {
        path.get() == "/workspace"
            && search.get().split('&').any(|p| p == "view=backlog")
    });
    let archived_active = Signal::derive(move || {
        path.get() == "/archived"
    });
    let starred_active = Signal::derive(move || {
        path.get() == "/workspace"
            && search.get().split('&').any(|p| p == "view=starred")
    });

    view! {
        {move || {
            let Some(store) = store else { return view! { <span/> }.into_any() };

            // Show skeleton placeholders while the sync store is hydrating.
            if !store.initialized().get() {
                return view! {
                    <div class="px-3 pt-4 pb-1">
                        <span class="text-[10px] font-semibold uppercase tracking-wider text-[var(--color-sidebar-foreground-muted)]">
                            "Workspace"
                        </span>
                    </div>
                    <div class="space-y-1 px-2">
                        <div class="h-7 bg-[var(--color-sidebar-hover)] rounded animate-pulse"/>
                        <div class="h-7 bg-[var(--color-sidebar-hover)] rounded animate-pulse w-3/4"/>
                        <div class="h-7 bg-[var(--color-sidebar-hover)] rounded animate-pulse w-2/3"/>
                    </div>
                }.into_any();
            }

            // Workspace-scoped views (team_id is None), excluding any whose
            // name collides with the hardcoded preset views above.
            const PRESET_NAMES: &[&str] = &["Issues", "Active", "Backlog", "Starred"];
            let mut workspace_views: Vec<trakkt_types::models::View> = store
                .views()
                .get()
                .into_iter()
                .filter(|v| {
                    v.team_id.is_none()
                        && !PRESET_NAMES.iter().any(|p| p.eq_ignore_ascii_case(&v.name))
                })
                .collect();
            workspace_views.sort_by_key(|v| v.position);

            view! {
                <SidebarSectionHeader label="Workspace"/>
                <div class="space-y-0.5">
                    <SidebarWorkspacePresetItem
                        href="/workspace?view=issues"
                        label="Issues"
                        is_active=issues_active
                    >
                        <Icon icon=phosphor_leptos::LIST_BULLETS weight=IconWeight::Light size="14px"/>
                    </SidebarWorkspacePresetItem>
                    <SidebarWorkspacePresetItem
                        href="/workspace?view=active"
                        label="Active"
                        is_active=active_active
                    >
                        {view_status_icon(IssueStatusVariant::Started, "14px".to_string())}
                    </SidebarWorkspacePresetItem>
                    <SidebarWorkspacePresetItem
                        href="/workspace?view=backlog"
                        label="Backlog"
                        is_active=backlog_active
                    >
                        {view_status_icon(IssueStatusVariant::Backlog, "14px".to_string())}
                    </SidebarWorkspacePresetItem>
                    <SidebarWorkspacePresetItem
                        href="/archived"
                        label="Archived"
                        is_active=archived_active
                    >
                        <Icon icon=phosphor_leptos::ARCHIVE weight=IconWeight::Light size="14px"/>
                    </SidebarWorkspacePresetItem>
                    <SidebarWorkspacePresetItem
                        href="/workspace?view=starred"
                        label="Starred"
                        is_active=starred_active
                    >
                        <Icon icon=phosphor_leptos::STAR weight=IconWeight::Light size="14px"/>
                    </SidebarWorkspacePresetItem>

                    // User-saved workspace views
                    {workspace_views.into_iter().map(|v| {
                        let href = format!("/views/{}", v.view_id);
                        let name = v.name.clone();
                        view! {
                            <SidebarEntityItem href=href name=name icon=phosphor_leptos::FUNNEL/>
                        }
                    }).collect_view()}
                </div>
            }.into_any()
        }}
    }
}

/// Preset workspace view item with custom active-state detection.
///
/// Unlike `SidebarNavItem` which only checks pathname, this component checks
/// both pathname and query string to differentiate between `/workspace?view=active`
/// and `/workspace?view=backlog`.
#[component]
fn SidebarWorkspacePresetItem(
    href: &'static str,
    label: &'static str,
    is_active: Signal<bool>,
    children: Children,
) -> impl IntoView {
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
            {children()}
            {label}
        </a>
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



/// Small, uppercase, muted section header (Linear-style).
#[component]
fn SidebarSectionHeader(label: &'static str) -> impl IntoView {
    view! {
        <div class="px-3 pt-4 pb-1 text-[10px] font-semibold uppercase tracking-wider text-[var(--color-sidebar-foreground-muted)]">
            {label}
        </div>
    }
}

/// A team's sub-navigation: clickable team name toggles expanded state,
/// showing/hiding the Issues sub-link. Right-click or click the dots menu
/// to open a context menu with "Settings" and "Leave team".
#[component]
fn SidebarTeamSubNav(
    team: trakkt_types::models::Team,
    issues_href: String,
) -> impl IntoView {
    let team_id = team.team_id.clone();
    let name = team.name.clone();
    let team_key = team.key.clone();

    let path = leptos_router::hooks::use_location().pathname;

    let issues_href_match = issues_href.clone();
    let settings_href = format!("/teams/{}/settings", team_key.to_lowercase());
    let settings_href_match = settings_href.clone();

    let archived_href = format!("/teams/{}/archived", team_key.to_lowercase());
    let archived_href_match_for_team = archived_href.clone();

    let issues_active = Signal::derive(move || path.get().starts_with(&issues_href_match));
    let settings_active = Signal::derive(move || path.get().starts_with(&settings_href_match));
    let team_archived_active = Signal::derive(move || path.get().starts_with(&archived_href_match_for_team));
    let team_active = Signal::derive(move || issues_active.get() || settings_active.get() || team_archived_active.get());

    // Raw query string WITHOUT the leading '?' (leptos_router strips it).
    let search = leptos_router::hooks::use_location().search;

    // Active/Backlog sub-item hrefs — use the new `view=` format.
    let active_href = format!("/teams/{}/issues?view=active", team_key.to_lowercase());
    let backlog_href = format!("/teams/{}/issues?view=backlog", team_key.to_lowercase());

    // Issues weight (for Phosphor icon fill/light toggle)
    let issues_weight = Signal::derive(move || {
        if issues_active.get() { IconWeight::Fill } else { IconWeight::Light }
    });

    let issues_href_match_for_active = issues_href.clone();
    let started_active = Signal::derive(move || {
        path.get().starts_with(&issues_href_match_for_active)
            && search.get().split('&').any(|p| p == "view=active")
    });
    let issues_href_match_for_backlog = issues_href.clone();
    let backlog_active = Signal::derive(move || {
        path.get().starts_with(&issues_href_match_for_backlog)
            && search.get().split('&').any(|p| p == "view=backlog")
    });

    let issues_no_filter_active = Signal::derive(move || {
        issues_active.get()
            && !search.get().split('&').any(|p| p.starts_with("view="))
    });

    // ── Expand/collapse state from shared context ──────────────────────────
    let expand_ctx = use_context::<SidebarExpandState>();

    let expand_ctx_for_read = expand_ctx.clone();
    let team_key_for_expand = team_key.clone();
    let is_expanded = Signal::derive(move || {
        expand_ctx_for_read
            .as_ref()
            .map(|ctx| ctx.0.get().get(&team_key_for_expand).copied().unwrap_or(false))
            .unwrap_or(false)
    });

    let set_expand = {
        let team_key_for_set = team_key.clone();
        move |value: bool| {
            if let Some(ref ctx) = expand_ctx {
                ctx.0.update(|map| { map.insert(team_key_for_set.clone(), value); });
                // Persist to localStorage
                #[cfg(target_arch = "wasm32")]
                {
                    let map = ctx.0.get_untracked();
                    if let Ok(json) = serde_json::to_string(&map)
                        && let Some(storage) = web_sys::window()
                            .and_then(|w| w.local_storage().ok().flatten())
                        && let Err(e) = storage.set_item("trakkt:sidebar:teams", &json)
                    {
                        tracing::warn!("Failed to persist sidebar state to localStorage: {e:?}");
                    }
                }
            }
        }
    };

    let (menu_open, set_menu_open) = signal(false);
    let menu_trigger_ref = NodeRef::<leptos::html::Div>::new();

    let store = use_context::<SyncStore>();
    let nav = leptos_router::hooks::use_navigate();

    let (show_leave_confirm, set_show_leave_confirm) = signal(false);

    // Expand when navigating INTO this team (transition from inactive to active).
    // Does NOT re-expand if already active (respects user manual collapse).
    Effect::new({
        let set_expand = set_expand.clone();
        move |prev_active: Option<bool>| {
            let active = team_active.get();
            let was_active = prev_active.unwrap_or(false);
            if active && !was_active {
                set_expand(true);
            }
            active
        }
    });

    let chevron_class = move || {
        if is_expanded.get() { "transition-transform duration-150" } else { "transition-transform duration-150 -rotate-90" }
    };

    let display_name = name.clone();
    let leave_confirm_message = format!(
        "Are you sure you want to leave {}? You'll no longer see this team's issues.",
        name,
    );
    let team_prefix = format!("/teams/{}/", issues_href.split('/').nth(2).unwrap_or(""));
    let settings_href_for_menu = settings_href.clone();
    let team_id_for_leave = team_id.clone();

    view! {
        <div class="mt-0.5">
            // Row wrapper — owns group hover for the entire row
            <div class="group flex items-center rounded-md hover:bg-[var(--color-sidebar-hover)] transition-colors">
                // Left zone: expand/collapse + right-click context menu
                <button
                    class="flex-1 min-w-0 flex items-center gap-2 px-3 py-1.5 text-sm font-medium text-[var(--color-sidebar-foreground)] text-left"
                    on:click={
                        let set_expand = set_expand.clone();
                        move |_| {
                            let current = is_expanded.get_untracked();
                            set_expand(!current);
                        }
                    }
                    on:contextmenu=move |ev| {
                        ev.prevent_default();
                        set_menu_open.set(true);
                    }
                >
                    <Icon icon=phosphor_leptos::CARET_DOWN weight=IconWeight::Bold size="12px" attr:class=chevron_class/>
                    <TeamIcon team=team size="16px"/>
                    <span class="flex-1 truncate">{display_name}</span>
                </button>
                // Right zone: actions (hover-reveal)
                <div class="flex items-center gap-1 pr-2 opacity-0 group-hover:opacity-100 transition-opacity">
                    <div node_ref=menu_trigger_ref>
                        <button
                            class="p-0.5 rounded text-[var(--color-sidebar-foreground-muted)] hover:text-[var(--color-sidebar-foreground)] hover:bg-[var(--color-sidebar-hover)] transition-colors"
                            on:click=move |_| set_menu_open.set(true)
                            title="More actions"
                        >
                            <Icon icon=phosphor_leptos::DOTS_THREE weight=IconWeight::Bold size="14px"/>
                        </button>
                    </div>
                </div>
            </div>

            // Context menu dropdown — portalled via Popover for positioning near trigger
            <Popover
                trigger_ref=menu_trigger_ref
                open=Signal::derive(move || menu_open.get())
                on_close=Callback::new(move |()| set_menu_open.set(false))
                placement=Placement::BOTTOM_START
                class="bg-popover border border-border rounded-lg shadow-lg py-1"
            >
                <a
                    href=settings_href_for_menu.clone()
                    class="block w-full text-left px-4 py-2 text-sm text-foreground hover:bg-secondary transition-colors"
                    on:click=move |_| set_menu_open.set(false)
                >
                    "Settings"
                </a>
                <button
                    class="w-full text-left px-4 py-2 text-sm text-foreground hover:bg-secondary transition-colors"
                    on:click=move |_| {
                        set_menu_open.set(false);
                        set_show_leave_confirm.set(true);
                    }
                >
                    "Leave team"
                </button>
            </Popover>

            // Indented sub-items — shown when expanded
            {move || is_expanded.get().then(|| {
                let ih = issues_href.clone();
                let ah = active_href.clone();
                let bh = backlog_href.clone();
                let arch_h = archived_href.clone();
                view! {
                    <div class="ml-4">
                        <SidebarSubNavItem href=ih label="Issues" is_active=issues_no_filter_active>
                            <Icon icon=phosphor_leptos::LIST_BULLETS weight=issues_weight size="14px"/>
                        </SidebarSubNavItem>
                        <SidebarSubNavItem href=ah label="Active" is_active=started_active>
                            {view_status_icon(IssueStatusVariant::Started, "14px".to_string())}
                        </SidebarSubNavItem>
                        <SidebarSubNavItem href=bh label="Backlog" is_active=backlog_active>
                            {view_status_icon(IssueStatusVariant::Backlog, "14px".to_string())}
                        </SidebarSubNavItem>
                        <SidebarSubNavItem href=arch_h label="Archived" is_active=team_archived_active>
                            <Icon icon=phosphor_leptos::ARCHIVE weight=IconWeight::Light size="14px"/>
                        </SidebarSubNavItem>
                    </div>
                }
            })}

            {
                let team_id = team_id_for_leave.clone();
                view! {
                    <ConfirmDialog
                        open=Signal::derive(move || show_leave_confirm.get())
                        title="Leave team?"
                        message=leave_confirm_message
                        confirm_text="Leave"
                        on_confirm=Callback::new(move |()| {
                            set_show_leave_confirm.set(false);
                            let team_id = team_id.clone();
                            let is_viewing_this_team = path.get_untracked().starts_with(&team_prefix);
                            let nav = nav.clone();
                            leptos::task::spawn_local(async move {
                                match crate::server_fns::teams::leave_team(team_id.clone()).await {
                                    Ok(()) => {
                                        if let Some(store) = store {
                                            store.remove_team(&team_id);
                                        }
                                        if is_viewing_this_team {
                                            nav("/my-issues", Default::default());
                                        }
                                    }
                                    Err(e) => {
                                        web_sys::console::warn_1(&format!("leave_team failed: {e}").into());
                                    }
                                }
                            });
                        })
                        on_cancel=Callback::new(move |()| set_show_leave_confirm.set(false))
                    />
                }
            }
        </div>
    }
}

/// Indented sub-nav item used within team sections.
///
/// Accepts `children` for the icon so callers can pass either Phosphor icons
/// or custom SVGs (e.g. status circle variants for Active/Backlog views).
#[component]
fn SidebarSubNavItem(
    href: String,
    label: &'static str,
    is_active: Signal<bool>,
    children: Children,
) -> impl IntoView {
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
            {children()}
            {label}
        </a>
    }
}

#[component]
fn SidebarInboxNavItem() -> impl IntoView {
    let path = leptos_router::hooks::use_location().pathname;
    let is_active = Memo::new(move |_| {
        let p = path.get();
        p == "/inbox" || p.starts_with("/inbox/")
    });
    let weight = Memo::new(move |_| {
        if is_active.get() { IconWeight::Fill } else { IconWeight::Light }
    });
    let class = move || {
        let base = "flex items-center gap-3 px-3 py-1.5 rounded-md text-sm transition-colors";
        if is_active.get() {
            format!("{base} bg-[var(--color-sidebar-active)] text-[var(--color-sidebar-foreground)] font-medium")
        } else {
            format!("{base} text-[var(--color-sidebar-foreground-secondary)] hover:text-[var(--color-sidebar-foreground)] hover:bg-[var(--color-sidebar-hover)]")
        }
    };

    let sync_store = use_context::<SyncStore>();
    let unread_count = Signal::derive(move || {
        sync_store
            .map(|store| store.notifications().get().iter().filter(|n| !n.read).count())
            .unwrap_or(0)
    });

    view! {
        <a href="/inbox" class=class>
            <Icon icon=phosphor_leptos::TRAY weight=weight size="18px"/>
            "Inbox"
            {move || {
                let count = unread_count.get();
                if count > 0 {
                    Some(view! {
                        <span class="ml-auto text-xs font-medium text-primary-foreground bg-primary rounded-full px-1.5 py-0.5 min-w-[1.25rem] text-center">
                            {count}
                        </span>
                    })
                } else {
                    None
                }
            }}
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
        let base = "flex items-center gap-3 px-3 py-1.5 rounded-md text-sm transition-colors";
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

/// Persistent banner shown when the workspace subscription is past due.
///
/// Reads the `UserContext` resource provided by the `App` component. Only
/// visible to workspace owners when `subscription_status == "past_due"`.
/// Clicking the link opens the Stripe billing portal.
#[component]
fn BillingBanner() -> impl IntoView {
    let user_ctx = expect_context::<LocalResource<Result<UserContext, ServerFnError>>>();

    let should_show = Signal::derive(move || {
        user_ctx
            .get()
            .and_then(|r| r.ok())
            .map(|ctx| {
                ctx.is_owner
                    && ctx.billing_enabled
                    && ctx.subscription_status.as_deref() == Some("past_due")
            })
            .unwrap_or(false)
    });

    let (portal_loading, set_portal_loading) = signal(false);

    let on_fix_click = move |_: web_sys::MouseEvent| {
        if portal_loading.get_untracked() {
            return;
        }
        set_portal_loading.set(true);
        leptos::task::spawn_local(async move {
            match crate::server_fns::billing::create_billing_portal_session().await {
                Ok(url) => {
                    #[cfg(target_arch = "wasm32")]
                    {
                        if let Some(window) = web_sys::window() {
                            let _ = window.location().set_href(&url);
                        }
                    }
                    let _ = url;
                }
                Err(e) => {
                    tracing::warn!("Failed to create billing portal session: {e}");
                    set_portal_loading.set(false);
                }
            }
        });
    };

    view! {
        <Show when=move || should_show.get()>
            <div class="bg-warning/10 border-b border-warning/30 px-4 py-2.5 flex items-center justify-between gap-3">
                <div class="flex items-center gap-2 text-sm text-warning-foreground">
                    <Icon icon=phosphor_leptos::WARNING weight=IconWeight::Fill size="16px" attr:class="flex-shrink-0"/>
                    <span>"Payment failed \u{2014} team invites are paused."</span>
                </div>
                <Button
                    variant=ButtonVariant::Ghost
                    size=ButtonSize::Sm
                    on:click=on_fix_click
                >
                    {move || {
                        if portal_loading.get() {
                            "Opening...".to_string()
                        } else {
                            "Update payment method \u{2192}".to_string()
                        }
                    }}
                </Button>
            </div>
        </Show>
    }
}
