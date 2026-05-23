// SPDX-License-Identifier: AGPL-3.0-or-later

//! Kode editor extension that adds "Attach file" to the `/` slash command menu.
//!
//! When selected, fires a trigger callback that the parent component uses to
//! open a native file picker. The actual upload and inline insertion are handled
//! by the existing `attachment_hooks` pipeline.

use std::sync::Arc;

use kode_leptos::extension::{Extension, ExtensionToolbarItem};
use leptos::prelude::*;

/// Phosphor paperclip icon SVG for the slash command menu.
const PAPERCLIP_SVG: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 256 256" fill="currentColor"><path d="M209.66,82.34l-120,120a48,48,0,0,1-67.88-67.88l120-120A32,32,0,0,1,187.31,60.1l-120,120a16,16,0,0,1-22.62-22.62l120-120a8,8,0,0,0-11.32-11.32l-120,120a32,32,0,0,0,45.26,45.26l120-120a48,48,0,0,0-67.88-67.88l-120,120A64,64,0,0,0,100.69,214.63l120-120a8,8,0,0,0-11-11.32Z"/></svg>"#;

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
