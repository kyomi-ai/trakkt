// SPDX-License-Identifier: AGPL-3.0-or-later

//! Unified API surface — shared by MCP and REST transports.
//!
//! Each domain operation is registered as an [`ApiOperation`] with typed
//! parameters, a JSON Schema for introspection, and an async handler. The MCP
//! router and the REST router both delegate to these operations, eliminating
//! duplicate business logic.

pub mod activities;
pub mod comments;
pub mod context;
pub mod issues;
pub mod labels;
pub mod milestones;
pub mod openapi;
pub mod projects;
pub mod relations;
pub mod statuses;
pub mod teams;

use std::future::Future;
use std::pin::Pin;

use axum::http::Method;
use schemars::Schema;

// ─────────────────────────────────────────────────────────────────────────────
// ApiCtx — per-request context passed to every operation handler
// ─────────────────────────────────────────────────────────────────────────────

/// Per-request context for API operations.
///
/// Borrows from application state so handlers avoid cloning large structures.
pub struct ApiCtx<'a> {
    pub db: &'a trakkt_core::DbPool,
    pub workspace_id: String,
    pub user_id: String,
    pub ws_manager: Option<&'a trakkt_auth::websocket::WebSocketManager>,
}

impl<'a> ApiCtx<'a> {
    /// Construct an [`ApiCtx`] from bearer-auth resolved fields and shared
    /// application state references. Takes raw fields rather than `&McpAuth`
    /// because `McpAuth` is intentionally private to `routes/mcp.rs`.
    pub fn from_bearer(
        workspace_id: String,
        user_id: String,
        db: &'a trakkt_core::DbPool,
        ws_manager: &'a trakkt_auth::websocket::WebSocketManager,
    ) -> Self {
        Self {
            db,
            workspace_id,
            user_id,
            ws_manager: Some(ws_manager),
        }
    }

    /// Construct an [`ApiCtx`] from Leptos server function context.
    ///
    /// Accepts `Option<&WebSocketManager>` directly because the Leptos
    /// `ServerContext` stores it as `Option<WebSocketManager>`.
    pub fn from_leptos(
        workspace_id: String,
        user_id: String,
        db: &'a trakkt_core::DbPool,
        ws_manager: Option<&'a trakkt_auth::websocket::WebSocketManager>,
    ) -> Self {
        Self {
            db,
            workspace_id,
            user_id,
            ws_manager,
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// ApiError — unified error type for API operations
// ─────────────────────────────────────────────────────────────────────────────

/// Error type returned by API operation handlers.
///
/// Intentionally a small subset of HTTP semantics — each variant maps to a
/// single status code. Transport layers (MCP JSON-RPC, REST) convert this into
/// their respective wire formats.
#[derive(Debug)]
pub enum ApiError {
    NotFound(String),
    BadRequest(String),
    Unauthorized(String),
    Forbidden(String),
    Conflict(String),
    Internal(String),
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ApiError::NotFound(msg) => write!(f, "not found: {msg}"),
            ApiError::BadRequest(msg) => write!(f, "bad request: {msg}"),
            ApiError::Unauthorized(msg) => write!(f, "unauthorized: {msg}"),
            ApiError::Forbidden(msg) => write!(f, "forbidden: {msg}"),
            ApiError::Conflict(msg) => write!(f, "conflict: {msg}"),
            ApiError::Internal(msg) => write!(f, "internal: {msg}"),
        }
    }
}

impl From<trakkt_core::Error> for ApiError {
    fn from(e: trakkt_core::Error) -> Self {
        match e {
            trakkt_core::Error::NotFound(msg) => ApiError::NotFound(msg),
            trakkt_core::Error::BadRequest(msg) => ApiError::BadRequest(msg),
            trakkt_core::Error::Forbidden(msg) => ApiError::Forbidden(msg),
            trakkt_core::Error::Conflict(msg) => ApiError::Conflict(msg),
            other => ApiError::Internal(other.to_string()),
        }
    }
}

impl From<serde_json::Error> for ApiError {
    fn from(e: serde_json::Error) -> Self {
        ApiError::BadRequest(format!("JSON error: {e}"))
    }
}

/// Result type alias for API operation handlers.
pub type ApiResult<T> = Result<T, ApiError>;

// ─────────────────────────────────────────────────────────────────────────────
// ApiOperation — type-erased operation descriptor
// ─────────────────────────────────────────────────────────────────────────────

/// Type-erased async handler for API operations.
pub type DynHandler = Box<
    dyn Fn(
            ApiCtx<'_>,
            serde_json::Value,
        ) -> Pin<Box<dyn Future<Output = ApiResult<serde_json::Value>> + Send + '_>>
        + Send
        + Sync,
>;

/// A registered API operation.
///
/// Stores enough metadata for both the MCP `tools/list` response and the REST
/// OpenAPI spec, plus a type-erased async handler that accepts raw JSON args.
pub struct ApiOperation {
    /// Machine-readable operation name (e.g. `"list_issues"`).
    pub name: &'static str,

    /// Human-readable description shown in tool listings.
    pub description: &'static str,

