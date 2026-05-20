// SPDX-License-Identifier: AGPL-3.0-or-later

//! Team creation modal — unified component for creating new issue-tracker
//! teams from both the sidebar "+" button and workspace settings page.
//!
//! Layout:
//! 1. Icon preview (clickable color picker) + Team name + Team key
//! 2. Collapsible advanced settings (estimation scale, toggles)
//! 3. Cancel / Create Team footer

use leptos::prelude::*;
use phosphor_leptos::Icon;

use crate::cache::store::SyncStore;
use crate::components::button::{Button, ButtonVariant};
use crate::components::dropdown::{Select, SelectVariant};
use crate::components::input::INPUT_CLASS;
use crate::components::modal::{Modal, ModalSize};
use crate::components::switch::Switch;
use crate::components::team_icon::{
    get_icon, DEFAULT_ICON_COLOR, DEFAULT_ICON_NAME, ICON_CATEGORIES, ICON_COLORS,
};

// ─── Key derivation helpers ─────────────────────────────────────────────────

/// Derive a team key from a name: uppercase first 3 alphabetic characters.
fn derive_key_from_name(name: &str) -> String {
    name.chars()
        .filter(|c| c.is_ascii_alphabetic())
        .take(3)
        .collect::<String>()
        .to_uppercase()
}

/// Validate a team key: 2-5 uppercase ASCII letters.
fn is_valid_key(key: &str) -> bool {
    let len = key.len();
    (2..=5).contains(&len) && key.chars().all(|c| c.is_ascii_uppercase())
}

// ─── Component ──────────────────────────────────────────────────────────────

