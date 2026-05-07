// SPDX-License-Identifier: AGPL-3.0-or-later

//! Admin notification helpers (Slack + email) for signups and other events.

/// Send Slack + email notifications for a new user signup.
///
/// Fire-and-forget — callers typically wrap this in `tokio::spawn`.
pub async fn notify_signup(
    slack_webhook_url: Option<&str>,
    _support_email: &str,
    email: &str,
    name: &str,
    user_id: &str,
) {
    // Slack notification
    if let Err(e) = send_signup_slack(slack_webhook_url, email, name, user_id).await {
        tracing::error!(
            user_id = %user_id,
            error = %e,
            "Failed to send signup Slack notification"
        );
    }

    // TODO: port EmailService from Kyomi for email notification to support
}

/// Send a Slack webhook notification for a new signup.
async fn send_signup_slack(
    webhook_url: Option<&str>,
    email: &str,
    name: &str,
    user_id: &str,
) -> trakkt_core::Result<()> {
    let webhook_url = match webhook_url {
        Some(url) if !url.is_empty() => url,
        _ => {
            tracing::debug!(
                "SLACK_FEEDBACK_WEBHOOK_URL not configured, skipping signup notification"
            );
            return Ok(());
        }
    };

    let name_display = if name.is_empty() {
        "Not provided"
    } else {
        name
    };

    let payload = serde_json::json!({
        "text": format!("New user signup: {email} ({name_display}, user_id={user_id})"),
    });

    let http = crate::http_client()?;
    let response = http
        .post(webhook_url)
        .json(&payload)
        .timeout(std::time::Duration::from_secs(10))
        .send()
        .await
        .map_err(|e| trakkt_core::Error::Internal(format!("Slack webhook POST failed: {e}")))?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(trakkt_core::Error::Internal(format!(
            "Slack webhook returned {status}: {body}"
        )));
    }

    tracing::info!(user_id = %user_id, email = %email, "Slack signup notification sent");
    Ok(())
}
