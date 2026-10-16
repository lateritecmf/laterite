//! The S3-compatible disk: AWS S3 itself, or any store that speaks its API
//! behind an endpoint override (Cloudflare R2, MinIO, Backblaze B2,
//! DigitalOcean Spaces, Wasabi). Behind the `s3` feature.

use std::time::Duration;

use aws_sdk_s3::config::{Credentials, Region};
use aws_sdk_s3::presigning::PresigningConfig;
use aws_sdk_s3::primitives::ByteStream as S3Body;
use aws_sdk_s3::types::{CompletedMultipartUpload, CompletedPart};
use aws_sdk_s3::Client;
use bytes::{Bytes, BytesMut};
use futures::TryStreamExt;
use laterite_core::strata::async_trait;
use serde::Deserialize;

use crate::driver::{BlobMeta, ByteStream, Disposition, StorageDriver, StorageError};

/// A write up to this size goes in one request; past it, in parts of this
/// size through a multipart upload, so a stream of any length lands without
/// being held in memory.
pub const PART_SIZE: usize = 16 * 1024 * 1024;

/// How an S3 disk is reached, as an application's configuration names it.
///
/// ```toml
/// [storage.disks.s3]
/// bucket = "acme-media"
/// region = "auto"
/// endpoint = "https://<account>.r2.cloudflarestorage.com"
/// access_key = "..."
/// secret_key = "..."
/// prefix = "media"
/// public_url = "https://media.acme.test"
/// ```
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct S3Config {
    pub bucket: String,
    /// The region name; `auto` for a store that ignores it.
    pub region: String,
    /// The store's URL for anything that is not AWS S3 itself.
    pub endpoint: Option<String>,
    /// Static credentials; unset, the SDK's default chain (environment,
    /// profile, instance role) is used.
    pub access_key: Option<String>,
    pub secret_key: Option<String>,
    /// A key prefix every blob goes under, with no slashes at the ends.
    pub prefix: String,
    /// The origin blobs are reachable at without a signature (a public bucket
    /// or a CDN in front of it); unset, the disk is private and serves through
    /// presigned URLs.
    pub public_url: Option<String>,
    /// Path-style addressing (`endpoint/bucket/key`), which MinIO and most
    /// self-hosted stores want; the default for any endpoint override.
    pub path_style: Option<bool>,
}

/// Blobs in a bucket.
pub struct S3Disk {
    client: Client,
    bucket: String,
    prefix: String,
    public_url: Option<String>,
}

impl S3Disk {
    /// A disk from its configuration. Static credentials are used when both
    /// keys are set; otherwise the SDK's default provider chain.
    pub async fn from_config(config: &S3Config) -> Self {
        let region = Region::new(if config.region.is_empty() {
            "auto".to_string()
        } else {
            config.region.clone()
        });
        let shared = aws_config::defaults(aws_config::BehaviorVersion::latest())
            .region(region)
            .load()
            .await;
        let mut builder = aws_sdk_s3::config::Builder::from(&shared);
        if let Some(endpoint) = &config.endpoint {
            builder = builder
                .endpoint_url(endpoint)
                .force_path_style(config.path_style.unwrap_or(true));
        } else if let Some(path_style) = config.path_style {
            builder = builder.force_path_style(path_style);
        }
        if let (Some(key), Some(secret)) = (&config.access_key, &config.secret_key) {
            builder = builder.credentials_provider(Credentials::new(
                key.clone(),
                secret.clone(),
                None,
                None,
                "laterite",
            ));
        }
        Self::with_client(Client::from_conf(builder.build()), config)
    }

    /// A disk over a client the application built itself.
    pub fn with_client(client: Client, config: &S3Config) -> Self {
        Self {
            client,
            bucket: config.bucket.clone(),
            prefix: config.prefix.trim_matches('/').to_string(),
            public_url: config
                .public_url
                .as_ref()
                .map(|u| u.trim_end_matches('/').to_string()),
        }
    }

    /// The object key for a blob path.
    pub fn key(&self, path: &str) -> String {
        let path = path.trim_start_matches('/');
        if self.prefix.is_empty() {
            path.to_string()
        } else {
            format!("{}/{path}", self.prefix)
        }
    }

    pub fn bucket(&self) -> &str {
        &self.bucket
    }

    pub fn client(&self) -> &Client {
        &self.client
    }

