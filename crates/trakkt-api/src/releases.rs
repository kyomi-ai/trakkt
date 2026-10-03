// SPDX-License-Identifier: AGPL-3.0-or-later

//! Release operations — shared handlers for MCP and REST surfaces.
//!
//! Each handler takes an [`ApiCtx`] and a typed params struct, returning
//! `ApiResult<serde_json::Value>`. Provides CRUD for releases and a query
//! for unreleased issues.

use axum::http::Method;

use trakkt_auth::release_service;
use trakkt_types::api::{
    CreateReleaseApiParams, GetReleaseApiParams, ListReleasesApiParams,
    ListUnreleasedIssuesApiParams,
};

use crate::{ApiCtx, ApiError, ApiOperation, ApiResult};

// ─────────────────────────────────────────────────────────────────────────────
// Handlers
// ─────────────────────────────────────────────────────────────────────────────

/// List all releases in the workspace, optionally filtered by team key.
pub async fn list_releases(
    ctx: &ApiCtx<'_>,
    params: ListReleasesApiParams,
) -> ApiResult<serde_json::Value> {
    let releases = release_service::list_releases(
        ctx.db,
        &ctx.workspace_id,
        params.team_key.as_deref(),
    )
    .await?;

    Ok(serde_json::to_value(&releases)?)
}

/// Get a single release by ID, including linked issues.
pub async fn get_release(
    ctx: &ApiCtx<'_>,
    params: GetReleaseApiParams,
) -> ApiResult<serde_json::Value> {
    let release = release_service::get_release(ctx.db, &params.release_id)
        .await?
        .ok_or_else(|| {
            ApiError::NotFound(format!("Release {} not found", params.release_id))
        })?;

    // Verify workspace ownership.
    if release.workspace_id != ctx.workspace_id {
        return Err(ApiError::NotFound(format!(
            "Release {} not found",
            params.release_id
        )));
    }

    Ok(serde_json::to_value(&release)?)
}

/// Create a new release with auto-linked issues from commit SHAs.
///
/// Resolves commit SHAs to issue IDs via `trakkt_github::schema::lookup_issues_by_ref`,
/// then delegates to the service layer for persistence.
pub async fn create_release(
    ctx: &ApiCtx<'_>,
    params: CreateReleaseApiParams,
) -> ApiResult<serde_json::Value> {
    // Resolve commit SHAs to issue IDs via github_links.
    let mut seen = std::collections::HashSet::new();
    let mut issue_ids: Vec<String> = Vec::new();

    for sha in &params.commit_shas {
        // Sanitize SHA for LIKE safety (same as lookup_commit handler).
        let safe_sha = sha.replace('%', "\\%").replace('_', "\\_");

        let results = trakkt_github::schema::lookup_issues_by_ref(
            ctx.db,
            &ctx.workspace_id,
            "commit",
            &safe_sha,
            true, // prefix match
        )
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

        for result in results {
            if seen.insert(result.issue_id.clone()) {
                issue_ids.push(result.issue_id);
            }
        }
    }

    let release = release_service::create_release(
        ctx.db,
        &ctx.workspace_id,
        &params.team_key,
        &params.tag_name,
        params.previous_tag.as_deref(),
        params.title.as_deref(),
        params.notes.as_deref(),
        &issue_ids,
        &ctx.user_id,
        ctx.ws_manager,
    )
    .await?;

    Ok(serde_json::to_value(&release)?)
}

/// List issues that are completed but not yet released.
pub async fn list_unreleased_issues(
    ctx: &ApiCtx<'_>,
    params: ListUnreleasedIssuesApiParams,
) -> ApiResult<serde_json::Value> {
    let issues = release_service::unreleased_issues(
        ctx.db,
        &ctx.workspace_id,
        params.team_key.as_deref(),
    )
    .await?;

    Ok(serde_json::to_value(&issues)?)
}

// ─────────────────────────────────────────────────────────────────────────────
// Operation registration
// ─────────────────────────────────────────────────────────────────────────────

/// Return all release-related API operations.
pub fn operations() -> Vec<ApiOperation> {
    vec![
        ApiOperation {
            name: "list_releases",
            description: "List all releases in the workspace, optionally filtered by team key.",
            scope: "issues:read",
            rest_method: Method::GET,
            rest_path: "/releases",
            json_schema: || schemars::schema_for!(ListReleasesApiParams),
            handler: Box::new(|ctx, value| {
                Box::pin(async move {
                    let params: ListReleasesApiParams = serde_json::from_value(value)?;
                    list_releases(&ctx, params).await
                })
            }),
            binary_input: None,
            binary_output: None,
        },
        ApiOperation {
            name: "get_release",
            description: "Get a single release by ID, including linked issues with details.",
            scope: "issues:read",
            rest_method: Method::GET,
            rest_path: "/releases/{id}",
            json_schema: || schemars::schema_for!(GetReleaseApiParams),
            handler: Box::new(|ctx, value| {
                Box::pin(async move {
                    let params: GetReleaseApiParams = serde_json::from_value(value)?;
                    get_release(&ctx, params).await
                })
            }),
            binary_input: None,
            binary_output: None,
        },
        ApiOperation {
            name: "create_release",
            description: "Create a new release. Auto-links issues by looking up commit SHAs in github_links and stamps released_at on matched issues.",
            scope: "issues:write",
            rest_method: Method::POST,
            rest_path: "/releases",
            json_schema: || schemars::schema_for!(CreateReleaseApiParams),
            handler: Box::new(|ctx, value| {
                Box::pin(async move {
                    let params: CreateReleaseApiParams = serde_json::from_value(value)?;
                    create_release(&ctx, params).await
                })
            }),
            binary_input: None,
            binary_output: None,
        },
        ApiOperation {
            name: "list_unreleased_issues",
            description: "List issues that are completed/cancelled but not yet included in any release (completed_at IS NOT NULL, released_at IS NULL).",
            scope: "issues:read",
            rest_method: Method::GET,
            rest_path: "/unreleased-issues",
            json_schema: || schemars::schema_for!(ListUnreleasedIssuesApiParams),
            handler: Box::new(|ctx, value| {
                Box::pin(async move {
                    let params: ListUnreleasedIssuesApiParams = serde_json::from_value(value)?;
                    list_unreleased_issues(&ctx, params).await
                })
            }),
            binary_input: None,
            binary_output: None,
        },
    ]
}
