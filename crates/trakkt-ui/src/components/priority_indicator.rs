// SPDX-License-Identifier: AGPL-3.0-or-later

//! Priority indicator — small colored square for issue priority level.
//!
//! Matches DESIGN.md "Priority Colors" table:
//! - 1 (Urgent): #DC2626 (error red)
//! - 2 (High):   #EA580C (orange-600)
//! - 3 (Medium): #CA8A04 (warning yellow)
//! - 4 (Low):    #6B6660 (text-secondary)
//! - 0 (None):   #9C9790 (text-muted)
//!
//! DESIGN.md Issue Row Pattern: "Priority: small colored square".

use leptos::prelude::*;

/// Maps a numeric priority to its color class and label.
fn priority_meta(priority: i32) -> (&'static str, &'static str) {
    match priority {
        1 => ("bg-[#DC2626]", "Urgent"),
        2 => ("bg-[#EA580C]", "High"),
        3 => ("bg-[#CA8A04]", "Medium"),
        4 => ("bg-[#6B6660]", "Low"),
        _ => ("bg-[#9C9790]", "None"),
    }
}

/// Priority indicator — a small colored square with a tooltip label.
///
/// # Usage
/// ```ignore
/// <PriorityIndicator priority=2/>
/// ```
#[component]
pub fn PriorityIndicator(priority: i32) -> impl IntoView {
    let (color, label) = priority_meta(priority);
    view! {
        <span class="inline-flex items-center shrink-0" title=label>
            <span class={format!("w-2.5 h-2.5 rounded-[2px] {color}")}></span>
        </span>
    }
}
