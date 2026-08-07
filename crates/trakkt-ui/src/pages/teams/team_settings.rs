// SPDX-License-Identifier: AGPL-3.0-or-later
use leptos::prelude::*;
use leptos_router::hooks::{use_navigate, use_params_map};
use phosphor_leptos::{Icon, IconWeight};
use trakkt_types::models::{EstimateScale, Team, TeamSettings};
use crate::cache::store::SyncStore;
use crate::components::{ActionStatus, Button, ButtonSize, ButtonVariant, Card, CardContent, CardDescription, CardHeader, CardTitle, ConfirmDialog, Select, SelectVariant, Skeleton, Switch, TeamIcon, TeamIconPicker, INPUT_CLASS};
use crate::pages::settings::team_labels::TeamLabelsPage;
use crate::components::popover::{Placement, Popover};
use crate::server_fns::teams::update_team_settings;

#[component]
pub fn TeamSettingsPage() -> impl IntoView {
    let params = use_params_map();
    let team_key = Signal::derive(move || params.read().get("key").unwrap_or_default());
    let store = use_context::<SyncStore>();
    let initialized = Signal::derive(move || store.is_some_and(|s| s.initialized().get()));
    let team = Memo::new(move |_| {
        let key = team_key.get();
        store.and_then(|s| s.teams().get().into_iter().find(|t| t.key.eq_ignore_ascii_case(&key)))
    });
    let back_href = Signal::derive(move || format!("/teams/{}/issues", team_key.get().to_lowercase()));

    view! {
        <div class="flex flex-col h-full">
            <div class="page-header h-14 px-5 flex items-center gap-3 shrink-0">
                <Button variant=ButtonVariant::GhostMuted size=ButtonSize::IconSm aria_label="Back to issues"
                    on:click=move |_| { let nav = use_navigate(); nav(&back_href.get_untracked(), Default::default()); }>
                    <Icon icon=phosphor_leptos::ARROW_LEFT weight=IconWeight::Regular size="20px"/>
                </Button>
                <h1 class="text-2xl font-display text-foreground">"Team Settings"</h1>
            </div>
            <div class="flex-1 overflow-y-auto p-4 md:p-6">
                <Show
                    when=move || initialized.get()
                    fallback=|| view! {
                        <div class="space-y-6">
                            <Card><CardHeader><Skeleton class="h-5 w-1/3"/></CardHeader>
                            <CardContent><Skeleton class="h-10 w-full"/></CardContent></Card>
                        </div>
                    }
                >
                    <TeamSettingsBody team=team/>
                </Show>
            </div>
        </div>
    }
}

#[component]
fn TeamSettingsBody(team: Memo<Option<Team>>) -> impl IntoView {
    view! {
        <Show
            when=move || team.get().is_some()
            fallback=|| view! { <div class="text-muted-foreground">"Team not found."</div> }
        >
            {move || team.get().map(|t| view! {
                <div class="space-y-6">
                    <TeamGeneralCard team=t.clone()/>
                    <TeamArchiveCard team=t.clone()/>
                    <TeamEstimateCard team=t.clone()/>
                    <TeamLabelsPage team_id=t.team_id.clone()/>
                    <TeamDangerZone team_id=t.team_id.clone() team_name=t.name.clone()/>
                </div>
            })}
        </Show>
    }
}

/// One icon change as the picker reports it: `(icon_type, icon_name,
/// icon_color)`. All three `None` means "remove this team's icon".
type IconChange = (Option<String>, Option<String>, Option<String>);

/// The preset name and colour a team snapshot is holding.
///
/// `(None, None)` for a team with a custom upload or no icon at all — the
/// picker has no preset selected in either case, which is what the picker's
/// swatch rings and preview are drawn from.
fn preset_of(team: &Team) -> (Option<String>, Option<String>) {
    if team.icon_type.as_deref() == Some("preset") {
        (team.icon_name.clone(), team.icon_color.clone())
    } else {
        (None, None)
    }
}

/// The team icon the picker is showing, and the one the server last confirmed.
///
/// `name` / `color` are optimistic: the picker writes them the moment the user
/// clicks a swatch, before the server has been asked. The last pair the server
/// actually acknowledged is held privately by [`IconSelection::with_saver`],
/// and is what a rejected save puts back — negating what is on screen would not
/// do, because a second change can have landed while the first was on the wire.
///
/// # Why all of this is owned by [`TeamGeneralCard`]
///
/// Same rule as [`ToggleState`](crate::pages::settings::notifications) records,
/// arrived at by TRA-9980: the `Action` and the `Effect` watching it must be
/// owned together, and by something that outlives the thing the user clicked.
/// Here that thing is `TeamIconPicker`, which renders inside a `<Popover>` —
/// its children sit behind a `<Show>`, so they are built on open and disposed
/// on close. An `Action` owned there is disposed by the user closing the
/// popover, and an `Effect` that outlived it would panic reading it; an
/// `Effect` owned there is simply never woken again, so a rejected save would
/// revert nothing and say nothing. Both live here, above the popover, along
/// with the signals they revert.
#[derive(Clone, Copy)]
struct IconSelection {
    name: RwSignal<Option<String>>,
    color: RwSignal<Option<String>>,
    save: Action<IconChange, Result<Team, ServerFnError>>,
}

impl IconSelection {
    /// Build the icon state for `team`, saving through the real server
    /// functions.
    fn new(team: &Team) -> Self {
        let team_id = team.team_id.clone();
        Self::with_saver(team, move |(icon_type, icon_name, icon_color): IconChange| {
            let team_id = team_id.clone();
            async move {
                if icon_type.is_none() && icon_name.is_none() && icon_color.is_none() {
                    crate::server_fns::teams::clear_team_icon(team_id).await
                } else {
                    crate::server_fns::teams::update_team_icon(
                        team_id, icon_type, icon_name, icon_color,
                    )
                    .await
                }
            }
        })
    }