    /// Required OAuth / API-token scope (e.g. `"issues:read"`).
    pub scope: &'static str,

    /// HTTP method for the REST surface.
    pub rest_method: Method,

    /// URL path template for the REST surface (e.g. `"/issues"`).
    pub rest_path: &'static str,

    /// Returns the JSON Schema for the operation's input parameters.
    pub json_schema: fn() -> Schema,

    /// Type-erased async handler.
    pub handler: DynHandler,
}

/// Collect all registered API operations.
///
/// Each domain module provides an `operations()` function that returns its
/// operations. They are aggregated here into a single registry.
pub fn all_operations() -> Vec<ApiOperation> {
    let mut ops = Vec::new();
    ops.extend(issues::operations());
    ops.extend(comments::operations());
    ops.extend(labels::operations());
    ops.extend(teams::operations());
    ops.extend(statuses::operations());
    ops.extend(relations::operations());
    ops.extend(projects::operations());
    ops.extend(milestones::operations());
    ops.extend(activities::operations());
    ops
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// Verify that `schemars` correctly handles `Option<Option<String>>` fields.
    ///
    /// The unified API surface uses double-Option to distinguish "field absent"
    /// (outer `None`) from "field explicitly set to null" (inner `None`) on
    /// update operations. This test ensures schemars generates a valid schema
    /// for that pattern.
    #[test]
    fn schemars_double_option_generates_valid_schema() {
        #[derive(Debug, schemars::JsonSchema)]
        struct TestUpdateParams {
            id: String,
            title: Option<Option<String>>,
            priority: Option<Option<i32>>,
        }

        let params = TestUpdateParams {
            id: "x".into(),
            title: None,
            priority: None,
        };
        let _ = format!("{params:?}");

        let schema = schemars::schema_for!(TestUpdateParams);
        let json = serde_json::to_value(&schema).expect("schema should serialize to JSON");

        // The schema should be a valid JSON object with properties.
        let properties = json
            .pointer("/properties")
            .expect("schema should have /properties");

        // `id` must be present and required.
        assert!(
            properties.get("id").is_some(),
            "schema should include 'id' property"
        );

        // `title` should be present — schemars represents Option<Option<T>> as
        // a nullable type (the exact representation varies by schemars version,
        // but the property must exist).
        assert!(
            properties.get("title").is_some(),
            "schema should include 'title' property for Option<Option<String>>"
        );

        // `priority` should also be present.
        assert!(
            properties.get("priority").is_some(),
            "schema should include 'priority' property for Option<Option<i32>>"
        );

        // `id` should be in the required list.
        let required = json
            .pointer("/required")
            .and_then(|v| v.as_array())
            .expect("schema should have a /required array");
        assert!(
            required.iter().any(|v| v.as_str() == Some("id")),
            "'id' should be in the required list"
        );

        // `title` and `priority` should NOT be required (they are Option<_>).
        assert!(
            !required.iter().any(|v| v.as_str() == Some("title")),
            "'title' should not be in the required list"
        );
        assert!(
            !required.iter().any(|v| v.as_str() == Some("priority")),
            "'priority' should not be in the required list"
        );
    }

    #[test]
    fn all_operations_includes_all_ops() {
        let ops = all_operations();
        let names: Vec<&str> = ops.iter().map(|op| op.name).collect();

        // Assert no duplicate operation names.
        let unique: std::collections::HashSet<&str> = names.iter().copied().collect();
        assert_eq!(unique.len(), ops.len(), "duplicate operation names detected");

        // Issue operations (6)
        assert!(names.contains(&"list_issues"));
        assert!(names.contains(&"get_issue"));
        assert!(names.contains(&"create_issue"));
        assert!(names.contains(&"update_issue"));
        assert!(names.contains(&"delete_issue"));
        assert!(names.contains(&"search_issues"));

        // Comment operations (1)
        assert!(names.contains(&"add_comment"));

        // Label operations (2)
        assert!(names.contains(&"list_labels"));
        assert!(names.contains(&"create_label"));

        // Team operations (1)
        assert!(names.contains(&"list_teams"));

        // Status operations (1)
        assert!(names.contains(&"list_statuses"));

        // Relation operations (3)
        assert!(names.contains(&"add_relation"));
        assert!(names.contains(&"remove_relation"));
        assert!(names.contains(&"list_issue_relations"));

        // Project operations (5)
        assert!(names.contains(&"list_projects"));
        assert!(names.contains(&"get_project"));
        assert!(names.contains(&"create_project"));
        assert!(names.contains(&"update_project"));
        assert!(names.contains(&"delete_project"));

        // Milestone operations (4)
        assert!(names.contains(&"list_milestones"));
        assert!(names.contains(&"create_milestone"));
        assert!(names.contains(&"update_milestone"));
        assert!(names.contains(&"delete_milestone"));

        // Activity operations (1)
        assert!(names.contains(&"list_issue_activities"));

        // Total: 6 + 1 + 2 + 1 + 1 + 3 + 5 + 4 + 1 = 24
        assert_eq!(ops.len(), 24, "expected 24 total operations");
    }
}
