// SPDX-License-Identifier: AGPL-3.0-or-later

//! Shared helpers for rendering GitHub-sourced activity data.

/// Extract `@author_login` from a GitHub activity's metadata JSON, if present.
/// Returns `None` if metadata is absent, the field is missing, or JSON is malformed.
pub fn github_author_login_from_metadata(metadata: Option<&str>) -> Option<String> {
    let meta_str = metadata?;
    match serde_json::from_str::<serde_json::Value>(meta_str) {
        Ok(meta) => meta
            .get("author_login")
            .and_then(|v| v.as_str())
            .map(|l| format!("@{l}")),
        Err(e) => {
            tracing::warn!(error = %e, "Failed to parse GitHub activity metadata");
            None
        }
    }
}