    /// The whole of [`IconSelection::new`] with the one thing a browser test
    /// cannot have — the request — left as a parameter.
    ///
    /// The seam is at the request and nowhere else, so the `Action`, the
    /// `Effect` watching its value, the revert and the toast are all the
    /// shipped code when a test drives them. `new` is the only caller in the
    /// shipped binary, and it passes the real server functions. This is the
    /// same trade `ToggleState::with_saver` makes, for the same reason.
    fn with_saver<Fut>(
        team: &Team,
        save_fn: impl Fn(IconChange) -> Fut + Send + Sync + 'static,
    ) -> Self
    where
        Fut: std::future::Future<Output = Result<Team, ServerFnError>> + Send + 'static,
    {
        let confirmed = preset_of(team);
        let name = RwSignal::new(confirmed.0.clone());
        let color = RwSignal::new(confirmed.1.clone());
        // The last pair the server acknowledged. Read only by the `Effect`
        // below, so it stays a local rather than a field.
        let baseline = RwSignal::new(confirmed);
        let save = Action::new(move |change: &IconChange| save_fn(change.clone()));

        // Before TRA-9983 both server calls were spawned with their results
        // discarded, so a rejected change — an expired session, a `Forbidden`,
        // a dropped connection — left the picker showing the icon the user had
        // just chosen and said nothing. They found out on the next reload, if
        // ever.
        let show_error = crate::components::toast::capture_error_toast();
        Effect::new(move |_| match save.value().get() {
            // The server's own snapshot of the team, not the value that was
            // sent: it is the authority on what "this team's icon" now is, and
            // it is what the next rejection has to fall back to.
            Some(Ok(saved)) => baseline.set(preset_of(&saved)),
            Some(Err(e)) => {
                tracing::warn!("Failed to save the team icon: {e}");
                let (confirmed_name, confirmed_color) = baseline.get_untracked();
                name.set(confirmed_name);
                color.set(confirmed_color);
                // A toast rather than the card's `validation_error` line: the
                // popover opens over that line and covers it, so the user
                // clicking a swatch would not see the message until they
                // dismissed the picker. The toast container is fixed to the
                // viewport corner and is read whether the picker is open,
                // closed, or was closed while the save was still on the wire.
                show_error("Could not save the team icon. Please try again.".to_owned());
            }
            None => {}
        });

        Self { name, color, save }
    }
}

#[component]
fn TeamGeneralCard(team: Team) -> impl IntoView {
    let team_id = team.team_id.clone();
    let (edit_name, set_edit_name) = signal(team.name.clone());
    let (edit_key, set_edit_key) = signal(team.key.clone());
    let (validation_error, set_validation_error) = signal(Option::<String>::None);

    let icon = IconSelection::new(&team);

    let save_action = Action::new({ let team_id = team_id.clone(); move |(name, key): &(String, String)| {
        let team_id = team_id.clone(); let name = name.clone(); let key = key.clone();
        async move { crate::server_fns::teams::update_team(team_id, Some(name), Some(key)).await }
    }});

    let nav = use_navigate();
    Effect::new(move |_| { if let Some(Ok(())) = save_action.value().get() { let k = edit_key.get().to_lowercase(); nav(&format!("/teams/{k}/settings"), Default::default()); } });

    let handle_save = move |_| {
        let n = edit_name.get_untracked(); let k = edit_key.get_untracked();
        if n.trim().is_empty() { set_validation_error.set(Some("Team name cannot be empty.".into())); return; }
        if k.len() < 2 || k.len() > 5 || !k.chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit()) {
            set_validation_error.set(Some("Key must be 2\u{2013}5 uppercase alphanumeric characters.".into())); return;
        }
        set_validation_error.set(None);
        save_action.dispatch((n, k));
    };

    view! {
        <Card>
            <CardHeader><div class="flex items-center justify-between"><div><CardTitle>"General"</CardTitle><CardDescription>"Update team name, key, and icon."</CardDescription></div><ActionStatus action=save_action/></div></CardHeader>
            <CardContent>
                <div class="space-y-4">
                    <div>
                        <label class="block text-sm font-medium text-foreground mb-2">"Team icon"</label>
                        <IconTrigger team=team.clone() selection=icon/>
                    </div>
                    <div>
                        <label class="block text-sm font-medium text-foreground mb-2">"Team name"</label>
                        <input type="text" class=INPUT_CLASS prop:value=edit_name on:input=move |ev| set_edit_name.set(event_target_value(&ev))/>
                    </div>
                    <div>
                        <label class="block text-sm font-medium text-foreground mb-2">"Team key"</label>
                        <input type="text" class=INPUT_CLASS maxlength="5" prop:value=edit_key on:input=move |ev| set_edit_key.set(event_target_value(&ev).to_uppercase())/>
                        <p class="text-xs text-muted-foreground mt-1">"2\u{2013}5 uppercase letters or digits. Used as issue ID prefix (e.g. ENG-42)."</p>
                    </div>
                    <Show when=move || validation_error.get().is_some()>
                        <p class="text-sm text-error-foreground">{move || validation_error.get().unwrap_or_default()}</p>
                    </Show>
                    <div class="flex justify-end"><Button on:click=handle_save>"Save"</Button></div>
                </div>
            </CardContent>
        </Card>
    }
}

/// The 40px team icon in the card, and the picker that opens under it.
///
/// The icon shown here is the team as the store holds it — server truth,
/// refreshed when a sync frame rebuilds [`TeamGeneralCard`]. It is deliberately
/// not painted from `selection`: nothing here is optimistic, so there is
/// nothing here to revert.
#[component]
fn IconTrigger(team: Team, selection: IconSelection) -> impl IntoView {
    let trigger_ref = NodeRef::<leptos::html::Div>::new();
    let picker_open = RwSignal::new(false);
    let team_for_picker = StoredValue::new(team.clone());
    // Built here rather than in the picker: this component is not rebuilt when
    // the popover opens and closes, so the callback outlives every picker that
    // reads it.
    let on_change = Callback::new(move |change: IconChange| {
        selection.save.dispatch(change);
    });
    view! {
        <div class="flex items-center gap-3">
            <div node_ref=trigger_ref role="button" tabindex="0" aria-label="Change team icon"
                class="cursor-pointer rounded-lg p-0.5 transition-all duration-200 hover:ring-2 hover:ring-ring hover:ring-offset-2 hover:ring-offset-background focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2"
                on:click=move |_| picker_open.update(|v| *v = !*v)
                on:keydown=move |e: web_sys::KeyboardEvent| { if e.key() == "Enter" || e.key() == " " { e.prevent_default(); picker_open.update(|v| *v = !*v); } }
            >
                <TeamIcon team=team.clone() size="40px"/>
            </div>
            <span class="text-sm text-muted-foreground">"Click to change"</span>
        </div>
        <Popover trigger_ref=trigger_ref open=Signal::from(picker_open) on_close=Callback::new(move |()| picker_open.set(false)) placement=Placement::BOTTOM_START>
            <div class="p-3 w-[300px] bg-popover border border-border rounded-lg shadow-lg">
                <TeamIconPicker
                    team=team_for_picker.get_value()
                    selected_name=selection.name
                    selected_color=selection.color
                    on_change=on_change
                />
            </div>
        </Popover>
    }
}

