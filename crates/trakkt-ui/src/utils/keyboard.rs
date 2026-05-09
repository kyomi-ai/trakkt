// SPDX-License-Identifier: AGPL-3.0-or-later

//! Shared keyboard utilities for Leptos pages.

/// Returns true if an input/textarea/select is currently focused,
/// or the target element has `contenteditable` set. When this returns
/// true, single-key shortcuts (j/k/c) should NOT fire so they don't
/// interfere with text editing.
pub fn is_input_focused(ev: &web_sys::KeyboardEvent) -> bool {
    use wasm_bindgen::JsCast;
    let Some(target) = ev.target() else {
        return false;
    };
    let Some(el) = target.dyn_ref::<web_sys::HtmlElement>() else {
        return false;
    };
    let tag = el.tag_name().to_uppercase();
    if matches!(tag.as_str(), "INPUT" | "TEXTAREA" | "SELECT") {
        return true;
    }
    // Check for contenteditable (kode editor, rich text fields).
    if el.is_content_editable() {
        return true;
    }
    false
}
