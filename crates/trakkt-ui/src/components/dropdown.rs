// SPDX-License-Identifier: AGPL-3.0-or-later

//! Linear-style searchable dropdown components.
//!
//! DESIGN.md § "Dropdowns": every metadata field on an issue opens a
//! searchable, keyboard-navigable dropdown with icon + checkmark + keyboard
//! hints. This module provides three components:
//!
//! - [`DropdownTrigger`] — compact button showing current value with icon
//!   and chevron.
//! - [`DropdownMenu`] — searchable menu that renders below a trigger via
//!   the [`Popover`](crate::components::popover::Popover) positioning
//!   system.
//! - [`DropdownItem`] — individual menu item with icon, label, checkmark,
//!   and optional keyboard shortcut.

use leptos::prelude::*;
use phosphor_leptos::Icon;

// ─────────────────────────────────────────────────────────────────────────────
// CSS class constants — derived from DESIGN.md § "Dropdowns"
// ─────────────────────────────────────────────────────────────────────────────

/// Trigger base classes.
///
/// DESIGN.md trigger spec:
/// - Border: `1px solid --border`, `--radius-sm` (4px)
/// - Font: DM Sans, 12px
/// - Padding: 4px 8px
/// - Hover: `border-color: --border-strong`, `color: --text`
/// - Transition: colors 200ms
/// - Focus-visible: ring-1 ring-ring
const TRIGGER_BASE: &str = "inline-flex items-center gap-1.5 whitespace-nowrap \
    border border-border rounded-[4px] px-2 py-1 \
    text-xs font-normal cursor-pointer \
    transition-colors duration-200 \
    hover:border-[--color-border-strong] hover:text-foreground \
    focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring";

/// Form-style trigger — taller, full-width, for settings/modals/forms.
/// Matches the height and feel of text inputs (h-11 = 44px).
const TRIGGER_FORM: &str = "flex h-11 w-full items-center justify-between whitespace-nowrap \
    rounded-md border border-border bg-transparent px-3 py-2 \
    text-sm cursor-pointer \
    transition-colors duration-200 \
    hover:border-[--color-border-strong] hover:text-foreground \
    focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring";

/// Chevron icon classes on the form-variant trigger.
const FORM_CHEVRON_CLASS: &str = "h-4 w-4 opacity-50";

/// Menu container classes.
///
/// DESIGN.md menu spec:
/// - Width: 220px
/// - Background: `--surface` (bg-card in Tailwind mapping)
/// - Border: `1px solid --border`, `--radius-md` (6px)
/// - Shadow: `shadow-lg`
/// - z-index handled by Popover portal
const MENU_CLASS: &str = "min-w-[220px] bg-card border border-border rounded-md shadow-lg \
    overflow-hidden";

/// Search input classes inside the menu.
///
/// DESIGN.md: 12px, no border, transparent bg, full width.
const SEARCH_INPUT_CLASS: &str = "w-full px-2.5 py-[5px] text-xs bg-transparent \
    text-foreground placeholder:text-muted-foreground \
    border-none outline-none";

/// Item base classes.
///
/// DESIGN.md item spec:
/// - Font: 13px
/// - Padding: `px-2.5 py-[5px]`, margin: `mx-1 my-px`
/// - Border radius: 3px
/// - Hover: `bg-surface-alt` (bg-secondary)
/// - Transition: colors
const ITEM_BASE: &str = "flex items-center gap-2 w-full cursor-default select-none \
    text-[13px] px-2.5 py-[5px] mx-1 my-px rounded-[3px] \
    transition-colors duration-100 \
    hover:bg-secondary";

/// Item selected state classes.
///
/// DESIGN.md: `bg-accent-light`, `color: --accent`, checkmark right-aligned.
const ITEM_SELECTED: &str = "bg-accent text-primary";

/// Checkmark icon classes — absolute right positioning within item.
const CHECK_CLASS: &str = "ml-auto flex items-center justify-center shrink-0 text-primary";

/// Keyboard shortcut label classes.
///
/// DESIGN.md: `font-mono text-[10px] text-muted-foreground` right-aligned.
const SHORTCUT_CLASS: &str = "ml-auto font-mono text-[10px] text-muted-foreground shrink-0";

