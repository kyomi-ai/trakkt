// SPDX-License-Identifier: AGPL-3.0-or-later

//! Feedback submission service.
//!
//! Owns the end-to-end "submit feedback" flow: validate, rate-limit, persist.
//! Callers (server functions) are thin adapters — they extract auth context,
//! call into this service, and map `trakkt_core::Result` onto their response type.

use trakkt_core::sql_compat;
use trakkt_core::DbPool;
use trakkt_types::models::Feedback;

/// Maximum screenshot size in bytes (2 MB).
///
/// Base64-encoded payloads are estimated at roughly 4/3 the raw size; anything
/// bigger is dropped from the context blob (a `screenshot_too_large` marker is
/// set instead) so one runaway paste cannot blow up the feedback row.
const MAX_SCREENSHOT_BYTES: usize = 2 * 1024 * 1024;

// ─── Row type ────────────────────────────────────────────────────────────────

/// Internal row type for deserialising `feedback` query results.
#[derive(sqlx::FromRow)]
struct FeedbackRow {
    id: String,
    user_id: String,
    workspace_id: String,
    feedback_type: String,
    description: String,
    screenshot_url: Option<String>,
    include_context: bool,
    context: Option<String>,
    status: String,
    created_at: String,
    resolved_at: Option<String>,
    resolution_notes: Option<String>,
    resolved_by: Option<String>,
}

impl FeedbackRow {
    fn into_dto(self) -> Feedback {
        Feedback {
            id: self.id,
            user_id: self.user_id,
            workspace_id: self.workspace_id,
            feedback_type: self.feedback_type,
            description: self.description,
            screenshot_url: self.screenshot_url,
            include_context: self.include_context,
            context: self.context,
            status: self.status,
            created_at: self.created_at,
            resolved_at: self.resolved_at,
            resolution_notes: self.resolution_notes,
            resolved_by: self.resolved_by,
        }
    }
}

// ─── Service functions ────────────────────────────────────────────────────────

/// Parameters for [`submit_feedback`].
pub struct SubmitFeedbackParams<'a> {
    pub workspace_id: &'a str,
    pub user_id: &'a str,
    pub feedback_type: &'a str,
    pub description: &'a str,
    pub screenshot_url: Option<&'a str>,
    pub include_context: bool,
    pub context_json: Option<&'a str>,
}

