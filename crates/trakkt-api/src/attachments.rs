// SPDX-License-Identifier: AGPL-3.0-or-later

//! Attachment operations — upload, download, list, and delete.
//!
//! Each handler takes an [`ApiCtx`] and a typed params struct, returning
//! `ApiResult<serde_json::Value>`. Binary data flows through base64 encoding
//! at the handler level; the REST transport layer may intercept
//! `BinaryInputSpec`/`BinaryOutputSpec` to handle multipart and raw responses.

use axum::http::Method;
use base64::Engine;

use trakkt_auth::attachment_service;
use trakkt_types::api::{
    DeleteAttachmentApiParams, DownloadAttachmentApiParams, ListAttachmentsApiParams,
    UploadAttachmentApiParams,
};

use crate::{ApiCtx, ApiError, ApiOperation, ApiResult, BinaryInputSpec, BinaryOutputSpec};

// ─────────────────────────────────────────────────────────────────────────────
// Handlers
// ─────────────────────────────────────────────────────────────────────────────

/// Upload a file attachment. Validates type/size, stores the file, and creates
/// the database record.
pub async fn upload_attachment(
    ctx: &ApiCtx<'_>,
    params: UploadAttachmentApiParams,
) -> ApiResult<serde_json::Value> {
    let storage = ctx.attachment_storage.ok_or_else(|| {
        ApiError::Internal("Attachment storage not configured".into())
    })?;

    // Decode base64 content
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(&params.content_base64)
        .map_err(|e| ApiError::BadRequest(format!("Invalid base64 content: {e}")))?;

    // Validate file type and size
    attachment_service::validate_file_type(&params.filename, &params.content_type)
        .map_err(|e| ApiError::BadRequest(e.to_string()))?;
    attachment_service::validate_file_size(bytes.len())
        .map_err(|e| ApiError::BadRequest(e.to_string()))?;

    // Generate a storage-level ID for the file path. The DB record will have
    // its own attachment_id; storage_path is an opaque key linking the two.
    let storage_file_id = uuid::Uuid::new_v4().to_string();

    // Store the file
    let storage_path = storage
        .store(
            &ctx.workspace_id,
            &storage_file_id,
            &bytes,
            &params.filename,
            &params.content_type,
        )
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    // Create the DB record
    let attachment = attachment_service::create_attachment(
        ctx.db,
        &ctx.workspace_id,
        &params.filename,
        &params.content_type,
        bytes.len() as i64,
        &storage_path,
        &ctx.user_id,
        ctx.ws_manager,
    )
    .await?;

    Ok(serde_json::json!({
        "attachment_id": attachment.attachment_id,
        "filename": attachment.filename,
        "content_type": attachment.content_type,
        "size_bytes": attachment.size_bytes,
        "url": format!("/api/v1/attachments/{}/download", attachment.attachment_id),
    }))
}

/// Download an attachment file by ID. Returns base64-encoded content for MCP
/// consumers; the REST transport layer intercepts `BinaryOutputSpec` to return
/// raw bytes with the correct Content-Type header.
pub async fn download_attachment(
    ctx: &ApiCtx<'_>,
    params: DownloadAttachmentApiParams,
) -> ApiResult<serde_json::Value> {
    let storage = ctx.attachment_storage.ok_or_else(|| {
        ApiError::Internal("Attachment storage not configured".into())
    })?;

    // Get the DB record (workspace-scoped)
    let attachment =
        attachment_service::get_attachment(ctx.db, &params.attachment_id, &ctx.workspace_id)
            .await?;

    // Retrieve from storage
    let bytes = storage
        .retrieve(&attachment.storage_path)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    // Return base64-encoded content — REST transport may intercept for raw bytes.
    let content_base64 = base64::engine::general_purpose::STANDARD.encode(&bytes);

    Ok(serde_json::json!({
        "attachment_id": attachment.attachment_id,
        "filename": attachment.filename,
        "content_type": attachment.content_type,
        "size_bytes": attachment.size_bytes,
        "content_base64": content_base64,
    }))
}

