// SPDX-License-Identifier: AGPL-3.0-or-later

//! Full-screen image lightbox overlay.
//!
//! Displays an image at maximum viewport size with a dark backdrop.
//! Closes on backdrop click or Escape key.

use leptos::prelude::*;
use phosphor_leptos::Icon;

use super::attachment_hooks::LightboxState;

/// Full-screen image lightbox overlay.
///
/// Renders nothing when `state` is `None`. When `Some(LightboxState)`, shows
/// the image in a centered overlay with close-on-click and Escape support.
#[component]
pub fn Lightbox(
    state: RwSignal<Option<LightboxState>>,
) -> impl IntoView {
    let close = move |_: web_sys::MouseEvent| state.set(None);

    // Close on Escape key (client-only)
    #[cfg(target_arch = "wasm32")]
    {
        use send_wrapper::SendWrapper;
        use wasm_bindgen::closure::Closure;
        use wasm_bindgen::JsCast;

        let handler = Closure::<dyn Fn(web_sys::KeyboardEvent)>::new(move |ev: web_sys::KeyboardEvent| {
            if ev.key() == "Escape" {
                state.set(None);
            }
        });

        if let Some(window) = web_sys::window() {
            let _ = window.add_event_listener_with_callback(
                "keydown",
                handler.as_ref().unchecked_ref(),
            );

            // Keep closure alive for the component's lifetime, clean up on unmount.
            // SendWrapper is needed because Closure<dyn Fn(KeyboardEvent)> is !Sync
            // but on_cleanup requires Send + Sync. On WASM this is a no-op wrapper.
            let cb_fn: js_sys::Function = handler.as_ref().unchecked_ref::<js_sys::Function>().clone();
            let cleanup_handler = SendWrapper::new(handler);
            let cleanup_window = SendWrapper::new(window);
            let cleanup_fn = SendWrapper::new(cb_fn);
            on_cleanup(move || {
                let _ = cleanup_window.remove_event_listener_with_callback("keydown", &cleanup_fn);
                drop(cleanup_handler);
            });
        }
    }

    move || {
        let s = state.get()?;
        Some(view! {
            <div
                class="fixed inset-0 z-50 flex items-center justify-center bg-black/80 backdrop-blur-sm cursor-zoom-out"
                on:click=close
            >
                <button
                    class="absolute top-4 right-4 text-white/80 hover:text-white transition-colors duration-200 focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring rounded"
                    on:click=move |ev: web_sys::MouseEvent| {
                        ev.stop_propagation();
                        state.set(None);
                    }
                    aria-label="Close lightbox"
                >
                    <Icon icon=phosphor_leptos::X weight=phosphor_leptos::IconWeight::Bold attr:class="h-6 w-6" />
                </button>
                <img
                    src=s.src
                    class="max-w-[90vw] max-h-[90vh] object-contain rounded-lg shadow-2xl"
                    on:click=move |ev: web_sys::MouseEvent| ev.stop_propagation()
                    alt="Lightbox preview"
                />
            </div>
        })
    }
}
