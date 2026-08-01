// SPDX-License-Identifier: AGPL-3.0-or-later

//! Folding a live server snapshot into a field the user might be editing.
//!
//! The settings pages read their data through server functions and refetch when
//! a `workspace_settings` or `notification_preferences` frame arrives. That is
//! what stops them showing a stale value forever — and it is also what can take
//! a half-typed workspace name away from the person typing it, because a
//! refetch delivers a whole snapshot and the naive thing to do with a snapshot
//! is to write all of it into the form.
//!
//! [`adopt_unless_edited`] is the rule those pages apply instead. It is a free
//! function over two signals rather than something the cards do for themselves,
//! because the cards must not own this state at all: state owned by a card is
//! re-seeded every time the card is rebuilt, which is exactly the clobber.

use leptos::prelude::*;

/// Take `remote` as the field's value, unless the user has edited it.
///
/// `local` is what is in the control. `baseline` is the last value the server
/// told us, and the comparison between them is the whole definition of "the
/// user has edited this": a field equal to its baseline is showing the server's
/// value untouched, so a new snapshot can replace it freely. A field that
/// differs is holding something the user typed and has not saved.
///
/// # What happens to the remote value mid-edit
///
/// It is not thrown away — it becomes the new `baseline`. That matters, and it
/// is the deliberate half of this decision:
///
/// * The edit in progress wins the display, because discarding it is silent
///   data loss and the user is the only one who can retype it.
/// * The remote value still becomes the thing the field is compared against, so
///   "edited" keeps meaning "differs from current server truth" rather than
///   "differs from whatever the server said when the page loaded". Without this,
///   a field could be stuck looking dirty against a value nobody holds any more,
///   and would never adopt another snapshot for the rest of the session.
/// * When the user saves, their value supersedes the remote one. That is
///   last-write-wins, which is what the API already does for two admins saving
///   concurrently — this rule does not invent a conflict policy, it matches the
///   one the server has.
/// * If the user abandons the edit by restoring the field to the baseline, it
///   is clean again and the next snapshot adopts normally.
///
/// The alternative — letting the remote value win — trades silent loss of the
/// user's work for silent loss of someone else's, and the user's work is the
/// half that cannot be recovered from the server.
pub fn adopt_unless_edited<T>(local: RwSignal<T>, baseline: RwSignal<T>, remote: T)
where
    T: PartialEq + Clone + Send + Sync + 'static,
{
    if local.get_untracked() == baseline.get_untracked() {
        local.set(remote.clone());
    }
    baseline.set(remote);
}

#[cfg(all(test, target_arch = "wasm32"))]
mod wasm_tests {
    use wasm_bindgen_test::{wasm_bindgen_test, wasm_bindgen_test_configure};

    use super::*;

    wasm_bindgen_test_configure!(run_in_browser);

    fn with_owner(test: impl FnOnce()) {
        let owner = Owner::new();
        owner.set();
        test();
    }

    #[wasm_bindgen_test]
    fn an_untouched_field_takes_the_incoming_value() {
        with_owner(|| {
            let local = RwSignal::new("Acme".to_owned());
            let baseline = RwSignal::new("Acme".to_owned());

            adopt_unless_edited(local, baseline, "Globex".to_owned());

            assert_eq!(
                local.get_untracked(),
                "Globex",
                "a field the user has not touched must follow the server — this is the \
                 staleness the refetch exists to fix"
            );
        });
    }

    #[wasm_bindgen_test]
    fn an_edited_field_keeps_what_the_user_typed() {
        with_owner(|| {
            let local = RwSignal::new("Acme".to_owned());
            let baseline = RwSignal::new("Acme".to_owned());

            local.set("Acme Holdi".to_owned()); // mid-word
            adopt_unless_edited(local, baseline, "Globex".to_owned());

            assert_eq!(
                local.get_untracked(),
                "Acme Holdi",
                "an unsaved edit must survive an incoming frame — losing it is silent data \
                 loss, and the server cannot give it back"
            );
        });
    }

    #[wasm_bindgen_test]
    fn the_incoming_value_still_becomes_the_baseline() {
        with_owner(|| {
            let local = RwSignal::new("Acme".to_owned());
            let baseline = RwSignal::new("Acme".to_owned());

            local.set("Acme Holdi".to_owned());
            adopt_unless_edited(local, baseline, "Globex".to_owned());

            assert_eq!(
                baseline.get_untracked(),
                "Globex",
                "the remote value is not discarded, it becomes what `edited` is measured \
                 against — otherwise the field stays dirty against a value nobody holds and \
                 never adopts another snapshot"
            );
        });
    }

