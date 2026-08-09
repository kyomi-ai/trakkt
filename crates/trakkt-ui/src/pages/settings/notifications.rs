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
    // The counter is resolved once, here, and moved into the fetcher.
    let sync_store = use_context::<crate::cache::store::SyncStore>();
    let prefs_version = sync_store.map(|s| s.notification_preferences_version());
    let prefs_resource = LocalResource::new(move || {
        // Tracked for the dependency, not for the value: `LocalResource` has no
        // separate source argument, so it re-runs on whatever its fetcher reads
        // synchronously. The `&` matters: `notification_preferences_version`
        // returns an `ArcSignal<u32>`, which is `Clone` and not `Copy`, so
        // binding by value would move out of the capture and leave the fetcher
        // `FnOnce`. The borrow is what keeps `track()` running on every fetch,
        // and `track()` on every fetch is the entire subscription — see
        // `a_local_resource_refetches_when_a_signal_its_fetcher_reads_bumps`
        // below.
        if let Some(version) = &prefs_version {
            version.track();
        }
        get_notification_preferences()
    });

    // One state object per switch, created here rather than inside the row.
    //
    // The rows render inside the suspense boundary, so every refetch rebuilds
    // them — and a row that owned its optimistic value had that value recreated
    // from the server snapshot on each rebuild. A click whose write was still on
    // the wire got silently reverted, and the `Effect` that would have reported
    // a failed save went with the old row, so the failure passed unremarked.
    // Owning the state here is what the workspace name field does, for the same
    // reason.
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
/// `save` lives here too, not in the row, and so does the `Effect` watching it.
/// Both halves of that are load-bearing, and for reasons that were measured
/// rather than reasoned about — see [`row_owned_save_probe`] at the foot of
/// this file, which drives a real `Action` through a real rebuild and records
/// what came out:
///
/// * Leave the `Effect` in the row and it is never woken again once the row is
///   disposed, so the answer arrives with nobody listening. A save that failed
///   during a rebuild reverted nothing and said nothing.
/// * Hoist the `Effect` but leave the `Action` in the row and it *is* woken —
///   and panics reading an arena entry the row took with it.
///
/// What does **not** happen is the request being lost. Its future runs to
/// completion whether the row is still there or not: `Action::dispatch` spawns
/// it holding its own reference-counted handles to the action's value, and the
/// `ScopedFuture` wrapper round it only re-sets the owner while polling. So
/// what hoisting protects is the observer, not the request.
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
        Self::with_saver(field, move |value| {
            update_notification_preference(field.to_string(), value)
        })
    }

    /// The whole of [`ToggleState::new`], with the one thing a browser test
    /// cannot have — the network call — left as a parameter.
    ///
    /// Everything downstream of the dispatch is the interesting part and is
    /// shared: the `Action`'s in-flight bookkeeping, the `Effect` watching its
    /// value, the revert to `baseline`, the toast. A test that reimplemented
    /// any of that would be asserting against a copy, so the seam is put here,
    /// at the request itself, and nothing else forks. `new` is the only caller
    /// in the shipped binary and it passes the real server function.
    fn with_saver<Fut>(
        field: &'static str,
        save_fn: impl Fn(bool) -> Fut + Send + Sync + 'static,
    ) -> Self
    where
        Fut: std::future::Future<Output = Result<NotificationPreferences, ServerFnError>>
            + Send
            + 'static,
    {
        let enabled = RwSignal::new(false);
        let baseline = RwSignal::new(false);
        let save = Action::new(move |new_value: &bool| save_fn(*new_value));

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
/// still on the wire, and disposing the `Action` and the `Effect` that stand
/// between the user and the answer to it.
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

// ── A save the test drives instead of the server ────────────────────────────

/// The fixture the browser tests below dispatch through a real [`Action`].
///
/// Clicking the switch dispatches a `#[server]` function, and a browser test
/// has no server to answer it. Substituting the request — and only the request
/// — is the same trade `live_update::latency_tests` makes when it puts a timer
/// where a fetch would be: the `Action`, its in-flight bookkeeping, the
/// `Effect` watching its value, the revert and the toast are all the shipped
/// code, reached through [`ToggleState::with_saver`].
///
/// The completion is a latch rather than a timer on purpose. "The save was
/// still on the wire when the row rebuilt" is an ordering, and a timer makes it
/// a race the test can lose without noticing — which is the shape of failure
/// this ticket exists to stop repeating.
///
/// [`Gate`], [`SaveLog`] and [`LocalFuture`] are the parts of that with nothing
/// preference-shaped about them, and they live in
/// [`crate::wasm_test_support`] so the team icon picker's tests drive their
/// save through the same latch rather than a second copy of it. What stays here
/// is only what knows about `NotificationPreferences`.
#[cfg(all(test, target_arch = "wasm32"))]
mod save_fixture {
    use std::sync::Arc;

    use leptos::prelude::*;
    use send_wrapper::SendWrapper;
    use trakkt_types::models::NotificationPreferences;

    pub use crate::wasm_test_support::{Gate, LocalFuture, SaveLog};

    /// What one fixture save resolves to — the shipped `Action`'s output type,
    /// unchanged.
    pub type SaveResult = Result<NotificationPreferences, ServerFnError>;

    /// A boxed fixture save, so the harness components below can take one as a
    /// plain prop instead of being generic over the future's type.
    pub type FixtureSave = Arc<dyn Fn(bool) -> LocalFuture<SaveResult> + Send + Sync>;

    /// A save that records itself, waits for `gate`, and then either succeeds
    /// or fails.
    pub fn gated_save(gate: Gate, log: SaveLog, succeeds: bool) -> FixtureSave {
        let shared = SendWrapper::new((gate, log));
        Arc::new(move |value: bool| {
            let (gate, log) = (*shared).clone();
            LocalFuture::new(async move {
                log.record_start();
                gate.wait().await;
                log.record_finish();
                if succeeds {
                    Ok(saved_preferences(value))
                } else {
                    Err(ServerFnError::new("the fixture save was told to fail"))
                }
            })
        })
    }

    /// A snapshot standing for whatever the server would have echoed back.
    ///
    /// The shipped code never reads this payload — the page refetches for that
    /// — so every flag simply takes `value`.
    fn saved_preferences(value: bool) -> NotificationPreferences {
        NotificationPreferences {
            preference_id: "fixture-preference".to_owned(),
            user_id: "fixture-user".to_owned(),
            workspace_id: "fixture-workspace".to_owned(),
            notify_status_changes: value,
            notify_comments: value,
            notify_assignments: value,
            notify_priority_changes: value,
            notify_label_changes: value,
            notify_due_date_changes: value,
            notify_estimate_changes: value,
            notify_milestone_changes: value,
            notify_project_changes: value,
            notify_team_changes: value,
            notify_relation_changes: value,
            notify_own_agent_actions: value,
            notify_own_api_actions: value,
            delivery_channel: "in_app".to_owned(),
        }
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
    use crate::wasm_test_support::{boot_leptos_executor, mount_container};

    use super::save_fixture::{FixtureSave, Gate, SaveLog, gated_save};
    use super::*;

    wasm_bindgen_test_configure!(run_in_browser);

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
    ///
    /// So this covers the value and not the `Action`.
    /// [`a_save_that_fails_after_the_row_is_rebuilt_still_puts_the_switch_back`]
    /// covers the `Action`: it clicks, and puts a fixture in place of the
    /// server rather than skipping the dispatch.
    #[wasm_bindgen_test]
    async fn an_in_flight_toggle_survives_the_row_being_rebuilt() {
        boot_leptos_executor();

        let owner = Owner::new();
        owner.set();

        let container = mount_container();
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
        boot_leptos_executor();

        let owner = Owner::new();
        owner.set();

        let container = mount_container();
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
        boot_leptos_executor();

        let owner = Owner::new();
        owner.set();

        let container = mount_container();
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

    // ── Driving the real `Action` ────────────────────────────────────────────

    /// Click something the way a user does.
    ///
    /// `pub(super)` for `row_owned_action_probe`, which dispatches its save the
    /// same way.
    pub(super) fn click(container: &web_sys::HtmlElement, selector: &str) {
        use wasm_bindgen::JsCast;
        let target: web_sys::HtmlElement = container
            .query_selector(selector)
            .expect("query failed")
            .unwrap_or_else(|| panic!("nothing matched {selector}"))
            .dyn_into()
            .unwrap_or_else(|_| panic!("{selector} did not match an html element"));
        target.click();
    }

    /// The message of every toast currently on screen, as the user reads it.
    ///
    /// Selected by the animation class `ToastItem` renders, which is the only
    /// thing distinguishing a toast in the DOM — it carries no role and no
    /// `aria-*`. If that class is ever renamed these tests fail rather than
    /// quietly stop looking, which is the behaviour worth having. The message
    /// `<p>` rather than the toast itself, because the toast's text also
    /// contains the "x" on its dismiss button.
    fn toast_texts(container: &web_sys::HtmlElement) -> Vec<String> {
        let nodes = container
            .query_selector_all(".animate-slide-in-right > p")
            .expect("query failed");
        (0..nodes.length())
            .filter_map(|i| nodes.item(i).and_then(|n| n.text_content()))
            .collect()
    }

    /// `NotificationsPage`'s shape with the server taken out.
    ///
    /// The state is built here — inside the toast provider and *above* the
    /// suspense boundary — because that is where the page builds it. The row is
    /// rendered inside the boundary, so a refetch rebuilds it, which is the
    /// event under test.
    #[component]
    fn ProbeRows(saver: FixtureSave, version: RwSignal<u32>, rebuilds: RwSignal<u32>) -> impl IntoView {
        let spec = &EVENT_TOGGLES[0];
        let state = ToggleState::with_saver(spec.field, move |value| saver(value));
        let row = ToggleRow { spec, state };

        let resource = LocalResource::new(move || {
            let v = version.get();
            async move {
                TimeoutFuture::new(1).await;
                v
            }
        });

        view! {
            <Transition fallback=|| view! { <p>"loading"</p> }>
                {move || Suspend::new(async move {
                    resource.await;
                    rebuilds.update_untracked(|n| *n += 1);
                    view! { <PreferenceToggle row=row /> }
                })}
            </Transition>
        }
    }

    /// Mount the harness and hand back its container.
    fn mount_probe_rows(
        saver: FixtureSave,
        version: RwSignal<u32>,
        rebuilds: RwSignal<u32>,
    ) -> (web_sys::HtmlElement, impl Sized) {
        let container = mount_container();
        let handle = leptos::mount::mount_to(container.clone(), move || {
            view! {
                <crate::components::toast::ToastProvider>
                    <ProbeRows saver=saver version=version rebuilds=rebuilds />
                </crate::components::toast::ToastProvider>
            }
        });
        (container, handle)
    }

    /// A real save, dispatched by a real click, failing after a real rebuild.
    ///
    /// `an_in_flight_toggle_survives_the_row_being_rebuilt` above writes to
    /// `enabled` instead of clicking, so nothing in it ever touches the
    /// `Action`. This one goes through `<Switch on_change>` →
    /// `PreferenceToggle` → `state.save.dispatch`, holds the request open
    /// across a genuine rebuild of the row, and only then lets it fail. What a
    /// reader sees at the end — `aria-checked` back to `false` — is only
    /// reachable if the `Action` dispatched before the rebuild and the `Effect`
    /// watching it both outlived that rebuild.
    ///
    /// # Why it cannot pass vacuously
    ///
    /// Three preconditions, each fatal on its own: `SaveLog::started` proves
    /// the click reached a real `Action` rather than only moving the switch;
    /// `rebuilds == 2` proves the boundary rebuilt the row; and
    /// `SaveLog::finished == 0` *at that moment* proves the save was still on
    /// the wire when it did, rather than having quietly resolved first. A timer
    /// instead of the gate would leave the last of those to chance.
    #[wasm_bindgen_test]
    async fn a_save_that_fails_after_the_row_is_rebuilt_still_puts_the_switch_back() {
        boot_leptos_executor();

        let owner = Owner::new();
        owner.set();

        let gate = Gate::default();
        let log = SaveLog::default();
        let version = RwSignal::new(0u32);
        let rebuilds = RwSignal::new(0u32);
        let (container, handle) = mount_probe_rows(
            gated_save(gate.clone(), log.clone(), false),
            version,
            rebuilds,
        );

        TimeoutFuture::new(150).await;
        assert_eq!(
            aria_checked(&container).as_deref(),
            Some("false"),
            "the switch should start from the server's value"
        );

        click(&container, "button[role='switch']");
        TimeoutFuture::new(30).await;
        assert_eq!(
            log.started(),
            1,
            "the click did not dispatch the action, so this test measured nothing"
        );
        assert_eq!(
            aria_checked(&container).as_deref(),
            Some("true"),
            "the switch must move under the finger"
        );

        // A refetch lands while the save is still on the wire.
        version.set(1);
        TimeoutFuture::new(200).await;
        assert_eq!(
            rebuilds.get_untracked(),
            2,
            "the row was not rebuilt, so this measured nothing"
        );
        assert_eq!(
            log.finished(),
            0,
            "the save resolved before the rebuild, so it never crossed one and this measured \
             nothing"
        );
        assert_eq!(
            aria_checked(&container).as_deref(),
            Some("true"),
            "the in-flight click must survive the rebuild"
        );

        // The server finally answers, and it answers no.
        gate.open();
        TimeoutFuture::new(60).await;
        assert_eq!(log.finished(), 1, "the save's future never completed");
        assert_eq!(
            aria_checked(&container).as_deref(),
            Some("false"),
            "a save that fails after its row was rebuilt must still put the switch back. If it \
             does not, the switch is left showing a value the server rejected, and the user is \
             never told"
        );

        drop(handle);
        container.remove();
    }

    /// A failed save has to say why the switch moved back.
    ///
    /// The revert on its own is indistinguishable from the click not having
    /// registered: the switch returns to where it was and nothing explains it.
    /// This asserts both halves of the answer — the value and the sentence.
    ///
    /// # Why it cannot pass vacuously
    ///
    /// `SaveLog::started` proves a real `Action` ran, `SaveLog::finished`
    /// proves it resolved, and the toast list is asserted empty *before* the
    /// gate opens, so the toast asserted afterwards cannot be one left over
    /// from mounting.
    #[wasm_bindgen_test]
    async fn a_failed_save_puts_the_switch_back_and_says_why() {
        boot_leptos_executor();

        let owner = Owner::new();
        owner.set();

        let gate = Gate::default();
        let log = SaveLog::default();
        let version = RwSignal::new(0u32);
        let rebuilds = RwSignal::new(0u32);
        let (container, handle) = mount_probe_rows(
            gated_save(gate.clone(), log.clone(), false),
            version,
            rebuilds,
        );

        TimeoutFuture::new(150).await;
        assert_eq!(aria_checked(&container).as_deref(), Some("false"));
        assert!(
            toast_texts(&container).is_empty(),
            "a toast was already on screen before anything was saved, so the assertion below \
             would not have measured this save"
        );

        click(&container, "button[role='switch']");
        TimeoutFuture::new(30).await;
        assert_eq!(
            log.started(),
            1,
            "the click did not dispatch the action, so this test measured nothing"
        );
        assert_eq!(aria_checked(&container).as_deref(), Some("true"));

        gate.open();
        TimeoutFuture::new(60).await;
        assert_eq!(log.finished(), 1, "the save's future never completed");

        assert_eq!(
            aria_checked(&container).as_deref(),
            Some("false"),
            "a failed save must put the switch back to what the server holds"
        );
        assert_eq!(
            toast_texts(&container),
            vec!["Could not save that preference. Please try again.".to_owned()],
            "a failed save must say so. Without it the switch flips back on its own and the \
             user cannot tell a rejected save from a click that never registered"
        );

        drop(handle);
        container.remove();
    }

    /// `LocalResource::new` takes no source argument, unlike the `Resource::new`
    /// every sibling page uses: it re-runs on whatever its fetcher reads
    /// synchronously. `NotificationsPage` above rests entirely on that, and a
    /// fetcher that did not re-track would leave the store's
    /// `notification_preferences` counter a signal with no subscriber — the
    /// exact defect the counter exists to fix, and one that every other test in
    /// this repository would stay green through. So the behaviour is pinned here
    /// rather than assumed.
    #[wasm_bindgen_test]
    async fn a_local_resource_refetches_when_a_signal_its_fetcher_reads_bumps() {
        boot_leptos_executor();

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

// ── What a row-owned save actually does when the row is rebuilt ─────────────

/// The measurement behind [`ToggleState`] owning `save` and the `Effect` that
/// watches it.
///
/// An earlier wording of that field's docs said an `Action` built in the row is
/// "disposed when the row is rebuilt, taking the in-flight request's result
/// with it". A review probe reported the opposite: that the future is not
/// cancelled, and that the watching effect still fires afterwards without
/// panicking. Neither was ever run against the other, so both are re-run here
/// in the production shape — a real click, a real `Action`, a real rebuild of
/// the row while the request is still open, and only then a failure.
///
/// One variable separates the two committed runs: which owner the `Effect`
/// belongs to. Everything else is held equal. The third column is the positive
/// control — the same harness with the shipped hoisting applied, obtained by
/// temporarily building both the `Action` and the `Effect` under the page's
/// owner. It is what makes the other two columns attributable to the hoisting
/// and not to the harness.
///
/// | | `Action` in row,<br>`Effect` in row | `Action` in row,<br>`Effect` on page | both on the page<br>(shipped) |
/// |---|---|---|---|
/// | the request's future ran to completion | yes | yes | yes |
/// | the effect was woken after the row was disposed | no | yes | yes |
/// | that wake finished | — | **no — it panicked** | yes |
/// | the switch went back to the server's value | **no** | **no** | yes |
///
/// The event logs behind those columns, verbatim. `rowN` is the Nth build of
/// the row; `effect entered` is recorded *before* `save.value()` is read, so a
/// run that panicked on the read leaves an `entered` line with no `read` after
/// it:
///
/// ```text
/// Effect in the row:
///   row0 effect entered, row0 effect read none, row0 disposed,
///   row1 effect entered, row1 effect read none
///
/// Effect on the page:
///   row0 effect entered, row0 effect read none, row0 disposed,
///   row1 effect entered, row1 effect read none, row0 effect entered
///                                               ^^^^^^^^^^^^^^^^^^^
///   no `read` follows: that run panicked in
///   reactive_graph-0.2.14/src/actions/action.rs:945, which is
///   `Action::value`'s `unwrap_signal!` on an arena entry that is gone.
///
/// Both on the page (the positive control, not committed):
///   row0 effect entered, row0 effect read none, row0 disposed,
///   row1 effect entered, row1 effect read none, row0 effect entered,
///   row0 effect read err, row0 reverted
///   ^ the row is still disposed; the save is not, so the failure is seen and
///     acted on. This is what the shipped page does.
/// ```
///
/// So the review probe was half right, and the old wording was right about the
/// outcome for the wrong reason:
///
/// * **The request is not lost.** It ran to completion in both runs.
///   `Action::dispatch` spawns the future holding its own reference-counted
///   handles to the action's value and version, and the `ScopedFuture` wrapper
///   round it only re-sets the owner while polling — it does not cancel.
///   Nothing takes the in-flight request's result anywhere.
/// * **The observer is lost.** A row-owned `Effect` is never woken again once
///   its row is disposed, so the answer arrives with nobody listening.
/// * **Hoisting the observer on its own does not fix it.** It is then woken,
///   and panics reading an `Action` the row took with it.
///
/// Which makes hoisting the whole of `save` load-bearing rather than merely
/// defensive — by a different route from the one the old wording described.
/// The consequence it named is real and is the `reverted` row above: in both
/// runs the switch is left showing a value the server rejected, and nothing is
/// said about it.
///
/// # How this is shaped to avoid the trap next door
///
/// `live_update::latency_tests` records two probes in this area that gave
/// clean, confident, wrong answers by measuring a proxy. The proxy on offer
/// here is "did the effect run", and it is not enough: an effect that runs and
/// panics on its first read is indistinguishable from one that never ran if all
/// you count is completed runs — and a panic inside a spawned task on this
/// target does not reach the test's call stack, so it fails silently. Hence a
/// line recorded on entry as well as after the read, and instruments held in
/// `Rc<Cell<_>>` rather than in signals, so that disposal cannot reach the
/// apparatus measuring disposal.
///
/// [`a_hoisted_watcher_over_a_row_owned_action_fares_no_better`] leaves an
/// uncaught panic in the browser console on every run. That panic is the
/// finding, not a defect in the test.
#[cfg(all(test, target_arch = "wasm32"))]
mod row_owned_save_probe {
    use std::cell::{Cell, RefCell};
    use std::rc::Rc;

    use gloo_timers::future::TimeoutFuture;
    use leptos::prelude::*;
    use send_wrapper::SendWrapper;
    use wasm_bindgen_test::{wasm_bindgen_test, wasm_bindgen_test_configure};

    use crate::wasm_test_support::{boot_leptos_executor, mount_container};

    use super::save_fixture::{FixtureSave, Gate, SaveLog, gated_save};
    use super::wasm_tests::click;

    wasm_bindgen_test_configure!(run_in_browser);

    /// Everything the probe records.
    ///
    /// `Rc<Cell<_>>` and `Rc<RefCell<_>>` rather than signals, deliberately: a
    /// probe about what disposal does must not be read through instruments that
    /// disposal can reach.
    #[derive(Clone, Default)]
    struct Instruments {
        rows_built: Rc<Cell<u32>>,
        rows_disposed: Rc<Cell<u32>>,
        events: Rc<RefCell<Vec<String>>>,
    }

    impl Instruments {
        fn record(&self, event: String) {
            self.events.borrow_mut().push(event);
        }

        fn events(&self) -> Vec<String> {
            self.events.borrow().clone()
        }
    }

    /// What one run left behind.
    struct Outcome {
        future_completed: bool,
        reverted: bool,
        events: Vec<String>,
    }

    /// A row that owns its `Action` — the shape TRA-9977 hoisted away from.
    ///
    /// `enabled` and `baseline` stay the page's, as they are in the shipped
    /// code, so the revert stays observable whatever becomes of the row.
    ///
    /// `watcher_owner` is the single variable: `None` builds the `Effect` here
    /// beside the `Action`, which is the pre-TRA-9977 shape exactly; `Some`
    /// builds it under the page's owner instead, leaving the `Action` alone in
    /// the row. Holding everything else equal is what makes the two runs a
    /// comparison rather than two anecdotes.
    #[component]
    fn RowOwningItsSave(
        enabled: RwSignal<bool>,
        baseline: RwSignal<bool>,
        saver: FixtureSave,
        probe: Instruments,
        watcher_owner: Option<Owner>,
    ) -> impl IntoView {
        let row = probe.rows_built.get();
        probe.rows_built.set(row + 1);

        let save = Action::new(move |value: &bool| saver(*value));

        let watcher = probe.clone();
        let build_watcher = move || {
            Effect::new(move |_| {
                // Recorded before the read as well as after, so a panic on
                // reading a disposed `Action` is distinguishable from the
                // effect simply never running again: the "entered" line then
                // stands alone with no "read" after it.
                watcher.record(format!("row{row} effect entered"));
                let value = save.value().get();
                let seen = match value {
                    None => "none",
                    Some(Ok(_)) => "ok",
                    Some(Err(_)) => "err",
                };
                watcher.record(format!("row{row} effect read {seen}"));
                if matches!(value, Some(Err(_))) {
                    enabled.set(baseline.get_untracked());
                    watcher.record(format!("row{row} reverted"));
                }
            });
        };
        match &watcher_owner {
            Some(owner) => owner.with(build_watcher),
            None => build_watcher(),
        }

        let on_dispose = SendWrapper::new(probe.clone());
        on_cleanup(move || {
            on_dispose
                .rows_disposed
                .set(on_dispose.rows_disposed.get() + 1);
            on_dispose.record(format!("row{row} disposed"));
        });

        view! {
            <button class="probe-save" on:click=move |_| {
                enabled.set(true);
                save.dispatch(true);
            }>"save"</button>
        }
    }

    /// The page's half: it holds the boundary, with the row inside it.
    #[component]
    fn ProbeHost(
        enabled: RwSignal<bool>,
        baseline: RwSignal<bool>,
        saver: FixtureSave,
        probe: Instruments,
        version: RwSignal<u32>,
        rebuilds: RwSignal<u32>,
        hoist_watcher: bool,
    ) -> impl IntoView {
        // Taken here, above the boundary: the owner `NotificationsPage` builds
        // its `ToggleState` under.
        let page_owner = Owner::current().expect("the probe host has no reactive owner");
        let watcher_owner = hoist_watcher.then_some(page_owner);

        let resource = LocalResource::new(move || {
            let v = version.get();
            async move {
                TimeoutFuture::new(1).await;
                v
            }
        });
        let shared = SendWrapper::new((saver, probe, watcher_owner));

        view! {
            <Transition fallback=|| view! { <p>"loading"</p> }>
                {move || {
                    let shared = shared.clone();
                    Suspend::new(async move {
                        resource.await;
                        rebuilds.update_untracked(|n| *n += 1);
                        let (saver, probe, watcher_owner) = (*shared).clone();
                        view! {
                            <RowOwningItsSave
                                enabled=enabled
                                baseline=baseline
                                saver=saver
                                probe=probe
                                watcher_owner=watcher_owner
                            />
                        }
                    })
                }}
            </Transition>
        }
    }

    /// Dispatch a row-owned save, rebuild the row while it is still on the
    /// wire, then let it fail — and report what is left.
    ///
    /// The preconditions live here rather than in the callers because a run
    /// that skipped the dispatch, the rebuild or the disposal would hand back a
    /// tidy-looking [`Outcome`] that measured nothing at all.
    async fn drive(hoist_watcher: bool) -> Outcome {
        let container = mount_container();
        let gate = Gate::default();
        let log = SaveLog::default();
        let probe = Instruments::default();
        let enabled = RwSignal::new(false);
        let baseline = RwSignal::new(false);
        let version = RwSignal::new(0u32);
        let rebuilds = RwSignal::new(0u32);

        let saver = gated_save(gate.clone(), log.clone(), false);
        let mounted_probe = probe.clone();
        let handle = leptos::mount::mount_to(container.clone(), move || {
            view! {
                <ProbeHost
                    enabled=enabled
                    baseline=baseline
                    saver=saver
                    probe=mounted_probe
                    version=version
                    rebuilds=rebuilds
                    hoist_watcher=hoist_watcher
                />
            }
        });

        TimeoutFuture::new(150).await;
        click(&container, "button.probe-save");
        TimeoutFuture::new(30).await;
        assert_eq!(
            log.started(),
            1,
            "the click did not dispatch the row-owned action, so this measured nothing"
        );
        assert!(
            enabled.get_untracked(),
            "the click did not move the optimistic value, so there is nothing left to revert \
             and this measured nothing"
        );

        version.set(1);
        TimeoutFuture::new(200).await;
        assert_eq!(
            rebuilds.get_untracked(),
            2,
            "the row was not rebuilt, so this measured nothing"
        );
        assert_eq!(
            probe.rows_disposed.get(),
            1,
            "the row that dispatched was not disposed, so this measured nothing"
        );
        assert_eq!(
            log.finished(),
            0,
            "the save resolved before the rebuild, so it never crossed one and this measured \
             nothing"
        );

        // The server finally answers, and it answers no.
        gate.open();
        TimeoutFuture::new(150).await;

        let outcome = Outcome {
            future_completed: log.finished() == 1,
            reverted: enabled.get_untracked() == baseline.get_untracked(),
            events: probe.events(),
        };
        drop(handle);
        container.remove();
        outcome
    }

    /// The pre-TRA-9977 shape: the failure is lost in silence.
    #[wasm_bindgen_test]
    async fn a_row_that_owns_its_whole_save_loses_the_failure_in_silence() {
        boot_leptos_executor();
        let owner = Owner::new();
        owner.set();

        let outcome = drive(false).await;

        assert!(
            outcome.future_completed,
            "the request's future did not run to completion. Disposing its row is not supposed \
             to cancel it — if this starts failing, that has changed and the table in this \
             module's docs is stale"
        );
        assert_eq!(
            outcome.events,
            [
                "row0 effect entered",
                "row0 effect read none",
                "row0 disposed",
                "row1 effect entered",
                "row1 effect read none",
            ],
            "a row-owned effect is expected to go quiet the moment its row is disposed, so the \
             log ends with the replacement row's first run and never returns to row0. A \
             `row0 effect read err` line appearing after `row0 disposed` would mean a row-owned \
             watcher survives its row after all"
        );
        assert!(
            !outcome.reverted,
            "the switch went back to the server's value with the whole save inside the row, \
             which is the outcome hoisting it exists to produce — so hoisting is now pointless \
             and this module's conclusion is wrong"
        );
    }

    /// Hoisting only the watcher moves the failure rather than removing it.
    ///
    /// This run leaves an uncaught panic in the browser console. That is the
    /// result being recorded, not a defect: the surviving effect is woken by
    /// the answer and dies reading an `Action` that went with the row.
    #[wasm_bindgen_test]
    async fn a_hoisted_watcher_over_a_row_owned_action_fares_no_better() {
        boot_leptos_executor();
        let owner = Owner::new();
        owner.set();

        let outcome = drive(true).await;

        assert!(
            outcome.future_completed,
            "the request's future did not run to completion even with the watcher hoisted"
        );
        assert_eq!(
            outcome.events,
            [
                "row0 effect entered",
                "row0 effect read none",
                "row0 disposed",
                "row1 effect entered",
                "row1 effect read none",
                "row0 effect entered",
            ],
            "the hoisted watcher is expected to be woken after its row was disposed and then to \
             panic reading the disposed `Action` — which is why the log ends on an `entered` \
             with no `read` after it. A `row0 effect read err` here would mean a disposed \
             `Action` is readable again, and hoisting the `Action` itself would no longer be \
             load-bearing"
        );
        assert!(
            !outcome.reverted,
            "hoisting the watcher alone was enough to get the switch back, so the `Action` no \
             longer needs hoisting and this module's conclusion is wrong"
        );
    }
}