/// Submit feedback end-to-end: validate, rate-limit, persist.
///
/// ### Behaviour
/// - Rejects `feedback_type` not in `{bug, feature, question}`.
/// - Rejects descriptions shorter than 10 chars (after trim).
/// - Rate-limited to 5 submissions per user per hour.
/// - Generates a `fb-{uuid_hex_12}` identifier and inserts into the `feedback` table.
/// - Merges any attached screenshot into the context JSON, capped at
///   [`MAX_SCREENSHOT_BYTES`].
pub async fn submit_feedback(
    db: &DbPool,
    params: &SubmitFeedbackParams<'_>,
) -> trakkt_core::Result<Feedback> {
    // Validate feedback type
    if !["bug", "feature", "question"].contains(&params.feedback_type) {
        return Err(trakkt_core::Error::BadRequest(
            "Invalid feedback type. Must be 'bug', 'feature', or 'question'.".into(),
        ));
    }

    // Validate description length
    let description = params.description.trim();
    if description.len() < 10 {
        return Err(trakkt_core::Error::BadRequest(
            "Description must be at least 10 characters.".into(),
        ));
    }

    // Rate limit: max 5 feedback submissions per user per hour
    let is_pg = db.is_postgres();
    let rate_limit_sql = if is_pg {
        "SELECT COUNT(*) FROM feedback WHERE user_id = $1 AND workspace_id = $2 AND created_at > NOW() - INTERVAL '1 hour'"
            .to_string()
    } else {
        "SELECT COUNT(*) FROM feedback WHERE user_id = $1 AND workspace_id = $2 AND created_at > datetime('now', '-1 hour')"
            .to_string()
    };
    let recent_count: i64 =
        trakkt_core::db_fetch_scalar!(db, i64, &rate_limit_sql, params.user_id, params.workspace_id)?;
    if recent_count >= 5 {
        return Err(trakkt_core::Error::TooManyRequests(
            "You've submitted too many feedback items recently. Please try again in an hour."
                .into(),
            3600,
        ));
    }

    // Generate feedback ID: fb-{uuid4_hex_first_12_chars}
    let feedback_id = format!(
        "fb-{}",
        &uuid::Uuid::new_v4().simple().to_string()[..12]
    );

    // Build context JSON — handle screenshot embedding
    let mut context_value: serde_json::Value = if params.include_context {
        params
            .context_json
            .and_then(|s| match serde_json::from_str::<serde_json::Value>(s.trim()) {
                Ok(v) if v.is_object() => Some(v),
                Ok(_) => {
                    tracing::warn!("Feedback context JSON is not an object; using empty object");
                    None
                }
                Err(e) => {
                    tracing::warn!(error = %e, "Failed to parse feedback context JSON");
                    None
                }
            })
            .unwrap_or_else(|| serde_json::json!({}))
    } else {
        serde_json::json!({})
    };

    // Don't store oversized screenshots in the screenshot_url column.
    let stored_screenshot_url = params.screenshot_url.filter(|s| {
        s.len() * 3 / 4 <= MAX_SCREENSHOT_BYTES
    });

    // Handle screenshot in context — screenshots are an explicit user action
    // (Capture Screen / Upload Image) and are forwarded unconditionally,
    // regardless of whether the user opted in to "Include technical details".
    if let Some(screenshot_b64) = params.screenshot_url {
        // Estimate decoded size: base64 is ~4/3 of original
        let estimated_size = screenshot_b64.len() * 3 / 4;
        if estimated_size <= MAX_SCREENSHOT_BYTES {
            if let Some(obj) = context_value.as_object_mut() {
                obj.insert(
                    "screenshot_base64".to_string(),
                    serde_json::Value::String(screenshot_b64.to_string()),
                );
            }
        } else if let Some(obj) = context_value.as_object_mut() {
            obj.insert(
                "screenshot_too_large".to_string(),
                serde_json::Value::Bool(true),
            );
        }
    }

    // Serialise the context to a JSON string for storage.
    let context_str = serde_json::to_string(&context_value)?;

    // Insert feedback
    let now = sql_compat::now(is_pg);
    let include_ctx_literal = if params.include_context {
        sql_compat::bool_true(is_pg)
    } else {
        sql_compat::bool_false(is_pg)
    };
    let json_cast = sql_compat::cast_to_json(is_pg, "$7");
    let sql = format!(
        "INSERT INTO feedback \
            (id, user_id, workspace_id, feedback_type, description, screenshot_url, \
             include_context, context, status, created_at) \
         VALUES ($1, $2, $3, $4, $5, $6, {include_ctx_literal}, {json_cast}, 'new', {now})"
    );
    trakkt_core::db_execute!(
        db,
        &sql,
        &feedback_id,
        params.user_id,
        params.workspace_id,
        params.feedback_type,
        description,
        stored_screenshot_url,
        &context_str
    )?;

    tracing::info!(
        feedback_id = %feedback_id,
        user_id = %params.user_id,
        feedback_type = %params.feedback_type,
        "Feedback submitted"
    );

    // Re-fetch to get the DB-assigned created_at.
    get_feedback(db, &feedback_id, params.workspace_id)
        .await?
        .ok_or_else(|| {
            trakkt_core::Error::Internal("Feedback row not found after insert".into())
        })
}

/// Fetch a single feedback item by ID, scoped to a workspace.
pub async fn get_feedback(
    db: &DbPool,
    id: &str,
    workspace_id: &str,
) -> trakkt_core::Result<Option<Feedback>> {
    let sql = "SELECT id, user_id, workspace_id, feedback_type, description, \
               screenshot_url, include_context, context, status, \
               created_at, resolved_at, resolution_notes, resolved_by \
               FROM feedback WHERE id = $1 AND workspace_id = $2";
    let row = trakkt_core::db_fetch_optional!(db, FeedbackRow, sql, id, workspace_id)?;
    Ok(row.map(FeedbackRow::into_dto))
}

/// List feedback items in a workspace, with optional status filter.
///
/// Results are ordered by creation time (newest first) and paginated via
/// `limit` / `offset`. LIMIT/OFFSET are inlined (sanitised i64, not user input).
pub async fn list_feedback(
    db: &DbPool,
    workspace_id: &str,
    status_filter: Option<&str>,
    limit: i64,
    offset: i64,
) -> trakkt_core::Result<Vec<Feedback>> {
    let rows = match status_filter {
        Some(status) => {
            let sql = format!(
                "SELECT id, user_id, workspace_id, feedback_type, description, \
                 screenshot_url, include_context, context, status, \
                 created_at, resolved_at, resolution_notes, resolved_by \
                 FROM feedback WHERE workspace_id = $1 AND status = $2 \
                 ORDER BY created_at DESC LIMIT {limit} OFFSET {offset}"
            );
            trakkt_core::db_fetch_all!(db, FeedbackRow, &sql, workspace_id, status)?
        }
        None => {
            let sql = format!(
                "SELECT id, user_id, workspace_id, feedback_type, description, \
                 screenshot_url, include_context, context, status, \
                 created_at, resolved_at, resolution_notes, resolved_by \
                 FROM feedback WHERE workspace_id = $1 \
                 ORDER BY created_at DESC LIMIT {limit} OFFSET {offset}"
            );
            trakkt_core::db_fetch_all!(db, FeedbackRow, &sql, workspace_id)?
        }
    };

    Ok(rows.into_iter().map(FeedbackRow::into_dto).collect())
}
