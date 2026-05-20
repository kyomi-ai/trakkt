// SPDX-License-Identifier: AGPL-3.0-or-later

//! Kode editor extension that adds "Attach file" to the `/` slash command menu.
//!
//! When selected, fires a trigger callback that the parent component uses to
//! open a native file picker. The actual upload and inline insertion are handled
//! by the existing `attachment_hooks` pipeline.

use std::sync::Arc;

use kode_leptos::extension::{Extension, ExtensionToolbarItem};
use leptos::prelude::*;

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
        }]
    }
}
