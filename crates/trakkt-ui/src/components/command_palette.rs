// SPDX-License-Identifier: AGPL-3.0-or-later

//! Command palette — Cmd+K quick navigation overlay.
//!
//! A centered overlay with search input and keyboard-navigable results list.
//! Provides quick navigation to pages and fuzzy search over issue titles.
//!
//! DESIGN.md lists CommandPalette as a required component and keyboard
//! navigation as a v1 requirement.

use leptos::prelude::*;
use leptos_router::hooks::use_navigate;
use phosphor_leptos::Icon;

use crate::server_fns::issues::list_issues;

// ─────────────────────────────────────────────────────────────────────────────
// Types
// ─────────────────────────────────────────────────────────────────────────────

/// A single action in the command palette results list.
#[derive(Clone, PartialEq)]
struct PaletteAction {
    /// Display label shown in the results list.
    label: String,
    /// Optional secondary text (e.g. issue number).
    description: Option<String>,
    /// Icon to show next to the label.
    icon: PaletteIcon,
    /// What happens when this action is executed.
    kind: ActionKind,
}

#[derive(Clone, PartialEq)]
enum PaletteIcon {
    ListChecks,
    Kanban,
    Gear,
    Plus,
    Article,
}

#[derive(Clone, PartialEq)]
enum ActionKind {
    /// Navigate to a fixed route.
    Navigate(String),
    /// Navigate to an issue by number.
    NavigateIssue(i32),
}

// ─────────────────────────────────────────────────────────────────────────────
// Static actions (always available)
// ─────────────────────────────────────────────────────────────────────────────

fn static_actions() -> Vec<PaletteAction> {
    vec![
        PaletteAction {
            label: "Go to Issues".to_string(),
            description: None,
            icon: PaletteIcon::ListChecks,
            kind: ActionKind::Navigate("/issues".to_string()),
        },
        PaletteAction {
            label: "Go to Board".to_string(),
            description: None,
            icon: PaletteIcon::Kanban,
            kind: ActionKind::Navigate("/board".to_string()),
        },
        PaletteAction {
            label: "Go to Settings".to_string(),
            description: None,
            icon: PaletteIcon::Gear,
            kind: ActionKind::Navigate("/settings".to_string()),
        },
        PaletteAction {
            label: "Create Issue".to_string(),
            description: None,
            icon: PaletteIcon::Plus,
            kind: ActionKind::Navigate("/issues?action=new".to_string()),
        },
    ]
}

// ─────────────────────────────────────────────────────────────────────────────
// Component
// ─────────────────────────────────────────────────────────────────────────────

