// SPDX-License-Identifier: AGPL-3.0-or-later

//! Shared canonical detail links and clipboard feedback.

use leptos::prelude::*;

use super::{Alert, AlertDescription, AlertVariant, Button, ButtonSize, ButtonVariant};

pub(crate) fn issue_link_path(
    route_team: &str,
    route_number: i32,
    loaded_team: &str,
    loaded_number: i32,
) -> Option<String> {
    (route_team == loaded_team && route_number == loaded_number)
        .then(|| format!("/issues/{loaded_team}-{loaded_number}"))
}

pub(crate) fn project_link_path(route_id: &str, loaded_id: &str) -> Option<String> {
    (route_id == loaded_id).then(|| format!("/projects/{loaded_id}"))
}

#[cfg(any(target_arch = "wasm32", test))]
fn absolute_link(origin: &str, path: &str) -> String {
    format!("{origin}{path}")
}

#[cfg(target_arch = "wasm32")]
const CLIPBOARD_ERROR: &str =
    "Could not copy link. Allow clipboard access in your browser and try again.";

#[cfg(target_arch = "wasm32")]
const CLIPBOARD_UNAVAILABLE: &str =
    "Clipboard is unavailable. Open this page over HTTPS and allow clipboard access.";

#[cfg(target_arch = "wasm32")]
async fn clipboard_result(promise: js_sys::Promise) -> Result<(), &'static str> {
    wasm_bindgen_futures::JsFuture::from(promise)
        .await
        .map(|_| ())
        .map_err(|_| CLIPBOARD_ERROR)
}

#[cfg(target_arch = "wasm32")]
async fn copy_link(path: &str) -> Result<(), &'static str> {
    use wasm_bindgen::JsCast;

    let window = web_sys::window().ok_or(CLIPBOARD_UNAVAILABLE)?;
    let origin = window
        .location()
        .origin()
        .map_err(|_| CLIPBOARD_UNAVAILABLE)?;
    let clipboard = js_sys::Reflect::get(
        window.navigator().as_ref(),
        &wasm_bindgen::JsValue::from_str("clipboard"),
    )
    .map_err(|_| CLIPBOARD_UNAVAILABLE)?;
    let clipboard = clipboard
        .dyn_into::<web_sys::Clipboard>()
        .map_err(|_| CLIPBOARD_UNAVAILABLE)?;
    clipboard_result(clipboard.write_text(&absolute_link(&origin, path))).await
}

