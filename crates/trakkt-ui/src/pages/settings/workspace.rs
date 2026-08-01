// SPDX-License-Identifier: AGPL-3.0-or-later

//! Workspace settings page — admin-only workspace configuration.
//!
//! Replaces `apps/frontend/src/components/settings/WorkspaceSettings.jsx`.
//! All data fetching uses server functions instead of REST API calls.

use leptos::prelude::*;

use trakkt_types::models::Team;

use crate::components::{
    ActionStatus, Badge, BadgeVariant, Button, ButtonSize, ButtonVariant, Card, CardContent,
    CardDescription, CardHeader, CardTitle, EmptyState, Select, SelectVariant, Skeleton,
    TeamCreationModal, TeamIcon, INPUT_CLASS,
};
use crate::server_fns::teams::{
    get_workspace_default_team_id, list_all_teams, set_workspace_default_team,
};
use crate::pages::settings::live_update::adopt_unless_edited;
use crate::server_fns::workspace::*;
use crate::types::WorkspaceSettingsData;

// ─────────────────────────────────────────────────────────────────────────────
// Main page
// ─────────────────────────────────────────────────────────────────────────────

/// The two-card skeleton shown until the first snapshot arrives.
fn loading_skeleton() -> impl IntoView {
    view! {
        <div class="space-y-6">
            <Card>
                <CardHeader>
                    <Skeleton class="h-5 w-1/3"/>
                    <Skeleton class="h-4 w-2/3 mt-1"/>
                </CardHeader>
                <CardContent>
                    <Skeleton class="h-10 w-full"/>
                </CardContent>
            </Card>
            <Card>
                <CardHeader>
                    <Skeleton class="h-5 w-1/3"/>
                    <Skeleton class="h-4 w-2/3 mt-1"/>
                </CardHeader>
                <CardContent>
                    <Skeleton class="h-20 w-full"/>
                </CardContent>
            </Card>
        </div>
    }
}

/// The select value that represents `data`'s auto-archive setting.
///
/// `None` → "" (not set), `Some(0)` → "0" (never), `Some(n)` → "n".
fn archive_value_of(data: &WorkspaceSettingsData) -> String {
    match data.default_auto_archive_days {
        None => String::new(),
        Some(d) => d.to_string(),
    }
}