#[component]
fn TeamDangerZone(team_id: String, team_name: String) -> impl IntoView {
    let (show_delete_confirm, set_show_delete_confirm) = signal(false);
    let nav = use_navigate();
    let store = use_context::<SyncStore>();
    let confirm_message = format!("Are you sure you want to delete \"{}\"? All issues in this team will become unassigned. This action cannot be undone.", team_name);
    view! {
        <Card>
            <CardHeader><CardTitle><span class="text-error-foreground">"Danger Zone"</span></CardTitle><CardDescription>"Destructive actions that cannot be undone."</CardDescription></CardHeader>
            <CardContent>
                <div class="flex items-center justify-between">
                    <div><p class="text-sm font-medium text-foreground">"Delete this team"</p><p class="text-xs text-muted-foreground">"Permanently delete this team and unassign all its issues."</p></div>
                    <Button variant=ButtonVariant::Destructive on:click=move |_| set_show_delete_confirm.set(true)>"Delete team"</Button>
                </div>
            </CardContent>
        </Card>
        { let team_id = team_id.clone(); view! {
            <ConfirmDialog open=Signal::derive(move || show_delete_confirm.get()) title="Delete team?" message=confirm_message confirm_text="Delete"
                on_confirm=Callback::new(move |()| { set_show_delete_confirm.set(false); let team_id = team_id.clone(); let nav = nav.clone();
                    leptos::task::spawn_local(async move { match crate::server_fns::teams::delete_team(team_id.clone(), None, None).await {
                        Ok(()) => { if let Some(store) = store { store.remove_team(&team_id); } nav("/my-issues", Default::default()); }
                        Err(e) => { web_sys::console::warn_1(&format!("delete_team failed: {e}").into()); }
                    }});
                })
                on_cancel=Callback::new(move |()| set_show_delete_confirm.set(false))
            />
        }}
    }
}

// ─── Team auto-archive settings card ─────────────────────────────────────

/// Build the dropdown options for auto-archive duration selection.
///
/// Returns `(value, label)` pairs for the `Select`. The first option is
/// "Workspace default" (value `""`), followed by "Never" and day-based options.
fn archive_days_options() -> Vec<(String, String)> {
    vec![
        ("".to_string(), "Workspace default".to_string()),
        ("0".to_string(), "Never".to_string()),
        ("7".to_string(), "7 days".to_string()),
        ("14".to_string(), "14 days".to_string()),
        ("30".to_string(), "30 days".to_string()),
        ("60".to_string(), "60 days".to_string()),
        ("90".to_string(), "90 days".to_string()),
    ]
}

/// Per-team card for configuring auto-archive duration.
///
/// Changes auto-save immediately by calling the `update_team_settings`
/// server function on every change. An `ActionStatus` indicator shows
/// saving/saved/error state next to the card title.
#[component]
fn TeamArchiveCard(team: Team) -> impl IntoView {
    let settings = team.settings.clone().unwrap_or_default();
    let team_id = team.team_id.clone();

    // Map Option<u32> → select value string:
    // None → "" (workspace default), Some(0) → "0" (never), Some(N) → "N"
    let initial_value = match settings.auto_archive_days {
        None => String::new(),
        Some(d) => d.to_string(),
    };
    let (archive_value, set_archive_value) = signal(initial_value);

    // Preserve the estimate settings from the original so we don't clobber
    // them when saving archive changes.
    let estimate_scale = settings.estimate_scale.clone();
    let estimate_allow_zero = settings.estimate_allow_zero;
    let estimate_extended = settings.estimate_extended;
    let estimate_count_unestimated = settings.estimate_count_unestimated;

    // Save action — sends the full TeamSettings JSON to the server
    let save_action = Action::new({
        let team_id = team_id.clone();
        move |settings_json: &String| {
            let team_id = team_id.clone();
            let json = settings_json.clone();
            async move { update_team_settings(team_id, json).await }
        }
    });

    // Event handler: parse the select value back to Option<u32> and save
    let on_archive_change = move |val: String| {
        set_archive_value.set(val.clone());
        let auto_archive_days = if val.is_empty() {
            None
        } else {
            match val.parse::<u32>() {
                Ok(d) => Some(d),
                Err(e) => {
                    tracing::warn!(error = %e, value = %val, "Failed to parse archive days value");
                    None
                }
            }
        };
        let ts = TeamSettings {
            auto_archive_days,
            estimate_scale: estimate_scale.clone(),
            estimate_allow_zero,
            estimate_extended,
            estimate_count_unestimated,
        };
        match serde_json::to_string(&ts) {
            Ok(json) => {
                save_action.dispatch(json);
            }
            Err(e) => tracing::warn!("Failed to serialize TeamSettings: {e}"),
        }
    };

    // Static options (constructed once, shared via Signal)
    let options = archive_days_options();
    let options_signal = Signal::derive(move || options.clone());

    view! {
        <Card>
            <CardHeader>
                <div class="flex items-center justify-between">
                    <div>
                        <CardTitle>"Auto-archive"</CardTitle>
                        <CardDescription>
                            "Automatically archive completed and cancelled issues after a set period. Archived issues are hidden from the default list."
                        </CardDescription>
                    </div>
                    <ActionStatus action=save_action/>
                </div>
            </CardHeader>
            <CardContent>
                <div class="space-y-4">
                    <div class="flex flex-col sm:flex-row sm:items-center gap-2">
                        <label class="text-sm font-medium text-foreground sm:w-48 flex-shrink-0">
                            "Archive after"
                        </label>
                        <div class="flex-1 max-w-sm">
                            <Select
                                value=archive_value
                                options=options_signal
                                on_change=Callback::new(on_archive_change)
                                variant=SelectVariant::Form
                                placeholder="Workspace default"
                            />
                        </div>
                    </div>
                    <p class="text-xs text-muted-foreground">
                        "When set to \u{2018}Workspace default\u{2019}, the workspace-level setting will be used."
                    </p>
                </div>
            </CardContent>
        </Card>
    }
}

