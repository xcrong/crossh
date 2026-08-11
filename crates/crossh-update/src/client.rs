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

fn client() -> Result<reqwest::Client, reqwest::Error> {
    reqwest::Client::builder()
        .user_agent(concat!("crossh/", env!("CARGO_PKG_VERSION")))
        .timeout(REQUEST_TIMEOUT)
        .build()
}

pub async fn fetch_manifest(url: &str) -> Result<UpdateManifest, UpdateError> {
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

    let response = client()?.get(&artifact.url).send().await?;
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

    let result = write_and_verify(response, &temporary, &artifact.sha256, artifact.size).await;
    if result.is_err() {
        let _ = tokio::fs::remove_file(&temporary).await;
    }
    result?;
    if let Err(error) = tokio::fs::rename(&temporary, &destination).await {
        let _ = tokio::fs::remove_file(&temporary).await;
        return Err(error.into());
    }
    Ok(destination)
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
}