    /// A URL a browser may PUT `path` to for `expires_in`, with the content
    /// type it has to send: what a direct upload uses.
    pub async fn presigned_put(
        &self,
        path: &str,
        content_type: &str,
        expires_in: Duration,
    ) -> Result<String, StorageError> {
        let config = PresigningConfig::expires_in(expires_in).map_err(other)?;
        let req = self
            .client
            .put_object()
            .bucket(&self.bucket)
            .key(self.key(path))
            .content_type(content_type)
            .presigned(config)
            .await
            .map_err(other)?;
        Ok(req.uri().to_string())
    }

    /// Copies an object within the bucket, from one blob path to another.
    pub async fn copy(&self, from: &str, to: &str) -> Result<(), StorageError> {
        self.client
            .copy_object()
            .bucket(&self.bucket)
            .copy_source(format!("{}/{}", self.bucket, self.key(from)))
            .key(self.key(to))
            .send()
            .await
            .map_err(other)?;
        Ok(())
    }

    async fn abort(&self, key: &str, upload_id: &str) {
        let _ = self
            .client
            .abort_multipart_upload()
            .bucket(&self.bucket)
            .key(key)
            .upload_id(upload_id)
            .send()
            .await;
    }
}

fn other(e: impl std::fmt::Display) -> StorageError {
    StorageError::Io(std::io::Error::other(e.to_string()))
}

#[async_trait]
impl StorageDriver for S3Disk {
    /// One request for a stream under [`PART_SIZE`]; a multipart upload of
    /// [`PART_SIZE`] parts past it, aborted if any part fails so nothing half
    /// written is left under a valid key.
    async fn put(&self, path: &str, data: ByteStream) -> Result<(), StorageError> {
        let key = self.key(path);
        let mut data = data;
        let mut buffer = BytesMut::with_capacity(64 * 1024);
        let mut ended = false;
        while buffer.len() < PART_SIZE {
            match data.try_next().await? {
                Some(chunk) => buffer.extend_from_slice(&chunk),
                None => {
                    ended = true;
                    break;
                }
            }
        }
        if ended {
            let len = buffer.len() as i64;
            self.client
                .put_object()
                .bucket(&self.bucket)
                .key(&key)
                .body(S3Body::from(buffer.freeze()))
                .content_length(len)
                .send()
                .await
                .map_err(other)?;
            return Ok(());
        }
        let started = self
            .client
            .create_multipart_upload()
            .bucket(&self.bucket)
            .key(&key)
            .send()
            .await
            .map_err(other)?;
        let upload_id = started.upload_id().unwrap_or_default().to_string();
        let mut parts: Vec<CompletedPart> = Vec::new();
        let mut number: i32 = 0;
        loop {
            // Everything read so far, past one part, is sent part by part.
            while buffer.len() >= PART_SIZE || (ended && !buffer.is_empty()) {
                let take = buffer.len().min(PART_SIZE);
                let part: Bytes = buffer.split_to(take).freeze();
                number += 1;
                let sent = self
                    .client
                    .upload_part()
                    .bucket(&self.bucket)
                    .key(&key)
                    .upload_id(&upload_id)
                    .part_number(number)
                    .content_length(part.len() as i64)
                    .body(S3Body::from(part))
                    .send()
                    .await;
                let sent = match sent {
                    Ok(s) => s,
                    Err(e) => {
                        self.abort(&key, &upload_id).await;
                        return Err(other(e));
                    }
                };
                parts.push(
                    CompletedPart::builder()
                        .part_number(number)
                        .set_e_tag(sent.e_tag().map(str::to_string))
                        .build(),
                );
            }
            if ended {
                break;
            }
            match data.try_next().await {
                Ok(Some(chunk)) => buffer.extend_from_slice(&chunk),
                Ok(None) => ended = true,
                Err(e) => {
                    self.abort(&key, &upload_id).await;
                    return Err(e.into());
                }
            }
        }
        let done = CompletedMultipartUpload::builder()
            .set_parts(Some(parts))
            .build();
        if let Err(e) = self
            .client
            .complete_multipart_upload()
            .bucket(&self.bucket)
            .key(&key)
            .upload_id(&upload_id)
            .multipart_upload(done)
            .send()
            .await
        {
            self.abort(&key, &upload_id).await;
            return Err(other(e));
        }
        Ok(())
    }

