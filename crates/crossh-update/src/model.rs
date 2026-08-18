//! Machine-readable release metadata shared by the updater and the UI.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

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
    pub targets: BTreeMap<String, UpdateArtifact>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct UpdateArtifact {
    pub url: String,
    pub filename: String,
    pub format: ArtifactFormat,
    pub sha256: String,
    pub size: u64,
    // Ed25519 更新协议签名预留（见 docs/remote-update-plan.md 安全边界）；
    // SHA-256 不能抵抗发布源被恶意改写，后续在此字段承载签名并把公钥固定在客户端。
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

/// 一次安装尝试的结果，由 updater 进程落盘、主应用下次启动时读取展示。
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct UpdateResult {
    pub success: bool,
    pub error: Option<String>,
}

pub(crate) fn update_result_path_in(cache_root: &Path) -> PathBuf {
    cache_root.join("crossh").join("update-result.json")
}

/// 记录这次安装的结果（updater 进程侧）。
pub fn record_update_result(result: &UpdateResult) {
    let cache = dirs::cache_dir().unwrap_or_else(std::env::temp_dir);
    record_update_result_in(&cache, result);
}

pub(crate) fn record_update_result_in(cache_root: &Path, result: &UpdateResult) {
    let path = update_result_path_in(cache_root);
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    if let Ok(json) = serde_json::to_string(result) {
        let _ = fs::write(&path, json);
    }
}

/// 读取并移除上次安装的结果（主应用启动时调用；成功后无需再展示）。
pub fn take_update_result() -> Option<UpdateResult> {
    let cache = dirs::cache_dir().unwrap_or_else(std::env::temp_dir);
    take_update_result_in(&cache)
}

pub(crate) fn take_update_result_in(cache_root: &Path) -> Option<UpdateResult> {
    let path = update_result_path_in(cache_root);
    let json = fs::read_to_string(&path).ok()?;
    // 先解析再删除：文件损坏时保留现场而不是把失败细节一并清掉。
    let result = serde_json::from_str(&json).ok()?;
    let _ = fs::remove_file(&path);
    Some(result)
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

    #[test]
    fn update_result_roundtrips_and_is_consumed_once() {
        use std::fs;
        use std::sync::atomic::{AtomicU64, Ordering};

        static NEXT_TEMP: AtomicU64 = AtomicU64::new(1);
        let cache_root = std::env::temp_dir().join(format!(
            "crossh-update-result-test-{}-{}",
            std::process::id(),
            NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&cache_root);
        fs::create_dir_all(&cache_root).unwrap();

        let failed = UpdateResult {
            success: false,
            error: Some("checksum mismatch".into()),
        };
        record_update_result_in(&cache_root, &failed);
        assert_eq!(
            take_update_result_in(&cache_root),
            Some(failed.clone()),
            "failed install must be readable by the next launch"
        );
        assert_eq!(
            take_update_result_in(&cache_root),
            None,
            "a consumed result must not resurface"
        );
        assert!(
            !update_result_path_in(&cache_root).exists(),
            "the result file must be removed after reading"
        );

        let ok = UpdateResult {
            success: true,
            error: None,
        };
        record_update_result_in(&cache_root, &ok);
        assert_eq!(take_update_result_in(&cache_root), Some(ok));
        let _ = fs::remove_dir_all(&cache_root);
    }
}