/// Visible detail-header action. `path` is present only for a loaded current entity.
#[component]
pub fn CopyLinkButton(#[prop(into)] path: Signal<Option<String>>) -> impl IntoView {
    let copied_path = RwSignal::new(None::<String>);
    let busy = RwSignal::new(false);
    #[cfg(target_arch = "wasm32")]
    let attempt = RwSignal::new(0_u64);
    let error = RwSignal::new(None::<(String, &'static str)>);

    #[cfg(target_arch = "wasm32")]
    let on_click = move |_| {
        let Some(current_path) = path.get_untracked() else {
            return;
        };
        if busy.get_untracked() {
            return;
        }
        attempt.update(|value| *value += 1);
        let current_attempt = attempt.get_untracked();
        busy.set(true);
        copied_path.set(None);
        error.set(None);
        leptos::task::spawn_local(async move {
            let result = copy_link(&current_path).await;
            busy.try_set(false);
            // The router can reuse this header while a clipboard request is pending.
            if path.try_get_untracked().flatten().as_ref() != Some(&current_path) {
                return;
            }
            match result {
                Ok(()) => {
                    copied_path.try_set(Some(current_path.clone()));
                    gloo_timers::future::TimeoutFuture::new(2_000).await;
                    if attempt.try_get_untracked() == Some(current_attempt) {
                        copied_path.try_set(None);
                    }
                }
                Err(message) => {
                    error.try_set(Some((current_path, message)));
                }
            }
        });
    };
    #[cfg(not(target_arch = "wasm32"))]
    let on_click = |_| {};

    view! {
        <div class="relative shrink-0">
            <Button
                variant=ButtonVariant::GhostMuted
                size=ButtonSize::Sm
                aria_label="Copy link"
                disabled=Signal::derive(move || path.get().is_none() || busy.get())
                on:click=on_click
            >
                <phosphor_leptos::Icon icon=phosphor_leptos::LINK size="16px"/>
                <span aria-live="polite">
                    {move || if path.get().is_some() && copied_path.get() == path.get() {
                        "Copied"
                    } else {
                        "Copy link"
                    }}
                </span>
            </Button>
            {move || error.get().filter(|(failed_path, _)| path.get().as_ref() == Some(failed_path))
                .map(|(_, message)| view! {
                    <div class="absolute top-full left-0 z-50 mt-1 w-56 shadow-md">
                        <Alert variant=AlertVariant::Error>
                            <AlertDescription>{message}</AlertDescription>
                        </Alert>
                    </div>
                })}
        </div>
    }
}

#[cfg(test)]
mod tests {
    use super::{absolute_link, issue_link_path, project_link_path};

    #[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
    #[cfg_attr(not(target_arch = "wasm32"), test)]
    fn canonical_detail_links_use_loaded_identifiers() {
        assert_eq!(
            absolute_link(
                "https://trakkt.example",
                &issue_link_path("TRA", 10076, "TRA", 10076)
                    .expect("current loaded issue has a link")
            ),
            "https://trakkt.example/issues/TRA-10076"
        );
        assert_eq!(
            absolute_link(
                "http://localhost:3276",
                &project_link_path("project-id", "project-id")
                    .expect("current loaded project has a link")
            ),
            "http://localhost:3276/projects/project-id"
        );
    }

    #[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
    #[cfg_attr(not(target_arch = "wasm32"), test)]
    fn stale_loaded_entities_have_no_copy_target() {
        assert_eq!(issue_link_path("TRA", 10077, "TRA", 10076), None);
        assert_eq!(issue_link_path("OTHER", 10076, "TRA", 10076), None);
        assert_eq!(project_link_path("next-project", "previous-project"), None);
        assert_eq!(
            project_link_path("next-project", "next-project"),
            Some("/projects/next-project".into())
        );
    }
}

#[cfg(all(test, target_arch = "wasm32"))]
mod wasm_tests {
    use super::{CLIPBOARD_ERROR, absolute_link, clipboard_result};
    use wasm_bindgen::JsValue;
    use wasm_bindgen_test::*;

    wasm_bindgen_test_configure!(run_in_browser);

    #[wasm_bindgen_test]
    fn canonical_link_excludes_browser_queries_and_fragments_without_navigation() {
        let window = web_sys::window().expect("browser test has a window");
        let history = window.history().expect("browser test can read history");
        let original = window.location().href().expect("browser test has a URL");
        history
            .replace_state_with_url(&JsValue::NULL, "", Some("?view=board&status=open#updates"))
            .expect("installing query and fragment for canonical link test");
        let before = window.location().href().expect("reading filtered test URL");
        let origin = window.location().origin().expect("reading browser origin");
        let link = absolute_link(&origin, "/projects/loaded-project");
        let after = window
            .location()
            .href()
            .expect("reading URL after building link");
        history
            .replace_state_with_url(&JsValue::NULL, "", Some(&original))
            .expect("restoring browser test URL");
        assert_eq!(link, format!("{origin}/projects/loaded-project"));
        assert!(!link.contains('?'));
        assert!(!link.contains('#'));
        assert_eq!(before, after);
    }

    #[wasm_bindgen_test]
    async fn successful_clipboard_promise_reports_success() {
        assert_eq!(
            clipboard_result(js_sys::Promise::resolve(&JsValue::UNDEFINED)).await,
            Ok(())
        );
    }

    #[wasm_bindgen_test]
    async fn rejected_clipboard_promise_reports_useful_error() {
        assert_eq!(
            clipboard_result(js_sys::Promise::reject(&JsValue::from_str(
                "NotAllowedError"
            )))
            .await,
            Err(CLIPBOARD_ERROR)
        );
    }
}
