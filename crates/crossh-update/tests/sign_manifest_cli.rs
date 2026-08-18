//! End-to-end tests for the crossh-sign-manifest CLI
//! (spec 20260818-update-manifest-ed25519-signature, contract 9).

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

fn bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_crossh-sign-manifest"))
}

static NEXT_TEMP: AtomicU64 = AtomicU64::new(1);

fn temp_dir() -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "crossh-sign-manifest-test-{}-{}",
        std::process::id(),
        NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir_all(&dir).expect("create temp dir");
    dir
}

fn write_unsigned_manifest(path: &Path, version: &str) {
    let json = format!(
        r#"{{
  "schema": 1,
  "version": "{version}",
  "notes": "test release",
  "release_url": "https://github.com/xcrong/crossh/releases/tag/v{version}",
  "targets": {{
    "macos-aarch64": {{
      "url": "https://github.com/xcrong/crossh/releases/download/v{version}/crossh-{version}-aarch64-macos.zip",
      "filename": "crossh-{version}-aarch64-macos.zip",
      "format": "zip",
      "sha256": "0000000000000000000000000000000000000000000000000000000000000000",
      "size": 1
    }}
  }}
}}"#
    );
    fs::write(path, json).expect("write manifest fixture");
}

fn run(args: &[&str]) -> std::process::Output {
    let mut command = bin();
    command.args(args);
    command.output().expect("run CLI")
}