#[component]
pub fn WorkspacePage() -> impl IntoView {
    // The settings this page shows live behind `get_workspace_settings`, not in
    // the sync store, so the store's workspace_settings counter is the only
    // reactive dependency that can tell it another admin renamed the workspace
    // or changed its auto-archive default. Without it the page shows whatever it
    // read on mount until it is navigated away from and back.
    let sync_store = use_context::<crate::cache::store::SyncStore>();
    let settings_version = Signal::derive(move || {
        sync_store
            .map(|s| s.workspace_settings_version().get())
            .unwrap_or(0)
    });
    let settings = Resource::new(move || settings_version.get(), |_| get_workspace_settings());

    // Every editable value lives here rather than in the cards, and that is the
    // whole fix. A refetch rebuilds the cards — that is what a suspense boundary
    // does — and a card that seeded its own `signal(data.workspace_name)` had
    // the admin's half-typed name replaced by the server's every time one
    // arrived. That was reachable with no race at all: the admin's own
    // auto-archive save produces a frame, since broadcasts are not
    // sender-excluded. Hoisting means a rebuild re-binds these same signals
    // instead of seeding new ones, and `adopt_unless_edited` decides what a
    // fresh snapshot is allowed to overwrite.
    //
    // The cards stay inside the suspense boundary, so the server still renders
    // this page. An earlier revision moved them out, on a measurement that the
    // rebuild also costs the caret. It does — but only when the fetcher resolves
    // without ever crossing a macrotask boundary, which a `#[server]` function
    // doing an HTTP round trip cannot do. `latency_tests` in `live_update.rs`
    // has the numbers.
    let name = RwSignal::new(String::new());
    let name_baseline = RwSignal::new(String::new());
    let archive = RwSignal::new(String::new());
    let archive_baseline = RwSignal::new(String::new());

    view! {
        <div class="p-4 sm:p-6">
            <h2 class="text-xl font-display text-foreground mb-4">"Workspace Settings"</h2>
            <p class="text-muted-foreground mb-6">
                "Configure workspace-wide preferences (admin only)."
            </p>

            <Transition fallback=loading_skeleton>
                {move || Suspend::new(async move {
                    match settings.await {
                        Ok(data) => {
                            // Folded in here rather than from an `Effect` so the
                            // first render already has the values. Effects do
                            // not run during SSR, so cards bound to signals an
                            // effect had not filled yet would be server-rendered
                            // empty — which is worse than the skeleton this
                            // arrangement exists to avoid.
                            adopt_unless_edited(name, name_baseline, data.workspace_name.clone());
                            adopt_unless_edited(archive, archive_baseline, archive_value_of(&data));
                            view! {
                                <div class="space-y-6">
                                    <WorkspaceNameCard name=name baseline=name_baseline/>
                                    <WorkspaceArchiveCard value=archive/>
                                    <TeamsSection/>
                                </div>
                            }.into_any()
                        },
                        Err(e) => {
                            let msg = e.to_string();
                            view! {
                                <Card>
                                    <div class="p-6">
                                        <p class="text-error-foreground">"Failed to load workspace settings: " {msg}</p>
                                    </div>
                                </Card>
                            }.into_any()
                        },
                    }
                })}
            </Transition>
        </div>
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Workspace Name Card
// ─────────────────────────────────────────────────────────────────────────────

/// The workspace name field.
///
/// `name` and `baseline` are owned by [`WorkspacePage`], not by this card. That
/// is the point: a card that seeded its own signal from a `data` prop had that
/// signal rebuilt — and the admin's half-typed name thrown away — every time a
/// `workspace_settings` frame arrived.
#[component]
fn WorkspaceNameCard(name: RwSignal<String>, baseline: RwSignal<String>) -> impl IntoView {
    let save_action = Action::new(|name: &String| {
        let name = name.clone();
        async move { update_workspace_name(name).await }
    });

    let on_blur = move |_| {
        let current = name.get();
        // Unchanged is not a save. Writing on every blur would emit a
        // workspace-wide sync frame for tabbing through the field, and each of
        // those frames now makes every other admin's page refetch.
        if !current.trim().is_empty() && current != baseline.get() {
            save_action.dispatch(current);
        }
    };

    view! {
        <Card>
            <CardHeader>
                <div class="flex items-center justify-between">
                    <div>
                        <CardTitle>"Workspace Name"</CardTitle>
                        <CardDescription>
                            "Give your workspace a meaningful name to help identify it."
                        </CardDescription>
                    </div>
                    <ActionStatus action=save_action/>
                </div>
            </CardHeader>
            <CardContent>
                <input
                    type="text"
                    class=INPUT_CLASS
                    placeholder="My Workspace"
                    prop:value=name
                    on:input=move |ev| name.set(event_target_value(&ev))
                    on:blur=on_blur
                />
            </CardContent>
        </Card>
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Workspace Auto-Archive Card
// ─────────────────────────────────────────────────────────────────────────────

/// Build dropdown options for workspace-level auto-archive duration.
///
/// Simpler than the team-level dropdown — no "Workspace default" option since
/// this IS the workspace default.
fn workspace_archive_options() -> Vec<(String, String)> {
    vec![
        ("".to_string(), "Not set".to_string()),
        ("0".to_string(), "Never".to_string()),
        ("7".to_string(), "7 days".to_string()),
        ("14".to_string(), "14 days".to_string()),
        ("30".to_string(), "30 days".to_string()),
        ("60".to_string(), "60 days".to_string()),
        ("90".to_string(), "90 days".to_string()),
    ]
}

/// The auto-archive default.
///
/// `value` is owned by [`WorkspacePage`] for the same reason the name field is:
/// a card that seeds its own signal has that seed re-applied on every refetch.
/// This control commits on change rather than on blur, so it has no half-typed
/// state to lose — but it does have an in-flight write, and the page's
/// `adopt_unless_edited` is what keeps an incoming frame from flipping the
/// selection back while that write is still on the wire.
#[component]
fn WorkspaceArchiveCard(value: RwSignal<String>) -> impl IntoView {
    let save_action = Action::new(|days: &Option<u32>| {
        let days = *days;
        async move { update_workspace_auto_archive(days).await }
    });

    let on_archive_change = move |val: String| {
        value.set(val.clone());
        let days = if val.is_empty() {
            None
        } else {
            match val.parse::<u32>() {
                Ok(d) => Some(d),
                Err(e) => {
                    tracing::warn!(error = %e, value = %val, "Failed to parse workspace archive days value");
                    None
                }
            }
        };
        save_action.dispatch(days);
    };

    let options = workspace_archive_options();
    let options_signal = Signal::derive(move || options.clone());

    view! {
        <Card>
            <CardHeader>
                <div class="flex items-center justify-between">
                    <div>
                        <CardTitle>"Auto-archive"</CardTitle>
                        <CardDescription>
                            "Default auto-archive duration for all teams. Individual teams can override this setting."
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
                                value=value
                                options=options_signal
                                on_change=Callback::new(on_archive_change)
                                variant=SelectVariant::Form
                                placeholder="Not set"
                            />
                        </div>
                    </div>
                    <p class="text-xs text-muted-foreground">
                        "When not set, the system default of 30 days is used. Teams can override this with their own setting."
                    </p>
                </div>
            </CardContent>
        </Card>
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Teams Section
// ─────────────────────────────────────────────────────────────────────────────

#[component]
fn TeamsSection() -> impl IntoView {
    let (version, set_version) = signal(0u32);
    let teams = Resource::new(move || version.get(), |_| list_all_teams());
    let ws_default_id = Resource::new(move || version.get(), |_| get_workspace_default_team_id());
    let (show_create_modal, set_show_create_modal) = signal(false);

    let set_default_action = Action::new(move |id: &String| {
        let id = id.clone();
        async move { set_workspace_default_team(id).await }
    });

    // Refresh the team list when the set-default action succeeds.
    Effect::new(move || {
        if let Some(Ok(())) = set_default_action.value().get() {
            set_version.update(|v| *v += 1);
        }
    });

    view! {
        <Card>
            <CardHeader>
                <div class="flex items-center justify-between">
                    <div>
                        <CardTitle>"Teams"</CardTitle>
                        <CardDescription>
                            "Manage issue-tracker teams in this workspace."
                        </CardDescription>
                    </div>
                    <Button
                        variant=ButtonVariant::Default
                        on:click=move |_| set_show_create_modal.set(true)
                    >
                        "Create Team"
                    </Button>
                </div>
            </CardHeader>
            <CardContent>
                <Transition fallback=move || view! {
                    <div class="space-y-3">
                        <Skeleton class="h-12 w-full"/>
                        <Skeleton class="h-12 w-full"/>
                    </div>
                }>
                    {move || Suspend::new(async move {
                        let default_id = ws_default_id.await.ok().flatten();
                        match teams.await {
                            Ok(team_list) if team_list.is_empty() => {
                                view! {
                                    <EmptyState
                                        title="No teams yet"
                                        description="Create your first team to start tracking issues"
                                    />
                                }.into_any()
                            }
                            Ok(team_list) => {
                                view! { <div>{team_rows(team_list, default_id, set_default_action)}</div> }.into_any()
                            }
                            Err(e) => {
                                view! {
                                    <p class="text-sm text-error-foreground">{e.to_string()}</p>
                                }.into_any()
                            }
                        }
                    })}
                </Transition>
            </CardContent>
        </Card>
        <TeamCreationModal
            show=Signal::derive(move || show_create_modal.get())
            on_close=Callback::new(move |()| {
                set_show_create_modal.set(false);
                set_version.update(|v| *v += 1);
            })
        />
    }
}

/// Render each team as a linked row with icon, name, key badge, and default controls.
fn team_rows(
    team_list: Vec<Team>,
    default_id: Option<String>,
    set_default_action: Action<String, Result<(), ServerFnError>>,
) -> impl IntoView {
    team_list
        .into_iter()
        .map(|team| {
            let is_ws_default =
                default_id.as_deref() == Some(team.team_id.as_str());
            let href = format!("/teams/{}/settings", team.key.to_lowercase());
            let team_id_for_action = team.team_id.clone();

            view! {
                <a
                    href=href
                    class="flex items-center gap-3 py-2.5 px-2 border-b border-border last:border-b-0 hover:bg-muted/50 transition-colors rounded-sm group"
                >
                    <TeamIcon team=team.clone() size="28px"/>
                    <span class="text-sm font-medium text-foreground flex-1 min-w-0 truncate">
                        {team.name.clone()}
                    </span>
                    <Badge variant=BadgeVariant::Secondary>
                        {team.key.clone()}
                    </Badge>
                    {if is_ws_default {
                        view! {
                            <Badge variant=BadgeVariant::Default>
                                "Workspace default"
                            </Badge>
                        }.into_any()
                    } else {
                        view! {
                            <Button
                                variant=ButtonVariant::Ghost
                                size=ButtonSize::Sm
                                on:click=move |e: web_sys::MouseEvent| {
                                    e.prevent_default();
                                    e.stop_propagation();
                                    set_default_action.dispatch(team_id_for_action.clone());
                                }
                            >
                                "Set as default"
                            </Button>
                        }.into_any()
                    }}
                </a>
            }
        })
        .collect_view()
}


// ── Browser tests ───────────────────────────────────────────────────────────

/// Tests that a live snapshot cannot take back what the admin is typing.
///
/// These mount the real [`WorkspaceNameCard`] and assert on the rendered
/// `<input>`, because the DOM is what the admin is actually reading. A test that
/// only checked the signals would pass against a card that rebuilt itself and
/// re-seeded from the server on every frame, which is the defect.
///
/// # What these prove, and what they do not
///
/// There is no `Suspend` anywhere in this module's tree: every test mounts the
/// card directly. So these cover the card and the adoption rule — that the card
/// reads the page's signal instead of a copy, and that a fresh snapshot leaves
/// an unsaved edit alone. `the_admin_keeps_the_caret_while_a_snapshot_lands`
/// belongs to that same scope: it shows the card updating in place when its own
/// signal moves, which is not the same as surviving a rebuild from above.
///
/// The page renders these cards inside a suspense boundary, so a refetch does
/// rebuild them. That the rebuild costs neither the text nor the caret at real
/// server-function latency is measured in
/// [`super::live_update`](crate::pages::settings::live_update), which drives the
/// production shape end to end and records the latency sweep behind it.
///
/// Neither module covers the resource shell: `get_workspace_settings` is a
/// server function and cannot run in a browser test, so nothing here drives a
/// real refetch. That the page wires the rule to the resource is established by
/// reading it.
///
/// Run with:
/// `wasm-pack test --headless --firefox crates/trakkt-ui --lib --features hydrate`
#[cfg(all(test, target_arch = "wasm32"))]
mod wasm_tests {
    use gloo_timers::future::TimeoutFuture;
    use wasm_bindgen::JsCast;
    use wasm_bindgen_test::{wasm_bindgen_test, wasm_bindgen_test_configure};

    use super::*;

    wasm_bindgen_test_configure!(run_in_browser);

    /// The card's `<input>` inside `container`, found the way a user finds it:
    /// by what is rendered.
    ///
    /// Scoped to the container rather than the document because these tests
    /// share one browser page — an unscoped lookup finds whichever card mounted
    /// first and silently tests the wrong one.
    fn name_input(container: &web_sys::HtmlElement) -> web_sys::HtmlInputElement {
        let node = container
            .query_selector("input[placeholder='My Workspace']")
            .expect("query failed")
            .expect("the workspace name input is not rendered");
        node.dyn_into::<web_sys::HtmlInputElement>()
            .expect("the workspace name field is not an input")
    }

    /// Type `text` into `input` the way a user does — set the value, then let
    /// the `on:input` handler see it.
    fn type_into(input: &web_sys::HtmlInputElement, text: &str) {
        input.set_value(text);
        let ev = web_sys::Event::new("input").expect("could not build an input event");
        input.dispatch_event(&ev).expect("could not dispatch input");
    }

    fn active_tag() -> Option<String> {
        web_sys::window()?
            .document()?
            .active_element()
            .map(|e| e.tag_name())
    }

    /// Mount the real card into a container of its own and run `test`.
    async fn with_mounted_card<F: Future<Output = ()>>(
        test: impl FnOnce(web_sys::HtmlElement, RwSignal<String>, RwSignal<String>) -> F,
    ) {
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

        let name = RwSignal::new("Acme".to_owned());
        let baseline = RwSignal::new("Acme".to_owned());

        let handle = leptos::mount::mount_to(container.clone(), move || {
            view! { <WorkspaceNameCard name=name baseline=baseline/> }
        });
        TimeoutFuture::new(30).await;

        test(container.clone(), name, baseline).await;

        // `UnmountHandle` tears the view down on drop; the container goes with it
        // so the next test's `query_selector` cannot find this one's card.
        drop(handle);
        container.remove();
    }

    #[wasm_bindgen_test]
    async fn an_incoming_snapshot_does_not_take_back_what_the_admin_typed() {
        with_mounted_card(|container, name, baseline| async move {
            let input = name_input(&container);
            assert_eq!(input.value(), "Acme", "the card should render the loaded name");

            // The admin starts renaming the workspace, mid-word.
            type_into(&input, "Acme Holdi");
            TimeoutFuture::new(20).await;

            // A workspace_settings frame arrives and the page adopts the
            // snapshot it refetched. This is reachable with no race at all —
            // the admin's own save produces one of these.
            adopt_unless_edited(name, baseline, "Globex".to_owned());
            TimeoutFuture::new(20).await;

            assert_eq!(
                name_input(&container).value(),
                "Acme Holdi",
                "the half-typed name must still be in the box. Losing it is silent data \
                 loss on a text field: the admin gets no warning, and the server cannot \
                 give the text back"
            );
        })
        .await;
    }

    #[wasm_bindgen_test]
    async fn an_untouched_field_shows_what_the_other_admin_changed_it_to() {
        with_mounted_card(|container, name, baseline| async move {
            // Nobody is typing; a rename arrives from another admin.
            adopt_unless_edited(name, baseline, "Globex".to_owned());
            TimeoutFuture::new(20).await;

            assert_eq!(
                name_input(&container).value(),
                "Globex",
                "with nothing to protect, the field must follow the server — this is the \
                 staleness the refetch was added for, and a fix that simply stopped \
                 adopting would defeat it"
            );
        })
        .await;
    }

    #[wasm_bindgen_test]
    async fn the_admin_keeps_the_caret_while_a_snapshot_lands() {
        with_mounted_card(|container, name, baseline| async move {
            let input = name_input(&container);
            input.focus().expect("could not focus the name field");
            assert_eq!(active_tag().as_deref(), Some("INPUT"));

            adopt_unless_edited(name, baseline, "Globex".to_owned());
            TimeoutFuture::new(20).await;

            let after = name_input(&container);
            assert!(
                input.is_same_node(Some(after.as_ref())),
                "the input was replaced rather than updated — a rebuilt field cannot keep \
                 either the caret or an unsaved edit"
            );
            assert_eq!(
                active_tag().as_deref(),
                Some("INPUT"),
                "the caret jumped out of the field mid-word. This is what rendering the \
                 card own its displayed value would. This test covers the card in \
                 isolation; that it also survives the page's suspense boundary rebuilding \
                 it is covered by `a_server_function_round_trip_costs_neither_the_text_nor_the_caret`"
            );
        })
        .await;
    }
}