/// Delete an attachment by ID. Only the original uploader can delete.
/// Removes the DB record first, then best-effort deletes the stored file.
pub async fn delete_attachment(
    ctx: &ApiCtx<'_>,
    params: DeleteAttachmentApiParams,
) -> ApiResult<serde_json::Value> {
    let storage = ctx.attachment_storage.ok_or_else(|| {
        ApiError::Internal("Attachment storage not configured".into())
    })?;

    // Delete DB record (returns storage_path for file cleanup)
    let storage_path = attachment_service::delete_attachment(
        ctx.db,
        &params.attachment_id,
        &ctx.workspace_id,
        &ctx.user_id,
        ctx.ws_manager,
    )
    .await?;

    // Delete from storage — best-effort, DB record is already gone.
    if let Err(e) = storage.delete(&storage_path).await {
        tracing::warn!(
            error = %e,
            attachment_id = %params.attachment_id,
            "Failed to delete attachment from storage (DB record already removed)"
        );
    }

    Ok(serde_json::json!({
        "message": format!("Attachment '{}' deleted", params.attachment_id)
    }))
}

/// List all attachments in the workspace, ordered by creation date (newest first).
pub async fn list_attachments(
    ctx: &ApiCtx<'_>,
    _params: ListAttachmentsApiParams,
) -> ApiResult<serde_json::Value> {
    let attachments =
        attachment_service::list_attachments(ctx.db, &ctx.workspace_id).await?;

    let response: Vec<serde_json::Value> = attachments
        .iter()
        .map(|a| {
            serde_json::json!({
                "attachment_id": a.attachment_id,
                "filename": a.filename,
                "content_type": a.content_type,
                "size_bytes": a.size_bytes,
                "uploaded_by": a.uploaded_by,
                "created_at": a.created_at,
                "url": format!("/api/v1/attachments/{}/download", a.attachment_id),
            })
        })
        .collect();
    Ok(serde_json::to_value(response)?)
}

// ─────────────────────────────────────────────────────────────────────────────
// Operation registration
// ─────────────────────────────────────────────────────────────────────────────

/// Return all attachment-related API operations.
pub fn operations() -> Vec<ApiOperation> {
    vec![
        ApiOperation {
            name: "upload_attachment",
            description: "Upload a file attachment. Returns the attachment metadata including download URL.",
            scope: "attachments:write",
            rest_method: Method::POST,
            rest_path: "/attachments",
            json_schema: || schemars::schema_for!(UploadAttachmentApiParams),
            handler: Box::new(|ctx, value| {
                Box::pin(async move {
                    let params: UploadAttachmentApiParams = serde_json::from_value(value)?;
                    upload_attachment(&ctx, params).await
                })
            }),
            binary_input: Some(BinaryInputSpec {
                multipart_field: "file",
                base64_param: "content_base64",
                filename_param: "filename",
                content_type_param: "content_type",
            }),
            binary_output: None,
        },
        ApiOperation {
            name: "download_attachment",
            description: "Download an attachment file by ID.",
            scope: "attachments:read",
            rest_method: Method::GET,
            rest_path: "/attachments/{attachment_id}/download",
            json_schema: || schemars::schema_for!(DownloadAttachmentApiParams),
            handler: Box::new(|ctx, value| {
                Box::pin(async move {
                    let params: DownloadAttachmentApiParams = serde_json::from_value(value)?;
                    download_attachment(&ctx, params).await
                })
            }),
            binary_input: None,
            binary_output: Some(BinaryOutputSpec {
                content_type_field: "content_type",
            }),
        },
        ApiOperation {
            name: "delete_attachment",
            description: "Delete an attachment by ID. Only the original uploader can delete.",
            scope: "attachments:write",
            rest_method: Method::DELETE,
            rest_path: "/attachments/{attachment_id}",
            json_schema: || schemars::schema_for!(DeleteAttachmentApiParams),
            handler: Box::new(|ctx, value| {
                Box::pin(async move {
                    let params: DeleteAttachmentApiParams = serde_json::from_value(value)?;
                    delete_attachment(&ctx, params).await
                })
            }),
            binary_input: None,
            binary_output: None,
        },
        ApiOperation {
            name: "list_attachments",
            description: "List all attachments in the workspace.",
            scope: "attachments:read",
            rest_method: Method::GET,
            rest_path: "/attachments",
            json_schema: || schemars::schema_for!(ListAttachmentsApiParams),
            handler: Box::new(|ctx, value| {
                Box::pin(async move {
                    let params: ListAttachmentsApiParams = serde_json::from_value(value)?;
                    list_attachments(&ctx, params).await
                })
            }),
            binary_input: None,
            binary_output: None,
        },
    ]
}
