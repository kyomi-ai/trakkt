// SPDX-License-Identifier: AGPL-3.0-or-later

//! "via {source}" attribution suffix for non-User action sources.
//!
//! Used in the activity timeline, comment headers, and inbox notifications
//! to show when an action originated from an agent (e.g. Claude) or API
//! integration (e.g. Slack).

use leptos::prelude::*;
use trakkt_types::enums::ActionSource;

/// Renders a "via {label}" suffix for Agent or Api action sources.
///
/// Returns an empty view for `ActionSource::User` (no suffix needed).
/// The "via" text uses `text-muted-foreground font-normal` to visually
/// de-emphasize it relative to the actor name.
///
/// Takes owned values to satisfy Leptos view `'static` lifetime requirements.
pub fn render_via_suffix(
    action_source: ActionSource,
    action_source_label: Option<String>,
) -> impl IntoView {
    match action_source {
        ActionSource::User => ().into_any(),
        ActionSource::Agent => {
            let label = action_source_label
                .unwrap_or_else(|| "Agent".to_string());
            view! {
                <span class="text-muted-foreground font-normal">" via "{label}</span>
            }
            .into_any()
        }
        ActionSource::Api => {
            let label = action_source_label
                .unwrap_or_else(|| "API".to_string());
            view! {
                <span class="text-muted-foreground font-normal">" via "{label}</span>
            }
            .into_any()
        }
    }
}
