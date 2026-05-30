// SPDX-License-Identifier: AGPL-3.0-or-later

//! Reusable attachment callbacks for the kode WYSIWYG editor.
//!
//! Provides `make_upload_callback`, `make_delete_callback`, and
//! `make_click_callback` — the three closures the `TreeWysiwygEditor` needs
//! to integrate with Trakkt's attachment REST API.

use std::sync::Arc;

use leptos::prelude::*;
use kode_leptos::{
    AttachmentInsert, AttachmentNodeType, ClickAttachmentRequest,
    DeleteAttachmentRequest, UploadComplete, UploadTrigger,
};

/// Maximum upload size (10 MB).
pub(crate) const MAX_FILE_SIZE: u64 = 10 * 1024 * 1024;

/// Allowed file extensions for upload.
pub(crate) const ALLOWED_EXTENSIONS: &[&str] = &[
    "png", "jpg", "jpeg", "gif", "webp", "svg",
    "pdf", "csv", "txt", "json", "log",
];

/// Allowed MIME content types for upload.
pub(crate) const ALLOWED_CONTENT_TYPES: &[&str] = &[
    "image/png", "image/jpeg", "image/gif", "image/webp", "image/svg+xml",
    "application/pdf", "text/csv", "text/plain", "application/json",
];

/// State for the image lightbox overlay.
#[derive(Clone, Debug)]
pub struct LightboxState {
    pub src: String,
}

/// Create the `on_upload` callback that uploads via fetch to `/api/v1/attachments`.
///
/// On success, signals `UploadComplete` with the inserted attachment metadata.
/// On failure (validation or network), signals `UploadComplete { insert: None }` to
/// remove the placeholder from the document.
///
/// When `issue_id` is provided, the upload request includes it as a query parameter
/// so the backend auto-links the attachment to the issue via the junction table.
pub fn make_upload_callback(
    upload_complete: RwSignal<Option<UploadComplete>>,
    issue_id: Option<String>,
    error_toast: impl Fn(String) + Clone + Send + Sync + 'static,
) -> Arc<dyn Fn(UploadTrigger) + Send + Sync> {
    Arc::new(move |trigger: UploadTrigger| {
        let placeholder_id = trigger.placeholder_id.clone();

        // Client-side size validation
        if trigger.size > MAX_FILE_SIZE {
            tracing::warn!(
                "Attachment rejected: {} exceeds max size ({} > {})",
                trigger.name, trigger.size, MAX_FILE_SIZE
            );
            error_toast("File exceeds 10 MB size limit".to_string());
            upload_complete.set(Some(UploadComplete {
                placeholder_id,
                insert: None,
            }));
            return;
        }

        // Client-side extension/content-type validation
        let ext = trigger.name.rsplit('.').next().unwrap_or("").to_lowercase();
        if !ALLOWED_EXTENSIONS.contains(&ext.as_str())
            && !ALLOWED_CONTENT_TYPES.contains(&trigger.content_type.as_str())
        {
            tracing::warn!(
                "Attachment rejected: {} has disallowed type (ext={}, content_type={})",
                trigger.name, ext, trigger.content_type
            );
            error_toast("File type not allowed".to_string());
            upload_complete.set(Some(UploadComplete {
                placeholder_id,
                insert: None,
            }));
            return;
        }

        // Spawn the async upload
        let issue_id = issue_id.clone();
        let error_toast = error_toast.clone();
        leptos::task::spawn_local(async move {
            match upload_file(&trigger.data, &trigger.name, &trigger.content_type, issue_id.as_deref()).await {
                Ok(resp) => {
                    let insert = if trigger.content_type.starts_with("image/") {
                        AttachmentInsert::Image {
                            src: resp.url,
                            alt: trigger.name,
                            attachment_id: Some(resp.attachment_id),
                            width: None,
                            height: None,
                        }
                    } else {
                        AttachmentInsert::File {
                            href: resp.url,
                            filename: trigger.name,
                            attachment_id: Some(resp.attachment_id),
                            size_bytes: Some(resp.size_bytes),
                            content_type: Some(trigger.content_type),
                        }
                    };
                    upload_complete.set(Some(UploadComplete {
                        placeholder_id,
                        insert: Some(insert),
                    }));
                }
                Err(e) => {
                    tracing::warn!("Attachment upload failed: {e}");
                    error_toast(format!("Upload failed: {e}"));
                    upload_complete.set(Some(UploadComplete {
                        placeholder_id,
                        insert: None,
                    }));
                }
            }
        });
    })
}

/// Create the `on_delete_attachment` callback that DELETEs via `/api/v1/attachments/{id}`.
///
/// When `attachment_id` is `None` (happens after markdown re-parse loses the attribute),
/// falls back to extracting the ID from the URL pattern `/api/v1/attachments/{id}/download`.
pub fn make_delete_callback(
    error_toast: impl Fn(String) + Clone + Send + Sync + 'static,
) -> Arc<dyn Fn(DeleteAttachmentRequest) + Send + Sync> {
    Arc::new(move |req: DeleteAttachmentRequest| {
        let attachment_id = req
            .attachment_id
            .or_else(|| extract_attachment_id_from_url(&req.src_or_href));
        if let Some(attachment_id) = attachment_id {
            let error_toast = error_toast.clone();
            leptos::task::spawn_local(async move {
                if let Err(e) = delete_attachment(&attachment_id).await {
                    tracing::warn!("Failed to delete attachment {attachment_id}: {e}");
                    error_toast(format!("Failed to remove attachment: {e}"));
                }
            });
        }
    })
}

