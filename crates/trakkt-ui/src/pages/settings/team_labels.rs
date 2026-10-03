// SPDX-License-Identifier: AGPL-3.0-or-later

//! Team-scoped labels management page — CRUD for labels scoped to a specific team.
//!
//! Mirrors the workspace-level `LabelsPage` but operates on team-scoped labels.
//! Also shows a read-only reference section for workspace-level labels so users
//! know what's already available globally.

use std::sync::Arc;

use leptos::prelude::*;

use crate::components::{
    Alert, AlertDescription, AlertVariant, Button, ButtonSize, ButtonVariant, Card, CardContent,
    CardDescription, CardHeader, CardTitle, ConfirmDialog, EmptyState, Modal, ModalSize, Skeleton,
    INPUT_CLASS,
};
use crate::pages::settings::labels::ColorPicker;
use crate::server_fns::labels::*;
use trakkt_types::models::Label;

// ─── Main page ────────────────────────────────────────────────────────────

#[component]
pub fn TeamLabelsPage(
    /// The team ID this settings page manages labels for.
    #[prop(into)]
    team_id: String,
) -> impl IntoView {
    let team_id_for_fetch = team_id.clone();
    let team_id_for_create = team_id.clone();

    // Data fetching with version-based refresh
    let (version, set_version) = signal(0u32);
    let labels = Resource::new(
        move || version.get(),
        {
            let tid = team_id_for_fetch.clone();
            move |_| {
                let tid = tid.clone();
                async move { list_labels(Some(tid)).await }
            }
        },
    );

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
    let (delete_error, set_delete_error) = signal(Option::<String>::None);

    // Actions
    let create_action = Action::new({
        let tid = team_id_for_create.clone();
        move |(name, color): &(String, String)| {
            let name = name.clone();
            let color = color.clone();
            let tid = tid.clone();
            async move { create_label(name, color, Some(tid)).await }
        }
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
        if let Some(result) = delete_action.value().get() {
            match result {
                Ok(_) => {
                    set_delete_error.set(None);
                    set_version.update(|v| *v += 1);
                }
                Err(e) => {
                    set_delete_error.set(Some(format!("Failed to delete label: {e}")));
                }
            }
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
        }
        .into_any()
    });

    view! {
        <Card>
            <CardHeader>
                <CardTitle>"Labels"</CardTitle>
                <CardDescription>"Manage labels specific to this team"</CardDescription>
            </CardHeader>
            <CardContent>
                // Delete error
                {move || delete_error.get().map(|e| view! {
                    <Alert variant=AlertVariant::Error class="mb-4">
                        <AlertDescription>{e}</AlertDescription>
                    </Alert>
                })}

                // Team-scoped label list
                <Transition fallback=move || view! {
                    <div class="space-y-3">
                        <Skeleton class="h-10 w-full"/>
                        <Skeleton class="h-10 w-full"/>
                        <Skeleton class="h-10 w-full"/>
                    </div>
                }>
                    {move || Suspend::new(async move {
                        match labels.await {
                            Ok(all_labels) => {
                                // Split into team-scoped (editable) and workspace (read-only)
                                let team_labels: Vec<Label> = all_labels.iter()
                                    .filter(|l| l.team_id.is_some())
                                    .cloned()
                                    .collect();
                                let workspace_labels: Vec<Label> = all_labels.into_iter()
                                    .filter(|l| l.team_id.is_none())
                                    .collect();

                                let team_section = if team_labels.is_empty() {
                                    view! {
                                        <EmptyState
                                            title="No team labels yet"
                                            description="Create your first team-specific label below"
                                            class="mb-6"
                                        />
                                    }.into_any()
                                } else {
                                    let rows = team_labels.into_iter().map(|label| {
                                        let label_for_edit = label.clone();
                                        let label_for_delete = label.clone();
                                        view! {
                                            <div class="flex items-center gap-3 py-2.5 px-1 group border-b border-border last:border-b-0 hover:bg-muted/50 transition-colors rounded-sm">
                                                // Color swatch
                                                <span
                                                    class="w-4 h-4 rounded-full flex-shrink-0"
                                                    style=format!("background-color: {}", label.color)
                                                />
                                                // Name
                                                <span class="text-sm font-medium text-foreground flex-1 min-w-0 truncate">
                                                    {label.name}
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
                                };

                                // Workspace labels read-only reference section
                                let workspace_section = if workspace_labels.is_empty() {
                                    view! { <div/> }.into_any()
                                } else {
                                    let ws_rows = workspace_labels.into_iter().map(|label| {
                                        view! {
                                            <div class="flex items-center gap-3 py-2 px-1 border-b border-border last:border-b-0">
                                                <span
                                                    class="w-4 h-4 rounded-full flex-shrink-0"
                                                    style=format!("background-color: {}", label.color)
                                                />
                                                <span class="text-sm text-muted-foreground flex-1 min-w-0 truncate">
                                                    {label.name}
                                                </span>
                                            </div>
                                        }
                                    }).collect_view();

                                    view! {
                                        <div class="border-t border-border pt-4 mt-4">
                                            <h3 class="text-sm font-semibold text-muted-foreground mb-3">"Workspace labels (shared)"</h3>
                                            <div>{ws_rows}</div>
                                        </div>
                                    }.into_any()
                                };

                                view! {
                                    <div>
                                        {team_section}
                                        {workspace_section}
                                    </div>
                                }.into_any()
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

                // Add team label form
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
    }
}
