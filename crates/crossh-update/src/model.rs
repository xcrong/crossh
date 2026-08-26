//! Machine-readable release metadata shared by the updater and the UI.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use semver::Version;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use url::Url;

use crate::signature::verify_manifest_signature_with_key;

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
    // Ed25519 签名（base64 编码 64 字节），覆盖去掉本字段后的 canonical 序列化。
    // 新客户端强制要求签名（缺省即拒绝更新）；旧客户端忽略该字段照常更新。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signature: Option<String>,
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
    #[error("release manifest is missing its signature")]
    MissingSignature,
    #[error("release manifest signature is invalid: {0}")]
    InvalidSignature(String),
}

pub fn parse_manifest(bytes: &[u8]) -> Result<UpdateManifest, ManifestError> {
    let key = crate::signature::pinned_verifying_key()?;
    parse_manifest_with_key(bytes, &key)
}

/// `parse_manifest` 的内核：用显式公钥验签（测试与工具复用同一验证逻辑）。
pub(crate) fn parse_manifest_with_key(
    bytes: &[u8],
    key: &ed25519_dalek::VerifyingKey,
) -> Result<UpdateManifest, ManifestError> {
    if bytes.len() > MAX_MANIFEST_BYTES {
        return Err(ManifestError::ManifestTooLarge);
    }
    let manifest: UpdateManifest = serde_json::from_slice(bytes)?;
    manifest.validate()?;
    // 签名验证是协议的安全门槛：缺失或无效签名一律拒绝，即使结构校验通过。
    verify_manifest_signature_with_key(&manifest, key)?;
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
pub(crate) fn record_update_result(result: &UpdateResult) {
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
    use crate::signature::{parse_signing_key, parse_verifying_key, sign_manifest};
    use base64::Engine;

    fn test_signing_key() -> ed25519_dalek::SigningKey {
        ed25519_dalek::SigningKey::from_bytes(&[0x42; 32])
    }

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
        let unsigned = UpdateManifest {
            schema: MANIFEST_SCHEMA,
            version: version.into(),
            notes: "notes".into(),
            release_url: None,
            targets: BTreeMap::from([(
                "macos-aarch64".into(),
                artifact(ArtifactFormat::Zip, "crossh-1.0.1-aarch64-macos.zip"),
            )]),
            signature: None,
        };
        sign_manifest(&unsigned, &test_signing_key()).expect("test signing must succeed")
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

    // ── spec 20260818-update-manifest-ed25519-signature ──────────────────────

    fn signed_manifest_bytes(version: &str) -> Vec<u8> {
        serde_json::to_vec(&manifest(version)).expect("signed manifest must serialize")
    }

    fn tamper(original: &[u8], mutate: impl FnOnce(&mut serde_json::Value)) -> Vec<u8> {
        let mut value: serde_json::Value =
            serde_json::from_slice(original).expect("tamper input must be JSON");
        mutate(&mut value);
        serde_json::to_vec(&value).expect("tampered manifest must serialize")
    }

    #[test]
    fn spec_20260818_manifest_sig_valid_signature_parses() {
        // 契约 1：合法签名 + 匹配公钥 → 完整解析。parse_manifest 与
        // parse_manifest_with_key 共享同一验证逻辑（with_key 为内核，
        // 全局公钥路径由 spec_20260818_manifest_sig_global_default_key_is_enforced
        // 单独证明）。
        let key = test_signing_key().verifying_key();
        let parsed = parse_manifest_with_key(&signed_manifest_bytes("1.0.1"), &key)
            .expect("a manifest signed with the matching key must parse");
        assert_eq!(parsed.version, "1.0.1");
        assert!(
            parsed.signature.is_some(),
            "signature must survive a roundtrip"
        );
    }

    #[test]
    fn spec_20260818_manifest_sig_global_default_key_is_enforced() {
        // 全局路径回归：parse_manifest 必须使用 DEFAULT_PUBLIC_KEY 验签，
        // 测试密钥签名的 manifest 在默认公钥下必须被拒绝（正式公钥 ≠ 测试公钥）。
        assert!(matches!(
            parse_manifest(&signed_manifest_bytes("1.0.1")),
            Err(ManifestError::InvalidSignature(_))
        ));
    }

    #[test]
    fn spec_20260818_manifest_sig_tampered_fields_rejected() {
        let original = signed_manifest_bytes("1.0.1");
        type TamperCase = (&'static str, fn(&mut serde_json::Value));
        let cases: Vec<TamperCase> = vec![
            ("version", |value| value["version"] = "9.9.9".into()),
            ("notes", |value| value["notes"] = "evil notes".into()),
            ("release_url", |value| {
                value["release_url"] = "https://evil.example".into()
            }),
            ("target.url", |value| {
                value["targets"]["macos-aarch64"]["url"] = "https://evil.example/crossh.zip".into()
            }),
            ("target.filename", |value| {
                value["targets"]["macos-aarch64"]["filename"] =
                    "crossh-9.9.9-aarch64-macos.zip".into()
            }),
            ("target.format+filename", |value| {
                value["targets"]["macos-aarch64"]["filename"] = "crossh-9.9.9.tar.gz".into();
                value["targets"]["macos-aarch64"]["format"] = "tar.gz".into();
            }),
            ("target.sha256", |value| {
                value["targets"]["macos-aarch64"]["sha256"] = "11".repeat(32).into()
            }),
            ("target.size", |value| {
                value["targets"]["macos-aarch64"]["size"] = 2u64.into()
            }),
        ];
        for (name, mutate) in cases {
            let bytes = tamper(&original, mutate);
            assert!(
                matches!(
                    parse_manifest(&bytes),
                    Err(ManifestError::InvalidSignature(_))
                ),
                "tampered field {name} must fail signature verification"
            );
        }
    }

    #[test]
    fn spec_20260818_manifest_sig_missing_signature_rejected() {
        let mut unsigned = manifest("1.0.1");
        unsigned.signature = None;
        let bytes = serde_json::to_vec(&unsigned).expect("serialize");
        assert!(matches!(
            parse_manifest(&bytes),
            Err(ManifestError::MissingSignature)
        ));
    }

    #[test]
    fn spec_20260818_manifest_sig_malformed_signature_rejected() {
        let mut invalid_base64 = manifest("1.0.1");
        invalid_base64.signature = Some("!!!not-base64!!!".into());
        assert!(matches!(
            parse_manifest(&serde_json::to_vec(&invalid_base64).unwrap()),
            Err(ManifestError::InvalidSignature(_))
        ));

        let mut too_short = manifest("1.0.1");
        too_short.signature = Some(base64::engine::general_purpose::STANDARD.encode([1u8; 63]));
        assert!(matches!(
            parse_manifest(&serde_json::to_vec(&too_short).unwrap()),
            Err(ManifestError::InvalidSignature(_))
        ));

        let mut too_long = manifest("1.0.1");
        too_long.signature = Some(base64::engine::general_purpose::STANDARD.encode([1u8; 65]));
        assert!(matches!(
            parse_manifest(&serde_json::to_vec(&too_long).unwrap()),
            Err(ManifestError::InvalidSignature(_))
        ));
    }

    #[test]
    fn spec_20260818_manifest_sig_semantically_equivalent_bytes_verify() {
        // 契约 5：签名作用于语义（canonical 序列化）而非原始字节。
        // 显式构造两种「语义等价、字节不同」的表示，不依赖 serde_json 的
        // 字段排序实现（preserve_order 会随构建图变化）。
        let key = test_signing_key().verifying_key();
        let original = String::from_utf8(signed_manifest_bytes("1.0.1")).unwrap();

        // 变体 1：空白/缩进变化。
        let value: serde_json::Value = serde_json::from_str(&original).expect("parse");
        let pretty = serde_json::to_string_pretty(&value).expect("serialize");
        assert_ne!(pretty, original, "pretty printing must change the bytes");
        parse_manifest_with_key(pretty.as_bytes(), &key)
            .expect("whitespace variations must verify against the same signature");

        // 变体 2：顶层字段顺序打乱（JSON 对象字段无序，语义相同）。
        let signature = value["signature"]
            .as_str()
            .expect("signed fixture has a signature");
        let shuffled = format!(
            r#"{{"signature":"{signature}","version":"1.0.1","targets":{{"macos-aarch64":{{"size":1,"sha256":"0000000000000000000000000000000000000000000000000000000000000000","format":"zip","filename":"crossh-1.0.1-aarch64-macos.zip","url":"https://github.com/xcrong/crossh/releases/download/v1/crossh-1.0.1-aarch64-macos.zip"}}}},"schema":1,"notes":"notes","release_url":null}}"#
        );
        assert_ne!(
            shuffled, original,
            "shuffled field order must change the bytes"
        );
        parse_manifest_with_key(shuffled.as_bytes(), &key)
            .expect("reordered fields must verify against the same signature");
    }

    #[test]
    fn spec_20260818_manifest_sig_wrong_key_rejected() {
        let signed = manifest("1.0.1");
        let other_key = ed25519_dalek::SigningKey::from_bytes(&[0x24; 32]);
        assert!(matches!(
            crate::signature::verify_manifest_signature_with_key(
                &signed,
                &other_key.verifying_key()
            ),
            Err(ManifestError::InvalidSignature(_))
        ));
        let unrelated = ed25519_dalek::SigningKey::from_bytes(&[7u8; 32]);
        assert!(matches!(
            crate::signature::verify_manifest_signature_with_key(
                &signed,
                &unrelated.verifying_key()
            ),
            Err(ManifestError::InvalidSignature(_))
        ));
    }

    #[test]
    fn spec_20260818_manifest_sig_signed_old_version_still_rejected_by_candidate() {
        // 重放攻击：旧版本合法签名的 manifest 仍被版本比较拒绝。
        let signed = manifest("1.0.0");
        let current = Version::parse("1.0.1").unwrap();
        assert!(
            signed
                .candidate(&current, UpdateTarget::MacosAarch64)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn spec_20260818_manifest_sig_legacy_client_ignores_signature_field() {
        // 模拟不认识 signature 字段的旧解析逻辑（serde 忽略未知字段）。
        #[derive(Deserialize)]
        struct LegacyManifest {
            schema: u32,
            version: String,
            #[serde(default)]
            notes: String,
            #[serde(default)]
            release_url: Option<String>,
            targets: BTreeMap<String, LegacyArtifact>,
        }
        #[derive(Deserialize)]
        struct LegacyArtifact {
            url: String,
            filename: String,
            format: ArtifactFormat,
            sha256: String,
            size: u64,
        }
        let parsed: LegacyManifest =
            serde_json::from_slice(&signed_manifest_bytes("1.0.1")).expect("legacy parse");
        assert_eq!(parsed.schema, MANIFEST_SCHEMA);
        assert_eq!(parsed.version, "1.0.1");
        assert_eq!(parsed.notes, "notes");
        assert!(parsed.release_url.is_none());
        let artifact = &parsed.targets["macos-aarch64"];
        assert_eq!(
            artifact.url,
            "https://github.com/xcrong/crossh/releases/download/v1/crossh-1.0.1-aarch64-macos.zip"
        );
        assert_eq!(artifact.filename, "crossh-1.0.1-aarch64-macos.zip");
        assert_eq!(artifact.format, ArtifactFormat::Zip);
        assert_eq!(artifact.sha256, "00".repeat(32));
        assert_eq!(artifact.size, 1);
    }

    #[test]
    fn spec_20260818_manifest_sig_sign_verify_roundtrip() {
        let key = test_signing_key();
        let mut unsigned = manifest("1.0.1");
        unsigned.signature = None;
        let signed = sign_manifest(&unsigned, &key).expect("signing cannot fail");
        assert!(signed.signature.is_some(), "sign must attach a signature");
        crate::signature::verify_manifest_signature_with_key(&signed, &key.verifying_key())
            .expect("roundtrip must verify");
    }

    #[test]
    fn spec_20260818_manifest_sig_invalid_signing_key_rejected() {
        assert!(parse_signing_key("not-base64").is_err());
        assert!(
            parse_signing_key(&base64::engine::general_purpose::STANDARD.encode([1u8; 31]))
                .is_err()
        );
    }

    #[test]
    fn spec_20260818_manifest_sig_default_public_key_is_wellformed() {
        let key = parse_verifying_key(crate::signature::DEFAULT_PUBLIC_KEY)
            .expect("pinned public key must be valid base64 of 32 bytes");
        assert_eq!(key.to_bytes().len(), 32);
    }
}