    #[wasm_bindgen_test]
    fn abandoning_an_edit_lets_the_field_follow_the_server_again() {
        with_owner(|| {
            let local = RwSignal::new("Acme".to_owned());
            let baseline = RwSignal::new("Acme".to_owned());

            local.set("Acme Holdi".to_owned());
            adopt_unless_edited(local, baseline, "Globex".to_owned());

            // The user gives up and restores the field to what the server holds.
            local.set("Globex".to_owned());
            adopt_unless_edited(local, baseline, "Initech".to_owned());

            assert_eq!(
                local.get_untracked(),
                "Initech",
                "a field is only 'edited' while it differs from server truth — once it \
                 matches again it must resume following the server, or one abandoned edit \
                 would freeze it for the rest of the session"
            );
        });
    }
}

// ── Why the cards stay inside the suspense boundary ─────────────────────────

/// The measurement behind leaving `Suspend` where it was.
///
/// A refetch rebuilds whatever the suspense boundary wraps. [`adopt_unless_edited`]
/// is what keeps that rebuild from taking the value in a field the admin is
/// editing. The open question was whether the rebuild *also* costs the caret,
/// because if it did the cards would have to move outside the boundary — which
/// would cost this page its server-rendered content.
///
/// It does, but only in shapes a `#[server]` function cannot have. Measured over
/// three passes, six identical observations per row, order-independent:
///
/// | fetcher resolves | `Resource` | `LocalResource` |
/// |---|---|---|
/// | on the first poll (never awaits) | caret kept | **caret lost** |
/// | after one microtask (`wake_by_ref`) | **caret lost** | caret kept |
/// | after a `0ms` timer | caret kept | caret kept |
/// | after `1ms` / `10ms` / `50ms` | caret kept | caret kept |
///
/// The two losing cells are degenerate and mutually exclusive, and neither is
/// reachable from a server function: an HTTP round trip always resolves on a
/// macrotask, which the `0ms` row already covers — `setTimeout(0)` is faster
/// than any network call, warm localhost included. So there is no latency
/// threshold to worry about; the boundary is "does the fetcher cross a
/// macrotask at all", and production is always on the safe side of it.
///
/// This is recorded rather than summarised because it was got wrong twice from
/// probes that each looked sound. Two earlier traps, both since ruled out:
/// node identity survives a detach-and-re-attach, so `is_same_node` reports
/// success even where the caret is lost; and focus on an element rendered
/// beside the boundary rather than inside it is never disturbed.
#[cfg(all(test, target_arch = "wasm32"))]
mod latency_tests {
    use std::future::Future;
    use std::pin::Pin;
    use std::task::{Context, Poll};

    use gloo_timers::future::TimeoutFuture;
    use leptos::prelude::*;
    use wasm_bindgen::JsCast;
    use wasm_bindgen_test::{wasm_bindgen_test, wasm_bindgen_test_configure};

    use crate::wasm_test_support::boot_leptos_executor;

    use super::*;

    wasm_bindgen_test_configure!(run_in_browser);

    /// A real timer that is nominally `Send`, so it can go inside a `Resource`.
    ///
    /// `TimeoutFuture` is `!Send`, which would otherwise leave `Resource` — the
    /// half of this that `WorkspacePage` uses — untestable at realistic latency.
    /// WASM is single-threaded, so the wrapper is sound.
    struct SendTimer(send_wrapper::SendWrapper<TimeoutFuture>);