fn output_text(output: &std::process::Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

#[test]
fn spec_20260818_manifest_sig_generate_emits_usable_key_pair() {
    let output = run(&["generate"]);
    assert!(output.status.success(), "generate must succeed");
    let text = output_text(&output);
    let public = text
        .lines()
        .find_map(|line| line.strip_prefix("Public key:  "))
        .expect("generate must print the public key");
    let secret = text
        .lines()
        .find_map(|line| line.strip_prefix("Secret key:  "))
        .expect("generate must print the secret key");
    assert_ne!(public, secret, "public and secret key must differ");
    assert_eq!(secret.len(), 44, "secret key must be base64 of 32 bytes");
    assert_eq!(public.len(), 44, "public key must be base64 of 32 bytes");
}

#[test]
fn spec_20260818_manifest_sig_sign_then_verify_roundtrip() {
    let dir = temp_dir();
    let manifest_path = dir.join("stable.json");
    write_unsigned_manifest(&manifest_path, "1.0.1");

    let generated = run(&["generate"]);
    assert!(generated.status.success());
    let text = output_text(&generated);
    let public = text
        .lines()
        .find_map(|line| line.strip_prefix("Public key:  "))
        .unwrap()
        .to_owned();
    let secret = text
        .lines()
        .find_map(|line| line.strip_prefix("Secret key:  "))
        .unwrap()
        .to_owned();

    let original = fs::read(&manifest_path).expect("read before signing");
    let signed = run(&["sign", manifest_path.to_str().unwrap(), &secret]);
    assert!(
        signed.status.success(),
        "sign must succeed: {}",
        String::from_utf8_lossy(&signed.stderr)
    );
    let after_sign = fs::read(&manifest_path).expect("read after signing");
    assert_ne!(original, after_sign, "sign must modify the manifest file");
    let signed_text = String::from_utf8_lossy(&after_sign);
    assert!(
        signed_text.contains("\"signature\":"),
        "signed manifest must contain a signature field"
    );

    let verified = run(&["verify", manifest_path.to_str().unwrap(), &public]);
    assert!(
        verified.status.success(),
        "verify must accept the signed manifest: {}",
        String::from_utf8_lossy(&verified.stderr)
    );
    assert!(output_text(&verified).contains("OK:"));

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn spec_20260818_manifest_sig_sign_reads_key_from_environment() {
    let dir = temp_dir();
    let manifest_path = dir.join("stable.json");
    write_unsigned_manifest(&manifest_path, "1.0.1");

    let generated = run(&["generate"]);
    let text = output_text(&generated);
    let secret = text
        .lines()
        .find_map(|line| line.strip_prefix("Secret key:  "))
        .unwrap()
        .to_owned();

    let mut command = bin();
    command
        .args(["sign", manifest_path.to_str().unwrap()])
        .env("CROSSH_UPDATE_SIGNING_KEY", &secret);
    let output = command.output().expect("run CLI");
    assert!(
        output.status.success(),
        "sign must read the key from CROSSH_UPDATE_SIGNING_KEY: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn spec_20260818_manifest_sig_verify_rejects_tampered_manifest() {
    let dir = temp_dir();
    let manifest_path = dir.join("stable.json");
    write_unsigned_manifest(&manifest_path, "1.0.1");

    let generated = run(&["generate"]);
    let text = output_text(&generated);
    let public = text
        .lines()
        .find_map(|line| line.strip_prefix("Public key:  "))
        .unwrap()
        .to_owned();
    let secret = text
        .lines()
        .find_map(|line| line.strip_prefix("Secret key:  "))
        .unwrap()
        .to_owned();

    let signed = run(&["sign", manifest_path.to_str().unwrap(), &secret]);
    assert!(signed.status.success());

    // 篡改 version 字段（保持结构合法，只破坏签名）。
    let tampered = dir.join("tampered.json");
    let mut json: serde_json::Value =
        serde_json::from_slice(&fs::read(&manifest_path).unwrap()).unwrap();
    json["version"] = "9.9.9".into();
    fs::write(&tampered, serde_json::to_vec(&json).unwrap()).unwrap();

    let verified = run(&["verify", tampered.to_str().unwrap(), &public]);
    assert!(
        !verified.status.success(),
        "verify must reject a tampered manifest"
    );
    let stderr = String::from_utf8_lossy(&verified.stderr);
    assert!(
        stderr.contains("signature"),
        "verify must report a signature failure, got: {stderr}"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn spec_20260818_manifest_sig_sign_without_key_fails_and_leaves_file_untouched() {
    let dir = temp_dir();
    let manifest_path = dir.join("stable.json");
    write_unsigned_manifest(&manifest_path, "1.0.1");
    let original = fs::read(&manifest_path).expect("read before signing");

    let output = run(&["sign", manifest_path.to_str().unwrap()]);
    assert!(!output.status.success(), "sign without a key must fail");
    assert_eq!(
        fs::read(&manifest_path).unwrap(),
        original,
        "failed sign must not modify the manifest"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn spec_20260818_manifest_sig_sign_with_invalid_key_fails_and_leaves_file_untouched() {
    let dir = temp_dir();
    let manifest_path = dir.join("stable.json");
    write_unsigned_manifest(&manifest_path, "1.0.1");
    let original = fs::read(&manifest_path).expect("read before signing");

    let output = run(&[
        "sign",
        manifest_path.to_str().unwrap(),
        "not-a-valid-base64-secret-key",
    ]);
    assert!(
        !output.status.success(),
        "sign with an invalid key must fail"
    );
    assert_eq!(
        fs::read(&manifest_path).unwrap(),
        original,
        "failed sign must not modify the manifest"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn spec_20260818_manifest_sig_verify_rejects_missing_signature() {
    let dir = temp_dir();
    let manifest_path = dir.join("stable.json");
    write_unsigned_manifest(&manifest_path, "1.0.1");

    let generated = run(&["generate"]);
    let text = output_text(&generated);
    let public = text
        .lines()
        .find_map(|line| line.strip_prefix("Public key:  "))
        .unwrap()
        .to_owned();

    let verified = run(&["verify", manifest_path.to_str().unwrap(), &public]);
    assert!(
        !verified.status.success(),
        "verify must reject an unsigned manifest"
    );

    let _ = fs::remove_dir_all(&dir);
}
