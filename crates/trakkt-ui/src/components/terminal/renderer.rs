// SPDX-License-Identifier: AGPL-3.0-or-later

//! DOM-based terminal renderer — converts the [`Grid`] cell buffer into a tree
//! of styled `<div>` rows and `<span>` runs that Leptos can diff efficiently.
//!
//! The renderer is intentionally *display-only*: keyboard / mouse input is
//! handled in a separate `input` module.

use leptos::prelude::*;

use super::{CellAttrs, Color, Grid, StyledSpan};

// ---------------------------------------------------------------------------
// ANSI 256-color palette → CSS
// ---------------------------------------------------------------------------

/// The 16 standard ANSI colors (indices 0–15) as hex strings.
const STANDARD_COLORS: [&str; 16] = [
    "#000000", // 0  black
    "#cd3131", // 1  red
    "#0dbc79", // 2  green
    "#e5e510", // 3  yellow
    "#2472c8", // 4  blue
    "#bc3fbc", // 5  magenta
    "#11a8cd", // 6  cyan
    "#e5e5e5", // 7  white
    "#666666", // 8  bright black
    "#f14c4c", // 9  bright red
    "#23d18b", // 10 bright green
    "#f5f543", // 11 bright yellow
    "#3b8eea", // 12 bright blue
    "#d670d6", // 13 bright magenta
    "#29b8db", // 14 bright cyan
    "#ffffff", // 15 bright white
];

/// The six intensity values used by the 6×6×6 colour cube (indices 16–231).
const CUBE_VALUES: [u8; 6] = [0, 95, 135, 175, 215, 255];

/// Convert a [`Color`] to a CSS colour string.
///
/// Returns `None` for [`Color::Default`] — callers should fall through to the
/// terminal's default foreground / background via CSS `inherit`.
fn color_to_css(color: &Color) -> Option<String> {
    match *color {
        Color::Default => None,
        Color::Indexed(i) => {
            if (i as usize) < 16 {
                Some(STANDARD_COLORS[i as usize].to_string())
            } else if i <= 231 {
                // 6×6×6 colour cube: index = 16 + 36*r + 6*g + b
                let idx = (i - 16) as usize;
                let r = CUBE_VALUES[idx / 36];
                let g = CUBE_VALUES[(idx / 6) % 6];
                let b = CUBE_VALUES[idx % 6];
                Some(format!("rgb({r},{g},{b})"))
            } else {
                // Grayscale ramp: indices 232–255 → gray = 8 + 10*(i-232)
                let gray = 8 + 10 * (i as u16 - 232);
                Some(format!("rgb({gray},{gray},{gray})"))
            }
        }
        Color::Rgb(r, g, b) => Some(format!("rgb({r},{g},{b})")),
    }
}

// ---------------------------------------------------------------------------
// Span → inline style builder
// ---------------------------------------------------------------------------

/// Build an inline `style` attribute string for a single [`StyledSpan`].
///
/// The default terminal foreground (#d4d4d4) and background (#1e1e1e) are
/// applied at the viewport level; per-span styles only emit properties that
/// deviate from those defaults.
fn span_style(span: &StyledSpan) -> String {
    let mut parts: Vec<String> = Vec::new();

    let is_inverse = span.attrs.contains(CellAttrs::INVERSE);

    // Resolve effective fg / bg (inverse swaps them).
    let (eff_fg, eff_bg) = if is_inverse {
        (&span.bg, &span.fg)
    } else {
        (&span.fg, &span.bg)
    };

    if let Some(css) = color_to_css(eff_fg) {
        parts.push(format!("color:{css}"));
    }
    if let Some(css) = color_to_css(eff_bg) {
        parts.push(format!("background-color:{css}"));
    }

    // When inverse is active and one side is Default, we need to explicitly
    // set the swapped default so the user actually sees the inversion.
    if is_inverse {
        if matches!(span.bg, Color::Default) {
            // effective fg is Default bg → dark background colour
            parts.push("color:#1e1e1e".to_string());
        }
        if matches!(span.fg, Color::Default) {
            // effective bg is Default fg → light text colour
            parts.push("background-color:#d4d4d4".to_string());
        }
    }

    if span.attrs.contains(CellAttrs::BOLD) {
        parts.push("font-weight:bold".to_string());
    }
    if span.attrs.contains(CellAttrs::DIM) {
        parts.push("opacity:0.5".to_string());
    }
    if span.attrs.contains(CellAttrs::ITALIC) {
        parts.push("font-style:italic".to_string());
    }

    // text-decoration can combine underline and line-through.
    let mut decorations = Vec::new();
    if span.attrs.contains(CellAttrs::UNDERLINE) {
        decorations.push("underline");
    }
    if span.attrs.contains(CellAttrs::STRIKETHROUGH) {
        decorations.push("line-through");
    }
    if !decorations.is_empty() {
        parts.push(format!("text-decoration:{}", decorations.join(" ")));
    }

    parts.join(";")
}

