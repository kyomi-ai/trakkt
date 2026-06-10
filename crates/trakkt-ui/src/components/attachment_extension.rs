// SPDX-License-Identifier: AGPL-3.0-or-later

//! Kode editor extension that adds "Attach file" to the `/` slash command menu.
//!
//! When selected, fires a trigger callback that the parent component uses to
//! open a native file picker. The actual upload and inline insertion are handled
//! by the existing `attachment_hooks` pipeline.

use std::sync::Arc;

use kode_leptos::extension::{Extension, ExtensionToolbarItem};
use leptos::prelude::*;

/// Phosphor paperclip icon SVG (Regular weight) for the slash command menu.
///
/// Must match the format used by kode's `BuiltinButton::icon_svg()`: a complete
/// `<svg>` element with `viewBox="0 0 256 256"`, `fill="currentColor"`, and no
/// explicit width/height (CSS controls sizing via `.kode-slash-menu-item-icon svg`).
const PAPERCLIP_SVG: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 256 256" fill="currentColor"><path d="M209.66,122.34a8,8,0,0,1,0,11.32l-82.05,82a56,56,0,0,1-79.2-79.21L147.67,35.73a40,40,0,1,1,56.61,56.55L105,193A24,24,0,1,1,71,159L154.3,74.38A8,8,0,1,1,165.7,85.6L82.39,170.31a8,8,0,1,0,11.27,11.36L192.93,81A24,24,0,1,0,159,47L59.76,147.68a40,40,0,1,0,56.53,56.62l82.06-82A8,8,0,0,1,209.66,122.34Z"/></svg>"#;

/// A kode Extension that contributes an "Attach file" item to the slash
/// command menu (via `toolbar_items`).
///
/// The `trigger` callback is invoked when the user selects the menu item.
/// The parent component should wire this to open a hidden `<input type="file">`
/// element, then feed the selected file through the existing upload pipeline.
pub struct AttachFileExtension {
    trigger: Arc<dyn Fn() + Send + Sync>,
}

impl AttachFileExtension {
    pub fn new(trigger: Arc<dyn Fn() + Send + Sync>) -> Self {
        Self { trigger }
    }
}

impl Extension for AttachFileExtension {
    fn name(&self) -> &str {
        "attach-file"
    }

    fn toolbar_items(&self) -> Vec<ExtensionToolbarItem> {
        let trigger = self.trigger.clone();
        vec![ExtensionToolbarItem {
            label: view! {
                <span class="flex items-center gap-2">
                    <phosphor_leptos::Icon icon=phosphor_leptos::PAPERCLIP size="16px"/>
                    "Attach file"
                </span>
            }
            .into_any(),
            title: "Attach file".to_string(),
            description: "Upload an image or file".to_string(),
            group: 10,
            action: Arc::new(move |_editor| {
                trigger();
            }),
            active_name: None,
            icon_svg: Some(PAPERCLIP_SVG.to_string()),
        }]
    }
}
