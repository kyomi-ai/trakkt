// SPDX-License-Identifier: AGPL-3.0-or-later

//! Team icon component — renders a team's icon based on its icon_type,
//! icon_name, and icon_color fields.
//!
//! Three states:
//! - **Preset icon**: Rounded square with `icon_color` background and a
//!   white Phosphor icon inside.
//! - **Custom icon**: `<img>` tag pointing to `/api/v1/teams/{id}/icon`. That
//!   endpoint requires an authenticated caller in the team's workspace; the tag
//!   is same-origin, so the browser attaches the `access_token` cookie itself.
//! - **Fallback**: Rounded square with neutral gray background and the
//!   first letter of the team name in white.

use leptos::prelude::*;
use phosphor_leptos::Icon;
use trakkt_types::models::Team;

/// Map an icon name string to the corresponding Phosphor icon constant.
///
/// Returns `None` for unrecognised names — the caller falls back to the
/// initial-letter rendering.
pub fn get_icon(name: &str) -> Option<phosphor_leptos::IconData> {
    match name {
        // General
        "lightning" => Some(phosphor_leptos::LIGHTNING),
        "rocket" => Some(phosphor_leptos::ROCKET),
        "target" => Some(phosphor_leptos::TARGET),
        "flag" => Some(phosphor_leptos::FLAG),
        "shield" => Some(phosphor_leptos::SHIELD),
        "star" => Some(phosphor_leptos::STAR),
        "heart" => Some(phosphor_leptos::HEART),
        "diamond" => Some(phosphor_leptos::DIAMOND),
        "cube" => Some(phosphor_leptos::CUBE),
        "globe" => Some(phosphor_leptos::GLOBE),
        // Engineering
        "code" => Some(phosphor_leptos::CODE),
        "terminal" => Some(phosphor_leptos::TERMINAL),
        "gear" => Some(phosphor_leptos::GEAR),
        "wrench" => Some(phosphor_leptos::WRENCH),
        "cpu" => Some(phosphor_leptos::CPU),
        "database" => Some(phosphor_leptos::DATABASE),
        "git-branch" => Some(phosphor_leptos::GIT_BRANCH),
        "bug" => Some(phosphor_leptos::BUG),
        // Product
        "layout" => Some(phosphor_leptos::LAYOUT),
        "palette" => Some(phosphor_leptos::PALETTE),
        "megaphone" => Some(phosphor_leptos::MEGAPHONE),
        "chart-line" => Some(phosphor_leptos::CHART_LINE),
        "users" => Some(phosphor_leptos::USERS),
        "lightbulb" => Some(phosphor_leptos::LIGHTBULB),
        "compass" => Some(phosphor_leptos::COMPASS),
        // Communication
        "chat" => Some(phosphor_leptos::CHAT),
        "envelope" => Some(phosphor_leptos::ENVELOPE),
        "bell" => Some(phosphor_leptos::BELL),
        "broadcast" => Some(phosphor_leptos::BROADCAST),
        _ => None,
    }
}

/// All available preset icon names, grouped by category.
///
/// Used by `TeamIconPicker` to render the selection grid.
pub const ICON_CATEGORIES: &[(&str, &[&str])] = &[
    (
        "General",
        &[
            "lightning", "rocket", "target", "flag", "shield", "star",
            "heart", "diamond", "cube", "globe",
        ],
    ),
    (
        "Engineering",
        &[
            "code", "terminal", "gear", "wrench", "cpu", "database",
            "git-branch", "bug",
        ],
    ),
    (
        "Product",
        &[
            "layout", "palette", "megaphone", "chart-line", "users",
            "lightbulb", "compass",
        ],
    ),
    (
        "Communication",
        &["chat", "envelope", "bell", "broadcast"],
    ),
];

/// All preset colour choices for team icons.
///
/// Two rows of 8 — used by `TeamIconPicker` to render the colour palette.
pub const ICON_COLORS: &[&str] = &[
    // Row 1
    "#EF4444", "#F97316", "#F59E0B", "#EAB308",
    "#84CC16", "#22C55E", "#10B981", "#14B8A6",
    // Row 2
    "#06B6D4", "#3B82F6", "#6366F1", "#8B5CF6",
    "#A855F7", "#EC4899", "#6B7280", "#1E293B",
];

