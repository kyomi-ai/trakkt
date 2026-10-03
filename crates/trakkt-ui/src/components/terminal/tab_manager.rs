// SPDX-License-Identifier: AGPL-3.0-or-later

//! Accessible terminal tabs with separate select and close actions.

use crate::components::button::{Button, ButtonSize, ButtonVariant, ToggleButton};
use leptos::prelude::*;
use phosphor_leptos::{Icon, IconWeight};

#[derive(Clone, Debug)]
pub struct SessionTab {
    pub session_id: String,
    pub label: String,
}

#[component]
pub fn TabBar(
    tabs: Signal<Vec<SessionTab>>,
    active_session: Signal<Option<String>>,
    on_select: Callback<String>,
    on_close: Callback<String>,
) -> impl IntoView {
    view! {
        <div class="flex items-center gap-2 px-5 pb-2 shrink-0 overflow-x-auto" role="tablist" aria-label="Terminal sessions">
            <For each=move || tabs.get() key=|tab| tab.session_id.clone() let(tab)>
                {
                    let select_id = tab.session_id.clone();
                    let active_id = tab.session_id.clone();
                    let aria_id = tab.session_id.clone();
                    let close_id = tab.session_id.clone();
                    let label_id = tab.session_id.clone();
                    let close_label = format!("Close session {}", tab.session_id);
                    view! {
                        <div class="flex items-center gap-1">
                            <ToggleButton variant=Signal::derive(move || if active_session.get().as_deref() == Some(active_id.as_str()) {
                                ButtonVariant::Active
                            } else { ButtonVariant::GhostMuted }) size=ButtonSize::Sm
                                attr:role="tab"
                                attr:data-session-id=tab.session_id
                                attr:aria-selected=move || active_session.get().as_deref() == Some(aria_id.as_str())
                                on:click=move |_| on_select.run(select_id.clone())>
                                <span class="truncate max-w-[180px]">{move || tabs.with(|tabs| tabs.iter()
                                    .find(|tab| tab.session_id == label_id).map(|tab| tab.label.clone()))}</span>
                            </ToggleButton>
                            <Button variant=ButtonVariant::GhostDestructive size=ButtonSize::IconXs aria_label=close_label
                                on:click=move |_| on_close.run(close_id.clone())>
                                <Icon icon=phosphor_leptos::X weight=IconWeight::Regular size="12px"/>
                            </Button>
                        </div>
                    }
                }
            </For>
        </div>
    }
}