// ─── Team estimate settings card ─────────────────────────────────────────

/// Build the dropdown options for estimate scale selection.
///
/// Returns `(value, label)` pairs for the `Select`. The first option is
/// always "Not in use" (value `""`), followed by the four scale variants.
fn estimate_scale_options() -> Vec<(String, String)> {
    let scales = [
        EstimateScale::Exponential,
        EstimateScale::Fibonacci,
        EstimateScale::Linear,
        EstimateScale::TShirt,
    ];
    let mut opts = vec![("".to_string(), "Not in use".to_string())];
    for scale in &scales {
        let label = format!("{} ({})", scale.display_label(), scale.preview());
        let value = match serde_json::to_string(scale) {
            Ok(s) => s.trim_matches('"').to_string(),
            Err(e) => {
                tracing::warn!(error = %e, "failed to serialize EstimateScale variant");
                continue;
            }
        };
        opts.push((value, label));
    }
    opts
}

/// Parse an estimate scale value string back to an `Option<EstimateScale>`.
///
/// Empty string maps to `None` (disabled). Recognized serde values map to
/// `Some(variant)`.
fn parse_estimate_scale(value: &str) -> Option<EstimateScale> {
    if value.is_empty() {
        return None;
    }
    let quoted = format!("\"{value}\"");
    match serde_json::from_str(&quoted) {
        Ok(scale) => Some(scale),
        Err(e) => {
            tracing::warn!(error = %e, value, "failed to deserialize EstimateScale");
            None
        }
    }
}

/// Per-team card showing estimate scale selection and toggle options.
///
/// Changes auto-save immediately by calling the `update_team_settings`
/// server function on every change. An `ActionStatus` indicator shows
/// saving/saved/error state next to the card title.
#[component]
fn TeamEstimateCard(team: Team) -> impl IntoView {
    let settings = team.settings.clone().unwrap_or_default();
    let team_id = team.team_id.clone();

    // Local reactive state for each setting
    let (scale, set_scale) = signal(
        settings
            .estimate_scale
            .as_ref()
            .and_then(|s| {
                match serde_json::to_string(s) {
                    Ok(v) => Some(v.trim_matches('"').to_string()),
                    Err(e) => {
                        tracing::warn!(error = %e, "failed to serialize team estimate scale");
                        None
                    }
                }
            })
            .unwrap_or_default(),
    );
    let (allow_zero, set_allow_zero) = signal(settings.estimate_allow_zero);
    let (extended, set_extended) = signal(settings.estimate_extended);
    let (count_unestimated, set_count_unestimated) = signal(settings.estimate_count_unestimated);

    // Preserve the auto_archive_days from the original settings so we don't
    // clobber it when saving estimate changes.
    let auto_archive_days = settings.auto_archive_days;

    // Save action — sends the full TeamSettings JSON to the server
    let save_action = Action::new({
        let team_id = team_id.clone();
        move |settings_json: &String| {
            let team_id = team_id.clone();
            let json = settings_json.clone();
            async move { update_team_settings(team_id, json).await }
        }
    });

    // Helper: build and dispatch a save from current signal values
    let persist = move || {
        let ts = TeamSettings {
            auto_archive_days,
            estimate_scale: parse_estimate_scale(&scale.get_untracked()),
            estimate_allow_zero: allow_zero.get_untracked(),
            estimate_extended: extended.get_untracked(),
            estimate_count_unestimated: count_unestimated.get_untracked(),
        };
        match serde_json::to_string(&ts) {
            Ok(json) => {
                save_action.dispatch(json);
            }
            Err(e) => tracing::warn!("Failed to serialize TeamSettings: {e}"),
        }
    };

    let has_scale = Memo::new(move |_| !scale.get().is_empty());

    // Static options (constructed once, shared via Signal)
    let options = estimate_scale_options();
    let options_signal = Signal::derive(move || options.clone());

    // Event handlers
    let on_scale_change = {
        move |val: String| {
            // When disabling estimates, reset toggles to defaults
            if val.is_empty() {
                set_allow_zero.set(false);
                set_extended.set(false);
                set_count_unestimated.set(true);
            }
            set_scale.set(val);
            persist();
        }
    };

    let on_allow_zero = {
        move |val: bool| {
            set_allow_zero.set(val);
            persist();
        }
    };

    let on_extended = {
        move |val: bool| {
            set_extended.set(val);
            persist();
        }
    };

    let on_count_unestimated = {
        move |val: bool| {
            set_count_unestimated.set(val);
            persist();
        }
    };

    view! {
        <Card>
            <CardHeader>
                <div class="flex items-center justify-between">
                    <div>
                        <CardTitle>"Estimates"</CardTitle>
                        <CardDescription>
                            "Configure how estimates work for this team"
                        </CardDescription>
                    </div>
                    <ActionStatus action=save_action/>
                </div>
            </CardHeader>
            <CardContent>
                <div class="space-y-4">
                    // Scale dropdown
                    <div class="flex flex-col sm:flex-row sm:items-center gap-2">
                        <label class="text-sm font-medium text-foreground sm:w-48 flex-shrink-0">
                            "Issue estimation"
                        </label>
                        <div class="flex-1 max-w-sm">
                            <Select
                                value=scale
                                options=options_signal
                                on_change=Callback::new(on_scale_change)
                                variant=SelectVariant::Form
                                placeholder="Not in use"
                            />
                        </div>
                    </div>

                    // Toggle options — only shown when a scale is selected
                    <Show when=move || has_scale.get()>
                        <div class="border-t border-border pt-4 space-y-3">
                            <div class="flex items-center justify-between">
                                <div>
                                    <span class="text-sm font-medium text-foreground">"Allow zero estimates"</span>
                                    <p class="text-xs text-muted-foreground mt-0.5">
                                        "Include a \"No estimate\" (0) option in the picker"
                                    </p>
                                </div>
                                <Switch
                                    checked=Signal::derive(move || allow_zero.get())
                                    on_change=Callback::new(on_allow_zero)
                                />
                            </div>
                            <div class="flex items-center justify-between">
                                <div>
                                    <span class="text-sm font-medium text-foreground">"Extended estimate scale"</span>
                                    <p class="text-xs text-muted-foreground mt-0.5">
                                        "Show additional larger point values beyond the base scale"
                                    </p>
                                </div>
                                <Switch
                                    checked=Signal::derive(move || extended.get())
                                    on_change=Callback::new(on_extended)
                                />
                            </div>
                            <div class="flex items-center justify-between">
                                <div>
                                    <span class="text-sm font-medium text-foreground">"Count unestimated issues"</span>
                                    <p class="text-xs text-muted-foreground mt-0.5">
                                        "Include issues without estimates in velocity and capacity totals"
                                    </p>
                                </div>
                                <Switch
                                    checked=Signal::derive(move || count_unestimated.get())
                                    on_change=Callback::new(on_count_unestimated)
                                />
                            </div>
                        </div>
                    </Show>
                </div>
            </CardContent>
        </Card>
    }
}

