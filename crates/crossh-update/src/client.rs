//! HTTPS release manifest and artifact transport.

use std::path::{Path, PathBuf};
use std::time::Duration;

use reqwest::StatusCode;
use sha2::{Digest, Sha256};
use thiserror::Error;
use tokio::io::AsyncWriteExt;

use super::model::{
    MAX_DOWNLOAD_BYTES, MAX_MANIFEST_BYTES, UpdateArtifact, UpdateManifest, parse_manifest,
    validate_https_url,
};
use crate::DEFAULT_ACCELERATE_PREFIX;

const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, Error)]
pub enum UpdateError {
    #[error("update request failed: {0}")]
    Request(#[from] reqwest::Error),
    #[error("update server returned HTTP {0}")]
    HttpStatus(StatusCode),
    #[error("update manifest is too large")]
    ManifestTooLarge,
    #[error("update manifest is invalid: {0}")]
    Manifest(#[from] super::model::ManifestError),
    #[error("update file operation failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("update download is too large")]
    TooLarge,
    #[error("update download size mismatch: expected {expected}, got {actual}")]
    SizeMismatch { expected: u64, actual: u64 },
    #[error("update checksum mismatch: expected {expected}, got {actual}")]
    ChecksumMismatch { expected: String, actual: String },
}

impl UpdateError {
    /// 是否为传输层错误（换通道重试有意义）；校验类错误不在此列。
    /// 加速源与 GitHub 原站两条通道共享此分类：传输错误触发回退，
    /// 校验失败（内容不可信）直接返回、不再换通道尝试。
    pub(crate) fn is_transport(&self) -> bool {
        matches!(self, UpdateError::Request(_) | UpdateError::HttpStatus(_))
    }
}

/// 把 GitHub release 资产 URL 重写为加速前缀 + 原始完整 URL；
/// 非 github.com 域名的 URL 原样返回（不重写）。
fn rewrite_url(url: &str, prefix: &str) -> String {
    if url.starts_with("https://github.com/") {
        format!("{prefix}{url}")
    } else {
        url.to_owned()
    }
}

/// 候选请求序列：加速优先、GitHub 原站兜底；重写不生效时去重为单候选。
fn candidate_urls(url: &str) -> Vec<String> {
    let accelerated = rewrite_url(url, DEFAULT_ACCELERATE_PREFIX);
    if accelerated == url {
        vec![url.to_owned()]
    } else {
        vec![accelerated, url.to_owned()]
    }
}

fn client() -> Result<reqwest::Client, reqwest::Error> {
    reqwest::Client::builder()
        .user_agent(concat!("crossh/", env!("CARGO_PKG_VERSION")))
        .timeout(REQUEST_TIMEOUT)
        .build()
}

pub async fn fetch_manifest(url: &str) -> Result<UpdateManifest, UpdateError> {
    // 加速源优先、GitHub 原站兜底；仅传输类错误触发回退，
    // 校验类错误（验签失败等）直接返回，不换通道重试。
    let mut last_error = None;
    for candidate in candidate_urls(url) {
        match fetch_manifest_once(&candidate).await {
            Ok(manifest) => return Ok(manifest),
            Err(error) if error.is_transport() => last_error = Some(error),
            Err(error) => return Err(error),
        }
    }
    Err(last_error.expect("candidate list is never empty"))
}

async fn fetch_manifest_once(url: &str) -> Result<UpdateManifest, UpdateError> {
    validate_https_url(url)?;
    let mut response = client()?.get(url).send().await?;
    if !response.status().is_success() {
        return Err(UpdateError::HttpStatus(response.status()));
    }
    validate_https_url(response.url().as_str())?;
    if response
        .content_length()
        .is_some_and(|size| size > MAX_MANIFEST_BYTES as u64)
    {
        return Err(UpdateError::ManifestTooLarge);
    }
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        let next_size = body
            .len()
            .checked_add(chunk.len())
            .ok_or(UpdateError::ManifestTooLarge)?;
        if next_size > MAX_MANIFEST_BYTES {
            return Err(UpdateError::ManifestTooLarge);
        }
        body.extend_from_slice(&chunk);
    }
    Ok(parse_manifest(&body)?)
}

pub async fn download_artifact(
    artifact: &UpdateArtifact,
    version: &str,
    target: &str,
) -> Result<PathBuf, UpdateError> {
    artifact.validate()?;
    let cache_dir = dirs::cache_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("crossh")
        .join("updates");
    tokio::fs::create_dir_all(&cache_dir).await?;

    let destination = cache_dir.join(format!(
        "crossh-{version}-{target}.{}",
        artifact.format.as_str()
    ));
    let temporary = destination.with_extension("part");

    // 加速源优先、GitHub 原站兜底；仅传输类错误触发回退，
    // 校验类错误（checksum/size 不匹配等）直接返回，不换通道重试。
    let mut last_error = None;
    for url in candidate_urls(&artifact.url) {
        match download_from(&url, artifact, &temporary).await {
            Ok(()) => {
                if let Err(error) = tokio::fs::rename(&temporary, &destination).await {
                    let _ = tokio::fs::remove_file(&temporary).await;
                    return Err(error.into());
                }
                return Ok(destination);
            }
            Err(error) if error.is_transport() => {
                let _ = tokio::fs::remove_file(&temporary).await;
                last_error = Some(error);
            }
            Err(error) => {
                let _ = tokio::fs::remove_file(&temporary).await;
                return Err(error);
            }
        }
    }
    Err(last_error.expect("candidate list is never empty"))
}

async fn download_from(
    url: &str,
    artifact: &UpdateArtifact,
    temporary: &Path,
) -> Result<(), UpdateError> {
    let response = client()?.get(url).send().await?;
    if !response.status().is_success() {
        return Err(UpdateError::HttpStatus(response.status()));
    }
    validate_https_url(response.url().as_str())?;
    if let Some(size) = response.content_length() {
        if size > MAX_DOWNLOAD_BYTES {
            return Err(UpdateError::TooLarge);
        }
        if size != artifact.size {
            return Err(UpdateError::SizeMismatch {
                expected: artifact.size,
                actual: size,
            });
        }
    }
    write_and_verify(response, temporary, &artifact.sha256, artifact.size).await
}

async fn write_and_verify(
    mut response: reqwest::Response,
    temporary: &Path,
    expected_checksum: &str,
    expected_size: u64,
) -> Result<(), UpdateError> {
    let mut file = tokio::fs::File::create(temporary).await?;
    let mut verifier = DownloadVerifier::new(expected_checksum, expected_size);

    while let Some(chunk) = response.chunk().await? {
        verifier.push(&chunk)?;
        file.write_all(&chunk).await?;
    }
    file.flush().await?;
    file.sync_all().await?;
    verifier.finish()
}

struct DownloadVerifier<'a> {
    hasher: Sha256,
    downloaded: u64,
    expected_checksum: &'a str,
    expected_size: u64,
}