/// Extract attachment ID from a download URL like `/api/v1/attachments/{id}/download`.
fn extract_attachment_id_from_url(url: &str) -> Option<String> {
    let stripped = url.strip_prefix("/api/v1/attachments/")?;
    let id = stripped.strip_suffix("/download").unwrap_or(stripped);
    if id.is_empty() || id.contains('/') {
        return None;
    }
    Some(id.to_string())
}

/// Create the `on_click_attachment` callback.
///
/// - Images: open in a lightbox overlay (via the provided signal).
/// - Files: open in a new browser tab.
pub fn make_click_callback(
    lightbox_signal: RwSignal<Option<LightboxState>>,
) -> Arc<dyn Fn(ClickAttachmentRequest) + Send + Sync> {
    Arc::new(move |req: ClickAttachmentRequest| {
        match req.node_type {
            AttachmentNodeType::Image => {
                lightbox_signal.set(Some(LightboxState {
                    src: req.src_or_href,
                }));
            }
            AttachmentNodeType::File => {
                if let Some(window) = web_sys::window()
                    && let Err(e) = window.open_with_url_and_target_and_features(&req.src_or_href, "_blank", "noopener,noreferrer")
                {
                    tracing::warn!("Failed to open file in new tab: {e:?}");
                }
            }
        }
    })
}

// ─────────────────────────────────────────────────────────────────────────────
// Internal: browser fetch helpers
// ─────────────────────────────────────────────────────────────────────────────

pub(crate) struct UploadResponse {
    pub(crate) attachment_id: String,
    pub(crate) url: String,
    pub(crate) size_bytes: u64,
}

/// Upload a file via the browser's fetch API (multipart form POST).
///
/// When `issue_id` is provided, it is appended as a query parameter so the
/// backend can auto-link the attachment to the issue after storing it.
pub(crate) async fn upload_file(data: &[u8], filename: &str, content_type: &str, issue_id: Option<&str>) -> Result<UploadResponse, String> {
    use js_sys::{Array, Uint8Array};
    use wasm_bindgen::JsCast;
    use wasm_bindgen_futures::JsFuture;
    use web_sys::{Blob, BlobPropertyBag, FormData, Request, RequestInit, Response};

    let window = web_sys::window().ok_or("no window")?;

    // Create a Blob from the raw bytes
    let uint8_array = Uint8Array::new_with_length(data.len() as u32);
    uint8_array.copy_from(data);
    let blob_parts = Array::new();
    blob_parts.push(&uint8_array.buffer());
    let opts = BlobPropertyBag::new();
    opts.set_type(content_type);
    let blob = Blob::new_with_buffer_source_sequence_and_options(&blob_parts, &opts)
        .map_err(|_| "Failed to create Blob")?;

    // Create FormData with the file
    let form_data = FormData::new().map_err(|_| "Failed to create FormData")?;
    form_data
        .append_with_blob_and_filename("file", &blob, filename)
        .map_err(|_| "Failed to append file to FormData")?;

    // Build the fetch request
    let init = RequestInit::new();
    init.set_method("POST");
    init.set_body_opt_form_data(Some(&form_data));
    init.set_credentials(web_sys::RequestCredentials::Include);

    let url = match issue_id {
        Some(id) => format!("/api/v1/attachments?issue_id={}", js_sys::encode_uri_component(id)),
        None => "/api/v1/attachments".to_string(),
    };
    let request = Request::new_with_str_and_init(&url, &init)
        .map_err(|_| "Failed to create Request")?;

    let resp_value = JsFuture::from(window.fetch_with_request(&request))
        .await
        .map_err(|_| "Fetch failed")?;
    let resp: Response = resp_value.unchecked_into();

    if !resp.ok() {
        return Err(format!("Upload failed with status {}", resp.status()));
    }

    let json = JsFuture::from(resp.json().map_err(|_| "Failed to get JSON body")?)
        .await
        .map_err(|_| "Failed to parse JSON")?;

    // Extract fields from the JSON response
    let attachment_id = js_sys::Reflect::get(&json, &"attachment_id".into())
        .ok()
        .and_then(|v| v.as_string())
        .ok_or("Missing attachment_id in response")?;
    let url = js_sys::Reflect::get(&json, &"url".into())
        .ok()
        .and_then(|v| v.as_string())
        .ok_or("Missing url in response")?;
    let size_bytes = js_sys::Reflect::get(&json, &"size_bytes".into())
        .ok()
        .and_then(|v| v.as_f64())
        .unwrap_or(0.0) as u64;

    Ok(UploadResponse { attachment_id, url, size_bytes })
}

/// Delete an attachment via the browser's fetch API (DELETE request).
async fn delete_attachment(attachment_id: &str) -> Result<(), String> {
    use wasm_bindgen::JsCast;
    use wasm_bindgen_futures::JsFuture;
    use web_sys::{Request, RequestInit, Response};

    let window = web_sys::window().ok_or("no window")?;

    let init = RequestInit::new();
    init.set_method("DELETE");
    init.set_credentials(web_sys::RequestCredentials::Include);

    let url = format!("/api/v1/attachments/{attachment_id}");
    let request = Request::new_with_str_and_init(&url, &init)
        .map_err(|_| "Failed to create DELETE request")?;

    let resp_value = JsFuture::from(window.fetch_with_request(&request))
        .await
        .map_err(|_| "DELETE fetch failed")?;
    let resp: Response = resp_value.unchecked_into();

    if !resp.ok() {
        return Err(format!("Delete failed with status {}", resp.status()));
    }

    Ok(())
}