/// Footer classes — keyboard hints bar.
const FOOTER_CLASS: &str = "border-t border-border px-2.5 py-1.5 text-[10px] text-muted-foreground";

// ─────────────────────────────────────────────────────────────────────────────
// DropdownTrigger
// ─────────────────────────────────────────────────────────────────────────────

/// Compact trigger button for a Linear-style dropdown.
///
/// Shows the current value with its icon and a chevron. When no value is set,
/// shows just the field name.
///
/// # Usage
/// ```ignore
/// <DropdownTrigger
///     label="Status"
///     value=Signal::derive(move || Some("In Progress".to_string()))
///     icon=Some(|| view! { <IssueStatusBadge status=IssueStatusVariant::InProgress size=12/> }.into_any())
///     on_click=Callback::new(move |()| set_open.update(|o| *o = !*o))
/// />
/// ```
#[component]
pub fn DropdownTrigger(
    /// Field name shown when no value is set (e.g., "Assignee", "Status").
    #[prop(into)]
    label: String,
    /// Current value text (e.g., "In Progress", "High").
    #[prop(into)]
    value: Signal<Option<String>>,
    /// Optional icon to show before the value (status circle, priority bars,
    /// label color dot). Rendered only when a value is set.
    #[prop(optional)]
    icon: Option<ChildrenFn>,
    /// Click handler — parent manages open/close state.
    on_click: Callback<()>,
) -> impl IntoView {
    // DESIGN.md: `--text-secondary` when no value, `--text` when has value
    let text_class = move || {
        if value.get().is_some() {
            "text-foreground"
        } else {
            "text-muted-foreground"
        }
    };

    let label_for_display = label.clone();

    view! {
        <button
            type="button"
            class=move || format!("{TRIGGER_BASE} {}", text_class())
            on:click=move |_| on_click.run(())
            aria-haspopup="listbox"
        >
            // Icon (only when value is set)
            {icon.map(|i| view! {
                <Show when=move || value.get().is_some()>
                    <span class="inline-flex items-center justify-center shrink-0">
                        {i()}
                    </span>
                </Show>
            })}

            // Label or value text
            <span class="truncate">
                {
                    let label = label_for_display.clone();
                    move || value.get().unwrap_or_else(|| label.clone())
                }
            </span>

            // Chevron
            <span class="text-[8px] text-muted-foreground leading-none shrink-0">"▾"</span>
        </button>
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// DropdownMenu
// ─────────────────────────────────────────────────────────────────────────────

/// Searchable dropdown menu that renders below a trigger.
///
/// This is the core interaction pattern used for status, priority, label,
/// and assignee pickers. Uses the [`Popover`] component for positioning,
/// click-outside detection, and Escape handling.
///
/// # Usage
/// ```ignore
/// <DropdownMenu
///     trigger_ref=trigger_ref
///     open=open.into()
///     on_close=Callback::new(move |()| set_open.set(false))
///     search_placeholder=Some("Change status...".to_string())
///     footer=Some(|| view! {
///         <span>"↑↓ navigate  ↵ select"</span>
///     }.into_any())
/// >
///     <DropdownItem label="Backlog" ... />
///     <DropdownItem label="Todo" ... />
/// </DropdownMenu>
/// ```
#[component]
pub fn DropdownMenu(
    /// Ref on the trigger element — drives Popover positioning.
    trigger_ref: NodeRef<leptos::html::Div>,
    /// Whether the menu is visible.
    #[prop(into)]
    open: Signal<bool>,
    /// Called when user clicks outside or presses Escape.
    on_close: Callback<()>,
    /// Placeholder text for search input (e.g., "Change status...").
    /// When `None`, no search input is rendered.
    #[prop(optional, into)]
    search_placeholder: Option<String>,
    /// Called when the user types in the search input. The parent should
    /// filter the items it passes as `children` based on this string.
    #[prop(optional)]
    on_search: Option<Callback<String>>,
    /// When `true`, the popover's `min-width` matches the trigger's width.
    /// Used by form-style selects where the menu should span the trigger.
    #[prop(optional)]
    match_width: bool,
    /// Optional keyboard hints footer.
    #[prop(optional)]
    footer: Option<ChildrenFn>,
    /// The menu items — rendered inside the scrollable area.
    children: ChildrenFn,
) -> impl IntoView {
    let search_placeholder_stored = StoredValue::new(search_placeholder);
    let on_search_stored = StoredValue::new(on_search);
    let footer_stored = StoredValue::new(footer);
    let children_stored = StoredValue::new(children);

    view! {
        <crate::components::popover::Popover
            trigger_ref=trigger_ref
            open=open
            on_close=on_close
            placement=crate::components::popover::Placement::BOTTOM_START
            match_width=match_width
            class=MENU_CLASS
        >
            // Search input (optional)
            {search_placeholder_stored.with_value(|sp| {
                sp.as_ref().map(|placeholder| {
                    let placeholder = placeholder.clone();
                    view! {
                        <div class="border-b border-border px-2.5 py-2">
                            <input
                                type="text"
                                placeholder=placeholder
                                class=SEARCH_INPUT_CLASS
                                on:input=move |ev| {
                                    on_search_stored.with_value(|cb| {
                                        if let Some(cb) = cb {
                                            cb.run(event_target_value(&ev));
                                        }
                                    });
                                }
                            />
                        </div>
                    }
                })
            })}

            // Scrollable items area
            <div
                class="max-h-[280px] overflow-y-auto py-1"
                role="listbox"
                style="scrollbar-width: thin;"
            >
                {children_stored.with_value(|c| c())}
            </div>

            // Footer (optional)
            {footer_stored.with_value(|f| {
                f.as_ref().map(|footer_fn| {
                    view! {
                        <div class=FOOTER_CLASS>
                            {footer_fn()}
                        </div>
                    }
                })
            })}
        </crate::components::popover::Popover>
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// DropdownItem
// ─────────────────────────────────────────────────────────────────────────────

/// Individual menu item inside a [`DropdownMenu`].
///
/// Renders an icon, label, optional checkmark (when selected), and optional
/// keyboard shortcut.
///
/// # Usage
/// ```ignore
/// <DropdownItem
///     label="In Progress"
///     selected=true
///     on_select=Callback::new(move |()| set_status("in_progress"))
///     icon=Some(|| view! { <IssueStatusBadge status=IssueStatusVariant::InProgress size=14/> }.into_any())
///     shortcut=Some("3".to_string())
/// />
/// ```
#[component]
pub fn DropdownItem(
    /// Item text.
    #[prop(into)]
    label: String,
    /// Whether this item is currently selected — shows checkmark on right.
    #[prop(into, default = false.into())]
    selected: Signal<bool>,
    /// Selection handler.
    on_select: Callback<()>,
    /// Optional left-side icon (status circle, priority bars, label dot).
    #[prop(optional)]
    icon: Option<ChildrenFn>,
    /// Optional right-aligned keyboard shortcut (e.g., "1", "esc").
    #[prop(optional, into)]
    shortcut: Option<String>,
) -> impl IntoView {
    let item_class = move || {
        if selected.get() {
            format!("{ITEM_BASE} {ITEM_SELECTED}")
        } else {
            ITEM_BASE.to_string()
        }
    };

    view! {
        <div
            class=item_class
            role="option"
            aria-selected=move || selected.get().to_string()
            on:click=move |_| on_select.run(())
        >
            // Icon (optional)
            {icon.map(|i| view! {
                <span class="inline-flex items-center justify-center shrink-0">
                    {i()}
                </span>
            })}

            // Label
            <span class="truncate">{label}</span>

            // Checkmark (when selected) or keyboard shortcut
            {
                let shortcut = shortcut.clone();
                move || {
                    if selected.get() {
                        view! { <span class=CHECK_CLASS>"✓"</span> }.into_any()
                    } else if let Some(ref sc) = shortcut {
                        view! { <span class=SHORTCUT_CLASS>{sc.clone()}</span> }.into_any()
                    } else {
                        ().into_any()
                    }
                }
            }
        </div>
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// SelectVariant + Select
// ─────────────────────────────────────────────────────────────────────────────

/// Visual variant for the [`Select`] trigger.
///
/// - `Compact` (default): 28px inline trigger matching filter-bar dropdowns.
/// - `Form`: 44px full-width trigger matching text-input height for
///   forms and settings pages.
#[derive(Default, Clone, Copy, PartialEq)]
pub enum SelectVariant {
    #[default]
    Compact,
    Form,
}

/// High-level select component with two trigger variants.
///
/// Wraps [`DropdownMenu`] + [`DropdownItem`] with either a compact
/// inline trigger or a form-field-height trigger.
///
/// - `Compact` (default): 28px, inline, matches filter bar dropdowns
/// - `Form`: 44px, full-width, matches text input height for forms/settings
#[component]
pub fn Select(
    /// Current selected value (controlled by parent).
    #[prop(into)]
    value: Signal<String>,
    /// Options list. Each entry is (value, label).
    #[prop(into)]
    options: Signal<Vec<(String, String)>>,
    /// Called when user selects an option.
    on_change: Callback<String>,
    /// Visual variant of the trigger.
    #[prop(optional)]
    variant: SelectVariant,
    /// Placeholder shown when value is empty (form variant only, typically).
    #[prop(optional, into)]
    placeholder: Option<String>,
) -> impl IntoView {
    let (is_open, set_is_open) = signal(false);
    let trigger_ref = NodeRef::<leptos::html::Div>::new();

    let placeholder_stored = StoredValue::new(placeholder);

    // Derive the label to display in the trigger: look up the current value
    // in options. If empty and a placeholder exists, show the placeholder.
    // If value not found in options, show the raw value.
    let display_label = Memo::new(move |_| {
        let val = value.get();
        if val.is_empty() {
            return placeholder_stored.with_value(|p| {
                p.clone().unwrap_or_else(String::new)
            });
        }
        options
            .get()
            .iter()
            .find(|(v, _)| *v == val)
            .map(|(_, l)| l.clone())
            .unwrap_or_else(|| val.clone())
    });

    // Whether we are currently showing a placeholder (empty value).
    let is_placeholder = move || {
        value.get().is_empty()
    };

    let on_trigger_click = move |_| {
        set_is_open.update(|open| *open = !*open);
    };

    let on_close = Callback::new(move |()| set_is_open.set(false));

    let trigger_view = match variant {
        SelectVariant::Compact => {
            view! {
                <div node_ref=trigger_ref class="inline-flex">
                    <button
                        type="button"
                        class=move || {
                            let text = if is_placeholder() {
                                "text-muted-foreground"
                            } else {
                                "text-foreground"
                            };
                            format!("{TRIGGER_BASE} {text}")
                        }
                        on:click=on_trigger_click
                        aria-expanded=move || is_open.get().to_string()
                        aria-haspopup="listbox"
                    >
                        <span class="truncate">{move || display_label.get()}</span>
                        <span class="text-[8px] text-muted-foreground leading-none shrink-0">"▾"</span>
                    </button>
                </div>
            }.into_any()
        }
        SelectVariant::Form => {
            view! {
                <div node_ref=trigger_ref class="w-full">
                    <button
                        type="button"
                        class=TRIGGER_FORM
                        on:click=on_trigger_click
                        aria-expanded=move || is_open.get().to_string()
                        aria-haspopup="listbox"
                    >
                        <span class=move || {
                            if is_placeholder() {
                                "line-clamp-1 text-muted-foreground"
                            } else {
                                "line-clamp-1"
                            }
                        }>{move || display_label.get()}</span>
                        <Icon icon=phosphor_leptos::CARET_DOWN attr:class=FORM_CHEVRON_CLASS/>
                    </button>
                </div>
            }.into_any()
        }
    };

    let use_match_width = variant == SelectVariant::Form;

    view! {
        {trigger_view}
        <DropdownMenu
            trigger_ref=trigger_ref
            open=Signal::derive(move || is_open.get())
            on_close=on_close
            match_width=use_match_width
        >
            {move || {
                options.get().into_iter().map(|(val_str, label_str)| {
                    let val_for_selected = val_str.clone();
                    let val_for_click = val_str.clone();
                    view! {
                        <DropdownItem
                            label=label_str
                            selected=Signal::derive(move || value.get() == val_for_selected)
                            on_select=Callback::new(move |()| {
                                on_change.run(val_for_click.clone());
                                set_is_open.set(false);
                            })
                        />
                    }
                }).collect_view()
            }}
        </DropdownMenu>
    }
}
