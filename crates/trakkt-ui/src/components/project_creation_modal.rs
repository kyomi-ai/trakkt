// SPDX-License-Identifier: AGPL-3.0-or-later

//! Project creation modal — unified component for creating new projects
//! from both the sidebar "+" button and the projects list page.
//!
//! Layout:
//! 1. Icon preview (clickable color picker) + Project name
//! 2. Description (optional textarea)
//! 3. Status, Lead, Start date, Target date
//! 4. Cancel / Create Project footer

use leptos::prelude::*;
use phosphor_leptos::Icon;

use crate::cache::store::SyncStore;
use crate::components::button::{Button, ButtonVariant};
use crate::components::dropdown::{Select, SelectVariant};
use crate::components::input::INPUT_CLASS;
use crate::components::modal::{Modal, ModalSize};
use crate::components::team_icon::{
    get_icon, DEFAULT_ICON_COLOR, DEFAULT_ICON_NAME, ICON_CATEGORIES, ICON_COLORS,
};
use crate::utils::project::status_label;

// ─── Component ──────────────────────────────────────────────────────────────

/// Modal for creating a new project.
///
/// Provides name, description, status, lead picker, date fields, and
/// icon/color picker. On successful creation, upserts the new project
/// into the SyncStore and navigates to its detail page.
#[component]
pub fn ProjectCreationModal(
    /// Whether the modal is visible.
    #[prop(into)]
    show: Signal<bool>,
    /// Called when the modal should close (cancel, backdrop, or success).
    on_close: Callback<()>,
) -> impl IntoView {
    // ── Form state ──────────────────────────────────────────────────────
    let (name, set_name) = signal(String::new());
    let (description, set_description) = signal(String::new());
    let (status, set_status) = signal(String::new());
    let (lead_id, set_lead_id) = signal(String::new());
    let (start_date, set_start_date) = signal(String::new());
    let (target_date, set_target_date) = signal(String::new());
    let (icon_color, set_icon_color) = signal(DEFAULT_ICON_COLOR.to_string());
    let (icon_name, set_icon_name) = signal(DEFAULT_ICON_NAME.to_string());
    let (show_icon_picker, set_show_icon_picker) = signal(false);

    // Submission state
    let (submitting, set_submitting) = signal(false);

    // ── Reset on open ───────────────────────────────────────────────────
    Effect::new(move || {
        if show.get() {
            set_name.set(String::new());
            set_description.set(String::new());
            set_status.set(String::new());
            set_lead_id.set(String::new());
            set_start_date.set(String::new());
            set_target_date.set(String::new());
            set_icon_color.set(DEFAULT_ICON_COLOR.to_string());
            set_icon_name.set(DEFAULT_ICON_NAME.to_string());
            set_show_icon_picker.set(false);
            set_submitting.set(false);
        }
    });

    // ── Validation ──────────────────────────────────────────────────────
    let can_submit = Memo::new(move |_| {
        !name.get().trim().is_empty() && !submitting.get()
    });

    // ── Members for lead picker ─────────────────────────────────────────
    let members = LocalResource::new(move || async move {
        match crate::server_fns::team::list_workspace_members().await {
            Ok(list) => list,
            Err(e) => {
                tracing::warn!("Failed to load workspace members for project lead picker: {e}");
                vec![]
            }
        }
    });

    let lead_options = Signal::derive(move || {
        let mut opts = vec![("".to_string(), "No lead".to_string())];
        if let Some(list) = members.get() {
            for m in list.iter() {
                let label = m.name.clone().unwrap_or_else(|| m.email.clone());
                opts.push((m.user_id.clone(), label));
            }
        }
        opts
    });

    // ── Status options ──────────────────────────────────────────────────
    let status_options = Signal::derive(move || {
        vec![
            ("".to_string(), "Default (Planned)".to_string()),
            ("planned".to_string(), status_label("planned")),
            ("in_progress".to_string(), status_label("in_progress")),
            ("paused".to_string(), status_label("paused")),
            ("completed".to_string(), status_label("completed")),
            ("cancelled".to_string(), status_label("cancelled")),
        ]
    });

    // ── Submit handler ──────────────────────────────────────────────────
    let store = use_context::<SyncStore>();
    let nav = leptos_router::hooks::use_navigate();
    let show_error = crate::components::toast::capture_error_toast();

    let on_submit = move |_| {
        let name_val = name.get_untracked().trim().to_string();
        if name_val.is_empty() {
            return;
        }

        let desc_val = {
            let d = description.get_untracked().trim().to_string();
            if d.is_empty() { None } else { Some(d) }
        };
        let icon_val = {
            let n = icon_name.get_untracked();
            if n == DEFAULT_ICON_NAME { None } else { Some(n) }
        };
        let color_val = {
            let c = icon_color.get_untracked();
            if c == DEFAULT_ICON_COLOR { None } else { Some(c) }
        };
        let lead_val = {
            let l = lead_id.get_untracked();
            if l.is_empty() { None } else { Some(l) }
        };
        let start_val = {
            let s = start_date.get_untracked();
            if s.is_empty() { None } else { Some(s) }
        };
        let target_val = {
            let t = target_date.get_untracked();
            if t.is_empty() { None } else { Some(t) }
        };
        let status_val = status.get_untracked();

        set_submitting.set(true);

        // Close the modal immediately. Signal updates during event handlers
        // are batched — the reactive flush (which disposes the modal's scope)
        // happens after this handler returns.
        on_close.run(());

        let nav = nav.clone();
        let show_error = show_error.clone();

        leptos::task::spawn_local(async move {
            let project = match crate::server_fns::projects::create_project(
                name_val,
                desc_val,
                icon_val,
                color_val,
                lead_val,
                start_val,
                target_val,
            )
            .await
            {
                Ok(project) => project,
                Err(e) => {
                    set_submitting.set(false);
                    show_error(format!("Failed to create project: {e}"));
                    return;
                }
            };

            let project_id = project.project_id.clone();

            // create_project does not accept a status parameter.
            // If the user selected a non-default status, update it now.
            if !status_val.is_empty() && status_val != "planned" {
                let updated = crate::server_fns::projects::update_project(
                    project_id.clone(),
                    None,
                    None,
                    None,
                    None,
                    Some(status_val),
                    None,
                    None,
                    None,
                )
                .await;

                match updated {
                    Ok(updated_project) => {
                        if let Some(store) = store {
                            store.upsert_project(updated_project);
                        }
                    }
                    Err(e) => {
                        tracing::warn!(error = %e, "Failed to update project status after creation");
                        // Still upsert the original project so it appears in the sidebar.
                        if let Some(store) = store {
                            store.upsert_project(project);
                        }
                    }
                }
            } else {
                if let Some(store) = store {
                    store.upsert_project(project);
                }
            }

            let href = format!("/projects/{project_id}");
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
                on:click=move |_| set_show_icon_picker.update(|v| *v = !*v)
            >
                {icon_data.map(|data| view! {
                    <Icon icon=data size="23px" color="white"/>
                })}
            </button>
        }
    };

    // ── Icon/color picker grid ──────────────────────────────────────────
    let icon_picker = move || {
        view! {
            <Show when=move || show_icon_picker.get()>
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
                {move || if submitting.get() { "Creating..." } else { "Create Project" }}
            </Button>
        }
        .into_any()
    });

    // ── Render ───────────────────────────────────────────────────────────
    view! {
        <Modal
            show=show
            on_close=on_close
            title="Create Project"
            size=ModalSize::Md
            footer=footer
        >
            <div class="flex flex-col gap-4">
                // Icon + Name row
                <div class="flex items-start gap-3">
                    <div class="flex flex-col items-center gap-1 pt-[22px]">
                        {icon_preview}
                    </div>
                    <div class="flex-1">
                        <label class="block text-sm font-medium text-foreground mb-1">
                            "Project name"
                        </label>
                        <input
                            type="text"
                            class=INPUT_CLASS
                            placeholder="e.g. Q3 Launch"
                            maxlength="100"
                            prop:value=move || name.get()
                            on:input=move |ev| set_name.set(event_target_value(&ev))
                            prop:disabled=move || submitting.get()
                        />
                    </div>
                </div>

                // Icon picker (shown when icon preview is clicked)
                {icon_picker}

                // Description
                <div>
                    <label class="block text-sm font-medium text-foreground mb-1">
                        "Description"
                    </label>
                    <textarea
                        class=format!("{INPUT_CLASS} min-h-[72px] resize-y")
                        placeholder="What is this project about?"
                        prop:value=move || description.get()
                        on:input=move |ev| set_description.set(event_target_value(&ev))
                        prop:disabled=move || submitting.get()
                    />
                </div>

                // Status + Lead row
                <div class="grid grid-cols-2 gap-3">
                    <div>
                        <label class="block text-sm font-medium text-foreground mb-1">
                            "Status"
                        </label>
                        <Select
                            value=Signal::derive(move || status.get())
                            options=status_options
                            on_change=Callback::new(move |v: String| set_status.set(v))
                            variant=SelectVariant::Form
                            placeholder="Planned".to_string()
                        />
                    </div>
                    <div>
                        <label class="block text-sm font-medium text-foreground mb-1">
                            "Lead"
                        </label>
                        <Select
                            value=Signal::derive(move || lead_id.get())
                            options=lead_options
                            on_change=Callback::new(move |v: String| set_lead_id.set(v))
                            variant=SelectVariant::Form
                            placeholder="No lead".to_string()
                        />
                    </div>
                </div>

                // Date row
                <div class="grid grid-cols-2 gap-3">
                    <div>
                        <label class="block text-sm font-medium text-foreground mb-1">
                            "Start date"
                        </label>
                        <input
                            type="date"
                            class=INPUT_CLASS
                            prop:value=move || start_date.get()
                            on:input=move |ev| set_start_date.set(event_target_value(&ev))
                            prop:disabled=move || submitting.get()
                        />
                    </div>
                    <div>
                        <label class="block text-sm font-medium text-foreground mb-1">
                            "Target date"
                        </label>
                        <input
                            type="date"
                            class=INPUT_CLASS
                            prop:value=move || target_date.get()
                            on:input=move |ev| set_target_date.set(event_target_value(&ev))
                            prop:disabled=move || submitting.get()
                        />
                    </div>
                </div>

            </div>
        </Modal>
    }
}
