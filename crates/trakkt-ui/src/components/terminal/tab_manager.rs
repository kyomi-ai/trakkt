// SPDX-License-Identifier: AGPL-3.0-or-later

//! Terminal tab bar — session-based tab strip for managing multiple terminal
//! sessions.  Renders a horizontal row of tabs with new/close controls.

use leptos::prelude::*;
use phosphor_leptos::{Icon, IconWeight};

/// Metadata for a single terminal session tab.
#[derive(Clone, Debug)]
pub struct SessionTab {
    pub session_id: String,
    pub label: String,
}

/// Horizontal tab bar for terminal sessions.
///
/// Displays one tab per session, highlights the active session, and exposes
/// callbacks for selecting, creating, and closing sessions.
///
/// # Usage
/// ```ignore
/// <TabBar
///     tabs=tabs_signal
///     active_session=active_signal
///     on_select=Callback::new(|id| { /* switch session */ })
///     on_new=Callback::new(|()| { /* create session */ })
///     on_close=Callback::new(|id| { /* close session */ })
/// />
/// ```
#[component]
pub fn TabBar(
    tabs: Signal<Vec<SessionTab>>,
    active_session: Signal<Option<String>>,
    on_select: Callback<String>,
    on_new: Callback<()>,
    on_close: Callback<String>,
) -> impl IntoView {
    view! {
        <div class="flex items-center gap-1 px-2 py-1 bg-[#1e1e1e] border-b border-[#333]">
            <For
                each=move || tabs.get()
                key=|tab| tab.session_id.clone()
                let(tab)
            >
                {
                    let session_id = tab.session_id.clone();
                    let label = tab.label.clone();
                    let aria_text = format!("Switch to session {label}");
                    let label_text = label.clone();
                    let select_id = session_id.clone();
                    let close_id = session_id.clone();

                    let is_active = {
                        let session_id = session_id.clone();
                        move || active_session.get().as_deref() == Some(session_id.as_str())
                    };

                    view! {
                        <button
                            type="button"
                            class=move || {
                                let base = "flex items-center gap-2 px-3 py-1.5 rounded-t text-sm cursor-pointer transition-colors duration-200 group/tab focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring";
                                if is_active() {
                                    format!("{base} bg-[#2d2d2d] text-white border-b-2 border-[#0D9488]")
                                } else {
                                    format!("{base} text-[#888] hover:text-[#ccc] hover:bg-[#252525]")
                                }
                            }
                            on:click=move |_| on_select.run(select_id.clone())
                            aria-label=aria_text
                        >
                            <span class="truncate max-w-[150px]">{label_text}</span>
                            <button
                                type="button"
                                class="opacity-0 group-hover/tab:opacity-100 inline-flex items-center justify-center h-4 w-4 rounded text-[#666] hover:text-[#fff] hover:bg-[#444] transition-colors duration-200 focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
                                aria-label="Close session"
                                on:click=move |ev: web_sys::MouseEvent| {
                                    ev.stop_propagation();
                                    on_close.run(close_id.clone());
                                }
                            >
                                <Icon icon=phosphor_leptos::X weight=IconWeight::Bold size="10px"/>
                            </button>
                        </button>
                    }
                }
            </For>

            // New session button
            <button
                type="button"
                class="p-1.5 rounded text-[#666] hover:text-white hover:bg-[#333] transition-colors duration-200 focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
                aria-label="New session"
                on:click=move |_| on_new.run(())
            >
                <Icon icon=phosphor_leptos::PLUS weight=IconWeight::Bold size="14px"/>
            </button>
        </div>
    }
}