// ── A save the test drives instead of the server ────────────────────────────

/// The fixture the browser tests below dispatch through a real [`Action`].
///
/// Clicking a swatch dispatches a `#[server]` function, and a browser test has
/// no server to answer it. Substituting the request — and only the request —
/// is what [`IconSelection::with_saver`] exists for: the `Action`, its
/// in-flight bookkeeping, the `Effect` watching its value, the revert and the
/// toast are all the shipped code when these tests drive them.
///
/// The one thing on the far side of the seam, and therefore not covered here,
/// is [`IconSelection::new`]'s choice between `clear_team_icon` and
/// `update_team_icon`. What the tests can and do pin is the value that choice
/// is made from: [`a_rejected_icon_removal_puts_the_icon_back`] asserts
/// "Remove icon" dispatches `(None, None, None)`, which is the condition that
/// branch reads.
#[cfg(all(test, target_arch = "wasm32"))]
mod icon_save_fixture {
    use std::cell::RefCell;
    use std::rc::Rc;
    use std::sync::Arc;

    use leptos::prelude::*;
    use send_wrapper::SendWrapper;
    use trakkt_types::models::Team;

    use crate::wasm_test_support::{Gate, LocalFuture, SaveLog};

    use super::IconChange;

    /// A boxed fixture save, so the harness below can take one as a plain prop
    /// instead of being generic over the future's type.
    pub type FixtureSave =
        Arc<dyn Fn(IconChange) -> LocalFuture<Result<Team, ServerFnError>> + Send + Sync>;

    /// Every change the fixture was asked to save, in order.
    ///
    /// `Rc<RefCell<_>>` rather than a signal for the same reason [`SaveLog`]
    /// is: an instrument in these tests must not be something disposal can
    /// reach.
    #[derive(Clone, Default)]
    pub struct ChangeLog(Rc<RefCell<Vec<IconChange>>>);

    impl ChangeLog {
        pub fn changes(&self) -> Vec<IconChange> {
            self.0.borrow().clone()
        }
    }

    /// A save that records itself, waits for `gate`, and then either echoes
    /// back the team the change would have produced or fails.
    ///
    /// The success payload is what the server returns: `update_team_icon` and
    /// `clear_team_icon` both hand back the whole updated `Team`.
    ///
    /// `outcomes` is read one entry per dispatch, in order, and the last entry
    /// stands for every dispatch after it — so a single-element `outcomes` is
    /// "always this answer", and `[true, false]` is "accept the first change,
    /// reject the second".
    pub fn gated_icon_save(
        base: Team,
        gate: Gate,
        log: SaveLog,
        changes: ChangeLog,
        outcomes: Vec<bool>,
    ) -> FixtureSave {
        assert!(!outcomes.is_empty(), "a fixture save needs at least one outcome");
        let dispatched = Rc::new(RefCell::new(0usize));
        let shared = SendWrapper::new((base, gate, log, changes, outcomes, dispatched));
        Arc::new(move |change: IconChange| {
            let (base, gate, log, changes, outcomes, dispatched) = (*shared).clone();
            LocalFuture::new(async move {
                log.record_start();
                changes.0.borrow_mut().push(change.clone());
                let nth = {
                    let mut n = dispatched.borrow_mut();
                    let nth = *n;
                    *n += 1;
                    nth
                };
                gate.wait().await;
                log.record_finish();
                if outcomes[nth.min(outcomes.len() - 1)] {
                    let mut saved = base;
                    saved.icon_type = change.0;
                    saved.icon_name = change.1;
                    saved.icon_color = change.2;
                    Ok(saved)
                } else {
                    Err(ServerFnError::new("the fixture save was told to fail"))
                }
            })
        })
    }
}

// ── Browser tests ───────────────────────────────────────────────────────────

/// Run with:
/// `wasm-pack test --headless --firefox crates/trakkt-ui --lib --features hydrate`
#[cfg(all(test, target_arch = "wasm32"))]
mod wasm_tests {
    use gloo_timers::future::TimeoutFuture;
    use leptos::prelude::*;
    use wasm_bindgen::JsCast;
    use wasm_bindgen_test::{wasm_bindgen_test, wasm_bindgen_test_configure};

    use crate::wasm_test_support::{boot_leptos_executor, Gate, SaveLog};

    use super::icon_save_fixture::{gated_icon_save, ChangeLog, FixtureSave};
    use super::*;

