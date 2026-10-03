// SPDX-License-Identifier: AGPL-3.0-or-later

//! Team key badge -- colored pill showing the team's short key.
//!
//! DESIGN.md Issue Row Pattern: "Labels: colored pills, text-xs px-1.5 py-0.5 rounded-sm"
//!
//! Dynamic background from the team's hex color with automatic text contrast
//! (white text on dark backgrounds, foreground text on light backgrounds).

use leptos::prelude::*;

use super::label_badge::is_dark_color;

/// Colored pill badge displaying a team's short key (e.g. "TRA", "KYO").
///
/// # Usage
/// ```ignore
/// <TeamKeyBadge team_key="TRA".to_string() color="#0D9488".to_string()/>
/// ```
#[component]
pub fn TeamKeyBadge(
    #[prop(into)] team_key: String,
    #[prop(into)] color: String,
) -> impl IntoView {
    let text_class = if is_dark_color(&color) {
        "text-white"
    } else {
        "text-foreground"
    };
    view! {
        <span
            class={format!("inline-flex items-center px-1.5 py-0.5 rounded-sm text-xs font-medium shrink-0 {text_class}")}
            style=format!("background-color: {color}")
        >
            {team_key}
        </span>
    }
}