    impl Future for SendTimer {
        type Output = ();
        fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
            Pin::new(&mut *self.0).poll(cx)
        }
    }

    /// Yields exactly once, re-woken immediately: the fastest a future can be
    /// while still crossing the async boundary.
    struct YieldOnce(bool);

    impl Future for YieldOnce {
        type Output = ();
        fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
            if self.0 {
                Poll::Ready(())
            } else {
                self.0 = true;
                cx.waker().wake_by_ref();
                Poll::Pending
            }
        }
    }

    /// The field the real cards are: it renders a signal it does not own.
    #[component]
    fn HoistedField(text: RwSignal<String>) -> impl IntoView {
        view! { <input class="probe" prop:value=text/> }
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

    fn field_in(container: &web_sys::HtmlElement) -> web_sys::HtmlInputElement {
        container
            .query_selector("input.probe")
            .expect("query failed")
            .expect("the probe field is not rendered")
            .dyn_into()
            .expect("the probe field is not an input")
    }

    fn focused_tag() -> String {
        web_sys::window()
            .expect("no window")
            .document()
            .expect("no document")
            .active_element()
            .map(|e| e.tag_name())
            .unwrap_or_else(|| "<none>".to_owned())
    }

    struct Outcome {
        focus_before: String,
        focus_after: String,
        value_after: String,
        boundary_runs: u32,
    }

    /// Drive `Resource` — what `WorkspacePage` uses — through the production
    /// shape: `Transition` + `Suspend`, a hoisted signal, and the snapshot
    /// folded in from inside the suspended body.
    async fn drive_resource(millis: Option<u32>) -> Outcome {
        let container = make_container();
        let version = RwSignal::new(0u32);
        let resource = Resource::new(
            move || version.get(),
            move |v| async move {
                match millis {
                    None => YieldOnce(false).await,
                    Some(ms) => {
                        SendTimer(send_wrapper::SendWrapper::new(TimeoutFuture::new(ms))).await
                    }
                }
                v
            },
        );
        let text = RwSignal::new(String::new());
        let baseline = RwSignal::new(String::new());
        let runs = RwSignal::new(0u32);

        let handle = leptos::mount::mount_to(container.clone(), move || {
            view! {
                <Transition fallback=|| view! { <p>"loading"</p> }>
                    {move || Suspend::new(async move {
                        let v = resource.await;
                        runs.update_untracked(|n| *n += 1);
                        adopt_unless_edited(text, baseline, format!("server-{v}"));
                        view! { <HoistedField text=text/> }
                    })}
                </Transition>
            }
        });

        TimeoutFuture::new(150).await;
        let field = field_in(&container);
        field.focus().expect("could not focus the field");
        let focus_before = focused_tag();

        // The admin starts typing, then a frame lands.
        text.set("Acme Holdi".to_owned());
        TimeoutFuture::new(30).await;
        version.set(1);
        TimeoutFuture::new(150).await;

        let outcome = Outcome {
            focus_before,
            focus_after: focused_tag(),
            value_after: field_in(&container).value(),
            boundary_runs: runs.get_untracked(),
        };
        drop(handle);
        container.remove();
        outcome
    }

    #[wasm_bindgen_test]
    async fn a_server_function_round_trip_costs_neither_the_text_nor_the_caret() {
        boot_leptos_executor();

        // 1ms stands in for the server function. Anything that resolves on a
        // macrotask behaves identically — the sweep in this module's docs covers
        // 0ms through 50ms on both resource types.
        let realistic = drive_resource(Some(1)).await;

        assert_eq!(
            realistic.focus_before, "INPUT",
            "the field never took focus, so nothing was measured"
        );
        assert_eq!(
            realistic.boundary_runs, 2,
            "the suspended body did not rebuild, so nothing was measured"
        );
        assert_eq!(
            realistic.value_after, "Acme Holdi",
            "the half-typed name must survive the rebuild — this is the data loss, and it \
             is `adopt_unless_edited` that prevents it, not where the cards render"
        );
        assert_eq!(
            realistic.focus_after, "INPUT",
            "the caret must survive too. If this ever fails, the cards have to move outside \
             the suspense boundary after all, at the cost of this page's server-rendered \
             content — do not make that trade without re-running the latency sweep in this \
             module's docs first"
        );
    }

    #[wasm_bindgen_test]
    async fn only_a_fetcher_that_never_crosses_a_macrotask_costs_the_caret() {
        boot_leptos_executor();

        // The shape that produced the original, wrong diagnosis. Kept so the
        // claim stays falsifiable: it is not that suspense boundaries are
        // harmless, it is that this one is only harmful somewhere production
        // cannot reach.
        let microtask_only = drive_resource(None).await;

        assert_eq!(microtask_only.focus_before, "INPUT");
        assert_eq!(microtask_only.boundary_runs, 2);
        assert_eq!(
            microtask_only.value_after, "Acme Holdi",
            "the text survives even here — losing the text and losing the caret have \
             different causes, and only the caret depends on the fetcher's timing"
        );
        assert_eq!(
            microtask_only.focus_after, "BODY",
            "a `Resource` whose fetcher resolves after only a microtask does lose the caret. \
             If this starts passing, Leptos has changed and the note in this module's docs \
             is stale"
        );
    }
}