    wasm_bindgen_test_configure!(run_in_browser);

    /// The colour the fixture team already has, and the one the user picks.
    const CONFIRMED_COLOR: &str = "#3B82F6";
    const CHOSEN_COLOR: &str = "#EF4444";
    /// A third colour, so "fell back to the accepted change" and "fell back to
    /// the opening snapshot" cannot be the same observation.
    const SECOND_COLOR: &str = "#22C55E";

    fn probe_team() -> Team {
        Team {
            team_id: "team-fixture".to_owned(),
            workspace_id: "workspace-fixture".to_owned(),
            name: "Engineering".to_owned(),
            key: "ENG".to_owned(),
            description: None,
            icon: None,
            icon_type: Some("preset".to_owned()),
            icon_name: Some("rocket".to_owned()),
            icon_color: Some(CONFIRMED_COLOR.to_owned()),
            member_count: 3,
            settings: None,
            created_at: "2026-01-01T00:00:00Z".to_owned(),
        }
    }

    /// [`TeamGeneralCard`]'s icon half with the request taken out.
    ///
    /// The `IconSelection` is built here, above `IconTrigger`, because that is
    /// where the card builds it — and the whole point of this ticket is that it
    /// must outlive the picker.
    #[component]
    fn ProbeIconCard(team: Team, saver: FixtureSave) -> impl IntoView {
        let selection = IconSelection::with_saver(&team, move |change| saver(change));
        view! { <IconTrigger team=team selection=selection/> }
    }

    fn make_container() -> web_sys::HtmlElement {
        let document = web_sys::window()
            .expect("no window")
            .document()
            .expect("no document");
        let container: web_sys::HtmlElement = document
            .create_element("div")
            .expect("could not create a container")
            .dyn_into()
            .expect("container is not an html element");
        document
            .body()
            .expect("no body")
            .append_child(&container)
            .expect("could not attach the container");
        container
    }

    fn mount_probe(saver: FixtureSave) -> (web_sys::HtmlElement, impl Sized) {
        let container = make_container();
        let handle = leptos::mount::mount_to(container.clone(), move || {
            view! {
                <crate::components::toast::ToastProvider>
                    <ProbeIconCard team=probe_team() saver=saver/>
                </crate::components::toast::ToastProvider>
            }
        });
        (container, handle)
    }

    /// The open picker.
    ///
    /// `<Popover>` portals its children to `document.body`, so this cannot be
    /// scoped to the test's own container. Asserting there is exactly one is
    /// what stops a popover left behind by another test being measured instead
    /// of this one's.
    fn picker() -> web_sys::Element {
        let document = web_sys::window()
            .expect("no window")
            .document()
            .expect("no document");
        let popovers = document
            .query_selector_all(".trakkt-popover")
            .expect("query failed");
        assert_eq!(
            popovers.length(),
            1,
            "expected exactly one open popover in the document"
        );
        popovers
            .item(0)
            .expect("the popover vanished between counting and reading it")
            .dyn_into()
            .expect("the popover is not an element")
    }

    fn picker_is_open() -> bool {
        web_sys::window()
            .expect("no window")
            .document()
            .expect("no document")
            .query_selector_all(".trakkt-popover")
            .expect("query failed")
            .length()
            > 0
    }

    /// Click something the way a user does.
    fn click(root: &web_sys::Element, selector: &str) {
        let target: web_sys::HtmlElement = root
            .query_selector(selector)
            .expect("query failed")
            .unwrap_or_else(|| panic!("nothing matched {selector}"))
            .dyn_into()
            .unwrap_or_else(|_| panic!("{selector} did not match an html element"));
        target.click();
    }

    /// Click the button reading `label`.
    ///
    /// "Remove icon" is a `<Button>`, so its name is its text rather than an
    /// attribute — matching on the text is matching on what the user reads.
    fn click_button_labelled(root: &web_sys::Element, label: &str) {
        let buttons = root.query_selector_all("button").expect("query failed");
        let target: web_sys::HtmlElement = (0..buttons.length())
            .filter_map(|i| buttons.item(i))
            .filter_map(|n| n.dyn_into::<web_sys::HtmlElement>().ok())
            .find(|el| el.text_content().is_some_and(|t| t.trim() == label))
            .unwrap_or_else(|| panic!("no button reads {label:?}"));
        target.click();
    }

    /// Open or close the picker by clicking the 40px team icon.
    async fn toggle_picker(container: &web_sys::HtmlElement) {
        click(container, "[aria-label='Change team icon']");
        TimeoutFuture::new(80).await;
    }

    /// The 48px preview at the top of the picker, as a `style` string.
    ///
    /// This is the icon the picker is showing: for a preset it carries
    /// `background-color: <colour>`, and for no icon at all it is the
    /// initial-letter fallback, which carries none.
    fn preview_style() -> String {
        picker()
            .query_selector("span[style*='width: 48px']")
            .expect("query failed")
            .expect("the picker preview is not rendered")
            .get_attribute("style")
            .expect("the preview has no style attribute")
    }

    /// The colour swatch drawn with the selection ring, by its hex value.
    fn ringed_swatch() -> Option<String> {
        let buttons = picker()
            .query_selector_all("button[title^='#']")
            .expect("query failed");
        (0..buttons.length())
            .filter_map(|i| buttons.item(i))
            .filter_map(|n| n.dyn_into::<web_sys::Element>().ok())
            .find(|el| {
                el.get_attribute("class")
                    .is_some_and(|c| c.contains("ring-foreground"))
            })
            .and_then(|el| el.get_attribute("title"))
    }

    /// The message of every toast currently on screen, as the user reads it.
    ///
    /// Selected by the animation class `ToastItem` renders, which is the only
    /// thing distinguishing a toast in the DOM — it carries no role and no
    /// `aria-*`. The message `<p>` rather than the toast itself, because the
    /// toast's text also contains the "x" on its dismiss button.
    fn toast_texts(container: &web_sys::HtmlElement) -> Vec<String> {
        let nodes = container
            .query_selector_all(".animate-slide-in-right > p")
            .expect("query failed");
        (0..nodes.length())
            .filter_map(|i| nodes.item(i).and_then(|n| n.text_content()))
            .collect()
    }

