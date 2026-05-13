// SPDX-License-Identifier: AGPL-3.0-or-later

//! Labels management page — workspace-level label CRUD.
//!
//! Provides a list of existing labels (color swatch + name + edit/delete),
//! an "Add Label" form with a preset color picker + custom hex input,
//! and edit/delete flows using Modal and ConfirmDialog.

use std::sync::Arc;

use leptos::prelude::*;

use crate::components::{
    Alert, AlertDescription, AlertVariant, Button, ButtonSize, ButtonVariant, Card, CardContent,
    CardHeader, CardTitle, CardDescription, ConfirmDialog, EmptyState, Modal, ModalSize, Skeleton,
    INPUT_CLASS,
};
use crate::server_fns::labels::*;
use trakkt_types::models::Label;

// ─── Preset colors ────────────────────────────────────────────────────────

const PRESET_COLORS: &[(&str, &str)] = &[
    ("#DC2626", "Red"),
    ("#EA580C", "Orange"),
    ("#CA8A04", "Yellow"),
    ("#15803D", "Green"),
    ("#0D9488", "Teal"),
    ("#2563EB", "Blue"),
    ("#7C3AED", "Violet"),
    ("#DB2777", "Pink"),
    ("#78716C", "Gray"),
    ("#1C1917", "Black"),
    ("#44403C", "Dark gray"),
    ("#A8A29E", "Light gray"),
];

// ─── Color picker sub-component ───────────────────────────────────────────

/// A grid of preset color circles plus a custom hex input.
#[component]
fn ColorPicker(
    /// The currently selected color hex (e.g. "#DC2626").
    #[prop(into)]
    value: Signal<String>,
    /// Called when the user picks a preset or enters a valid custom hex.
    on_change: Callback<String>,
) -> impl IntoView {
    let (custom_hex, set_custom_hex) = signal(String::new());

    let on_custom_input = move |ev: leptos::ev::Event| {
        let raw = event_target_value(&ev);
        set_custom_hex.set(raw.clone());
        // Accept 4-char (#RGB) or 7-char (#RRGGBB) hex
        let trimmed = raw.trim();
        if (trimmed.len() == 4 || trimmed.len() == 7) && trimmed.starts_with('#') {
            let hex_part = &trimmed[1..];
            if hex_part.chars().all(|c| c.is_ascii_hexdigit()) {
                on_change.run(trimmed.to_string());
            }
        }
    };

    view! {
        <div class="space-y-3">
            // Preset color grid
            <div class="flex flex-wrap gap-2">
                {PRESET_COLORS.iter().map(|(hex, label)| {
                    let hex = *hex;
                    let label = *label;
                    let is_selected = move || value.get() == hex;
                    view! {
                        <button
                            type="button"
                            class="w-7 h-7 rounded-full transition-all duration-200 focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
                            class:ring-2=is_selected
                            class:ring-offset-2=is_selected
                            class:ring-primary=is_selected
                            style=format!("background-color: {hex}")
                            title=label
                            on:click=move |_| {
                                set_custom_hex.set(String::new());
                                on_change.run(hex.to_string());
                            }
                        />
                    }
                }).collect_view()}
            </div>
            // Custom hex input
            <div class="flex items-center gap-2">
                <div
                    class="w-7 h-7 rounded-full border border-border flex-shrink-0"
                    style=move || format!("background-color: {}", value.get())
                />
                <input
                    type="text"
                    class=INPUT_CLASS
                    placeholder="#HEX"
                    maxlength="7"
                    prop:value=custom_hex
                    on:input=on_custom_input
                />
            </div>
        </div>
    }
}

// ─── Main page ────────────────────────────────────────────────────────────

