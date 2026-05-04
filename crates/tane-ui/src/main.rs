// SPDX-License-Identifier: AGPL-3.0-or-later
#![recursion_limit = "512"]

//! WASM entry point for the Leptos frontend.

use tane_ui::app::App;
use leptos::prelude::*;

fn main() {
    console_error_panic_hook::set_once();
    mount_to_body(App);

    if let Some(window) = web_sys::window()
        && let Some(document) = window.document()
        && let Some(loading) = document.get_element_by_id("tane-loading")
    {
        loading.remove();
    }
}
