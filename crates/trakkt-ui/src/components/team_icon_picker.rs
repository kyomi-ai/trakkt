// SPDX-License-Identifier: AGPL-3.0-or-later

//! Team icon picker — lets users choose a preset icon + colour for a team.
//!
//! Layout (Linear-inspired):
//! 1. Large preview of the current team icon (48px)
//! 2. Colour palette — 2 rows of 8 swatches
//! 3. Icon grid — grouped by category with small labels
//! 4. Custom icon upload (file input)
//! 5. "Remove icon" clear button at the bottom

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
///
/// # Why the selection lives at the caller
///
/// `selected_name` and `selected_color` are what the picker paints from, and
/// they move the moment the user clicks — before the server has been asked. So
/// they are the values a rejected save has to put back, and the caller is the
/// only place that can: this component renders inside a `<Popover>`, whose
/// children are behind a `<Show>` and are therefore built afresh on every open
/// and disposed on every close. State owned here is disposed with them, so a
/// save still on the wire when the popover closes would come back to nothing
/// to revert. `ToggleState` in `pages/settings/notifications.rs` records the
/// same rule for the same reason.
///
/// The consequence for everything below: no closure in this component's view
/// may read an arena item created *by* this component — no `Memo`, no
/// `Signal::derive`, no `StoredValue` — over the selection. A disposed render
/// effect stays subscribed to the caller's signals and is still woken when the
/// revert writes to them; if it then reads a wrapper that went with the popover
/// it panics on a disposed reactive value, which is exactly the fault #282
/// shipped. Read `selected_name` / `selected_color` directly and clone plain
/// values into the closure instead.
#[component]
pub fn TeamIconPicker(
    /// The team being edited. Used to show the current icon state.
    team: Team,
    /// The preset icon name the picker is showing, optimistically. Must be
    /// created by the caller, above the popover — see the note above.
    selected_name: RwSignal<Option<String>>,
    /// The preset colour the picker is showing, optimistically. Same ownership
    /// rule as `selected_name`.
    selected_color: RwSignal<Option<String>>,
    /// Callback fired on every selection change.
    /// Arguments: `(Option<icon_type>, Option<icon_name>, Option<icon_color>)`.
    on_change: Callback<(Option<String>, Option<String>, Option<String>)>,
) -> impl IntoView {
    // ── Handlers ──────────────────────────────────────────────────────────

    let on_color_click = move |color: &'static str| {
        let new_color = Some(color.to_string());
        // Default to "rocket" if no icon chosen yet.
        let name = selected_name.get_untracked()
            .or_else(|| Some(DEFAULT_ICON_NAME.to_string()));
        selected_color.set(new_color.clone());
        selected_name.set(name.clone());
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
        selected_name.set(new_name.clone());
        selected_color.set(color.clone());
        on_change.run((
            Some("preset".to_string()),
            new_name,
            color,
        ));
    };

    let on_clear = move |_| {
        selected_name.set(None);
        selected_color.set(None);
        on_change.run((None, None, None));
    };

    // Upload error message signal
    let (upload_error, set_upload_error) = signal(Option::<String>::None);

    // File upload handler
    let team_id_for_upload = team.team_id.clone();
    let on_file_change = move |ev: leptos::ev::Event| {
        use wasm_bindgen::JsCast;

        let target = ev.target();
        let input: Option<web_sys::HtmlInputElement> = target
            .and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok());
        let Some(input) = input else { return };
        let Some(files) = input.files() else { return };
        let Some(file) = files.get(0) else { return };

        // Client-side size validation
        if file.size() as usize > 51_200 {
            set_upload_error.set(Some("File too large (max 50KB)".to_string()));
            // Reset the input so the same file can be re-selected
            input.set_value("");
            return;
        }

        set_upload_error.set(None);

        let team_id = team_id_for_upload.clone();
        let set_upload_error = set_upload_error;

        leptos::task::spawn_local(async move {
            let result = upload_team_icon_file(&team_id, &file).await;
            match result {
                Ok(()) => {
                    // Clear the preset selection — the Axum handler already
                    // persisted the upload, so do NOT call on_change here
                    // (that would trigger update_team_icon which wipes icon_data).
                    selected_name.set(None);
                    selected_color.set(None);
                }
                Err(msg) => {
                    set_upload_error.set(Some(msg));
                }
            }
        });

        // Reset the input so the same file can be re-selected
        input.set_value("");
    };

    // ── Render ─────────────────────────────────────────────────────────────

    view! {
        <div class="flex flex-col gap-4 max-h-[400px] overflow-y-auto">
            // Preview — the team with the current selection applied.
            //
            // Composed inline from a cloned `Team` rather than through a `Memo`
            // so that this render effect reads nothing this component owns:
            // once the popover closes it is disposed but still subscribed to
            // the caller's signals, and a revert arriving then would wake it
            // into a disposed `Memo`. See this component's docs.
            <div class="flex items-center justify-center py-2">
                {
                    let base = team.clone();
                    move || {
                        let mut t = base.clone();
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
                        view! { <TeamIcon team=t size="48px"/> }
                    }
                }
            </div>

            // Colour palette
            <div class="flex flex-col gap-1.5">
                <span class="text-xs text-muted-foreground font-medium">"Color"</span>
                <div class="grid grid-cols-8 gap-1.5">
                    {ICON_COLORS.iter().map(|&color| {
                        view! {
                            <button
                                type="button"
                                class=move || {
                                    let base = "w-7 h-7 rounded-md transition-all duration-200 focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring";
                                    if selected_color.get().as_deref() == Some(color) {
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
                                    view! {
                                        <button
                                            type="button"
                                            class=move || {
                                                let base = "w-8 h-8 rounded-md flex items-center justify-center transition-colors duration-200 focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring";
                                                if selected_name.get().as_deref() == Some(name) {
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

            // Custom upload section
            <div class="border-t border-border pt-2">
                <label class="flex items-center justify-center gap-2 px-3 py-2 text-sm text-muted-foreground hover:text-foreground hover:bg-muted rounded-md cursor-pointer transition-colors duration-200">
                    <Icon icon=phosphor_leptos::UPLOAD_SIMPLE size="16px"/>
                    "Upload custom icon"
                    <input
                        type="file"
                        accept="image/svg+xml,image/png,image/jpeg"
                        class="hidden"
                        on:change=on_file_change
                    />
                </label>
                <p class="text-[10px] text-muted-foreground text-center mt-1">
                    "SVG, PNG, or JPG \u{2022} Max 50KB"
                </p>
                {move || upload_error.get().map(|msg| view! {
                    <p class="text-[10px] text-destructive text-center mt-1">{msg}</p>
                })}
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

// ─── Upload helper ────────────────────────────────────────────────────────────

/// Upload a file to the team icon endpoint via fetch + FormData.
#[cfg(target_arch = "wasm32")]
async fn upload_team_icon_file(
    team_id: &str,
    file: &web_sys::File,
) -> Result<(), String> {
    use wasm_bindgen::JsCast;
    use wasm_bindgen_futures::JsFuture;

    let form_data = web_sys::FormData::new()
        .map_err(|e| format!("FormData error: {e:?}"))?;
    form_data
        .append_with_blob_and_filename("icon", file, &file.name())
        .map_err(|e| format!("FormData append error: {e:?}"))?;

    let opts = web_sys::RequestInit::new();
    opts.set_method("POST");
    opts.set_body(form_data.as_ref());

    let url = format!("/api/v1/teams/{team_id}/icon");
    let request = web_sys::Request::new_with_str_and_init(&url, &opts)
        .map_err(|e| format!("Request error: {e:?}"))?;

    let window = web_sys::window().ok_or("no window")?;
    let resp_val = JsFuture::from(window.fetch_with_request(&request))
        .await
        .map_err(|e| format!("fetch error: {e:?}"))?;

    let resp: web_sys::Response = resp_val
        .dyn_into()
        .map_err(|e| format!("response cast error: {e:?}"))?;

    if !resp.ok() {
        let status = resp.status();
        // Try to get error message from response body
        let body_text = match resp.text() {
            Ok(promise) => match JsFuture::from(promise).await {
                Ok(val) => val.as_string().unwrap_or_default(),
                Err(_) => String::new(),
            },
            Err(_) => String::new(),
        };
        return Err(format!("Upload failed ({}): {}", status, body_text));
    }

    Ok(())
}

/// Server-side stub — never called at runtime but satisfies the compiler
/// when SSR compiles the component module.
#[cfg(not(target_arch = "wasm32"))]
async fn upload_team_icon_file(
    _team_id: &str,
    _file: &web_sys::File,
) -> Result<(), String> {
    Err("upload only available in browser".into())
}
