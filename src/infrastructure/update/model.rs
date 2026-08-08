//! Machine-readable release metadata shared by the updater and the UI.

use std::collections::BTreeMap;
use std::path::Path;

use semver::Version;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use url::Url;

pub const MANIFEST_SCHEMA: u32 = 1;
pub const MAX_MANIFEST_BYTES: usize = 1024 * 1024;
pub const MAX_DOWNLOAD_BYTES: u64 = 1024 * 1024 * 1024;

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct UpdateManifest {
    pub schema: u32,
    pub version: String,
    #[serde(default)]
    pub notes: String,
    #[serde(default)]
    pub release_url: Option<String>,
    #[serde(default)]
    pub published_at: Option<String>,
    pub targets: BTreeMap<String, UpdateArtifact>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct UpdateArtifact {
    pub url: String,
    pub filename: String,
    pub format: ArtifactFormat,
    pub sha256: String,
    pub size: u64,
    #[serde(default)]
    pub signature: Option<String>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ArtifactFormat {
    Zip,
    AppImage,
    #[serde(rename = "tar.gz")]
    TarGz,
}

impl ArtifactFormat {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Zip => "zip",
            Self::AppImage => "appimage",
            Self::TarGz => "tar.gz",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum UpdateTarget {
    MacosAarch64,
    MacosX86_64,
    LinuxAarch64,
    LinuxX86_64,
    WindowsAarch64,
    WindowsX86_64,
}

impl UpdateTarget {
    pub fn current() -> Option<Self> {
        if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
            Some(Self::MacosAarch64)
        } else if cfg!(all(target_os = "macos", target_arch = "x86_64")) {
            Some(Self::MacosX86_64)
        } else if cfg!(all(target_os = "linux", target_arch = "aarch64")) {
            Some(Self::LinuxAarch64)
        } else if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
            Some(Self::LinuxX86_64)
        } else if cfg!(all(target_os = "windows", target_arch = "aarch64")) {
            Some(Self::WindowsAarch64)
        } else if cfg!(all(target_os = "windows", target_arch = "x86_64")) {
            Some(Self::WindowsX86_64)
        } else {
            None
        }
    }

    pub const fn key(self) -> &'static str {
        match self {
            Self::MacosAarch64 => "macos-aarch64",
            Self::MacosX86_64 => "macos-x86_64",
            Self::LinuxAarch64 => "linux-aarch64",
            Self::LinuxX86_64 => "linux-x86_64",
            Self::WindowsAarch64 => "windows-aarch64",
            Self::WindowsX86_64 => "windows-x86_64",
        }
    }
}

#[derive(Clone, Debug)]
pub struct UpdateCandidate {
    pub version: Version,
    pub notes: String,
    pub release_url: Option<String>,
    pub artifact: UpdateArtifact,
    pub target: UpdateTarget,
}

#[derive(Debug, Error)]
pub enum ManifestError {
    #[error("invalid release manifest JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("release manifest exceeds the size limit")]
    ManifestTooLarge,
    #[error("unsupported release manifest schema {0}")]
    UnsupportedSchema(u32),
    #[error("invalid release version: {0}")]
    InvalidVersion(String),
    #[error("release manifest has no artifact for {0}")]
    MissingTarget(&'static str),
    #[error("invalid artifact URL")]
    InvalidUrl,
    #[error("artifact URL must use HTTPS")]
    InsecureUrl,
    #[error("artifact filename is unsafe")]
    UnsafeFilename,
    #[error("artifact format does not match filename")]
    FormatMismatch,
    #[error("artifact size is invalid")]
    InvalidSize,
    #[error("artifact checksum is invalid")]
    InvalidChecksum,
    #[error("release notes are too large")]
    NotesTooLarge,
}

pub fn parse_manifest(bytes: &[u8]) -> Result<UpdateManifest, ManifestError> {
    if bytes.len() > MAX_MANIFEST_BYTES {
        return Err(ManifestError::ManifestTooLarge);
    }
    let manifest: UpdateManifest = serde_json::from_slice(bytes)?;
    manifest.validate()?;
    Ok(manifest)
}

pub(crate) fn validate_https_url(value: &str) -> Result<(), ManifestError> {
    let url = Url::parse(value).map_err(|_| ManifestError::InvalidUrl)?;
    if url.scheme() != "https"
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Err(ManifestError::InsecureUrl);
    }
    Ok(())
}

