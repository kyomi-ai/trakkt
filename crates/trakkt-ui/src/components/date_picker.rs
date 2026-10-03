// SPDX-License-Identifier: AGPL-3.0-or-later

//! DatePicker component — calendar popover for selecting dates.
//!
//! Provides a trigger button (matching [`DropdownTrigger`] styling) with a
//! calendar icon, and a popover with quick shortcuts ("Today", "Tomorrow",
//! "Next week"), a month-view calendar grid, and an optional "Clear" action.
//!
//! Date values are ISO 8601 strings (`"2026-05-13"`). Calendar math is pure
//! Rust via `chrono`.

use chrono::{Datelike, Duration, NaiveDate, Weekday};
use leptos::prelude::*;
use phosphor_leptos::Icon;

use crate::components::button::{Button, ButtonSize, ButtonVariant, ToggleButton};
use crate::components::popover::{Placement, Popover};

// ─────────────────────────────────────────────────────────────────────────────
// CSS class constants — derived from DESIGN.md § "Dropdowns" trigger spec
// ─────────────────────────────────────────────────────────────────────────────

/// Trigger classes — matches `DropdownTrigger` TRIGGER_BASE from dropdown.rs.
const TRIGGER_BASE: &str = "inline-flex items-center gap-1.5 whitespace-nowrap \
    border border-border rounded-[4px] px-2 py-1 \
    text-xs font-normal cursor-pointer \
    transition-colors duration-200 \
    hover:border-[var(--color-border-strong)] hover:text-foreground \
    focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring";

// ─────────────────────────────────────────────────────────────────────────────
// Calendar grid math — pure Rust, no DOM dependency
// ─────────────────────────────────────────────────────────────────────────────

/// Generate a 6x7 (42-day) calendar grid starting from Monday of the week
/// containing the first day of the given month.
fn calendar_grid(year: i32, month: u32) -> Vec<NaiveDate> {
    let first_of_month = NaiveDate::from_ymd_opt(year, month, 1)
        .expect("calendar_grid called with invalid year/month");
    // Monday = 0, Sunday = 6
    let start_offset = first_of_month.weekday().num_days_from_monday();
    let grid_start = first_of_month - Duration::days(start_offset as i64);
    (0..42).map(|i| grid_start + Duration::days(i)).collect()
}

/// Format a `NaiveDate` as "May 13, 2026".
fn format_display_date(date: &NaiveDate) -> String {
    format!("{}", date.format("%b %-d, %Y"))
}

/// Month name for calendar header.
fn month_name(month: u32) -> &'static str {
    match month {
        1 => "January",
        2 => "February",
        3 => "March",
        4 => "April",
        5 => "May",
        6 => "June",
        7 => "July",
        8 => "August",
        9 => "September",
        10 => "October",
        11 => "November",
        12 => "December",
        _ => "Unknown",
    }
}

/// 2-letter day-of-week headers (Monday start).
const DAY_HEADERS: [&str; 7] = ["Mo", "Tu", "We", "Th", "Fr", "Sa", "Su"];