// ---------------------------------------------------------------------------
// Row rendering helper
// ---------------------------------------------------------------------------

/// Render a single grid row as a `<div>` containing styled `<span>` elements.
fn render_row(spans: Vec<StyledSpan>) -> impl IntoView {
    let children = spans
        .into_iter()
        .map(|span| {
            let style = span_style(&span);
            let text = span.text;
            if style.is_empty() {
                view! { <span>{text}</span> }.into_any()
            } else {
                view! { <span style=style>{text}</span> }.into_any()
            }
        })
        .collect_view();

    view! {
        <div>{children}</div>
    }
}

// ---------------------------------------------------------------------------
// TerminalRenderer component
// ---------------------------------------------------------------------------

/// CSS for the blinking cursor animation, injected once via a `<style>` tag.
const CURSOR_KEYFRAMES: &str = "\
@keyframes terminal-cursor-blink {\
  0%, 100% { opacity: 1; }\
  50% { opacity: 0; }\
}";

/// DOM-based terminal renderer.
///
/// Reads the [`Grid`] signal each frame and produces a `<div>` tree of styled
/// rows.  A blinking block cursor is overlaid at the current cursor position
/// when `modes.cursor_visible` is true.
///
/// **This component is display-only** — keyboard and mouse input are handled
/// separately in `input.rs`.
#[component]
pub fn TerminalRenderer(
    /// The terminal grid state, updated by the VTE handler.
    grid: RwSignal<Grid>,
) -> impl IntoView {
    // -----------------------------------------------------------------------
    // Render the grid rows reactively.
    //
    // For the initial implementation we re-render all rows whenever the grid
    // signal changes.  A future optimisation pass can use the dirty-row set
    // and per-row memo signals to skip unchanged rows.
    // -----------------------------------------------------------------------

    let rows_view = move || {
        grid.with(|g| {
            (0..g.rows)
                .map(|row_idx| {
                    let spans = g.row_to_styled_spans(row_idx);
                    render_row(spans)
                })
                .collect_view()
        })
    };

    // -----------------------------------------------------------------------
    // Cursor overlay — a positioned block that blinks via CSS animation.
    // We use `ch` / `em` units so the overlay tracks the monospace grid
    // without needing JS measurement.
    // -----------------------------------------------------------------------

    let cursor_view = move || {
        grid.with(|g| {
            if !g.modes.cursor_visible {
                return None;
            }

            let col = g.cursor.col.min(g.cols.saturating_sub(1));
            let row = g.cursor.row.min(g.rows.saturating_sub(1));

            // Position: each character is 1ch wide; each row is 1.2em tall
            // (matching line-height).
            let left = format!("{}ch", col);
            let top = format!("calc({} * 1.2em)", row);

            Some(view! {
                <div
                    style=format!(
                        "position:absolute;\
                         left:{left};\
                         top:{top};\
                         width:1ch;\
                         height:1.2em;\
                         background-color:#d4d4d4;\
                         animation:terminal-cursor-blink 1s step-end infinite;\
                         pointer-events:none;"
                    )
                />
            })
        })
    };

    // -----------------------------------------------------------------------
    // Viewport container
    // -----------------------------------------------------------------------

    view! {
        <style>{CURSOR_KEYFRAMES}</style>
        <div
            tabindex="0"
            style="\
                font-family: 'JetBrains Mono', 'Cascadia Code', 'Fira Code', 'Menlo', monospace;\
                background-color: #1e1e1e;\
                color: #d4d4d4;\
                line-height: 1.2;\
                white-space: pre;\
                overflow: hidden;\
                outline: none;\
                position: relative;\
                padding: 4px;\
            "
        >
            {rows_view}
            {cursor_view}
        </div>
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn standard_colors_lookup() {
        assert_eq!(color_to_css(&Color::Indexed(0)), Some("#000000".into()));
        assert_eq!(color_to_css(&Color::Indexed(1)), Some("#cd3131".into()));
        assert_eq!(color_to_css(&Color::Indexed(15)), Some("#ffffff".into()));
    }

    #[test]
    fn color_cube_indices() {
        // Index 16 = cube (0,0,0) → rgb(0,0,0)
        assert_eq!(color_to_css(&Color::Indexed(16)), Some("rgb(0,0,0)".into()));
        // Index 21 = cube (0,0,5) → rgb(0,0,255)
        assert_eq!(
            color_to_css(&Color::Indexed(21)),
            Some("rgb(0,0,255)".into())
        );
        // Index 196 = 16 + 36*5 + 6*0 + 0 = 196 → rgb(255,0,0)
        assert_eq!(
            color_to_css(&Color::Indexed(196)),
            Some("rgb(255,0,0)".into())
        );
        // Index 231 = 16 + 36*5 + 6*5 + 5 = 231 → rgb(255,255,255)
        assert_eq!(
            color_to_css(&Color::Indexed(231)),
            Some("rgb(255,255,255)".into())
        );
    }

    #[test]
    fn grayscale_ramp() {
        // Index 232 → gray = 8 + 10*0 = 8
        assert_eq!(
            color_to_css(&Color::Indexed(232)),
            Some("rgb(8,8,8)".into())
        );
        // Index 255 → gray = 8 + 10*23 = 238
        assert_eq!(
            color_to_css(&Color::Indexed(255)),
            Some("rgb(238,238,238)".into())
        );
    }

    #[test]
    fn rgb_color() {
        assert_eq!(
            color_to_css(&Color::Rgb(128, 64, 32)),
            Some("rgb(128,64,32)".into())
        );
    }

    #[test]
    fn default_color_returns_none() {
        assert_eq!(color_to_css(&Color::Default), None);
    }

    #[test]
    fn span_style_default_is_empty() {
        let span = StyledSpan {
            text: "hello".into(),
            fg: Color::Default,
            bg: Color::Default,
            attrs: CellAttrs::default(),
        };
        assert_eq!(span_style(&span), "");
    }

    #[test]
    fn span_style_bold_italic() {
        let mut attrs = CellAttrs::default();
        attrs.insert(CellAttrs::BOLD);
        attrs.insert(CellAttrs::ITALIC);
        let span = StyledSpan {
            text: "x".into(),
            fg: Color::Default,
            bg: Color::Default,
            attrs,
        };
        let style = span_style(&span);
        assert!(style.contains("font-weight:bold"), "missing bold: {style}");
        assert!(
            style.contains("font-style:italic"),
            "missing italic: {style}"
        );
    }

    #[test]
    fn span_style_underline_and_strikethrough() {
        let mut attrs = CellAttrs::default();
        attrs.insert(CellAttrs::UNDERLINE);
        attrs.insert(CellAttrs::STRIKETHROUGH);
        let span = StyledSpan {
            text: "x".into(),
            fg: Color::Default,
            bg: Color::Default,
            attrs,
        };
        let style = span_style(&span);
        assert!(
            style.contains("text-decoration:underline line-through"),
            "missing combined decoration: {style}"
        );
    }

    #[test]
    fn span_style_inverse_swaps_colors() {
        let mut attrs = CellAttrs::default();
        attrs.insert(CellAttrs::INVERSE);
        let span = StyledSpan {
            text: "x".into(),
            fg: Color::Indexed(1),  // red
            bg: Color::Indexed(15), // white
            attrs,
        };
        let style = span_style(&span);
        // Inverse: effective fg = bg (white #ffffff), effective bg = fg (red #cd3131)
        assert!(
            style.contains("color:#ffffff"),
            "inverse fg wrong: {style}"
        );
        assert!(
            style.contains("background-color:#cd3131"),
            "inverse bg wrong: {style}"
        );
    }

    #[test]
    fn span_style_inverse_with_defaults() {
        let mut attrs = CellAttrs::default();
        attrs.insert(CellAttrs::INVERSE);
        let span = StyledSpan {
            text: "x".into(),
            fg: Color::Default,
            bg: Color::Default,
            attrs,
        };
        let style = span_style(&span);
        // Default fg (#d4d4d4) should become bg, default bg (#1e1e1e) should become fg.
        assert!(
            style.contains("color:#1e1e1e"),
            "inverse default fg wrong: {style}"
        );
        assert!(
            style.contains("background-color:#d4d4d4"),
            "inverse default bg wrong: {style}"
        );
    }

    #[test]
    fn span_style_dim() {
        let mut attrs = CellAttrs::default();
        attrs.insert(CellAttrs::DIM);
        let span = StyledSpan {
            text: "x".into(),
            fg: Color::Default,
            bg: Color::Default,
            attrs,
        };
        let style = span_style(&span);
        assert!(style.contains("opacity:0.5"), "missing dim: {style}");
    }

    #[test]
    fn span_style_fg_bg_colors() {
        let span = StyledSpan {
            text: "x".into(),
            fg: Color::Rgb(255, 0, 0),
            bg: Color::Rgb(0, 255, 0),
            attrs: CellAttrs::default(),
        };
        let style = span_style(&span);
        assert!(
            style.contains("color:rgb(255,0,0)"),
            "missing fg: {style}"
        );
        assert!(
            style.contains("background-color:rgb(0,255,0)"),
            "missing bg: {style}"
        );
    }
}