#[component]
pub fn LabelsPage() -> impl IntoView {
    // Data fetching with version-based refresh
    let (version, set_version) = signal(0u32);
    let labels = Resource::new(move || version.get(), |_| list_labels(None));

    // Create label form state
    let (new_name, set_new_name) = signal(String::new());
    let (new_color, set_new_color) = signal("#0D9488".to_string());
    let (create_error, set_create_error) = signal(Option::<String>::None);

    // Edit modal state
    let (show_edit_modal, set_show_edit_modal) = signal(false);
    let (edit_label_id, set_edit_label_id) = signal(String::new());
    let (edit_name, set_edit_name) = signal(String::new());
    let (edit_color, set_edit_color) = signal(String::new());
    let (edit_error, set_edit_error) = signal(Option::<String>::None);

    // Delete confirm dialog state
    let dialog_open = RwSignal::new(false);
    let (delete_label_id, set_delete_label_id) = signal(String::new());
    let (delete_label_name, set_delete_label_name) = signal(String::new());

    // Actions
    let create_action = Action::new(move |(name, color): &(String, String)| {
        let name = name.clone();
        let color = color.clone();
        async move { create_label(name, color, None).await }
    });

    let update_action = Action::new(move |(id, name, color): &(String, String, String)| {
        let id = id.clone();
        let name = name.clone();
        let color = color.clone();
        async move { update_label(id, name, color).await }
    });

    let delete_action = Action::new(move |id: &String| {
        let id = id.clone();
        async move { delete_label(id).await }
    });

    // React to action completions
    Effect::new(move || {
        if let Some(result) = create_action.value().get() {
            match result {
                Ok(_) => {
                    set_new_name.set(String::new());
                    set_new_color.set("#0D9488".to_string());
                    set_create_error.set(None);
                    set_version.update(|v| *v += 1);
                }
                Err(e) => {
                    set_create_error.set(Some(e.to_string()));
                }
            }
        }
    });

    Effect::new(move || {
        if let Some(result) = update_action.value().get() {
            match result {
                Ok(_) => {
                    set_show_edit_modal.set(false);
                    set_edit_error.set(None);
                    set_version.update(|v| *v += 1);
                }
                Err(e) => {
                    set_edit_error.set(Some(e.to_string()));
                }
            }
        }
    });

    Effect::new(move || {
        if let Some(result) = delete_action.value().get()
            && result.is_ok()
        {
            set_version.update(|v| *v += 1);
        }
    });

    // Helpers
    let open_edit = move |label: &Label| {
        set_edit_label_id.set(label.label_id.clone());
        set_edit_name.set(label.name.clone());
        set_edit_color.set(label.color.clone());
        set_edit_error.set(None);
        set_show_edit_modal.set(true);
    };

    let request_delete = move |label: &Label| {
        set_delete_label_id.set(label.label_id.clone());
        set_delete_label_name.set(label.name.clone());
        dialog_open.set(true);
    };

    // Confirm dialog callbacks
    let on_confirm_delete = Callback::new(move |()| {
        dialog_open.set(false);
        let id = delete_label_id.get_untracked();
        if !id.is_empty() {
            delete_action.dispatch(id);
        }
    });

    let on_cancel_delete = Callback::new(move |()| {
        dialog_open.set(false);
    });

    // Edit modal callbacks
    let on_close_edit = Callback::new(move |()| {
        set_show_edit_modal.set(false);
        set_edit_error.set(None);
    });

    let edit_modal_footer: Arc<dyn Fn() -> AnyView + Send + Sync> = Arc::new(move || {
        let name_empty = edit_name.get().trim().is_empty();
        view! {
            <Button
                variant=ButtonVariant::Outline
                on:click=move |_| set_show_edit_modal.set(false)
            >
                "Cancel"
            </Button>
            <Button
                variant=ButtonVariant::Default
                disabled=name_empty
                on:click=move |_| {
                    let id = edit_label_id.get_untracked();
                    let name = edit_name.get_untracked();
                    let color = edit_color.get_untracked();
                    if !name.trim().is_empty() && !id.is_empty() {
                        update_action.dispatch((id, name, color));
                    }
                }
            >
                "Save"
            </Button>
        }.into_any()
    });

    view! {
        <div class="p-4 sm:p-6">
            <h2 class="text-xl font-display text-foreground mb-4">"Labels"</h2>
            <p class="text-muted-foreground mb-6">
                "Manage labels for organizing issues."
            </p>

            <Card>
                <CardHeader>
                    <CardTitle>"Labels"</CardTitle>
                    <CardDescription>"Manage labels for organizing issues"</CardDescription>
                </CardHeader>
                <CardContent>
                    // Label list
                    <Transition fallback=move || view! {
                        <div class="space-y-3">
                            <Skeleton class="h-10 w-full"/>
                            <Skeleton class="h-10 w-full"/>
                            <Skeleton class="h-10 w-full"/>
                        </div>
                    }>
                        {move || Suspend::new(async move {
                            match labels.await {
                                Ok(mut label_list) => {
                                    // list_labels returns ORDER BY name ASC from the service layer
                                    if label_list.is_empty() {
                                        view! {
                                            <EmptyState
                                                title="No labels yet"
                                                description="Create your first label to start organizing issues"
                                                class="mb-6"
                                            />
                                        }.into_any()
                                    } else {
                                        let rows = label_list.into_iter().map(|label| {
                                            let label_for_edit = label.clone();
                                            let label_for_delete = label.clone();
                                            view! {
                                                <div class="flex items-center gap-3 py-2.5 px-1 group border-b border-border last:border-b-0 hover:bg-muted/50 transition-colors rounded-sm">
                                                    // Color swatch
                                                    <span
                                                        class="w-4 h-4 rounded-full flex-shrink-0"
                                                        style=format!("background-color: {}", label.color)
                                                    />
                                                    // Name + scope indicator
                                                    <span class="text-sm font-medium text-foreground flex-1 min-w-0 truncate">
                                                        {label.name}
                                                    </span>
                                                    <span class="text-xs text-muted-foreground shrink-0">
                                                        {if label.team_id.is_some() { "team" } else { "workspace" }}
                                                    </span>
                                                    // Actions
                                                    <div class="flex items-center gap-1 opacity-0 group-hover:opacity-100 transition-opacity">
                                                        <Button
                                                            variant=ButtonVariant::GhostMuted
                                                            size=ButtonSize::IconSm
                                                            aria_label="Edit label"
                                                            on:click=move |_| open_edit(&label_for_edit)
                                                        >
                                                            <phosphor_leptos::Icon icon=phosphor_leptos::PENCIL_SIMPLE size="16px"/>
                                                        </Button>
                                                        <Button
                                                            variant=ButtonVariant::GhostDestructive
                                                            size=ButtonSize::IconSm
                                                            aria_label="Delete label"
                                                            on:click=move |_| request_delete(&label_for_delete)
                                                        >
                                                            <phosphor_leptos::Icon icon=phosphor_leptos::TRASH size="16px"/>
                                                        </Button>
                                                    </div>
                                                </div>
                                            }
                                        }).collect_view();

                                        view! {
                                            <div class="mb-6">
                                                {rows}
                                            </div>
                                        }.into_any()
                                    }
                                },
                                Err(e) => {
                                    let msg = e.to_string();
                                    view! {
                                        <Alert variant=AlertVariant::Error class="mb-6">
                                            <AlertDescription>{msg}</AlertDescription>
                                        </Alert>
                                    }.into_any()
                                },
                            }
                        })}
                    </Transition>

                    // Add label form
                    <div class="border-t border-border pt-4 mt-2">
                        <h3 class="text-sm font-semibold text-foreground mb-3">"Add Label"</h3>
                        <div class="space-y-3">
                            <div>
                                <label class="block text-sm font-medium text-foreground mb-1">
                                    "Name"
                                </label>
                                <input
                                    type="text"
                                    class=INPUT_CLASS
                                    placeholder="e.g. bug, feature, enhancement"
                                    prop:value=new_name
                                    on:input=move |ev| set_new_name.set(event_target_value(&ev))
                                />
                            </div>
                            <div>
                                <label class="block text-sm font-medium text-foreground mb-1">
                                    "Color"
                                </label>
                                <ColorPicker
                                    value=Signal::derive(move || new_color.get())
                                    on_change=Callback::new(move |c: String| set_new_color.set(c))
                                />
                            </div>

                            // Create error
                            {move || create_error.get().map(|e| view! {
                                <Alert variant=AlertVariant::Error>
                                    <AlertDescription>{e}</AlertDescription>
                                </Alert>
                            })}

                            <div class="flex justify-end">
                                <Button
                                    variant=ButtonVariant::Default
                                    size=ButtonSize::Sm
                                    disabled=MaybeProp::from(Signal::derive(move || new_name.get().trim().is_empty()))
                                    on:click=move |_| {
                                        let name = new_name.get_untracked();
                                        let color = new_color.get_untracked();
                                        if !name.trim().is_empty() {
                                            create_action.dispatch((name, color));
                                        }
                                    }
                                >
                                    "Add"
                                </Button>
                            </div>
                        </div>
                    </div>
                </CardContent>
            </Card>

            // Edit Label Modal
            <Modal
                show=Signal::from(show_edit_modal)
                on_close=on_close_edit
                title="Edit Label"
                size=ModalSize::Sm
                footer=edit_modal_footer
            >
                <div class="space-y-4">
                    <div>
                        <label class="block text-sm font-medium text-foreground mb-1">
                            "Name"
                        </label>
                        <input
                            type="text"
                            class=INPUT_CLASS
                            placeholder="Label name"
                            prop:value=edit_name
                            on:input=move |ev| set_edit_name.set(event_target_value(&ev))
                        />
                    </div>
                    <div>
                        <label class="block text-sm font-medium text-foreground mb-1">
                            "Color"
                        </label>
                        <ColorPicker
                            value=Signal::derive(move || edit_color.get())
                            on_change=Callback::new(move |c: String| set_edit_color.set(c))
                        />
                    </div>

                    // Edit error
                    {move || edit_error.get().map(|e| view! {
                        <Alert variant=AlertVariant::Error>
                            <AlertDescription>{e}</AlertDescription>
                        </Alert>
                    })}
                </div>
            </Modal>

            // Delete Confirm Dialog
            <ConfirmDialog
                open=Signal::from(dialog_open)
                title=Signal::derive(move || format!("Delete label '{}'?", delete_label_name.get()))
                message="This will remove the label from all issues."
                confirm_text="Delete"
                on_confirm=on_confirm_delete
                on_cancel=on_cancel_delete
            />
        </div>
    }
}