    const REJECTION_MESSAGE: &str = "Could not save the team icon. Please try again.";

    /// A rejected icon change must take the icon off the screen again, and say
    /// why.
    ///
    /// Before TRA-9983 both server calls were `let _ = …`, so the picker went
    /// on showing the colour the user had just picked however the request
    /// ended. They learnt otherwise on the next reload, if ever.
    ///
    /// # Why it cannot pass vacuously
    ///
    /// `SaveLog::started` proves the click reached a real `Action` rather than
    /// only repainting the picker; `SaveLog::finished` proves the request
    /// resolved; and the toast list is asserted empty before the gate opens, so
    /// the toast asserted afterwards cannot be one left over from mounting.
    #[wasm_bindgen_test]
    async fn a_rejected_icon_change_puts_the_picker_back_and_says_why() {
        boot_leptos_executor();

        let owner = Owner::new();
        owner.set();

        let gate = Gate::default();
        let log = SaveLog::default();
        let changes = ChangeLog::default();
        let (container, handle) = mount_probe(gated_icon_save(
            probe_team(),
            gate.clone(),
            log.clone(),
            changes.clone(),
            vec![false],
        ));

        TimeoutFuture::new(60).await;
        toggle_picker(&container).await;
        assert!(
            preview_style().contains(&format!("background-color: {CONFIRMED_COLOR}")),
            "the picker should open on the team's saved colour, got {}",
            preview_style()
        );
        assert_eq!(ringed_swatch().as_deref(), Some(CONFIRMED_COLOR));
        assert!(
            toast_texts(&container).is_empty(),
            "a toast was already on screen before anything was saved, so the assertion below \
             would not have measured this save"
        );

        click(&picker(), &format!("button[title='{CHOSEN_COLOR}']"));
        TimeoutFuture::new(40).await;
        assert_eq!(
            log.started(),
            1,
            "the click did not dispatch the action, so this test measured nothing"
        );
        assert!(
            preview_style().contains(&format!("background-color: {CHOSEN_COLOR}")),
            "the picker must move under the finger, got {}",
            preview_style()
        );
        assert_eq!(ringed_swatch().as_deref(), Some(CHOSEN_COLOR));

        // The server finally answers, and it answers no.
        gate.open();
        TimeoutFuture::new(80).await;
        assert_eq!(log.finished(), 1, "the save's future never completed");

        assert!(
            preview_style().contains(&format!("background-color: {CONFIRMED_COLOR}")),
            "a rejected icon change must put the picker back to what the server holds. Left \
             showing the new colour, the user is told a change succeeded that did not, and \
             finds out on the next reload. Got {}",
            preview_style()
        );
        assert_eq!(
            ringed_swatch().as_deref(),
            Some(CONFIRMED_COLOR),
            "the selection ring must move back with the preview, or the picker still claims \
             the rejected colour is the team's"
        );
        assert_eq!(
            toast_texts(&container),
            vec![REJECTION_MESSAGE.to_owned()],
            "a rejected icon change must say so. Without it the icon reverts on its own and \
             the user cannot tell a rejected save from a click that never registered"
        );

        drop(handle);
        container.remove();
    }

    /// A change rejected *after* the picker is closed still reverts and still
    /// reports.
    ///
    /// This is the ownership the ticket turns on. `<Popover>` renders its
    /// children behind a `<Show>`, so closing the picker disposes it entirely.
    /// An `Action` owned down there goes with it and the `Effect` reading it
    /// panics on a disposed arena entry; an `Effect` owned down there is never
    /// woken again and the answer arrives with nobody listening. Both live on
    /// [`IconSelection`], above the popover, and so do the signals the revert
    /// writes to — which is why reopening shows the team's own colour rather
    /// than the rejected one.
    ///
    /// # Why it cannot pass vacuously
    ///
    /// `SaveLog::finished == 0` is asserted *after* the picker is closed, which
    /// is what proves the request was still on the wire when it was disposed
    /// rather than having quietly resolved first. The gate makes that an
    /// ordering rather than a race.
    #[wasm_bindgen_test]
    async fn an_icon_change_rejected_after_the_picker_closes_still_puts_it_back() {
        boot_leptos_executor();

        let owner = Owner::new();
        owner.set();

        let gate = Gate::default();
        let log = SaveLog::default();
        let changes = ChangeLog::default();
        let (container, handle) = mount_probe(gated_icon_save(
            probe_team(),
            gate.clone(),
            log.clone(),
            changes.clone(),
            vec![false],
        ));

        TimeoutFuture::new(60).await;
        toggle_picker(&container).await;
        click(&picker(), &format!("button[title='{CHOSEN_COLOR}']"));
        TimeoutFuture::new(40).await;
        assert_eq!(
            log.started(),
            1,
            "the click did not dispatch the action, so this test measured nothing"
        );

        // The user dismisses the picker while the save is still open.
        toggle_picker(&container).await;
        assert!(
            !picker_is_open(),
            "the picker did not close, so nothing was disposed and this measured nothing"
        );
        assert_eq!(
            log.finished(),
            0,
            "the save resolved before the picker closed, so it never outlived a disposal and \
             this measured nothing"
        );

        gate.open();
        TimeoutFuture::new(80).await;
        assert_eq!(log.finished(), 1, "the save's future never completed");
        assert_eq!(
            toast_texts(&container),
            vec![REJECTION_MESSAGE.to_owned()],
            "a save that fails after the picker is closed must still report. If it does not, \
             the `Effect` watching the action went with the popover and the failure passed \
             unremarked"
        );

        toggle_picker(&container).await;
        assert!(
            preview_style().contains(&format!("background-color: {CONFIRMED_COLOR}")),
            "reopening the picker must show the team's own colour, not the rejected one. Got \
             {}",
            preview_style()
        );
        assert_eq!(ringed_swatch().as_deref(), Some(CONFIRMED_COLOR));

        toggle_picker(&container).await;
        drop(handle);
        container.remove();
    }

