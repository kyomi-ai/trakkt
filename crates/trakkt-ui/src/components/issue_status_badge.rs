// SPDX-License-Identifier: AGPL-3.0-or-later

//! Issue status badge — SVG circle-variant icon + label for issue status.
//!
//! Different from `StatusBadge` which is a generic semantic pill (success/error/warning).
//! This component renders an issue-specific status indicator with an SVG icon
//! and text label, matching DESIGN.md § "Status Icons (circle variants)".
//!
//! Shape rule: Status icons are ROUND (circles). Priority icons are SQUARE.
//!
//! Status icon colors per DESIGN.md:
//! - Backlog:     text-muted-foreground (dashed circle)
//! - Unstarted:   text-muted-foreground (empty circle)
//! - Started:     text-primary / teal   (half-filled circle)
//! - Completed:   text-primary / teal   (filled circle + checkmark)
//! - Cancelled:   text-muted-foreground (circle + X)

use leptos::prelude::*;

/// Issue status variants matching `StatusCategory` values.
#[derive(Clone, Copy, PartialEq)]
pub enum IssueStatusVariant {
    Backlog,
    Unstarted,
    Started,
    Completed,
    Cancelled,
}

impl IssueStatusVariant {
    /// Parse a status category string into a variant.
    pub fn parse(s: &str) -> Self {
        match s {
            "unstarted" => Self::Unstarted,
            "started" => Self::Started,
            "completed" => Self::Completed,
            "cancelled" => Self::Cancelled,
            _ => Self::Backlog,
        }
    }

    /// Tailwind text-color class for the SVG icon (uses `currentColor`).
    pub fn icon_color_class(self) -> &'static str {
        match self {
            Self::Backlog => "text-muted-foreground",
            // DESIGN.md: --text-secondary (#6B6660). text-muted-foreground
            // maps to the same value today; update if tokens diverge.
            Self::Unstarted => "text-muted-foreground",
            Self::Started => "text-primary",
            Self::Completed => "text-primary",
            Self::Cancelled => "text-muted-foreground",
        }
    }

    /// Human-readable label text.
    fn label(self) -> &'static str {
        match self {
            Self::Backlog => "Backlog",
            Self::Unstarted => "Unstarted",
            Self::Started => "Started",
            Self::Completed => "Completed",
            Self::Cancelled => "Cancelled",
        }
    }
}

/// Render the SVG icon for a status variant at the given pixel size.
fn view_status_icon(variant: IssueStatusVariant, size: String) -> impl IntoView {
    match variant {
        IssueStatusVariant::Backlog => view_backlog(size).into_any(),
        IssueStatusVariant::Unstarted => view_unstarted(size).into_any(),
        IssueStatusVariant::Started => view_started(size).into_any(),
        IssueStatusVariant::Completed => view_completed(size).into_any(),
        IssueStatusVariant::Cancelled => view_cancelled(size).into_any(),
    }
}

/// Backlog: dashed circle.
fn view_backlog(size: String) -> impl IntoView {
    view! {
        <svg
            width=size.clone()
            height=size
            viewBox="0 0 16 16"
            fill="none"
            xmlns="http://www.w3.org/2000/svg"
        >
            <circle cx="8" cy="8" r="6" stroke="currentColor" stroke-width="1.5" fill="none" stroke-dasharray="2.5 2.5"/>
        </svg>
    }
}

/// Unstarted: empty circle (stroke only).
fn view_unstarted(size: String) -> impl IntoView {
    view! {
        <svg
            width=size.clone()
            height=size
            viewBox="0 0 16 16"
            fill="none"
            xmlns="http://www.w3.org/2000/svg"
        >
            <circle cx="8" cy="8" r="6" stroke="currentColor" stroke-width="1.5" fill="none"/>
        </svg>
    }
}

/// Started: half-filled circle (right half filled).
fn view_started(size: String) -> impl IntoView {
    view! {
        <svg
            width=size.clone()
            height=size
            viewBox="0 0 16 16"
            fill="none"
            xmlns="http://www.w3.org/2000/svg"
        >
            <circle cx="8" cy="8" r="6" stroke="currentColor" stroke-width="1.5" fill="none"/>
            <path d="M8 2a6 6 0 0 1 0 12" fill="currentColor"/>
        </svg>
    }
}

/// Completed: filled circle with white checkmark.
fn view_completed(size: String) -> impl IntoView {
    view! {
        <svg
            width=size.clone()
            height=size
            viewBox="0 0 16 16"
            fill="none"
            xmlns="http://www.w3.org/2000/svg"
        >
            <circle cx="8" cy="8" r="6" fill="currentColor"/>
            <path d="M5.5 8l2 2 3-3.5" stroke="white" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round" fill="none"/>
        </svg>
    }
}

/// Cancelled: circle with X.
fn view_cancelled(size: String) -> impl IntoView {
    view! {
        <svg
            width=size.clone()
            height=size
            viewBox="0 0 16 16"
            fill="none"
            xmlns="http://www.w3.org/2000/svg"
        >
            <circle cx="8" cy="8" r="6" stroke="currentColor" stroke-width="1.5" fill="none"/>
            <path d="M6 6l4 4M10 6l-4 4" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" fill="none"/>
        </svg>
    }
}

/// Issue status badge with an SVG circle-variant icon.
///
/// When `show_label` is true, includes a text label next to the icon (for detail pages).
/// When false (default), renders only the icon (for issue list rows per DESIGN.md).
#[component]
pub fn IssueStatusBadge(
    status: IssueStatusVariant,
    #[prop(default = false)] show_label: bool,
    /// Icon size in pixels. Defaults to 14.
    #[prop(default = 14)]
    size: u32,
) -> impl IntoView {
    let label_text = status.label();
    let color_class = status.icon_color_class();

    let wrapper_class = if show_label {
        format!("inline-flex items-center gap-1.5 text-sm text-muted-foreground")
    } else {
        String::new()
    };

    view! {
        <span
            class=wrapper_class
            title=if show_label { None } else { Some(label_text) }
            role=if show_label { None } else { Some("img") }
            aria-label=if show_label { None } else { Some(format!("Status: {label_text}")) }
        >
            <span class=format!("inline-flex items-center justify-center shrink-0 {color_class}")>
                {view_status_icon(status, size.to_string())}
            </span>
            {if show_label { Some(label_text) } else { None }}
        </span>
    }
}