/// Command palette overlay — Cmd+K to open, Escape to close.
///
/// Renders a centered modal with a search input and keyboard-navigable
/// results list. Static navigation actions are always shown; as the user
/// types, matching issue titles appear below them.
#[component]
pub fn CommandPalette(
    /// Whether the palette is visible.
    #[prop(into)]
    show: Signal<bool>,
    /// Called when the palette should close.
    on_close: Callback<()>,
) -> impl IntoView {
    let (search, set_search) = signal(String::new());
    let (selected_index, set_selected_index) = signal(0usize);
    let input_ref = NodeRef::<leptos::html::Input>::new();

    // Reset state when the palette opens.
    Effect::new(move || {
        if show.get() {
            set_search.set(String::new());
            set_selected_index.set(0);
            // Focus the input on next frame (after mount).
            request_animation_frame(move || {
                if let Some(input) = input_ref.get() {
                    let _ = input.focus();
                }
            });
        }
    });

    // ── Issue search (fires when search text has 2+ chars) ──────────────────
    // Resource returns Vec<IssueWithDetails> (serde-compatible). Conversion
    // to PaletteAction happens in the Memo below.
    let issue_results = Resource::new(
        move || search.get(),
        move |query| async move {
            if query.len() < 2 {
                return Vec::new();
            }
            list_issues(None, None, None, None, Some(query), Some(10), None)
                .await
                .unwrap_or_default()
        },
    );

    // ── Combined results (static + dynamic) ─────────────────────────────────
    let filtered_results = Memo::new(move |_| {
        let query = search.get().to_lowercase();
        let mut results: Vec<PaletteAction> = static_actions()
            .into_iter()
            .filter(|a| query.is_empty() || a.label.to_lowercase().contains(&query))
            .collect();

        // Append issue search results (converted from IssueWithDetails).
        if let Some(issues) = issue_results.get() {
            results.extend(issues.into_iter().map(|issue| PaletteAction {
                label: issue.title,
                description: Some(format!("{}-{}", issue.team_key, issue.number)),
                icon: PaletteIcon::Article,
                kind: ActionKind::NavigateIssue(issue.number),
            }));
        }

        results
    });

    let result_count = Memo::new(move |_| filtered_results.get().len());

    // ── Execute the selected action ─────────────────────────────────────────
    let execute_action = move |action: &PaletteAction| {
        let nav = use_navigate();
        match &action.kind {
            ActionKind::Navigate(path) => {
                nav(path, Default::default());
            }
            ActionKind::NavigateIssue(number) => {
                nav(&format!("/issues/{number}"), Default::default());
            }
        }
        on_close.run(());
    };

    // ── Keyboard handler ────────────────────────────────────────────────────
    let on_keydown = move |ev: web_sys::KeyboardEvent| {
        let key = ev.key();
        match key.as_str() {
            "ArrowDown" => {
                ev.prevent_default();
                let count = result_count.get_untracked();
                if count > 0 {
                    set_selected_index.update(|i| *i = (*i + 1) % count);
                }
            }
            "ArrowUp" => {
                ev.prevent_default();
                let count = result_count.get_untracked();
                if count > 0 {
                    set_selected_index.update(|i| {
                        *i = if *i == 0 { count - 1 } else { *i - 1 };
                    });
                }
            }
            "Enter" => {
                ev.prevent_default();
                let results = filtered_results.get_untracked();
                let idx = selected_index.get_untracked();
                if let Some(action) = results.get(idx) {
                    execute_action(action);
                }
            }
            "Escape" => {
                ev.prevent_default();
                on_close.run(());
            }
            _ => {}
        }
    };

    // Reset selected index when search changes.
    Effect::new(move || {
        let _ = search.get();
        set_selected_index.set(0);
    });

    view! {
        <Show when=move || show.get()>
            // Backdrop
            <div
                class="fixed inset-0 z-[1100] flex items-start justify-center pt-[20vh] bg-black/50 animate-fade-in-fast"
                on:click=move |ev: web_sys::MouseEvent| {
                    // Close on backdrop click (not bubbled from palette).
                    let target = ev.target();
                    let current_target = ev.current_target();
                    if target == current_target {
                        on_close.run(());
                    }
                }
                on:keydown=on_keydown
            >
                // Palette container
                <div
                    class="w-full max-w-lg mx-4 bg-background border border-border rounded-lg shadow-lg animate-zoom-fade-in overflow-hidden flex flex-col max-h-[60vh]"
                    on:click=|ev: web_sys::MouseEvent| ev.stop_propagation()
                >
                    // Search input
                    <div class="px-4 py-3 border-b border-border flex items-center gap-3">
                        <span class="text-muted-foreground shrink-0">
                            <Icon icon=phosphor_leptos::MAGNIFYING_GLASS size="20px"/>
                        </span>
                        <input
                            node_ref=input_ref
                            type="text"
                            placeholder="Type a command or search issues..."
                            class="flex-1 bg-transparent text-sm text-foreground placeholder:text-muted-foreground focus:outline-none"
                            prop:value=move || search.get()
                            on:input=move |ev| set_search.set(event_target_value(&ev))
                        />
                        <kbd class="hidden sm:inline-flex items-center px-1.5 py-0.5 text-xs font-mono text-muted-foreground bg-surface-alt border border-border rounded">
                            "Esc"
                        </kbd>
                    </div>

                    // Results list
                    <div class="overflow-y-auto flex-1 py-2" role="listbox">
                        {move || {
                            let results = filtered_results.get();
                            let sel = selected_index.get();
                            if results.is_empty() {
                                view! {
                                    <div class="px-4 py-6 text-center text-sm text-muted-foreground">
                                        "No results found"
                                    </div>
                                }.into_any()
                            } else {
                                results.into_iter().enumerate().map(|(idx, action)| {
                                    let is_selected = idx == sel;
                                    let label = action.label.clone();
                                    let description = action.description.clone();
                                    let icon = action.icon.clone();
                                    let action_for_click = action.clone();
                                    view! {
                                        <button
                                            class=move || {
                                                if is_selected {
                                                    "w-full flex items-center gap-3 px-4 py-2.5 text-left text-sm transition-colors bg-primary/10 text-foreground"
                                                } else {
                                                    "w-full flex items-center gap-3 px-4 py-2.5 text-left text-sm transition-colors hover:bg-surface-alt text-foreground"
                                                }
                                            }
                                            role="option"
                                            aria-selected=move || is_selected
                                            on:click={
                                                let action = action_for_click.clone();
                                                move |_| execute_action(&action)
                                            }
                                            on:mouseenter=move |_| set_selected_index.set(idx)
                                        >
                                            <span class="text-muted-foreground shrink-0">
                                                {palette_icon_view(icon.clone())}
                                            </span>
                                            <span class="flex-1 truncate">{label.clone()}</span>
                                            {description.as_ref().map(|d| view! {
                                                <span class="font-mono text-xs text-muted-foreground shrink-0">
                                                    {d.clone()}
                                                </span>
                                            })}
                                        </button>
                                    }
                                }).collect_view().into_any()
                            }
                        }}
                    </div>

                    // Footer hint
                    <div class="px-4 py-2 border-t border-border flex items-center gap-4 text-xs text-muted-foreground">
                        <span class="flex items-center gap-1">
                            <kbd class="px-1 py-0.5 font-mono bg-surface-alt border border-border rounded">"↑"</kbd>
                            <kbd class="px-1 py-0.5 font-mono bg-surface-alt border border-border rounded">"↓"</kbd>
                            " navigate"
                        </span>
                        <span class="flex items-center gap-1">
                            <kbd class="px-1 py-0.5 font-mono bg-surface-alt border border-border rounded">"↵"</kbd>
                            " select"
                        </span>
                    </div>
                </div>
            </div>
        </Show>
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Helpers
// ─────────────────────────────────────────────────────────────────────────────

/// Render the appropriate Phosphor icon for a palette action.
fn palette_icon_view(icon: PaletteIcon) -> AnyView {
    match icon {
        PaletteIcon::ListChecks => {
            view! { <Icon icon=phosphor_leptos::LIST_CHECKS size="16px"/> }.into_any()
        }
        PaletteIcon::Kanban => {
            view! { <Icon icon=phosphor_leptos::KANBAN size="16px"/> }.into_any()
        }
        PaletteIcon::Gear => {
            view! { <Icon icon=phosphor_leptos::GEAR size="16px"/> }.into_any()
        }
        PaletteIcon::Plus => {
            view! { <Icon icon=phosphor_leptos::PLUS size="16px"/> }.into_any()
        }
        PaletteIcon::Article => {
            view! { <Icon icon=phosphor_leptos::ARTICLE size="16px"/> }.into_any()
        }
    }
}
