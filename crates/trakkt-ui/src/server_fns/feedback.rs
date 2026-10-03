// SPDX-License-Identifier: AGPL-3.0-or-later

//! Server function for submitting user feedback.
//!
//! Thin adapter over [`trakkt_auth::feedback_service::submit_feedback`] —
//! extracts auth, calls the service, returns. All validation and persistence
//! live in the service layer.

use leptos::prelude::*;

// Helpers — delegate to shared extractors in parent module
#[cfg(feature = "ssr")]
use super::{AuthenticatedContext, IntoServerFnError};

/// Submit user feedback (bug report, feature request, or question).
///
/// Delegates to `trakkt_auth::feedback_service::submit_feedback` for validation,
/// rate limiting, and persistence. Feedback is not available in personal mode.
#[server(prefix = "/leptos-api")]
pub async fn submit_feedback(
    feedback_type: String,
    description: String,
    screenshot_url: Option<String>,
    include_context: bool,
    context_json: Option<String>,
) -> Result<(), ServerFnError> {
    let ac = AuthenticatedContext::extract().await?;

    // Feedback routes to our issue tracker — disable in personal mode.
    if ac.ctx.config.is_personal() {
        return Err(ServerFnError::new(
            "Feedback is not available in personal mode",
        ));
    }

    let params = trakkt_auth::feedback_service::SubmitFeedbackParams {
        workspace_id: &ac.ws_id,
        user_id: &ac.auth.user_id,
        feedback_type: &feedback_type,
        description: &description,
        screenshot_url: screenshot_url.as_deref(),
        include_context,
        context_json: context_json.as_deref(),
    };
    trakkt_auth::feedback_service::submit_feedback(ac.db(), &params)
        .await
        .into_sfn()?;

    Ok(())
}