/// Return today's date on WASM or a safe fallback on the server.
fn today_or_fallback() -> NaiveDate {
    #[cfg(target_arch = "wasm32")]
    {
        chrono::Local::now().date_naive()
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        NaiveDate::from_ymd_opt(2000, 1, 1).expect("constant date")
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// DatePicker component
// ─────────────────────────────────────────────────────────────────────────────

/// Date picker with calendar popover.
///
/// Shows a compact trigger button with a calendar icon and the currently
/// selected date (or a placeholder). Clicking opens a popover with quick
/// shortcuts, a month calendar grid, and an optional clear action.
///
/// # Usage
/// ```ignore
/// let (date, set_date) = signal(None::<String>);
/// <DatePicker
///     value=Signal::derive(move || date.get())
///     on_change=Callback::new(move |v| set_date.set(v))
/// />
/// ```
#[component]
pub fn DatePicker(
    /// Current date value (ISO 8601: "2026-05-13"), or None.
    #[prop(into)]
    value: Signal<Option<String>>,
    /// Called when a date is selected or cleared.
    on_change: Callback<Option<String>>,
    /// Placeholder when no date set. Default: "No due date"
    #[prop(default = "No due date")]
    placeholder: &'static str,
) -> impl IntoView {
    // ── Internal open state ──
    let (open, set_open) = signal(false);

    // ── Calendar navigation state (viewed month/year) ──
    let (view_year, set_view_year) = signal(0_i32);
    let (view_month, set_view_month) = signal(0_u32);

    // ── Keyboard-focused day within the calendar grid ──
    let (focused_date, set_focused_date) = signal(None::<NaiveDate>);

    // ── Trigger ref for Popover positioning ──
    let trigger_ref = NodeRef::<leptos::html::Div>::new();

    // ── Today's date (client-side only) ──
    // On the server, we return None so no overdue/due-soon styling is applied
    // during SSR. The client will hydrate with the correct value.
    let today = move || -> Option<NaiveDate> {
        #[cfg(target_arch = "wasm32")]
        {
            Some(chrono::Local::now().date_naive())
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            None
        }
    };

    // ── Parse the current value into a NaiveDate ──
    let parsed_value = move || -> Option<NaiveDate> {
        value.get().and_then(|s| NaiveDate::parse_from_str(&s, "%Y-%m-%d").ok())
    };

    // ── Initialize calendar view to selected date's month or today ──
    // When the popover opens, set the viewed month to match the current
    // selection (or today if nothing selected).
    let initialize_view = move || {
        let target = parsed_value()
            .or_else(today)
            .unwrap_or_else(today_or_fallback);
        set_view_year.set(target.year());
        set_view_month.set(target.month());
        set_focused_date.set(Some(parsed_value().unwrap_or(target)));
    };

    // ── Toggle popover ──
    let do_toggle = move || {
        if !open.get_untracked() {
            initialize_view();
        }
        set_open.update(|o| *o = !*o);
    };

    let toggle_open = move |_: leptos::ev::MouseEvent| {
        do_toggle();
    };

    let trigger_keydown = move |ev: leptos::ev::KeyboardEvent| {
        if ev.key() == "Enter" || ev.key() == " " {
            ev.prevent_default();
            do_toggle();
        }
    };

    // ── Close popover ──
    let close = Callback::new(move |()| {
        set_open.set(false);
    });

    // ── Select a date and close ──
    let select_date = move |date: NaiveDate| {
        on_change.run(Some(date.format("%Y-%m-%d").to_string()));
        set_open.set(false);
    };

    // ── Navigate months ──
    let prev_month = move |_| {
        let m = view_month.get_untracked();
        let y = view_year.get_untracked();
        if m == 1 {
            set_view_month.set(12);
            set_view_year.set(y - 1);
        } else {
            set_view_month.set(m - 1);
        }
    };

    let next_month = move |_| {
        let m = view_month.get_untracked();
        let y = view_year.get_untracked();
        if m == 12 {
            set_view_month.set(1);
            set_view_year.set(y + 1);
        } else {
            set_view_month.set(m + 1);
        }
    };

    // ── Keyboard navigation ──
    // This handler is compiled on all targets but only fires on WASM (no
    // keyboard events during SSR). We avoid `#[cfg(target_arch = "wasm32")]`
    // here because the Leptos `view!` macro needs the binding to exist on
    // all targets.
    let on_keydown = move |ev: leptos::ev::KeyboardEvent| {
        let key = ev.key();
        match key.as_str() {
            "ArrowUp" | "ArrowDown" | "ArrowLeft" | "ArrowRight" => {
                ev.prevent_default();
                let delta = match key.as_str() {
                    "ArrowUp" => -7,
                    "ArrowDown" => 7,
                    "ArrowLeft" => -1,
                    "ArrowRight" => 1,
                    _ => 0,
                };
                let current = focused_date.get_untracked()
                    .or_else(parsed_value)
                    .or_else(today)
                    .unwrap_or_else(today_or_fallback);
                let new_date = current + Duration::days(delta);
                set_focused_date.set(Some(new_date));
                // Update viewed month if focus crosses a month boundary
                if new_date.month() != view_month.get_untracked()
                    || new_date.year() != view_year.get_untracked()
                {
                    set_view_year.set(new_date.year());
                    set_view_month.set(new_date.month());
                }
            }
            "Enter" => {
                ev.prevent_default();
                if let Some(date) = focused_date.get_untracked() {
                    select_date(date);
                }
            }
            _ => {}
        }
    };

    // ── Trigger display ──
    let trigger_text = move || -> String {
        match parsed_value() {
            Some(d) => format_display_date(&d),
            None => placeholder.to_string(),
        }
    };

    // Determine trigger color based on date status:
    // - No value: text-muted-foreground
    // - Overdue: text-destructive
    // - Due soon (<=3 days): text-warning-foreground for icon, normal for text
    // - Has value: text-foreground
    let trigger_class = move || -> String {
        let base = TRIGGER_BASE;
        match parsed_value() {
            None => format!("{base} text-muted-foreground"),
            Some(date) => {
                if let Some(t) = today() {
                    if date < t {
                        format!("{base} text-[var(--color-destructive)]")
                    } else {
                        format!("{base} text-foreground")
                    }
                } else {
                    format!("{base} text-foreground")
                }
            }
        }
    };

    // Icon color: separate from text for "due soon" state
    let icon_class = move || -> &'static str {
        match parsed_value() {
            None => "text-muted-foreground",
            Some(date) => {
                if let Some(t) = today() {
                    if date < t {
                        "text-[var(--color-destructive)]"
                    } else if date <= t + Duration::days(3) {
                        "text-[var(--color-warning-foreground)]"
                    } else {
                        ""
                    }
                } else {
                    ""
                }
            }
        }
    };

    // ── Quick shortcut dates ──
    let today_date = move || today_or_fallback();
    let tomorrow_date = move || today_date() + Duration::days(1);
    let next_week_date = move || {
        let t = today_date();
        // Next Monday
        let days_until_monday = (Weekday::Mon.num_days_from_monday() as i64
            + 7
            - t.weekday().num_days_from_monday() as i64)
            % 7;
        // If today is Monday, next week = next Monday (7 days)
        let days = if days_until_monday == 0 { 7 } else { days_until_monday };
        t + Duration::days(days)
    };

    // Check which shortcut matches the current value
    let is_today_selected = move || parsed_value() == Some(today_date());
    let is_tomorrow_selected = move || parsed_value() == Some(tomorrow_date());
    let is_next_week_selected = move || parsed_value() == Some(next_week_date());

    // ── Calendar grid data ──
    let grid = move || calendar_grid(view_year.get(), view_month.get());

    // ── Header text ──
    let header_text = move || {
        format!("{} {}", month_name(view_month.get()), view_year.get())
    };

    // ── Has value (for showing Clear button) ──
    let has_value = move || value.get().is_some();

    view! {
        // Trigger
        <div
            node_ref=trigger_ref
            class=trigger_class
            on:click=toggle_open
            on:keydown=trigger_keydown
            role="button"
            tabindex="0"
            aria-haspopup="dialog"
            aria-expanded=move || open.get().to_string()
        >
            <span class=move || format!("inline-flex items-center justify-center shrink-0 {}", icon_class())>
                <Icon icon=phosphor_leptos::CALENDAR_BLANK size="14px"/>
            </span>
            <span class="truncate">{trigger_text}</span>
        </div>

        // Popover
        <Popover
            trigger_ref=trigger_ref
            open=Signal::from(open)
            on_close=close
            placement=Placement::BOTTOM_START
            class="w-[280px] bg-card border border-border rounded-md shadow-lg overflow-hidden"
        >
            <div
                tabindex="0"
                on:keydown=on_keydown
            >
                // ── Quick shortcuts ──
                <div class="flex items-center gap-1.5 px-3 py-2.5 border-b border-border">
                    <ToggleButton
                        variant=Signal::derive(move || {
                            if is_today_selected() {
                                ButtonVariant::PillActive
                            } else {
                                ButtonVariant::Pill
                            }
                        })
                        size=ButtonSize::Pill
                        on:click=move |_| select_date(today_date())
                    >
                        "Today"
                    </ToggleButton>
                    <ToggleButton
                        variant=Signal::derive(move || {
                            if is_tomorrow_selected() {
                                ButtonVariant::PillActive
                            } else {
                                ButtonVariant::Pill
                            }
                        })
                        size=ButtonSize::Pill
                        on:click=move |_| select_date(tomorrow_date())
                    >
                        "Tomorrow"
                    </ToggleButton>
                    <ToggleButton
                        variant=Signal::derive(move || {
                            if is_next_week_selected() {
                                ButtonVariant::PillActive
                            } else {
                                ButtonVariant::Pill
                            }
                        })
                        size=ButtonSize::Pill
                        on:click=move |_| select_date(next_week_date())
                    >
                        "Next week"
                    </ToggleButton>
                </div>

                // ── Month/year header with navigation ──
                <div class="flex items-center justify-between px-3 py-2">
                    <Button
                        variant=ButtonVariant::GhostMuted
                        size=ButtonSize::IconXs
                        aria_label="Previous month"
                        on:click=prev_month
                    >
                        <Icon icon=phosphor_leptos::CARET_LEFT size="14px"/>
                    </Button>
                    <span class="text-sm font-semibold text-foreground select-none">
                        {header_text}
                    </span>
                    <Button
                        variant=ButtonVariant::GhostMuted
                        size=ButtonSize::IconXs
                        aria_label="Next month"
                        on:click=next_month
                    >
                        <Icon icon=phosphor_leptos::CARET_RIGHT size="14px"/>
                    </Button>
                </div>

                // ── Day-of-week headers ──
                <div class="grid grid-cols-7 px-3">
                    {DAY_HEADERS.iter().map(|d| view! {
                        <div class="flex items-center justify-center h-9 text-[11px] text-muted-foreground uppercase select-none">
                            {*d}
                        </div>
                    }).collect_view()}
                </div>

                // ── Calendar grid ──
                <div class="grid grid-cols-7 px-3 pb-2">
                    {move || {
                        let current_month = view_month.get();
                        let selected = parsed_value();
                        let today_val = today();
                        let focused = focused_date.get();

                        grid().into_iter().map(|date| {
                            let is_current_month = date.month() == current_month;
                            let is_selected = Some(date) == selected;
                            let is_today = Some(date) == today_val;
                            let is_focused = Some(date) == focused;

                            // Build cell classes
                            let cell_class = {
                                let mut classes = String::from(
                                    "flex items-center justify-center h-9 w-9 \
                                     text-[13px] cursor-pointer select-none \
                                     transition-colors duration-200 rounded-md"
                                );

                                if is_selected {
                                    classes.push_str(" bg-primary text-primary-foreground");
                                } else if is_today {
                                    classes.push_str(" ring-1 ring-primary text-foreground hover:bg-accent/50");
                                } else if !is_current_month {
                                    classes.push_str(" text-muted-foreground opacity-40 hover:bg-accent/30");
                                } else {
                                    classes.push_str(" text-foreground hover:bg-accent");
                                }

                                if is_focused && !is_selected {
                                    classes.push_str(" ring-1 ring-ring");
                                }

                                classes
                            };

                            view! {
                                <div
                                    class=cell_class
                                    on:click=move |_| select_date(date)
                                    role="gridcell"
                                    aria-selected=is_selected.to_string()
                                    aria-label=format_display_date(&date)
                                >
                                    {date.day()}
                                </div>
                            }
                        }).collect_view()
                    }}
                </div>

                // ── Clear action (only when date is set) ──
                <Show when=has_value>
                    <div class="border-t border-border px-3 py-2">
                        <Button
                            variant=ButtonVariant::GhostMuted
                            size=ButtonSize::Sm
                            on:click=move |_| {
                                on_change.run(None);
                                set_open.set(false);
                            }
                        >
                            "Clear"
                        </Button>
                    </div>
                </Show>
            </div>
        </Popover>
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn calendar_grid_starts_on_monday() {
        let grid = calendar_grid(2026, 5);
        // May 2026 starts on a Friday → grid should start on Monday April 27
        assert_eq!(
            grid[0],
            NaiveDate::from_ymd_opt(2026, 4, 27).expect("2026-04-27 to be a real calendar date")
        );
        assert_eq!(grid[0].weekday(), Weekday::Mon);
    }

    #[test]
    fn calendar_grid_has_42_cells() {
        let grid = calendar_grid(2026, 5);
        assert_eq!(grid.len(), 42);
    }

    #[test]
    fn calendar_grid_ends_on_sunday() {
        let grid = calendar_grid(2026, 5);
        assert_eq!(grid[41].weekday(), Weekday::Sun);
    }

    #[test]
    fn calendar_grid_february_leap_year() {
        // 2024 is a leap year
        let grid = calendar_grid(2024, 2);
        // Feb 1, 2024 is a Thursday → grid starts Monday Jan 29
        assert_eq!(
            grid[0],
            NaiveDate::from_ymd_opt(2024, 1, 29).expect("2024-01-29 to be a real calendar date")
        );
        // Feb 29 should be in the grid
        let feb29 =
            NaiveDate::from_ymd_opt(2024, 2, 29).expect("2024-02-29 to exist because 2024 is a leap year");
        assert!(grid.contains(&feb29));
    }

    #[test]
    fn calendar_grid_month_starting_monday() {
        // June 2026 starts on a Monday → grid should start on June 1
        let grid = calendar_grid(2026, 6);
        assert_eq!(
            grid[0],
            NaiveDate::from_ymd_opt(2026, 6, 1).expect("2026-06-01 to be a real calendar date")
        );
    }

    #[test]
    fn format_display_date_works() {
        let date = NaiveDate::from_ymd_opt(2026, 5, 13).expect("2026-05-13 to be a real calendar date");
        assert_eq!(format_display_date(&date), "May 13, 2026");
    }

    #[test]
    fn format_display_date_single_digit_day() {
        let date = NaiveDate::from_ymd_opt(2026, 1, 3).expect("2026-01-03 to be a real calendar date");
        assert_eq!(format_display_date(&date), "Jan 3, 2026");
    }

    #[test]
    fn month_name_all_months() {
        assert_eq!(month_name(1), "January");
        assert_eq!(month_name(6), "June");
        assert_eq!(month_name(12), "December");
    }
}
