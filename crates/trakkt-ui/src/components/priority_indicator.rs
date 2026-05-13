// SPDX-License-Identifier: AGPL-3.0-or-later

//! Priority indicator — SVG icon for issue priority level.
//!
//! DESIGN.md § "Priority Icons (3 bars + urgent exclamation)":
//! - 1 (Urgent): Red rounded square with white exclamation mark (#DC2626)
//! - 2 (High):   3 ascending bars, all filled (currentColor)
//! - 3 (Medium): 3 ascending bars, 2 filled (currentColor + 0.25 opacity unfilled)
//! - 4 (Low):    3 ascending bars, 1 filled (currentColor + 0.25 opacity unfilled)
//! - 0 (None):   Horizontal dash line (currentColor at lower opacity)
//!
//! Shape rule: Priority icons are SQUARE (rectangles with small radius).
//! This distinguishes them from status icons which are ROUND (circles).

use leptos::prelude::*;

/// Returns the aria-label for a given priority level.
fn priority_label(priority: i32) -> &'static str {
    match priority {
        1 => "Urgent",
        2 => "High",
        3 => "Medium",
        4 => "Low",
        _ => "None",
    }
}

/// Priority indicator — an SVG icon with a tooltip label.
///
/// Default size is 14px (w-3.5 h-3.5). Pass `size` for a different pixel size.
///
/// # Usage
/// ```ignore
/// <PriorityIndicator priority=2/>
/// <PriorityIndicator priority=1 size=16/>
/// ```
#[component]
pub fn PriorityIndicator(
    priority: i32,
    /// Icon size in pixels. Defaults to 14.
    #[prop(default = 14)]
    size: u32,
) -> impl IntoView {
    let label = priority_label(priority);
    let size_str = size.to_string();

    view! {
        <span
            class="inline-flex items-center justify-center shrink-0"
            title=label
            aria-label=label
            role="img"
        >
            {match priority {
                1 => view_urgent(&size_str).into_any(),
                2 => view_bars(&size_str, 3).into_any(),
                3 => view_bars(&size_str, 2).into_any(),
                4 => view_bars(&size_str, 1).into_any(),
                _ => view_none(&size_str).into_any(),
            }}
        </span>
    }
}

/// Urgent: red rounded square with white exclamation mark.
fn view_urgent(size: &str) -> impl IntoView {
    view! {
        <svg
            width=size.to_string()
            height=size.to_string()
            viewBox="0 0 14 14"
            fill="none"
            xmlns="http://www.w3.org/2000/svg"
        >
            // Red rounded-square background
            <rect x="0" y="0" width="14" height="14" rx="2" fill="#DC2626"/>
            // Exclamation stem (white, thin vertical rect)
            <rect x="6" y="3" width="2" height="5.5" rx="0.5" fill="white"/>
            // Exclamation dot (white, small rect)
            <rect x="6" y="9.75" width="2" height="2" rx="0.5" fill="white"/>
        </svg>
    }
}

/// 3 ascending bars icon. `filled_count` determines how many bars (left to right)
/// are fully opaque vs dimmed to 0.25 opacity.
///
/// Bar layout in a 14x14 viewBox:
/// - Bar 1 (shortest): x=1, width=3, height=5, bottom-aligned
/// - Bar 2 (medium):   x=5.5, width=3, height=9, bottom-aligned
/// - Bar 3 (tallest):  x=10, width=3, height=13, bottom-aligned
fn view_bars(size: &str, filled_count: u8) -> impl IntoView {
    let bar1_opacity = if filled_count >= 1 { "1" } else { "0.25" };
    let bar2_opacity = if filled_count >= 2 { "1" } else { "0.25" };
    let bar3_opacity = if filled_count >= 3 { "1" } else { "0.25" };

    view! {
        <svg
            width=size.to_string()
            height=size.to_string()
            viewBox="0 0 14 14"
            fill="none"
            xmlns="http://www.w3.org/2000/svg"
        >
            // Bar 1 (shortest, left)
            <rect x="1" y="9" width="3" height="5" rx="0.5" fill="currentColor" opacity=bar1_opacity/>
            // Bar 2 (medium, center)
            <rect x="5.5" y="5" width="3" height="9" rx="0.5" fill="currentColor" opacity=bar2_opacity/>
            // Bar 3 (tallest, right)
            <rect x="10" y="1" width="3" height="13" rx="0.5" fill="currentColor" opacity=bar3_opacity/>
        </svg>
    }
}

/// None: horizontal dash line centered in the viewBox.
fn view_none(size: &str) -> impl IntoView {
    view! {
        <svg
            width=size.to_string()
            height=size.to_string()
            viewBox="0 0 14 14"
            fill="none"
            xmlns="http://www.w3.org/2000/svg"
        >
            // Centered horizontal dash
            <rect x="2" y="6" width="10" height="2" rx="0.5" fill="currentColor" opacity="0.4"/>
        </svg>
    }
}