/// Default colour when an icon is selected but no colour has been chosen.
pub const DEFAULT_ICON_COLOR: &str = "#3B82F6";

/// Default icon name when a colour is selected but no icon has been chosen.
pub const DEFAULT_ICON_NAME: &str = "rocket";

/// Reusable team icon display.
///
/// Renders the appropriate visual based on the team's icon configuration:
/// preset Phosphor icon, custom uploaded image, or initial-letter fallback.
///
/// # Usage
/// ```ignore
/// <TeamIcon team=team.clone() size="24px"/>
/// ```
#[component]
pub fn TeamIcon(
    /// The team whose icon to render.
    team: Team,
    /// CSS size for width and height. Default: `"24px"`.
    #[prop(default = "24px")]
    size: &'static str,
) -> impl IntoView {
    let icon_type = team.icon_type.clone();
    let icon_name = team.icon_name.clone();
    let icon_color = team.icon_color.clone();
    let team_name = team.name.clone();
    let team_id = team.team_id.clone();

    // Compute the inner icon size — roughly 58% of the container for good
    // visual balance inside the rounded square.
    let inner_size = compute_inner_size(size);

    match icon_type.as_deref() {
        Some("preset") => {
            let bg = icon_color.unwrap_or_else(|| DEFAULT_ICON_COLOR.to_string());
            let icon_data = icon_name
                .as_deref()
                .and_then(get_icon);

            match icon_data {
                Some(data) => {
                    view! {
                        <span
                            class="inline-flex items-center justify-center rounded-md shrink-0"
                            style=format!(
                                "width: {size}; height: {size}; background-color: {bg};"
                            )
                        >
                            <Icon icon=data size=inner_size.clone() color="white"/>
                        </span>
                    }.into_any()
                }
                // icon_name didn't resolve — fall back to initial letter
                None => render_fallback(&team_name, size).into_any(),
            }
        }
        Some("custom") => {
            let src = format!("/api/v1/teams/{team_id}/icon");
            view! {
                <img
                    src=src
                    alt=format!("{team_name} icon")
                    class="rounded-md shrink-0 object-cover"
                    style=format!("width: {size}; height: {size};")
                />
            }.into_any()
        }
        _ => render_fallback(&team_name, size).into_any(),
    }
}

/// Render the initial-letter fallback: neutral gray rounded square with
/// the first character of the team name in white.
fn render_fallback(name: &str, size: &str) -> impl IntoView {
    let initial = name
        .chars()
        .next()
        .unwrap_or('?')
        .to_uppercase()
        .to_string();

    // Font size ~ 45% of container size for single-letter balance.
    let font_size = compute_font_size(size);

    view! {
        <span
            class="inline-flex items-center justify-center rounded-md shrink-0 bg-muted text-muted-foreground font-semibold"
            style=format!(
                "width: {size}; height: {size}; font-size: {font_size};"
            )
        >
            {initial}
        </span>
    }
}

/// Parse a pixel size string (e.g. `"24px"`) and return ~58% for the
/// inner icon. Returns the original string as-is if parsing fails.
fn compute_inner_size(size: &str) -> String {
    parse_px(size)
        .map(|px| format!("{}px", (px as f64 * 0.58).round() as u32))
        .unwrap_or_else(|| size.to_string())
}

/// Parse a pixel size string and return ~45% for the fallback font size.
fn compute_font_size(size: &str) -> String {
    parse_px(size)
        .map(|px| format!("{}px", (px as f64 * 0.45).round() as u32))
        .unwrap_or_else(|| "11px".to_string())
}

/// Extract the numeric part from a `"NNpx"` string.
fn parse_px(size: &str) -> Option<u32> {
    size.strip_suffix("px")
        .and_then(|s| s.parse::<u32>().ok())
}
