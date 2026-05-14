// SPDX-License-Identifier: AGPL-3.0-or-later

//! Team icon picker — lets users choose a preset icon + colour for a team.
//!
//! Layout (Linear-inspired):
//! 1. Large preview of the current team icon (48px)
//! 2. Colour palette — 2 rows of 8 swatches
//! 3. Icon grid — grouped by category with small labels
//! 4. "Remove icon" clear button at the bottom

use leptos::prelude::*;
use phosphor_leptos::Icon;
use trakkt_types::models::Team;

use crate::components::button::{Button, ButtonSize, ButtonVariant};
use crate::components::team_icon::{
    get_icon, TeamIcon, DEFAULT_ICON_COLOR, DEFAULT_ICON_NAME, ICON_CATEGORIES, ICON_COLORS,
};

/// Icon picker for choosing a team's preset icon and colour.
///
/// Fires `on_change` with `(icon_type, icon_name, icon_color)` whenever
/// the user clicks a colour swatch, an icon, or "Remove icon". The caller
/// is responsible for persisting the change via `update_team_icon` /
/// `clear_team_icon`.
#[component]
pub fn TeamIconPicker(
    /// The team being edited. Used to show the current icon state.
    team: Team,
    /// Callback fired on every selection change.
    /// Arguments: `(Option<icon_type>, Option<icon_name>, Option<icon_color>)`.
    on_change: Callback<(Option<String>, Option<String>, Option<String>)>,
) -> impl IntoView {
    // Local signals for optimistic preview. Seeded from the team's current
    // icon state so the preview reflects changes immediately.
    let initial_name = team.icon_name.clone();
    let initial_color = team.icon_color.clone();
    let has_preset = team.icon_type.as_deref() == Some("preset");

    let (selected_name, set_selected_name) = signal(
        if has_preset { initial_name } else { None },
    );
    let (selected_color, set_selected_color) = signal(
        if has_preset { initial_color } else { None },
    );

    // Build a derived Team for the preview that reflects local edits.
    let team_for_preview = team.clone();
    let preview_team = Memo::new(move |_| {
        let mut t = team_for_preview.clone();
        let name = selected_name.get();
        let color = selected_color.get();
        if name.is_some() || color.is_some() {
            t.icon_type = Some("preset".to_string());
            t.icon_name = Some(
                name.unwrap_or_else(|| DEFAULT_ICON_NAME.to_string()),
            );
            t.icon_color = Some(
                color.unwrap_or_else(|| DEFAULT_ICON_COLOR.to_string()),
            );
        } else {
            t.icon_type = None;
            t.icon_name = None;
            t.icon_color = None;
        }
        t
    });

    // ── Handlers ──────────────────────────────────────────────────────────

    let on_color_click = move |color: &'static str| {
        let new_color = Some(color.to_string());
        // Default to "rocket" if no icon chosen yet.
        let name = selected_name.get_untracked()
            .or_else(|| Some(DEFAULT_ICON_NAME.to_string()));
        set_selected_color.set(new_color.clone());
        set_selected_name.set(name.clone());
        on_change.run((
            Some("preset".to_string()),
            name,
            new_color,
        ));
    };

    let on_icon_click = move |name: &'static str| {
        let new_name = Some(name.to_string());
        // Default to blue if no colour chosen yet.
        let color = selected_color.get_untracked()
            .or_else(|| Some(DEFAULT_ICON_COLOR.to_string()));
        set_selected_name.set(new_name.clone());
        set_selected_color.set(color.clone());
        on_change.run((
            Some("preset".to_string()),
            new_name,
            color,
        ));
    };

    let on_clear = move |_| {
        set_selected_name.set(None);
        set_selected_color.set(None);
        on_change.run((None, None, None));
    };

    // ── Render ─────────────────────────────────────────────────────────────

    view! {
        <div class="flex flex-col gap-4">
            // Preview
            <div class="flex items-center justify-center py-2">
                {move || {
                    let t = preview_team.get();
                    view! { <TeamIcon team=t size="48px"/> }
                }}
            </div>

            // Colour palette
            <div class="flex flex-col gap-1.5">
                <span class="text-xs text-muted-foreground font-medium">"Color"</span>
                <div class="grid grid-cols-8 gap-1.5">
                    {ICON_COLORS.iter().map(|&color| {
                        let is_selected = {
                            let color = color.to_string();
                            Signal::derive(move || {
                                selected_color.get().as_deref() == Some(color.as_str())
                            })
                        };
                        view! {
                            <button
                                type="button"
                                class=move || {
                                    let base = "w-7 h-7 rounded-md transition-all duration-200 focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring";
                                    if is_selected.get() {
                                        format!("{base} ring-2 ring-foreground ring-offset-2 ring-offset-background")
                                    } else {
                                        format!("{base} hover:scale-110")
                                    }
                                }
                                style=format!("background-color: {color};")
                                title=color
                                on:click=move |_| on_color_click(color)
                            />
                        }
                    }).collect_view()}
                </div>
            </div>

            // Icon grid
            <div class="flex flex-col gap-2">
                <span class="text-xs text-muted-foreground font-medium">"Icon"</span>
                {ICON_CATEGORIES.iter().map(|(label, icons)| {
                    view! {
                        <div class="flex flex-col gap-1">
                            <span class="text-[10px] text-muted-foreground uppercase tracking-wider">
                                {*label}
                            </span>
                            <div class="flex flex-wrap gap-1">
                                {icons.iter().map(|&name| {
                                    let icon_data = get_icon(name);
                                    let is_selected = {
                                        let name = name.to_string();
                                        Signal::derive(move || {
                                            selected_name.get().as_deref() == Some(name.as_str())
                                        })
                                    };
                                    view! {
                                        <button
                                            type="button"
                                            class=move || {
                                                let base = "w-8 h-8 rounded-md flex items-center justify-center transition-colors duration-200 focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring";
                                                if is_selected.get() {
                                                    format!("{base} bg-accent text-accent-foreground")
                                                } else {
                                                    format!("{base} text-muted-foreground hover:bg-muted hover:text-foreground")
                                                }
                                            }
                                            title=name
                                            on:click=move |_| on_icon_click(name)
                                        >
                                            {icon_data.map(|data| view! {
                                                <Icon icon=data size="18px"/>
                                            })}
                                        </button>
                                    }
                                }).collect_view()}
                            </div>
                        </div>
                    }
                }).collect_view()}
            </div>

            // Clear button
            <div class="border-t border-border pt-2">
                <Button
                    variant=ButtonVariant::GhostMuted
                    size=ButtonSize::Sm
                    on:click=on_clear
                    class="w-full justify-center"
                >
                    "Remove icon"
                </Button>
            </div>
        </div>
    }
}