    async fn get(&self, path: &str) -> Result<ByteStream, StorageError> {
        let out = self
            .client
            .get_object()
            .bucket(&self.bucket)
            .key(self.key(path))
            .send()
            .await
            .map_err(|e| {
                StorageError::Io(std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    e.to_string(),
                ))
            })?;
        Ok(Box::pin(tokio_util::io::ReaderStream::new(
            out.body.into_async_read(),
        )))
    }

    /// Missing is success, as for every driver.
    async fn delete(&self, path: &str) -> Result<(), StorageError> {
        self.client
            .delete_object()
            .bucket(&self.bucket)
            .key(self.key(path))
            .send()
            .await
            .map_err(other)?;
        Ok(())
    }

    async fn metadata(&self, path: &str) -> Result<Option<BlobMeta>, StorageError> {
        match self
            .client
            .head_object()
            .bucket(&self.bucket)
            .key(self.key(path))
            .send()
            .await
        {
            Ok(head) => Ok(Some(BlobMeta {
                size: head.content_length().unwrap_or(0).max(0) as u64,
            })),
            Err(e) => {
                let service = e.as_service_error();
                if service.is_some_and(|s| s.is_not_found()) {
                    Ok(None)
                } else {
                    Err(other(e))
                }
            }
        }
    }

    fn public_url(&self, path: &str) -> Option<String> {
        let origin = self.public_url.as_ref()?;
        Some(format!("{origin}/{}", self.key(path)))
    }

    /// A presigned GET, with the disposition the serve route asked for.
    async fn signed_url(
        &self,
        path: &str,
        expires_in: Duration,
        disposition: Disposition,
    ) -> Result<Option<String>, StorageError> {
        let config = PresigningConfig::expires_in(expires_in).map_err(other)?;
        let mut req = self
            .client
            .get_object()
            .bucket(&self.bucket)
            .key(self.key(path));
        if disposition == Disposition::Attachment {
            req = req.response_content_disposition("attachment");
        }
        let signed = req.presigned(config).await.map_err(other)?;
        Ok(Some(signed.uri().to_string()))
    }
}

impl std::fmt::Debug for S3Disk {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("S3Disk")
            .field("bucket", &self.bucket)
            .field("prefix", &self.prefix)
            .field("public", &self.public_url.is_some())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn disk(prefix: &str, public: Option<&str>) -> S3Disk {
        let config = aws_sdk_s3::Config::builder()
            .behavior_version_latest()
            .region(Region::new("auto"))
            .endpoint_url("http://127.0.0.1:1")
            .force_path_style(true)
            .credentials_provider(Credentials::new("k", "s", None, None, "test"))
            .build();
        S3Disk::with_client(
            Client::from_conf(config),
            &S3Config {
                bucket: "acme".into(),
                prefix: prefix.into(),
                public_url: public.map(str::to_string),
                ..Default::default()
            },
        )
    }

    #[test]
    fn keys_carry_the_prefix_and_public_urls_the_origin() {
        let d = disk("/media/", Some("https://cdn.acme.test/"));
        assert_eq!(d.key("blobs/ab/cd/abcd"), "media/blobs/ab/cd/abcd");
        assert_eq!(
            d.public_url("blobs/ab/cd/abcd").as_deref(),
            Some("https://cdn.acme.test/media/blobs/ab/cd/abcd")
        );
        let bare = disk("", None);
        assert_eq!(bare.key("/blobs/x"), "blobs/x");
        assert_eq!(bare.public_url("blobs/x"), None);
    }

    #[tokio::test]
    async fn a_presigned_url_names_the_key_and_carries_a_signature() {
        let d = disk("media", None);
        let url = d
            .signed_url("blobs/x", Duration::from_secs(300), Disposition::Attachment)
            .await
            .unwrap()
            .unwrap();
        assert!(
            url.starts_with("http://127.0.0.1:1/acme/media/blobs/x?"),
            "{url}"
        );
        assert!(
            url.contains("X-Amz-Signature=")
                && url.contains("response-content-disposition=attachment"),
            "{url}"
        );
        let put = d
            .presigned_put("quarantine/y", "image/png", Duration::from_secs(300))
            .await
            .unwrap();
        assert!(
            put.contains("/acme/media/quarantine/y?") && put.contains("X-Amz-Expires=300"),
            "{put}"
        );
    }
}
