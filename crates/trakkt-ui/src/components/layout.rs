// SPDX-License-Identifier: AGPL-3.0-or-later

//! App layout — sidebar + content area.

use leptos::prelude::*;
use leptos_router::components::Outlet;

use wasm_bindgen::closure::Closure;
use wasm_bindgen::JsCast;

use phosphor_leptos::{Icon, IconWeight};

use std::collections::HashMap;
use trakkt_types::enums::FavoriteTarget;
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
    // The leader tab: additionally start the engine and dial the WebSocket —
    //                 immediately if it wins the lock, or later on promotion
    //                 when the previous leader's tab closes.
    //
    // Both live-update paths wait on `hydration_gate`. Hydration replaces whole
    // store lists at once, so anything applied while it is still in flight gets
    // wiped by the `set_*` that lands after it — and the leader's cursor has
    // already moved past, so nothing re-delivers it.
    //
    // The two wait differently because their transports differ. The socket is
    // simply not dialed until hydration finishes: nothing has been received
    // yet, so delaying it only reorders. The cross-tab channel cannot be
    // treated the same way — it has no replay, so a late subscription would
    // *drop* whatever other tabs posted in that window rather than delay it.
    // So the subscription goes up immediately and its messages are held in a
    // FIFO until the gate opens. See `cache::broadcast_queue`.
    #[cfg(target_arch = "wasm32")]
    {
        use crate::cache::broadcast_queue::BroadcastQueue;
        use crate::cache::delete_route::DeleteRoute;
        use crate::cache::hydration_gate::HydrationGate;
        use crate::cache::idb_writer::IdbWriter;
        use crate::cache::sync_engine;
        use crate::cache::tab_leader::{self, Leadership, SyncBroadcast};
        use crate::cache::websocket;
        use crate::server_fns::context::UserContext;

        let user_ctx = expect_context::<LocalResource<Result<UserContext, ServerFnError>>>();

        // The WebSocket handle is built here, at component setup, and dialed
        // later — only by the leader, and only once hydration has finished.
        // Building it up front is what lets `provide_context` run synchronously
        // in the reactive scope, where pages resolving `WebSocketClient` can
        // actually see it: context is a setup-time snapshot, so a handle
        // provided from inside the effect below would be invisible to every
        // page. One handle for the tab's whole life also means a page mounted
        // before this tab is promoted observes the real `connection_state`
        // afterwards, instead of holding a stale disconnected handle.
        let ws_client = websocket::disconnected();
        provide_context(ws_client.clone());
        {
            let ws_for_cleanup = ws_client.clone();
            on_cleanup(move || websocket::disconnect(&ws_for_cleanup));
        }

        // Latch that hydration opens, and that both the dial and the cross-tab
        // message queue wait on. Lives at setup because its halves can run in
        // different executions of the effect below: a promoted follower
        // hydrated long ago, while the first tab hydrates and takes leadership
        // in a single pass.
        let hydration_gate = HydrationGate::new();

        // The owner the sync engine's connection-state watcher is registered
        // under. It has to be this one — created here, at setup — rather than
        // whichever owner is current when the leader half below runs.
        //
        // That half runs inside the effect body, and an `Effect` re-run calls
        // `Owner::with_cleanup` on its own owner: everything the previous run
        // created is disposed. A watcher registered from in there would be torn
        // down by the next re-run, leaving the socket reconnecting on its own
        // backoff with nothing left to notice it reaching `Connected` — so no
        // `sync_bootstrap` or `sync_delta` would ever go out again and the tab
        // would look connected while it had silently stopped syncing.
        //
        // A child of the component's owner, so the watcher is disposed with the
        // Layout and not before. Same reasoning as the handles above; this one
        // is a reactive scope rather than a browser handle.
        let engine_owner = Owner::new();

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
        // The cache writer, set only once this tab holds the leadership lock. A
        // follower never has one — which is the whole reason its deletes travel
        // over the broadcast channel instead. The message handler below reads it
        // on every message, so a tab promoted after the handler was installed
        // starts servicing other tabs' deletes without re-registering anything.
        let cache_writer: StoredValue<send_wrapper::SendWrapper<Option<IdbWriter>>> =
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

                // 1. Hydrate from IDB (instant cached data), then open the gate
                //    the leader's dial is waiting on.
                leptos::task::spawn_local(sync_engine::hydrate_then_open_gate(
                    workspace_id.clone(),
                    sync_store,
                    hydration_gate.clone(),
                ));

                // 2. Subscribe to the cross-tab channel. A follower's entire
                //    live-update path runs through here; the leader opens the
                //    same channel to publish on (it never receives its own
                //    messages back) and to service the cache deletes follower
                //    tabs ask it to perform.
                //
                //    The subscription is registered now and its messages are
                //    queued, rather than the subscription itself being delayed
                //    until hydration finishes. That ordering matters both ways:
                //    delaying it would lose messages outright, and applying
                //    them on arrival would hand them to lists hydration is
                //    about to replace.
                //
                //    The queue is created here, in the same synchronous block
                //    that spawned hydration above — so there is no arrangement
                //    in which a queue exists to fill but no hydration exists to
                //    release it, and the backlog is bounded by hydration
                //    finishing.
                match SyncBroadcast::open(&workspace_id) {
                    Ok(channel) => {
                        let queue = BroadcastQueue::new(move |message| {
                            cache_writer.with_value(|writer| {
                                crate::cache::apply::apply_broadcast(
                                    &sync_store,
                                    (**writer).as_ref(),
                                    message,
                                );
                            });
                        });
                        leptos::task::spawn_local(sync_engine::release_when_hydrated(
                            hydration_gate.clone(),
                            queue.clone(),
                        ));
                        channel.set_on_message(move |message| queue.deliver(message));
                        // Until this tab wins the lock it owns no cache writer,
                        // so its own deletes go to the tab that does.
                        sync_store.set_delete_route(DeleteRoute::delegated(channel.clone()));
                        *broadcast.write_value() =
                            send_wrapper::SendWrapper::new(Some(channel));
                    }
                    Err(e) => tracing::warn!(
                        "sync: no BroadcastChannel ({e:?}) — this tab will not see the \
                         leader's updates until it reloads, and cannot ask the leader to \
                         delete anything from the shared cache"
                    ),
                }

                // 3. Stand for election. The callback fires immediately if no
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

            // Registering the message callback and the connection-state watcher
            // before the socket exists is safe — and required: the dial below
            // happens on a later turn of the event loop, so the engine is
            // listening well before the first byte can arrive.
            //
            // The watcher goes under `engine_owner` rather than this effect
            // run's own owner, which is what keeps it alive across any later
            // re-run of this effect. See where `engine_owner` is created.
            let writer = sync_engine::start_sync_engine(
                &engine_owner,
                &ws_client,
                &sync_store,
                &workspace_id,
                broadcast.with_value(|channel| (**channel).clone()),
            );

            // This tab now owns every write to the shared cache. Its own deletes
            // go straight onto the queue rather than round-tripping through the
            // channel to itself, and the handler installed above starts
            // enqueueing the deletes other tabs ask for.
            sync_store.set_delete_route(DeleteRoute::owned(writer.clone()));
            *cache_writer.write_value() = send_wrapper::SendWrapper::new(Some(writer));

            // Dial once hydration is done, with a token already in hand.
            //
            // The token is a JWT in both deployment modes: personal mode issues
            // one like any other (the server bypasses auth for the WebSocket, so
            // it is simply ignored there), multi-user mode requires it. Fetching
            // it *before* the first dial is what removes the old
            // connect-with-nothing → 4001 close → reconnect-with-a-JWT churn on
            // every multi-user page load.
            //
            // If the token cannot be fetched we still dial. In personal mode the
            // connection succeeds regardless; in multi-user mode the server
            // closes it, which is precisely the event that drives the existing
            // backoff loop — and that loop re-fetches the token on every
            // attempt. Refusing to dial would instead leave the tab with no
            // socket and no path back to one.
            leptos::task::spawn_local(sync_engine::dial_when_hydrated(
                hydration_gate.clone(),
                ws_client.clone(),
                user_id,
                workspace_id,
                async {
                    match crate::server_fns::auth::get_ws_token().await {
                        Ok(token) => token,
                        Err(e) => {
                            tracing::warn!(
                                "sync: could not fetch a WebSocket token — dialing without one; \
                                 an unauthenticated close will trigger the reconnect loop, which \
                                 fetches a fresh token per attempt: {e}"
                            );
                            String::new()
                        }
                    }
                },
            ));
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
                                <SidebarEntityItem href=href name=name icon=phosphor_leptos::FOLDER favorite_type=FavoriteTarget::Project favorite_id=fav_id/>
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
    target_type: FavoriteTarget,
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
                    .any(|f| f.target_type == target_type.as_str() && f.target_id == tid)
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
        let tt = target_type.as_str().to_string();
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
    #[prop(optional)] favorite_type: Option<FavoriteTarget>,
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
    // Resolved here, at component setup, rather than inside the closure below:
    // the eight collection getters build a fresh arena-registered `Signal`
    // wrapper on each call, so calling one from a closure that re-runs abandons
    // a wrapper per evaluation and is one refactor from the disposed-value
    // panic. This is the site TRA-9995 found in that shape. The nine
    // `*_version()` counters no longer behave this way — TRA-9996 moved them to
    // `ArcSignal`, which has no owner — so the rule is now specific to the
    // collections. See the getter notes on `SyncStore`, and [[TRA-9998]].
    let notifications = sync_store.map(|store| store.notifications());
    let unread_count = Signal::derive(move || {
        notifications
            .map(|list| list.get().iter().filter(|n| n.is_unread_in_inbox()).count())
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

// ── Browser tests ───────────────────────────────────────────────────────────

/// What the sidebar's unread badge counts, asserted on the badge itself.
///
/// These mount the real `SidebarInboxNavItem` and read the number out of the
/// DOM. The alternative — evaluating the count expression from a test — restates
/// the predicate under test and would agree with it however wrong it is, which
/// is how TRA-9995 survived a suite that already covered the store this badge
/// reads from.
#[cfg(all(test, target_arch = "wasm32"))]
mod wasm_tests {
    use gloo_timers::future::TimeoutFuture;
    use leptos::prelude::*;
    use leptos_router::components::Router;
    use trakkt_types::enums::ActionSource;
    use trakkt_types::models::Notification;
    use trakkt_types::sync::{entity_types, SyncAction, SyncActionType};
    use wasm_bindgen_test::{wasm_bindgen_test, wasm_bindgen_test_configure};

    use crate::cache::apply::apply_action_to_memory;
    use crate::cache::store::SyncStore;
    use crate::wasm_test_support::{boot_leptos_executor, mount_container};

    use super::*;

    wasm_bindgen_test_configure!(run_in_browser);

    /// One unread, undeleted notification.
    ///
    /// Built as the model rather than as JSON so that a field renamed on
    /// [`Notification`] changes this fixture and the payload [`update_frame`]
    /// serialises together, instead of leaving the two agreeing only by hand.
    fn unread(notification_id: &str) -> Notification {
        Notification {
            notification_id: notification_id.to_owned(),
            workspace_id: "ws-1".to_owned(),
            user_id: "usr-alice".to_owned(),
            issue_id: "issue-1".to_owned(),
            notification_type: "assigned".to_owned(),
            read: false,
            issue_title: Some("A leaky issue".to_owned()),
            issue_number: Some(42),
            team_key: Some("TRA".to_owned()),
            actor_id: Some("usr-bob".to_owned()),
            actor_name: Some("Bob".to_owned()),
            action_source: ActionSource::User,
            action_source_label: None,
            created_at: "2026-07-26T00:00:00Z".to_owned(),
            deleted_at: None,
            context_id: None,
        }
    }

    /// The frame the server delivers for one notification it has just changed.
    ///
    /// `notification_service::change_notifications` records one `Update` per
    /// affected row carrying the whole row as `sync_log_service::sync_payload`
    /// serialised it, and `commit_and_deliver` sends it to every session the
    /// recipient has open — including the one that asked for the change. A
    /// soft-delete and a mark-read are the same frame with a different row
    /// inside it, which is why both tests below build theirs here.
    fn update_frame(notification: &Notification) -> SyncAction {
        SyncAction {
            sync_id: 1,
            entity_type: entity_types::NOTIFICATION.to_owned(),
            entity_id: notification.notification_id.clone(),
            workspace_id: notification.workspace_id.clone(),
            action: SyncActionType::Update,
            data: Some(
                serde_json::to_value(notification)
                    .expect("serializing a Notification the way `sync_payload` does"),
            ),
            timestamp: "2026-07-26T01:00:00Z".to_owned(),
        }
    }

    /// Mount the real sidebar item with `store` in context, as `Layout` provides
    /// it. The caller drops the handle and removes the container when done.
    ///
    /// The `<Router>` is not decoration: `SidebarInboxNavItem` calls
    /// `use_location` to decide whether it is the active item, and that panics
    /// outside a router context. Nothing here asserts on the active state — the
    /// router is the price of mounting the component unmodified rather than a
    /// stand-in that would prove nothing about the badge in the sidebar.
    fn mount_inbox_nav_item(store: SyncStore) -> (impl Sized, web_sys::HtmlElement) {
        let container = mount_container();
        let handle = leptos::mount::mount_to(container.clone(), move || {
            provide_context(store);
            view! { <Router><SidebarInboxNavItem/></Router> }
        });
        (handle, container)
    }

    /// The number the badge is showing, or `None` when no badge is rendered.
    ///
    /// The `<span>` is the only one in the anchor: the tray icon renders an
    /// `<svg>` and the "Inbox" label is a bare text node. So this selector finds
    /// the badge or finds nothing, and "nothing" is the count reaching zero
    /// rather than a selector that stopped matching.
    fn badge_text(container: &web_sys::HtmlElement) -> Option<String> {
        container
            .query_selector("a[href=\"/inbox\"] span")
            .expect("querying the mounted sidebar item for its unread badge")
            .map(|span| {
                span.text_content()
                    .expect("an element node always has textContent")
            })
    }

    /// Deleting an unread notification has to take it off the badge.
    ///
    /// Deleting from the inbox is a *soft* delete: `bulk_delete_notifications`
    /// stamps `deleted_at` and the row stays in this tab's store, arriving as an
    /// `Update` — `cache::apply`'s
    /// `a_soft_deleted_notification_frame_keeps_the_row_and_stamps_it` pins that
    /// half. So a badge that counts `!read` alone goes on counting a row the
    /// inbox no longer lists, and goes on counting it until the page is
    /// reloaded. That is TRA-9995.
    ///
    /// Two notifications rather than one so the assertion is a count that
    /// dropped and not a badge that vanished: at zero the badge is not rendered
    /// at all, and an element missing for some unrelated reason would read the
    /// same.
    #[wasm_bindgen_test]
    async fn the_badge_stops_counting_a_notification_the_user_deleted() {
        boot_leptos_executor();

        let store = SyncStore::new();
        store.set_notifications(vec![unread("ntf-1"), unread("ntf-2")]);
        let (handle, container) = mount_inbox_nav_item(store);

        TimeoutFuture::new(100).await;
        assert_eq!(
            badge_text(&container).as_deref(),
            Some("2"),
            "the badge is not showing the two unread notifications the store was \
             seeded with, so nothing below this line measures what deleting one \
             does — fix this first"
        );

        let mut deleted = unread("ntf-1");
        deleted.deleted_at = Some("2026-07-26T01:00:00Z".to_owned());
        apply_action_to_memory(&store, &update_frame(&deleted));

        TimeoutFuture::new(100).await;
        assert_eq!(
            badge_text(&container).as_deref(),
            Some("1"),
            "the badge still counts a notification the user deleted. The row is \
             still in the store — the delete stamped `deleted_at` instead of \
             evicting it — so the count has to exclude it explicitly, the way \
             `notification_service::count_unread` does with \
             `read = false AND deleted_at IS NULL`"
        );

        drop(handle);
        container.remove();
    }

    /// Marking an unread notification read has to take it off the badge too.
    ///
    /// Same frame, same store, the other half of the predicate — so this is what
    /// stops a fix for the delete case from being written as `deleted_at
    /// IS NULL` alone. It arrives here by the sync frame rather than by the
    /// inbox's optimistic `upsert_notification`, because the frame is the path
    /// that has to work for the tab the user is *not* looking at.
    #[wasm_bindgen_test]
    async fn the_badge_stops_counting_a_notification_the_user_read() {
        boot_leptos_executor();

        let store = SyncStore::new();
        store.set_notifications(vec![unread("ntf-1"), unread("ntf-2")]);
        let (handle, container) = mount_inbox_nav_item(store);

        TimeoutFuture::new(100).await;
        assert_eq!(
            badge_text(&container).as_deref(),
            Some("2"),
            "the badge is not showing the two unread notifications the store was \
             seeded with, so nothing below this line measures what reading one \
             does — fix this first"
        );

        let mut read = unread("ntf-1");
        read.read = true;
        apply_action_to_memory(&store, &update_frame(&read));

        TimeoutFuture::new(100).await;
        assert_eq!(
            badge_text(&container).as_deref(),
            Some("1"),
            "the badge still counts a notification that has been read in this \
             workspace, so it is no longer the count of things needing attention \
             that it exists to be"
        );

        drop(handle);
        container.remove();
    }
}
