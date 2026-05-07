// SPDX-License-Identifier: AGPL-3.0-or-later

//! Issue status badge — colored dot + label for issue status.
//!
//! Different from `StatusBadge` which is a generic semantic pill (success/error/warning).
//! This component renders an issue-specific status indicator with a small colored dot
//! and text label, matching the Issue Row Pattern in DESIGN.md.
//!
//! Status colors match DESIGN.md "Status Colors" table:
//! - Backlog:     #9C9790 (text-muted)
//! - Todo:        #2563EB (info blue)
//! - In Progress: #0D9488 (accent teal)
//! - Done:        #15803D (success green)
//! - Cancelled:   #6B6660 (text-secondary)

use leptos::prelude::*;

/// Issue status variants matching the database `status` column values.
#[derive(Clone, Copy, PartialEq)]
pub enum IssueStatusVariant {
    Backlog,
    Todo,
    InProgress,
    Done,
    Cancelled,
}

impl IssueStatusVariant {
    /// Parse a database status string into a variant.
    pub fn parse(s: &str) -> Self {
        match s {
            "todo" => Self::Todo,
            "in_progress" => Self::InProgress,
            "done" => Self::Done,
            "cancelled" => Self::Cancelled,
            _ => Self::Backlog,
        }
    }

    /// Tailwind background class for the colored dot.
    pub fn dot_color(self) -> &'static str {
        match self {
            Self::Backlog => "bg-[#9C9790]",
            Self::Todo => "bg-[#2563EB]",
            Self::InProgress => "bg-[#0D9488]",
            Self::Done => "bg-[#15803D]",
            Self::Cancelled => "bg-[#6B6660]",
        }
    }

    /// Human-readable label text.
    fn label(self) -> &'static str {
        match self {
            Self::Backlog => "Backlog",
            Self::Todo => "Todo",
            Self::InProgress => "In Progress",
            Self::Done => "Done",
            Self::Cancelled => "Cancelled",
        }
    }
}

/// Issue status badge with a colored dot.
///
/// When `show_label` is true, includes a text label next to the dot (for detail pages).
/// When false (default), renders only the dot (for issue list rows per DESIGN.md).
#[component]
pub fn IssueStatusBadge(
    status: IssueStatusVariant,
    #[prop(default = false)] show_label: bool,
) -> impl IntoView {
    let dot_class = format!("w-2 h-2 rounded-full shrink-0 {}", status.dot_color());
    let label_text = status.label();
    view! {
        <span
            class=if show_label { "inline-flex items-center gap-1.5 text-sm text-muted-foreground" } else { "" }
            title=if show_label { "" } else { label_text }
            role=if show_label { "" } else { "img" }
            aria-label=if show_label { String::new() } else { format!("Status: {label_text}") }
        >
            <span class=dot_class></span>
            {if show_label { Some(label_text) } else { None }}
        </span>
    }
}
