// SPDX-License-Identifier: AGPL-3.0-or-later

//! Notification preferences settings page.
//!
//! Lets users configure which event types generate notifications and whether
//! to receive self-notifications for agent/API actions.

use leptos::prelude::*;
use phosphor_leptos::{Icon, IconWeight};

use crate::components::{
    Alert, AlertDescription, AlertVariant, Card, CardContent, CardHeader, CardTitle,
    SettingsPageSkeleton, Switch,
};
use crate::pages::settings::live_update::adopt_unless_edited;
use crate::server_fns::notifications::{get_notification_preferences, update_notification_preference};
use trakkt_types::models::NotificationPreferences;

// ─────────────────────────────────────────────────────────────────────────────
// Main page
// ─────────────────────────────────────────────────────────────────────────────

#[component]
pub fn NotificationsPage() -> impl IntoView {
    // Preferences are read through a server function, not from the sync store,
    // so the store's counter is the only reactive dependency that can tell this
    // page the same user changed a toggle on another tab or another device.
    //
    // The counter is resolved once, here, and not inside the fetcher.
    // `notification_preferences_version` builds a fresh `Signal` wrapper on
    // every call, and each wrapper is an owner-registered arena item — calling
    // it from a closure that re-runs allocates another one per run, under
    // whichever owner happens to be current at the time.
    let sync_store = use_context::<crate::cache::store::SyncStore>();
    let prefs_version = sync_store.map(|s| s.notification_preferences_version());
    let prefs_resource = LocalResource::new(move || {
        // Tracked for the dependency, not for the value: `LocalResource` has no
        // separate source argument, so it re-runs on whatever its fetcher reads
        // synchronously.
        if let Some(version) = prefs_version {
            version.track();
        }
        get_notification_preferences()
    });

    // One state object per switch, created here rather than inside the row.
    //
    // The rows render inside the suspense boundary, so every refetch rebuilds
    // them — and a row that owned its optimistic value had that value recreated
    // from the server snapshot on each rebuild. A click whose write was still on
    // the wire got silently reverted, and the `Action` carrying it was disposed
    // with the old row, so a failed save lost even its revert. Owning the state
    // here is what the workspace name field does, for the same reason.
    let event_rows = ToggleRow::build(EVENT_TOGGLES);
    let agent_rows = ToggleRow::build(AGENT_TOGGLES);
    let rows_for_adoption = [event_rows.clone(), agent_rows.clone()].concat();

    view! {
        <div class="p-4 sm:p-6">
            <h2 class="text-xl font-display text-foreground mb-4">"Notification Preferences"</h2>
            <p class="text-muted-foreground mb-6">
                "Control which events notify you and how."
            </p>

            <Transition fallback=move || view! { <SettingsPageSkeleton /> }>
                {move || {
                    let event_rows = event_rows.clone();
                    let agent_rows = agent_rows.clone();
                    let rows_for_adoption = rows_for_adoption.clone();
                    Suspend::new(async move {
                        match prefs_resource.await {
                            Ok(loaded) => {
                                // Folded in here, not from an `Effect`, so the
                                // first render already has the values — effects
                                // do not run during SSR. Per switch rather than
                                // over the snapshot as a whole: a coarse
                                // comparison would let one in-flight toggle
                                // block every other field from ever updating
                                // again.
                                for row in &rows_for_adoption {
                                    adopt_unless_edited(
                                        row.state.enabled,
                                        row.state.baseline,
                                        (row.spec.read)(&loaded),
                                    );
                                }
                                view! {
                                    <div class="space-y-6">
                                        <EventTypesCard rows=event_rows />
                                        <AgentApiCard rows=agent_rows />
                                        <DeliveryCard />
                                    </div>
                                }.into_any()
                            }
                            Err(e) => {
                                let msg = e.to_string();
                                view! {
                                    <Card>
                                        <div class="p-6">
                                            <Alert variant=AlertVariant::Error>
                                                <AlertDescription>
                                                    "Failed to load notification preferences: " {msg}
                                                </AlertDescription>
                                            </Alert>
                                        </div>
                                    </Card>
                                }.into_any()
                            }
                        }
                    })
                }}
            </Transition>
        </div>
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Switch definitions and their client-side state
// ─────────────────────────────────────────────────────────────────────────────

/// One switch on this page: what it writes, what it says, and how to read its
/// current value out of a snapshot.
///
/// The rows are a table rather than thirteen hand-written call sites so that the
/// page can build one [`ToggleState`] per switch without a second list to keep
/// in step with the first.
struct ToggleSpec {
    /// The database column, and the value `update_notification_preference` takes.
    field: &'static str,
    label: &'static str,
    description: &'static str,
    read: fn(&NotificationPreferences) -> bool,
}

const EVENT_TOGGLES: &[ToggleSpec] = &[
    ToggleSpec {
        field: "notify_status_changes",
        label: "Status changes",
        description: "When an issue's status is updated",
        read: |p| p.notify_status_changes,
    },
    ToggleSpec {
        field: "notify_comments",
        label: "Comments",
        description: "When a comment is added to a watched issue",
        read: |p| p.notify_comments,
    },
    ToggleSpec {
        field: "notify_assignments",
        label: "Assignments",
        description: "When an issue is assigned to someone",
        read: |p| p.notify_assignments,
    },
    ToggleSpec {
        field: "notify_priority_changes",
        label: "Priority changes",
        description: "When an issue's priority is updated",
        read: |p| p.notify_priority_changes,
    },
    ToggleSpec {
        field: "notify_label_changes",
        label: "Label changes",
        description: "When labels are added or removed from an issue",
        read: |p| p.notify_label_changes,
    },
    ToggleSpec {
        field: "notify_due_date_changes",
        label: "Due date changes",
        description: "When an issue's due date is set, changed, or cleared",
        read: |p| p.notify_due_date_changes,
    },
    ToggleSpec {
        field: "notify_estimate_changes",
        label: "Estimate changes",
        description: "When an issue's estimate is changed",
        read: |p| p.notify_estimate_changes,
    },
    ToggleSpec {
        field: "notify_milestone_changes",
        label: "Milestone changes",
        description: "When an issue's milestone is set, changed, or cleared",
        read: |p| p.notify_milestone_changes,
    },
    ToggleSpec {
        field: "notify_project_changes",
        label: "Project changes",
        description: "When an issue's project is set, changed, or cleared",
        read: |p| p.notify_project_changes,
    },
    ToggleSpec {
        field: "notify_team_changes",
        label: "Team changes",
        description: "When an issue is moved between teams",
        read: |p| p.notify_team_changes,
    },
    ToggleSpec {
        field: "notify_relation_changes",
        label: "Relation changes",
        description: "When a relation is added to an issue",
        read: |p| p.notify_relation_changes,
    },
];

const AGENT_TOGGLES: &[ToggleSpec] = &[
    ToggleSpec {
        field: "notify_own_agent_actions",
        label: "Notify me of actions by agents on my behalf",
        description: "MCP agents, automation bots, and similar integrations",
        read: |p| p.notify_own_agent_actions,
    },
    ToggleSpec {
        field: "notify_own_api_actions",
        label: "Notify me of actions by API integrations on my behalf",
        description: "API token-based integrations and scripts",
        read: |p| p.notify_own_api_actions,
    },
];

/// One switch's client-side state, owned by [`NotificationsPage`].
///
/// `enabled` is what the switch shows; `baseline` is the last value the server
/// confirmed. The pair means the same thing here as it does for the workspace
/// name field, and feeds the same [`adopt_unless_edited`] rule — "edited" is
/// just narrower: for a text field it is what the user typed, for a switch it is
/// an optimistic value the server has not echoed back yet. Either way it is the
/// value a fresh snapshot must not overwrite.
///
/// `save` lives here too, not in the row. An `Action` created inside the row is
/// disposed when the row is rebuilt, taking the in-flight request's result with
/// it — so a save that failed during a rebuild would never revert the switch and
/// never report anything.
///
/// `checked` is the read-only view of `enabled` that [`PreferenceToggle`] hands
/// to `<Switch>`, and it is built here for a sharper reason than the others:
/// building it in the row is what made `/settings/notifications` panic on load
/// (#282, reverted by #283). See its field comment.
#[derive(Clone, Copy)]
struct ToggleState {
    enabled: RwSignal<bool>,
    /// What `<Switch checked=…>` reads. Constructed under the page's owner.
    ///
    /// A `Signal<T>` is an owner-registered arena item however it is built —
    /// `Signal::derive`, `Signal::from`, `.into()`, or a `#[prop(into)]`
    /// conversion written at a call site — and it is disposed with whichever
    /// owner was current when it was built. Built inside [`PreferenceToggle`],
    /// it is disposed every time the page's `Transition` rebuilds the row,
    /// while `enabled` goes on living at the page.
    ///
    /// That gap is what panicked. `Signal::derive` passes reads through rather
    /// than caching them, so the switch's `class` `RenderEffect` subscribed to
    /// `enabled` itself, not to the wrapper. The rebuild disposed the wrapper
    /// but left that subscription in place; the next write to `enabled` woke the
    /// dead effect, it read the disposed wrapper, and
    /// `reactive_graph` raised "tried to access a reactive value that has
    /// already been disposed", which surfaced as `unreachable!()` in tachys'
    /// class rendering.
    ///
    /// So the rule this field exists to keep is: a `Signal<T>` must be built
    /// under an owner that outlives every effect that reads it. Swapping
    /// `Signal::derive` for `.into()` does not help — same owner, same arena,
    /// same bug.
    checked: Signal<bool>,
    baseline: RwSignal<bool>,
    save: Action<bool, Result<NotificationPreferences, ServerFnError>>,
}

/// A switch and the state that outlives its row.
#[derive(Clone, Copy)]
struct ToggleRow {
    spec: &'static ToggleSpec,
    state: ToggleState,
}

impl ToggleRow {
    /// Build the rows for `specs`, creating their state under the caller's
    /// reactive owner.
    ///
    /// Call this from the page, never from a row: signals created under a row's
    /// owner are disposed when that row is rebuilt, which is the whole bug.
    fn build(specs: &'static [ToggleSpec]) -> Vec<ToggleRow> {
        specs
            .iter()
            .map(|spec| ToggleRow {
                spec,
                state: ToggleState::new(spec.field),
            })
            .collect()
    }
}

impl ToggleState {
    fn new(field: &'static str) -> Self {
        let enabled = RwSignal::new(false);
        let baseline = RwSignal::new(false);
        let save = Action::new(move |new_value: &bool| {
            let value = *new_value;
            async move { update_notification_preference(field.to_string(), value).await }
        });

        // A failed save used to revert the switch and log, and nothing more:
        // the switch flipped back on its own with no explanation. It is
        // surfaced now because the revert is otherwise indistinguishable from
        // the user's click not registering.
        let show_error = crate::components::toast::capture_error_toast();
        Effect::new(move |_| {
            if let Some(Err(e)) = save.value().get() {
                tracing::warn!("Failed to update notification preference {field}: {e}");
                // Back to what the server holds, rather than negating what is on
                // screen: the snapshot can have moved under an in-flight write,
                // and only the baseline is known to be true.
                enabled.set(baseline.get_untracked());
                show_error("Could not save that preference. Please try again.".to_owned());
            }
        });

        Self {
            enabled,
            checked: Signal::from(enabled),
            baseline,
            save,
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Event types card
// ─────────────────────────────────────────────────────────────────────────────

#[component]
fn EventTypesCard(rows: Vec<ToggleRow>) -> impl IntoView {
    view! {
        <Card>
            <CardHeader>
                <div class="flex items-center gap-2">
                    <Icon icon=phosphor_leptos::FUNNEL weight=IconWeight::Regular size="20px" attr:class="text-muted-foreground"/>
                    <CardTitle>"Event Types"</CardTitle>
                </div>
            </CardHeader>
            <CardContent>
                <p class="text-sm text-muted-foreground mb-4">
                    "Choose which types of issue events generate notifications."
                </p>
                <div class="space-y-1">
                    {rows.into_iter()
                        .map(|row| view! { <PreferenceToggle row=row /> })
                        .collect_view()}
                </div>
            </CardContent>
        </Card>
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Agent & API card
// ─────────────────────────────────────────────────────────────────────────────

#[component]
fn AgentApiCard(rows: Vec<ToggleRow>) -> impl IntoView {
    view! {
        <Card>
            <CardHeader>
                <div class="flex items-center gap-2">
                    <Icon icon=phosphor_leptos::ROBOT weight=IconWeight::Regular size="20px" attr:class="text-muted-foreground"/>
                    <CardTitle>"Agent & API"</CardTitle>
                </div>
            </CardHeader>
            <CardContent>
                <p class="text-sm text-muted-foreground mb-4">
                    "By default, actions you trigger through agents or the API do not generate "
                    "self-notifications. Enable these to be notified of your own automated actions."
                </p>
                <div class="space-y-1">
                    {rows.into_iter()
                        .map(|row| view! { <PreferenceToggle row=row /> })
                        .collect_view()}
                </div>
            </CardContent>
        </Card>
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Delivery card
// ─────────────────────────────────────────────────────────────────────────────

#[component]
fn DeliveryCard() -> impl IntoView {
    view! {
        <Card>
            <CardHeader>
                <div class="flex items-center gap-2">
                    <Icon icon=phosphor_leptos::PAPER_PLANE_TILT weight=IconWeight::Regular size="20px" attr:class="text-muted-foreground"/>
                    <CardTitle>"Delivery"</CardTitle>
                </div>
            </CardHeader>
            <CardContent>
                <p class="text-sm text-muted-foreground mb-4">
                    "How you receive notifications."
                </p>
                <div class="space-y-3">
                    // In-app — active
                    <div class="flex items-center justify-between py-2">
                        <div class="flex items-center gap-3">
                            <Icon icon=phosphor_leptos::BELL weight=IconWeight::Regular size="18px" attr:class="text-foreground"/>
                            <div>
                                <p class="text-sm font-medium text-foreground">"In-app"</p>
                                <p class="text-xs text-muted-foreground">"Notifications appear in your inbox"</p>
                            </div>
                        </div>
                        <span class="text-xs font-medium text-primary bg-primary/10 px-2 py-1 rounded">"Active"</span>
                    </div>

                    // Email — coming soon
                    <div class="flex items-center justify-between py-2 opacity-50">
                        <div class="flex items-center gap-3">
                            <Icon icon=phosphor_leptos::ENVELOPE weight=IconWeight::Regular size="18px" attr:class="text-muted-foreground"/>
                            <div>
                                <p class="text-sm font-medium text-muted-foreground">"Email"</p>
                                <p class="text-xs text-muted-foreground">"Receive notifications via email"</p>
                            </div>
                        </div>
                        <span class="text-xs font-medium text-muted-foreground bg-muted px-2 py-1 rounded">"Coming soon"</span>
                    </div>
                </div>
            </CardContent>
        </Card>
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Toggle row
// ─────────────────────────────────────────────────────────────────────────────

/// A single preference toggle row.
///
/// The row owns no reactive value at all, and that is load-bearing twice over.
/// Its optimistic value, its baseline and its save action live on
/// [`NotificationsPage`], because this row is rebuilt every time the page
/// refetches and anything it owned would be rebuilt with it — discarding a click
/// still on the wire and disposing the request carrying it.
///
/// The `Signal<bool>` handed to `<Switch>` lives there too, as
/// [`ToggleState::checked`]. Constructing one here — `Signal::derive`,
/// `.into()`, anything — is what made this page panic on load in #282. Do not
/// build a `Signal` in this function.
#[component]
fn PreferenceToggle(row: ToggleRow) -> impl IntoView {
    let state = row.state;
    let on_change = Callback::new(move |new_value: bool| {
        // Optimistic: the switch has to move under the finger. Until the server
        // echoes this back it counts as an edit, which is what stops an
        // unrelated snapshot from taking it away.
        state.enabled.set(new_value);
        state.save.dispatch(new_value);
    });

    view! {
        <div class="flex items-center justify-between py-2">
            <div class="flex-1 min-w-0 pr-4">
                <p class="text-sm font-medium text-foreground">{row.spec.label}</p>
                <p class="text-xs text-muted-foreground">{row.spec.description}</p>
            </div>
            <Switch
                checked=state.checked
                on_change=on_change
            />
        </div>
    }
}

// ── Browser tests ───────────────────────────────────────────────────────────

/// Run with:
/// `wasm-pack test --headless --firefox crates/trakkt-ui --lib --features hydrate`
#[cfg(all(test, target_arch = "wasm32"))]
mod wasm_tests {
    use std::cell::Cell;
    use std::rc::Rc;

    use gloo_timers::future::TimeoutFuture;
    use leptos::prelude::*;
    use wasm_bindgen_test::{wasm_bindgen_test, wasm_bindgen_test_configure};

    use crate::cache::store::SyncStore;

    use super::*;

    wasm_bindgen_test_configure!(run_in_browser);

    /// `LocalResource::new` takes no source argument, unlike the `Resource::new`
    /// every sibling page uses: it re-runs on whatever its fetcher reads
    /// synchronously. `NotificationsPage` above rests entirely on that, and a
    /// fetcher that did not re-track would leave the store's
    /// `notification_preferences` counter a signal with no subscriber — the
    /// exact defect the counter exists to fix, and one that every other test in
    /// this repository would stay green through. So the behaviour is pinned here
    /// rather than assumed.
    fn make_container() -> web_sys::HtmlElement {
        use wasm_bindgen::JsCast;
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

    /// What the switch is telling the user and a screen reader.
    fn aria_checked(container: &web_sys::HtmlElement) -> Option<String> {
        container
            .query_selector("button[role='switch']")
            .expect("query failed")
            .expect("the switch is not rendered")
            .get_attribute("aria-checked")
    }

    /// A click still on the wire must survive the row being rebuilt.
    ///
    /// The rows render inside a suspense boundary, so any refetch rebuilds them.
    /// While the row owned its own optimistic value, that rebuild recreated the
    /// value from the last server snapshot and the click vanished — the switch
    /// flipped back with no explanation, and the `Action` carrying the write was
    /// disposed along with it.
    ///
    /// The optimistic value is set directly rather than by clicking, because
    /// clicking dispatches a real server function that has no server to reach in
    /// a browser test. What it stands for is exactly the state a click leaves
    /// behind: `enabled` moved, `baseline` not yet, because the server has not
    /// echoed anything back.
    #[wasm_bindgen_test]
    async fn an_in_flight_toggle_survives_the_row_being_rebuilt() {
        let owner = Owner::new();
        owner.set();

        let container = make_container();
        let version = RwSignal::new(0u32);
        let resource = LocalResource::new(move || {
            let v = version.get();
            async move {
                TimeoutFuture::new(1).await;
                v
            }
        });
        let rebuilds = RwSignal::new(0u32);

        // Built under this test's owner, standing in for the page's.
        let row = ToggleRow::build(EVENT_TOGGLES)[0];

        let handle = leptos::mount::mount_to(container.clone(), move || {
            view! {
                <Transition fallback=|| view! { <p>"loading"</p> }>
                    {move || Suspend::new(async move {
                        resource.await;
                        rebuilds.update_untracked(|n| *n += 1);
                        view! { <PreferenceToggle row=row /> }
                    })}
                </Transition>
            }
        });

        TimeoutFuture::new(150).await;
        assert_eq!(
            aria_checked(&container).as_deref(),
            Some("false"),
            "the switch should start from the server's value"
        );

        // The user flips it. The write is still on the wire, so the server has
        // echoed nothing and the baseline is untouched.
        row.state.enabled.set(true);
        TimeoutFuture::new(30).await;
        assert_eq!(
            aria_checked(&container).as_deref(),
            Some("true"),
            "the switch must move under the finger"
        );

        // Another frame lands — the user's own change from a second tab, say —
        // and the boundary rebuilds the row.
        version.set(1);
        TimeoutFuture::new(150).await;

        assert_eq!(
            rebuilds.get_untracked(),
            2,
            "the row was not rebuilt, so this measured nothing"
        );
        assert_eq!(
            aria_checked(&container).as_deref(),
            Some("true"),
            "the in-flight click must survive the rebuild. If it does not, the switch \
             silently flips back while the write is still in progress, and the user is \
             told nothing"
        );

        drop(handle);
        container.remove();
    }

    /// A write that lands *after* a rebuild still reaches the switch.
    ///
    /// This is the sequence #282 shipped without and `/settings/notifications`
    /// panicked on. Its sibling above writes to `enabled` and *then* rebuilds;
    /// that order never touches the signal again once the old row is gone, so
    /// it stayed green while the page was broken. The order here is the other
    /// way round — rebuild first, then write — which is what a second frame
    /// arriving after a refetch actually does.
    ///
    /// What made that fatal: `PreferenceToggle` built the `Signal<bool>` for
    /// `<Switch checked=…>` itself, so the rebuild disposed it while
    /// `state.enabled` lived on at the page. `Signal::derive` passes reads
    /// through rather than caching them, so the old switch's `class` effect was
    /// subscribed to `state.enabled` directly; this write woke it, it read the
    /// disposed wrapper, and `reactive_graph` raised "tried to access a reactive
    /// value that has already been disposed" — surfacing as `unreachable!()` in
    /// tachys' class rendering. `ToggleState::checked` is what removes the
    /// dangling read.
    ///
    /// # What this test can and cannot see
    ///
    /// It asserts the DOM, not the panic. A wasm panic raised inside a queued
    /// render effect does not unwind into this test's call stack — it escapes as
    /// an uncaught exception, which is exactly why `wasm-pack` reported green
    /// through the regression. What this catches is the consequence: the write
    /// is dropped and the switch stops tracking `enabled`. The panic itself is
    /// asserted on in the browser, by `e2e/tests/sync/panic-probe.spec.ts`,
    /// which fails on any `pageerror` or panicking console line. Both are
    /// needed; neither replaces the other.
    #[wasm_bindgen_test]
    async fn a_write_after_the_row_is_rebuilt_still_reaches_the_switch() {
        let owner = Owner::new();
        owner.set();

        let container = make_container();
        let version = RwSignal::new(0u32);
        let resource = LocalResource::new(move || {
            let v = version.get();
            async move {
                TimeoutFuture::new(1).await;
                v
            }
        });
        let rebuilds = RwSignal::new(0u32);

        // Built under this test's owner, standing in for the page's — the row
        // must not own it.
        let row = ToggleRow::build(EVENT_TOGGLES)[0];

        let handle = leptos::mount::mount_to(container.clone(), move || {
            view! {
                <Transition fallback=|| view! { <p>"loading"</p> }>
                    {move || Suspend::new(async move {
                        let v = resource.await;
                        rebuilds.update_untracked(|n| *n += 1);
                        // The page folds the refetched snapshot in from inside
                        // the suspended body, before rendering the rows
                        // (`NotificationsPage`, the `adopt_unless_edited` loop).
                        // That is the write this test exists for, and doing it
                        // out here instead — after the rebuild has settled —
                        // does not reproduce the fault.
                        adopt_unless_edited(row.state.enabled, row.state.baseline, v > 0);
                        view! { <PreferenceToggle row=row /> }
                    })}
                </Transition>
            }
        });

        TimeoutFuture::new(150).await;
        assert_eq!(aria_checked(&container).as_deref(), Some("false"));

        // The refetch lands: the boundary rebuilds the row, and the snapshot
        // folded in during that rebuild writes to the page-owned `enabled` the
        // row being torn down is reading through.
        version.set(1);
        TimeoutFuture::new(200).await;
        assert_eq!(
            rebuilds.get_untracked(),
            2,
            "the row was not rebuilt, so this measured nothing"
        );

        assert_eq!(
            aria_checked(&container).as_deref(),
            Some("true"),
            "the refetched value did not reach the switch. The row is reading `enabled` \
             through a `Signal` disposed with the previous rebuild, which is the \
             disposed-value panic this page was reverted for — build that `Signal` in \
             `ToggleState::new`, under the page's owner, not in `PreferenceToggle`"
        );

        drop(handle);
        container.remove();
    }

    /// The row still follows the server for values the user has not touched.
    #[wasm_bindgen_test]
    async fn a_toggle_follows_the_server_when_nothing_is_in_flight() {
        let owner = Owner::new();
        owner.set();

        let container = make_container();
        let row = ToggleRow::build(EVENT_TOGGLES)[0];
        let handle = leptos::mount::mount_to(container.clone(), move || {
            view! { <PreferenceToggle row=row /> }
        });
        TimeoutFuture::new(30).await;
        assert_eq!(aria_checked(&container).as_deref(), Some("false"));

        // A snapshot arrives with this preference on, and nothing local to keep.
        adopt_unless_edited(row.state.enabled, row.state.baseline, true);
        TimeoutFuture::new(30).await;

        assert_eq!(
            aria_checked(&container).as_deref(),
            Some("true"),
            "with nothing in flight the switch must follow the server — a fix that simply \
             stopped adopting would leave the page stale, which is what this ticket set out \
             to fix"
        );

        drop(handle);
        container.remove();
    }

    #[wasm_bindgen_test]
    async fn a_local_resource_refetches_when_a_signal_its_fetcher_reads_bumps() {
        let owner = Owner::new();
        owner.set();

        let store = SyncStore::new();
        // Resolved once and tracked inside the fetcher — the same shape
        // `NotificationsPage` uses, so a change to that shape breaks this.
        let version = store.notification_preferences_version();
        let runs = Rc::new(Cell::new(0u32));

        let counted = Rc::clone(&runs);
        let _resource = LocalResource::new(move || {
            version.track();
            counted.set(counted.get() + 1);
            async move {}
        });

        TimeoutFuture::new(20).await;
        let before = runs.get();
        assert!(before > 0, "the fetcher never ran at all");

        store.bump_notification_preferences_version();
        TimeoutFuture::new(20).await;

        assert!(
            runs.get() > before,
            "`LocalResource` did not re-run its fetcher when a signal that fetcher reads \
             changed. `NotificationsPage` wires the store's notification_preferences counter \
             in exactly this way, so this failing means that page never refetches and the \
             counter has no subscriber at all."
        );
    }
}
