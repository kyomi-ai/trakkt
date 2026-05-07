// SPDX-License-Identifier: AGPL-3.0-or-later

//! Label badge — colored pill for issue labels.
//!
//! DESIGN.md Issue Row Pattern: "Labels: colored pills, text-xs px-1.5 py-0.5 rounded-sm"
//!
//! Dynamic background from the label's hex color with automatic text contrast
//! (white text on dark backgrounds, dark text on light backgrounds).

use leptos::prelude::*;

/// Determine whether a hex color is dark enough to need white text overlay.
///
/// Uses the ITU-R BT.601 relative luminance formula (same as used by W3C WCAG).
fn is_dark_color(hex: &str) -> bool {
    let hex = hex.trim_start_matches('#');
    if hex.len() < 6 {
        return true;
    }
    let r = u8::from_str_radix(&hex[0..2], 16).unwrap_or(0) as f64;
    let g = u8::from_str_radix(&hex[2..4], 16).unwrap_or(0) as f64;
    let b = u8::from_str_radix(&hex[4..6], 16).unwrap_or(0) as f64;
    let luminance = (0.299 * r + 0.587 * g + 0.114 * b) / 255.0;
    luminance < 0.5
}

/// Colored pill badge for an issue label.
///
/// # Usage
/// ```ignore
/// <LabelBadge name="bug".to_string() color="#DC2626".to_string()/>
/// ```
#[component]
pub fn LabelBadge(
    #[prop(into)] name: String,
    #[prop(into)] color: String,
) -> impl IntoView {
    let text_class = if is_dark_color(&color) {
        "text-white"
    } else {
        "text-stone-900"
    };
    view! {
        <span
            class={format!("inline-flex items-center px-1.5 py-0.5 rounded-sm text-xs font-medium {text_class}")}
            style=format!("background-color: {color}")
        >
            {name}
        </span>
    }
}