impl<'a> DownloadVerifier<'a> {
    fn new(expected_checksum: &'a str, expected_size: u64) -> Self {
        Self {
            hasher: Sha256::new(),
            downloaded: 0,
            expected_checksum,
            expected_size,
        }
    }

    fn push(&mut self, chunk: &[u8]) -> Result<(), UpdateError> {
        self.downloaded = self
            .downloaded
            .checked_add(chunk.len() as u64)
            .ok_or(UpdateError::TooLarge)?;
        if self.downloaded > MAX_DOWNLOAD_BYTES {
            return Err(UpdateError::TooLarge);
        }
        self.hasher.update(chunk);
        Ok(())
    }

    fn finish(self) -> Result<(), UpdateError> {
        if self.downloaded != self.expected_size {
            return Err(UpdateError::SizeMismatch {
                expected: self.expected_size,
                actual: self.downloaded,
            });
        }
        let actual = hex::encode(self.hasher.finalize());
        if !actual.eq_ignore_ascii_case(self.expected_checksum) {
            return Err(UpdateError::ChecksumMismatch {
                expected: self.expected_checksum.to_owned(),
                actual,
            });
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::DEFAULT_ACCELERATE_PREFIX;

    fn checksum(bytes: &[u8]) -> String {
        let mut hasher = Sha256::new();
        hasher.update(bytes);
        hex::encode(hasher.finalize())
    }

    #[test]
    fn verifier_accepts_arbitrary_chunk_boundaries() {
        let bytes = "crossh-update".as_bytes();
        let expected = checksum(bytes);
        let mut verifier = DownloadVerifier::new(&expected, bytes.len() as u64);
        for chunk in bytes.chunks(2) {
            verifier.push(chunk).unwrap();
        }
        verifier.finish().unwrap();
    }

    #[test]
    fn verifier_rejects_size_and_checksum_mismatches() {
        let expected = checksum(b"crossh");
        let mut short = DownloadVerifier::new(&expected, 7);
        short.push(b"crossh").unwrap();
        assert!(matches!(
            short.finish(),
            Err(UpdateError::SizeMismatch {
                expected: 7,
                actual: 6
            })
        ));

        let mut corrupt = DownloadVerifier::new(&expected, 6);
        corrupt.push(b"crosSh").unwrap();
        assert!(matches!(
            corrupt.finish(),
            Err(UpdateError::ChecksumMismatch { .. })
        ));
    }

    #[test]
    fn spec_20260818_update_accel_rewrite_url_prefixes_github_release_urls() {
        let url =
            "https://github.com/xcrong/crossh/releases/download/v1/crossh-1.0.0-aarch64-macos.zip";
        assert_eq!(
            rewrite_url(url, DEFAULT_ACCELERATE_PREFIX),
            format!("{DEFAULT_ACCELERATE_PREFIX}{url}")
        );
    }

    #[test]
    fn spec_20260818_update_accel_rewrite_url_covers_latest_download_path() {
        let url = "https://github.com/xcrong/crossh/releases/latest/download/stable.json";
        assert_eq!(
            rewrite_url(url, DEFAULT_ACCELERATE_PREFIX),
            "https://gh-proxy.com/https://github.com/xcrong/crossh/releases/latest/download/stable.json"
        );
    }

    #[test]
    fn spec_20260818_update_accel_rewrite_url_leaves_non_github_urls_unchanged() {
        let url = "https://example.com/crossh/stable.json";
        assert_eq!(rewrite_url(url, DEFAULT_ACCELERATE_PREFIX), url);
    }

    #[test]
    fn spec_20260818_update_accel_candidate_urls_are_accelerated_first_and_deduped() {
        let github = "https://github.com/xcrong/crossh/releases/latest/download/stable.json";
        assert_eq!(
            candidate_urls(github),
            vec![
                "https://gh-proxy.com/https://github.com/xcrong/crossh/releases/latest/download/stable.json"
                    .to_owned(),
                github.to_owned(),
            ]
        );

        let non_github = "https://example.com/stable.json";
        assert_eq!(candidate_urls(non_github), vec![non_github.to_owned()]);
    }

    #[test]
    fn spec_20260818_update_accel_transport_errors_are_retryable_verification_errors_are_not() {
        assert!(UpdateError::HttpStatus(StatusCode::BAD_GATEWAY).is_transport());

        assert!(!UpdateError::ManifestTooLarge.is_transport());
        assert!(
            !UpdateError::Manifest(super::super::model::ManifestError::MissingSignature)
                .is_transport()
        );
        assert!(!UpdateError::TooLarge.is_transport());
        assert!(
            !UpdateError::SizeMismatch {
                expected: 1,
                actual: 2,
            }
            .is_transport()
        );
        assert!(
            !UpdateError::ChecksumMismatch {
                expected: "a".into(),
                actual: "b".into(),
            }
            .is_transport()
        );
        assert!(!UpdateError::Io(std::io::Error::other("disk")).is_transport());
    }

    #[tokio::test]
    async fn spec_20260818_update_accel_request_error_is_classified_as_transport() {
        // 127.0.0.1:1 无服务监听，连接立即被拒绝，构造真实的传输层错误。
        // no_proxy() 必须显式禁用，否则本机 http_proxy 代理会转发请求并返回 502。
        let client = reqwest::Client::builder().no_proxy().build().unwrap();
        let error = client.get("http://127.0.0.1:1/").send().await.unwrap_err();
        assert!(UpdateError::from(error).is_transport());
    }
}
