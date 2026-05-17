// SPDX-License-Identifier: AGPL-3.0-or-later

//! Attachment storage abstraction — local filesystem and S3-compatible backends.
//!
//! The `AttachmentStorage` trait defines a uniform interface for storing, retrieving,
//! and deleting attachment file data. Use `create_storage` to instantiate the
//! appropriate backend based on application configuration.

use async_trait::async_trait;
use trakkt_core::Result;

#[async_trait]
pub trait AttachmentStorage: Send + Sync {
    /// Store file bytes, return the storage_path for later retrieval.
    async fn store(
        &self,
        workspace_id: &str,
        attachment_id: &str,
        bytes: &[u8],
        filename: &str,
        content_type: &str,
    ) -> Result<String>;

    /// Retrieve file bytes from storage.
    async fn retrieve(&self, storage_path: &str) -> Result<Vec<u8>>;

    /// Delete a stored file.
    async fn delete(&self, storage_path: &str) -> Result<()>;
}

// ── Local Filesystem Storage ────────────────────────────────────────────────

pub struct LocalAttachmentStorage {
    base_path: String,
}

impl LocalAttachmentStorage {
    pub fn new(base_path: String) -> Self {
        Self { base_path }
    }
}

#[async_trait]
impl AttachmentStorage for LocalAttachmentStorage {
    async fn store(
        &self,
        workspace_id: &str,
        attachment_id: &str,
        bytes: &[u8],
        filename: &str,
        content_type: &str,
    ) -> Result<String> {
        let ext = extension_from_filename(filename, content_type);
        let dir = format!("{}/{}", self.base_path, workspace_id);
        tokio::fs::create_dir_all(&dir).await.map_err(|e| {
            trakkt_core::Error::Internal(format!("Failed to create attachment directory: {e}"))
        })?;

        let stored_name = format!("{attachment_id}.{ext}");
        let path = format!("{dir}/{stored_name}");
        tokio::fs::write(&path, bytes).await.map_err(|e| {
            trakkt_core::Error::Internal(format!("Failed to write attachment: {e}"))
        })?;

        let storage_path = format!("{workspace_id}/{stored_name}");
        Ok(storage_path)
    }

    async fn retrieve(&self, storage_path: &str) -> Result<Vec<u8>> {
        let full_path = format!("{}/{}", self.base_path, storage_path);
        tokio::fs::read(&full_path).await.map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                trakkt_core::Error::NotFound("Attachment file not found".into())
            } else {
                trakkt_core::Error::Internal(format!("Failed to read attachment: {e}"))
            }
        })
    }

    async fn delete(&self, storage_path: &str) -> Result<()> {
        let full_path = format!("{}/{}", self.base_path, storage_path);
        match tokio::fs::remove_file(&full_path).await {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(trakkt_core::Error::Internal(format!(
                "Failed to delete attachment: {e}"
            ))),
        }
    }
}

// ── S3 Storage ────��──────────────────────────────���──────────────────────────

pub struct S3AttachmentStorage {
    bucket: s3::Bucket,
}

impl S3AttachmentStorage {
    pub fn new(
        endpoint: &str,
        bucket_name: &str,
        access_key: &str,
        secret_key: &str,
        region: &str,
    ) -> Result<Self> {
        let region = s3::Region::Custom {
            region: region.to_string(),
            endpoint: endpoint.to_string(),
        };
        let credentials = s3::creds::Credentials::new(
            Some(access_key),
            Some(secret_key),
            None,
            None,
            None,
        )
        .map_err(|e| trakkt_core::Error::Internal(format!("S3 credentials error: {e}")))?;

        let bucket = *s3::Bucket::new(bucket_name, region, credentials)
            .map_err(|e| trakkt_core::Error::Internal(format!("S3 bucket error: {e}")))?
            .with_path_style();

        Ok(Self { bucket })
    }
}

#[async_trait]
impl AttachmentStorage for S3AttachmentStorage {
    async fn store(
        &self,
        workspace_id: &str,
        attachment_id: &str,
        bytes: &[u8],
        filename: &str,
        content_type: &str,
    ) -> Result<String> {
        let ext = extension_from_filename(filename, content_type);
        let storage_path = format!("{workspace_id}/{attachment_id}.{ext}");

        self.bucket
            .put_object_with_content_type(&storage_path, bytes, content_type)
            .await
            .map_err(|e| trakkt_core::Error::Internal(format!("S3 upload failed: {e}")))?;

        Ok(storage_path)
    }

    async fn retrieve(&self, storage_path: &str) -> Result<Vec<u8>> {
        let response = self
            .bucket
            .get_object(storage_path)
            .await
            .map_err(|e| trakkt_core::Error::Internal(format!("S3 download failed: {e}")))?;

        if response.status_code() == 404 {
            return Err(trakkt_core::Error::NotFound(
                "Attachment not found in S3".into(),
            ));
        }

        Ok(response.to_vec())
    }

    async fn delete(&self, storage_path: &str) -> Result<()> {
        self.bucket
            .delete_object(storage_path)
            .await
            .map_err(|e| trakkt_core::Error::Internal(format!("S3 delete failed: {e}")))?;

        Ok(())
    }
}

// ── Helpers ───────────────────────────────────────────────────────────���─────

/// Extract extension from the original filename, falling back to content-type derivation.
fn extension_from_filename<'a>(filename: &'a str, content_type: &str) -> &'a str {
    if let Some(ext) = filename.rsplit('.').next()
        && !ext.is_empty()
        && ext.len() <= 10
        && ext != filename
    {
        return ext;
    }
    match content_type {
        "image/png" => "png",
        "image/jpeg" => "jpg",
        "image/gif" => "gif",
        "image/webp" => "webp",
        "image/svg+xml" => "svg",
        "application/pdf" => "pdf",
        "text/csv" => "csv",
        "text/plain" => "txt",
        "application/json" => "json",
        _ => "bin",
    }
}

/// Create the appropriate storage backend from config.
pub fn create_storage(config: &trakkt_core::Config) -> Result<Box<dyn AttachmentStorage>> {
    match config.attachment_storage.as_str() {
        "s3" => {
            let endpoint = config.attachment_s3_endpoint.as_deref().ok_or_else(|| {
                trakkt_core::Error::Internal(
                    "ATTACHMENT_S3_ENDPOINT required for S3 storage".into(),
                )
            })?;
            let bucket = config.attachment_s3_bucket.as_deref().ok_or_else(|| {
                trakkt_core::Error::Internal(
                    "ATTACHMENT_S3_BUCKET required for S3 storage".into(),
                )
            })?;
            let access_key = config.attachment_s3_access_key.as_deref().ok_or_else(|| {
                trakkt_core::Error::Internal(
                    "ATTACHMENT_S3_ACCESS_KEY required for S3 storage".into(),
                )
            })?;
            let secret_key = config.attachment_s3_secret_key.as_deref().ok_or_else(|| {
                trakkt_core::Error::Internal(
                    "ATTACHMENT_S3_SECRET_KEY required for S3 storage".into(),
                )
            })?;
            let region = config
                .attachment_s3_region
                .as_deref()
                .unwrap_or("us-east-1");

            let storage =
                S3AttachmentStorage::new(endpoint, bucket, access_key, secret_key, region)?;
            Ok(Box::new(storage))
        }
        _ => Ok(Box::new(LocalAttachmentStorage::new(
            config.attachment_local_path.clone(),
        ))),
    }
}