impl UpdateManifest {
    pub fn validate(&self) -> Result<(), ManifestError> {
        if self.schema != MANIFEST_SCHEMA {
            return Err(ManifestError::UnsupportedSchema(self.schema));
        }
        Version::parse(self.version.trim())
            .map_err(|_| ManifestError::InvalidVersion(self.version.clone()))?;
        if self.notes.len() > MAX_MANIFEST_BYTES {
            return Err(ManifestError::NotesTooLarge);
        }
        if let Some(release_url) = &self.release_url {
            validate_https_url(release_url)?;
        }
        for artifact in self.targets.values() {
            artifact.validate()?;
        }
        Ok(())
    }

    pub fn candidate(
        &self,
        current: &Version,
        target: UpdateTarget,
    ) -> Result<Option<UpdateCandidate>, ManifestError> {
        let version = Version::parse(self.version.trim())
            .map_err(|_| ManifestError::InvalidVersion(self.version.clone()))?;
        if version <= *current {
            return Ok(None);
        }
        let artifact = self
            .targets
            .get(target.key())
            .cloned()
            .ok_or(ManifestError::MissingTarget(target.key()))?;
        Ok(Some(UpdateCandidate {
            version,
            notes: self.notes.clone(),
            release_url: self.release_url.clone(),
            artifact,
            target,
        }))
    }
}

impl UpdateArtifact {
    pub fn validate(&self) -> Result<(), ManifestError> {
        validate_https_url(&self.url)?;
        if self.filename.is_empty()
            || Path::new(&self.filename)
                .file_name()
                .and_then(|name| name.to_str())
                != Some(self.filename.as_str())
            || self.filename.contains('\\')
        {
            return Err(ManifestError::UnsafeFilename);
        }
        let expected_extension = match self.format {
            ArtifactFormat::Zip => ".zip",
            ArtifactFormat::AppImage => ".AppImage",
            ArtifactFormat::TarGz => ".tar.gz",
        };
        if !self.filename.ends_with(expected_extension) {
            return Err(ManifestError::FormatMismatch);
        }
        if self.size == 0 || self.size > MAX_DOWNLOAD_BYTES {
            return Err(ManifestError::InvalidSize);
        }
        if self.sha256.len() != 64 || hex::decode(&self.sha256).is_err() {
            return Err(ManifestError::InvalidChecksum);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn artifact(format: ArtifactFormat, filename: &str) -> UpdateArtifact {
        UpdateArtifact {
            url: format!("https://github.com/xcrong/crossh/releases/download/v1/{filename}"),
            filename: filename.into(),
            format,
            sha256: "00".repeat(32),
            size: 1,
            signature: None,
        }
    }

    fn manifest(version: &str) -> UpdateManifest {
        UpdateManifest {
            schema: MANIFEST_SCHEMA,
            version: version.into(),
            notes: "notes".into(),
            release_url: None,
            published_at: None,
            targets: BTreeMap::from([(
                "macos-aarch64".into(),
                artifact(ArtifactFormat::Zip, "crossh-1.0.1-aarch64-macos.zip"),
            )]),
        }
    }

    #[test]
    fn target_mapping_is_stable() {
        assert_eq!(UpdateTarget::MacosAarch64.key(), "macos-aarch64");
        assert_eq!(UpdateTarget::WindowsX86_64.key(), "windows-x86_64");
    }

    #[test]
    fn newer_manifest_produces_candidate() {
        let manifest = manifest("1.0.1");
        let current = Version::parse("1.0.0").unwrap();
        let candidate = manifest
            .candidate(&current, UpdateTarget::MacosAarch64)
            .unwrap()
            .unwrap();
        assert_eq!(candidate.version, Version::parse("1.0.1").unwrap());
    }

    #[test]
    fn equal_or_older_manifest_is_ignored() {
        let current = Version::parse("1.0.1").unwrap();
        assert!(
            manifest("1.0.1")
                .candidate(&current, UpdateTarget::MacosAarch64)
                .unwrap()
                .is_none()
        );
        assert!(
            manifest("1.0.0")
                .candidate(&current, UpdateTarget::MacosAarch64)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn unsafe_artifact_paths_are_rejected() {
        let mut item = artifact(ArtifactFormat::Zip, "crossh.zip");
        item.filename = "../crossh.zip".into();
        assert!(matches!(
            item.validate(),
            Err(ManifestError::UnsafeFilename)
        ));
    }

    #[test]
    fn non_https_artifacts_are_rejected() {
        let mut item = artifact(ArtifactFormat::Zip, "crossh.zip");
        item.url = "http://example.com/crossh.zip".into();
        assert!(matches!(item.validate(), Err(ManifestError::InsecureUrl)));
    }

    #[test]
    fn insecure_release_links_are_rejected() {
        let mut item = manifest("1.0.1");
        item.release_url = Some("http://example.com/crossh".into());
        assert!(matches!(item.validate(), Err(ManifestError::InsecureUrl)));
    }
}