    /// A rejected removal puts the icon back too.
    ///
    /// "Remove icon" is the arm that reaches `clear_team_icon`, and it was
    /// discarding its result just as the other one was. This also pins the
    /// value [`IconSelection::new`]'s branch reads — all three fields `None` —
    /// since the fixture stands in for the branch itself.
    #[wasm_bindgen_test]
    async fn a_rejected_icon_removal_puts_the_icon_back() {
        boot_leptos_executor();

        let owner = Owner::new();
        owner.set();

        let gate = Gate::default();
        let log = SaveLog::default();
        let changes = ChangeLog::default();
        let (container, handle) = mount_probe(gated_icon_save(
            probe_team(),
            gate.clone(),
            log.clone(),
            changes.clone(),
            vec![false],
        ));

        TimeoutFuture::new(60).await;
        toggle_picker(&container).await;
        assert!(preview_style().contains(&format!("background-color: {CONFIRMED_COLOR}")));

        click_button_labelled(&picker(), "Remove icon");
        TimeoutFuture::new(40).await;
        assert_eq!(
            changes.changes(),
            vec![(None, None, None)],
            "\"Remove icon\" must dispatch an all-empty change — that is the condition \
             `IconSelection::new` reads to call `clear_team_icon` rather than \
             `update_team_icon`"
        );
        assert!(
            !preview_style().contains("background-color"),
            "the preview must drop to the initial-letter fallback under the finger, got {}",
            preview_style()
        );

        gate.open();
        TimeoutFuture::new(80).await;
        assert_eq!(log.finished(), 1, "the save's future never completed");

        assert!(
            preview_style().contains(&format!("background-color: {CONFIRMED_COLOR}")),
            "a rejected removal must put the icon back. Left cleared, the picker claims a \
             team has no icon when the server still holds one. Got {}",
            preview_style()
        );
        assert_eq!(
            toast_texts(&container),
            vec![REJECTION_MESSAGE.to_owned()],
            "a rejected removal must say so, on the same terms as a rejected change"
        );

        toggle_picker(&container).await;
        drop(handle);
        container.remove();
    }

    /// A rejection falls back to the last change the server accepted, not to
    /// whatever the team held when the card was built.
    ///
    /// The card is rebuilt from a fresh snapshot when the sync frame carrying
    /// an accepted change arrives, but that frame is not instantaneous, and the
    /// user can pick again before it lands. Falling back to the card's opening
    /// snapshot would then undo a change the server had already accepted — a
    /// second silent loss, in the code written to stop the first.
    ///
    /// This is what reading the `Ok` payload is for: `update_team_icon` and
    /// `clear_team_icon` both return the updated `Team`, and it is the server's
    /// own statement of what the icon now is.
    ///
    /// # Why it cannot pass vacuously
    ///
    /// The accepted colour is asserted on screen before the second change is
    /// made, so a first save that silently did nothing would fail here rather
    /// than leave the final assertion measuring the opening snapshot by
    /// coincidence — the two are different colours precisely so they cannot be
    /// confused.
    #[wasm_bindgen_test]
    async fn a_rejection_falls_back_to_the_last_accepted_change() {
        boot_leptos_executor();

        let owner = Owner::new();
        owner.set();

        let gate = Gate::default();
        let log = SaveLog::default();
        let changes = ChangeLog::default();
        let (container, handle) = mount_probe(gated_icon_save(
            probe_team(),
            gate.clone(),
            log.clone(),
            changes.clone(),
            vec![true, false],
        ));

        TimeoutFuture::new(60).await;
        toggle_picker(&container).await;

        // First change: accepted.
        click(&picker(), &format!("button[title='{CHOSEN_COLOR}']"));
        gate.open();
        TimeoutFuture::new(80).await;
        assert_eq!(log.finished(), 1, "the first save never completed");
        assert!(
            preview_style().contains(&format!("background-color: {CHOSEN_COLOR}")),
            "the first change was not accepted, so the fallback below would be measuring \
             nothing. Got {}",
            preview_style()
        );

        // Second change: rejected.
        click(&picker(), &format!("button[title='{SECOND_COLOR}']"));
        TimeoutFuture::new(80).await;
        assert_eq!(log.finished(), 2, "the second save never completed");

        assert!(
            preview_style().contains(&format!("background-color: {CHOSEN_COLOR}")),
            "a rejection must fall back to the last colour the server accepted, not to the \
             one the card opened on — the latter throws away an accepted change. Got {}",
            preview_style()
        );
        assert_eq!(ringed_swatch().as_deref(), Some(CHOSEN_COLOR));

        toggle_picker(&container).await;
        drop(handle);
        container.remove();
    }

    /// A change the server accepts is kept, and nothing is said about it.
    ///
    /// The control for the three tests above: a revert that fired regardless of
    /// the answer would satisfy every one of them and would throw away every
    /// icon change the user ever made.
    #[wasm_bindgen_test]
    async fn an_accepted_icon_change_is_kept_and_says_nothing() {
        boot_leptos_executor();

        let owner = Owner::new();
        owner.set();

        let gate = Gate::default();
        let log = SaveLog::default();
        let changes = ChangeLog::default();
        let (container, handle) = mount_probe(gated_icon_save(
            probe_team(),
            gate.clone(),
            log.clone(),
            changes.clone(),
            vec![true],
        ));

        TimeoutFuture::new(60).await;
        toggle_picker(&container).await;
        click(&picker(), &format!("button[title='{CHOSEN_COLOR}']"));
        TimeoutFuture::new(40).await;
        assert_eq!(
            log.started(),
            1,
            "the click did not dispatch the action, so this test measured nothing"
        );

        gate.open();
        TimeoutFuture::new(80).await;
        assert_eq!(log.finished(), 1, "the save's future never completed");

        assert!(
            preview_style().contains(&format!("background-color: {CHOSEN_COLOR}")),
            "an accepted change must stay on screen. Got {}",
            preview_style()
        );
        assert_eq!(ringed_swatch().as_deref(), Some(CHOSEN_COLOR));
        assert!(
            toast_texts(&container).is_empty(),
            "an accepted change must not report a failure"
        );

        toggle_picker(&container).await;
        drop(handle);
        container.remove();
    }
}