/// Modal for creating a new issue-tracker team.
///
/// Provides name, key, icon color picker, and collapsible advanced settings
/// (estimation scale, toggles). On successful creation, upserts the new
/// team into the SyncStore and navigates to its issues page.
#[component]
pub fn TeamCreationModal(
    /// Whether the modal is visible.
    #[prop(into)]
    show: Signal<bool>,
    /// Called when the modal should close (cancel, backdrop, or success).
    on_close: Callback<()>,
) -> impl IntoView {
    // ── Form state ──────────────────────────────────────────────────────
    let (name, set_name) = signal(String::new());
    let (key, set_key) = signal(String::new());
    let (key_manually_edited, set_key_manually_edited) = signal(false);
    let (icon_color, set_icon_color) = signal(DEFAULT_ICON_COLOR.to_string());
    let (icon_name, set_icon_name) = signal(DEFAULT_ICON_NAME.to_string());
    let (show_color_picker, set_show_color_picker) = signal(false);
    let (advanced_open, set_advanced_open) = signal(false);

    // Advanced settings
    let (estimate_scale, set_estimate_scale) = signal(String::new());
    let (allow_zero, set_allow_zero) = signal(false);
    let (extended_scale, set_extended_scale) = signal(false);
    let (count_unestimated, set_count_unestimated) = signal(true);

    // Submission state
    let (submitting, set_submitting) = signal(false);
    let (error, set_error) = signal(Option::<String>::None);

    // ── Reset on open ───────────────────────────────────────────────────
    Effect::new(move || {
        if show.get() {
            set_name.set(String::new());
            set_key.set(String::new());
            set_key_manually_edited.set(false);
            set_icon_color.set(DEFAULT_ICON_COLOR.to_string());
            set_icon_name.set(DEFAULT_ICON_NAME.to_string());
            set_show_color_picker.set(false);
            set_advanced_open.set(false);
            set_estimate_scale.set(String::new());
            set_allow_zero.set(false);
            set_extended_scale.set(false);
            set_count_unestimated.set(true);
            set_submitting.set(false);
            set_error.set(None);
        }
    });

    // ── Key auto-derive ─────────────────────────────────────────────────
    let on_name_input = move |ev: leptos::ev::Event| {
        let val = event_target_value(&ev);
        set_name.set(val.clone());
        if !key_manually_edited.get_untracked() {
            set_key.set(derive_key_from_name(&val));
        }
    };

    let on_key_input = move |ev: leptos::ev::Event| {
        let raw = event_target_value(&ev);
        let filtered: String = raw
            .chars()
            .filter(|c| c.is_ascii_alphabetic())
            .take(5)
            .collect::<String>()
            .to_uppercase();
        set_key.set(filtered);
        set_key_manually_edited.set(true);
    };

    // ── Validation ──────────────────────────────────────────────────────
    let key_format_ok = Memo::new(move |_| {
        let k = key.get();
        k.is_empty() || is_valid_key(&k)
    });

    let can_submit = Memo::new(move |_| {
        let n = name.get();
        let k = key.get();
        !n.trim().is_empty() && is_valid_key(&k) && !submitting.get()
    });

    // ── Submit handler ──────────────────────────────────────────────────
    let store = use_context::<SyncStore>();
    let nav = leptos_router::hooks::use_navigate();

    let on_submit = move |_| {
        let name_val = name.get_untracked().trim().to_string();
        let key_val = key.get_untracked();
        if name_val.is_empty() || !is_valid_key(&key_val) {
            return;
        }

        let icon_name_val = icon_name.get_untracked();
        let icon_color_val = icon_color.get_untracked();
        let est_scale = estimate_scale.get_untracked();
        let est_allow_zero = allow_zero.get_untracked();
        let est_extended = extended_scale.get_untracked();
        let est_count_unestimated = count_unestimated.get_untracked();

        // Close the modal immediately. Signal updates during event handlers
        // are batched — the reactive flush (which disposes the modal's scope)
        // happens after this handler returns. The spawn_local below is
        // scheduled before the flush, so its captured values are safe.
        on_close.run(());

        let nav = nav.clone();

        leptos::task::spawn_local(async move {
            let team = match crate::server_fns::teams::create_team(
                name_val,
                key_val,
                None,
                None,
            )
            .await
            {
                Ok(team) => team,
                Err(e) => {
                    tracing::error!(error = %e, "Failed to create team");
                    return;
                }
            };

            let team_id = team.team_id.clone();
            let team_key = team.key.clone();

            if let Err(e) = crate::server_fns::teams::update_team_icon(
                team_id.clone(),
                Some("preset".to_string()),
                Some(icon_name_val),
                Some(icon_color_val),
            )
            .await
            {
                tracing::warn!(error = %e, "Failed to set team icon after creation");
            }

            let has_advanced_changes = !est_scale.is_empty()
                || est_allow_zero
                || est_extended
                || !est_count_unestimated;

            if has_advanced_changes {
                let settings = trakkt_types::models::TeamSettings {
                    auto_archive_days: None,
                    estimate_scale: match est_scale.as_str() {
                        "exponential" => {
                            Some(trakkt_types::models::EstimateScale::Exponential)
                        }
                        "fibonacci" => {
                            Some(trakkt_types::models::EstimateScale::Fibonacci)
                        }
                        "linear" => Some(trakkt_types::models::EstimateScale::Linear),
                        "t_shirt" => Some(trakkt_types::models::EstimateScale::TShirt),
                        _ => None,
                    },
                    estimate_allow_zero: est_allow_zero,
                    estimate_extended: est_extended,
                    estimate_count_unestimated: est_count_unestimated,
                };

                match serde_json::to_string(&settings) {
                    Ok(json) => {
                        if let Err(e) = crate::server_fns::teams::update_team_settings(
                            team_id.clone(),
                            json,
                        )
                        .await
                        {
                            tracing::warn!(
                                error = %e,
                                "Failed to update team settings after creation"
                            );
                        }
                    }
                    Err(e) => {
                        tracing::warn!(
                            error = %e,
                            "Failed to serialize team settings"
                        );
                    }
                }
            }

            if let Some(store) = store {
                match crate::server_fns::teams::get_team_by_key(team_key.clone()).await {
                    Ok(updated_team) => store.upsert_team(updated_team),
                    Err(_) => store.upsert_team(team),
                }
            }

            let href = format!("/teams/{}/issues", team_key.to_lowercase());
            nav(&href, Default::default());
        });
    };

    // ── Icon preview ────────────────────────────────────────────────────
    let icon_preview = move || {
        let color = icon_color.get();
        let name = icon_name.get();
        let icon_data = get_icon(&name);

        view! {
            <button
                type="button"
                class="inline-flex items-center justify-center rounded-md shrink-0 cursor-pointer transition-all duration-200 hover:ring-2 hover:ring-ring hover:ring-offset-2 hover:ring-offset-background focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2 focus-visible:ring-offset-background"
                style=format!("width: 40px; height: 40px; background-color: {};", color)
                on:click=move |_| set_show_color_picker.update(|v| *v = !*v)
            >
                {icon_data.map(|data| view! {
                    <Icon icon=data size="23px" color="white"/>
                })}
            </button>
        }
    };

    // ── Color picker grid ───────────────────────────────────────────────
    let icon_picker = move || {
        view! {
            <Show when=move || show_color_picker.get()>
                <div class="mt-2 space-y-3">
                    // Color palette
                    <div class="grid grid-cols-8 gap-1.5">
                        {ICON_COLORS.iter().map(|&color| {
                            let is_selected = {
                                let color = color.to_string();
                                Signal::derive(move || icon_color.get() == color)
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
                                    on:click=move |_| {
                                        set_icon_color.set(color.to_string());
                                    }
                                />
                            }
                        }).collect_view()}
                    </div>

                    // Icon shape grid
                    {ICON_CATEGORIES.iter().map(|(label, icons)| {
                        view! {
                            <div>
                                <p class="text-xs text-muted-foreground mb-1">{*label}</p>
                                <div class="flex flex-wrap gap-1">
                                    {icons.iter().map(|&icon_id| {
                                        let is_selected = {
                                            let id = icon_id.to_string();
                                            Signal::derive(move || icon_name.get() == id)
                                        };
                                        view! {
                                            <button
                                                type="button"
                                                class=move || {
                                                    let base = "w-8 h-8 rounded-md flex items-center justify-center transition-colors duration-200 focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring";
                                                    if is_selected.get() {
                                                        format!("{base} bg-accent text-primary")
                                                    } else {
                                                        format!("{base} text-muted-foreground hover:bg-secondary hover:text-foreground")
                                                    }
                                                }
                                                title=icon_id
                                                on:click=move |_| {
                                                    set_icon_name.set(icon_id.to_string());
                                                }
                                            >
                                                {get_icon(icon_id).map(|data| view! {
                                                    <Icon icon=data size="16px"/>
                                                })}
                                            </button>
                                        }
                                    }).collect_view()}
                                </div>
                            </div>
                        }
                    }).collect_view()}
                </div>
            </Show>
        }
    };

    // ── Estimation scale options ─────────────────────────────────────────
    let estimate_options = Signal::derive(move || {
        vec![
            ("".to_string(), "Disabled".to_string()),
            (
                "exponential".to_string(),
                "Exponential \u{2014} 1, 2, 4, 8, 16".to_string(),
            ),
            (
                "fibonacci".to_string(),
                "Fibonacci \u{2014} 1, 2, 3, 5, 8".to_string(),
            ),
            (
                "linear".to_string(),
                "Linear \u{2014} 1, 2, 3, 4, 5".to_string(),
            ),
            (
                "t_shirt".to_string(),
                "T-Shirt \u{2014} XS, S, M, L, XL".to_string(),
            ),
        ]
    });

    // ── Modal footer ────────────────────────────────────────────────────
    let footer: ChildrenFn = std::sync::Arc::new(move || {
        view! {
            <Button
                variant=ButtonVariant::Secondary
                on:click=move |_| on_close.run(())
            >
                "Cancel"
            </Button>
            <Button
                variant=ButtonVariant::Default
                disabled=Signal::derive(move || !can_submit.get())
                on:click=on_submit.clone()
            >
                {move || if submitting.get() { "Creating..." } else { "Create Team" }}
            </Button>
        }
        .into_any()
    });

    // ── Render ───────────────────────────────────────────────────────────
    view! {
        <Modal
            show=show
            on_close=on_close
            title="Create Team"
            size=ModalSize::Md
            footer=footer
        >
            <div class="flex flex-col gap-4">
                // Icon + Name row
                <div class="flex items-start gap-3">
                    <div class="flex flex-col items-center gap-1 pt-[22px]">
                        {icon_preview}
                    </div>
                    <div class="flex-1 flex flex-col gap-3">
                        // Team name
                        <div>
                            <label class="block text-sm font-medium text-foreground mb-1">
                                "Team name"
                            </label>
                            <input
                                type="text"
                                class=INPUT_CLASS
                                placeholder="e.g. Engineering"
                                maxlength="50"
                                prop:value=move || name.get()
                                on:input=on_name_input
                                prop:disabled=move || submitting.get()
                            />
                        </div>

                        // Team key
                        <div>
                            <label class="block text-sm font-medium text-foreground mb-1">
                                "Team key"
                            </label>
                            <input
                                type="text"
                                class=INPUT_CLASS
                                placeholder="e.g. ENG"
                                prop:value=move || key.get()
                                on:input=on_key_input
                                prop:disabled=move || submitting.get()
                                maxlength="5"
                            />
                            <Show when=move || !key_format_ok.get()>
                                <p class="mt-1 text-xs text-error-foreground">
                                    "2-5 uppercase letters required"
                                </p>
                            </Show>
                        </div>
                    </div>
                </div>

                // Icon picker (shown when icon preview is clicked)
                {icon_picker}

                // Error message
                {move || error.get().map(|e| view! {
                    <p class="text-sm text-error-foreground">{e}</p>
                })}

                // Advanced settings (collapsible)
                <div class="border-t border-border pt-3">
                    <button
                        type="button"
                        class="flex items-center gap-1.5 text-sm text-muted-foreground hover:text-foreground transition-colors duration-200 w-full focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring rounded"
                        on:click=move |_| set_advanced_open.update(|v| *v = !*v)
                    >
                        <Icon
                            icon=phosphor_leptos::CARET_RIGHT
                            size="14px"
                            attr:class=move || {
                                if advanced_open.get() {
                                    "transition-transform duration-200 rotate-90"
                                } else {
                                    "transition-transform duration-200"
                                }
                            }
                        />
                        "Advanced Settings"
                    </button>

                    <Show when=move || advanced_open.get()>
                        <div class="flex flex-col gap-4 mt-3 pl-1">
                            // Estimation scale
                            <div>
                                <label class="block text-sm font-medium text-foreground mb-1">
                                    "Estimation scale"
                                </label>
                                <Select
                                    value=Signal::derive(move || estimate_scale.get())
                                    options=estimate_options
                                    on_change=Callback::new(move |v: String| set_estimate_scale.set(v))
                                    variant=SelectVariant::Form
                                    placeholder="Disabled".to_string()
                                />
                            </div>

                            // Toggle switches
                            <div class="flex flex-col gap-3">
                                <Switch
                                    checked=Signal::derive(move || allow_zero.get())
                                    on_change=Callback::new(move |v: bool| set_allow_zero.set(v))
                                    label="Allow zero estimates".to_string()
                                />
                                <Switch
                                    checked=Signal::derive(move || extended_scale.get())
                                    on_change=Callback::new(move |v: bool| set_extended_scale.set(v))
                                    label="Extended estimate scale".to_string()
                                />
                                <Switch
                                    checked=Signal::derive(move || count_unestimated.get())
                                    on_change=Callback::new(move |v: bool| set_count_unestimated.set(v))
                                    label="Count unestimated issues".to_string()
                                />
                            </div>
                        </div>
                    </Show>
                </div>
            </div>
        </Modal>
    }
}
