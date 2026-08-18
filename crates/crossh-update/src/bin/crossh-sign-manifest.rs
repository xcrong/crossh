//! Sign and verify the release update manifest (stable.json).
//!
//! Usage:
//!   crossh-sign-manifest generate
//!   crossh-sign-manifest sign <manifest.json> [signing-key-b64]
//!   crossh-sign-manifest verify <manifest.json> [public-key-b64]
//!
//! The signing key is read from the argument or the CROSSH_UPDATE_SIGNING_KEY
//! environment variable. The verifier defaults to the public key pinned into
//! this crate (DEFAULT_PUBLIC_KEY), so release CI self-verification exercises
//! exactly the same trust anchor as the shipped client.

use std::env;
use std::fs;
use std::path::Path;
use std::process::ExitCode;

use base64::Engine;
use crossh_update::{
    ManifestError, UpdateManifest, parse_manifest, parse_signing_key, parse_verifying_key,
    sign_manifest,
};

const USAGE: &str = "\
crossh-sign-manifest — sign and verify the Crossh update manifest

Usage:
  crossh-sign-manifest generate
  crossh-sign-manifest sign <manifest.json> [signing-key-b64]
  crossh-sign-manifest verify <manifest.json> [public-key-b64]

The signing key may also be provided via the CROSSH_UPDATE_SIGNING_KEY
environment variable. verify uses the pinned DEFAULT_PUBLIC_KEY unless a
public key argument is given.";

fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("generate") => generate(),
        Some("sign") => sign(&args[1..]),
        Some("verify") => verify(&args[1..]),
        _ => {
            eprintln!("{USAGE}");
            return ExitCode::FAILURE;
        }
    };
    match result {
        Ok(message) => {
            println!("{message}");
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::FAILURE
        }
    }
}

fn generate() -> Result<String, String> {
    let mut seed = [0u8; 32];
    getrandom::fill(&mut seed).map_err(|error| format!("cannot gather randomness: {error}"))?;
    let signing_key = ed25519_dalek::SigningKey::from_bytes(&seed);
    let public_key = signing_key.verifying_key();
    let secret = base64::engine::general_purpose::STANDARD.encode(signing_key.to_bytes());
    let public = base64::engine::general_purpose::STANDARD.encode(public_key.to_bytes());
    Ok(format!(
        "Public key:  {public}\nSecret key:  {secret}\n\n\
Store the secret key in an encrypted offline backup and in the GitHub Actions \
secret CROSSH_UPDATE_SIGNING_KEY. It is printed only once.\n\
Add the public key as DEFAULT_PUBLIC_KEY in crates/crossh-update/src/signature.rs."
    ))
}

fn sign(args: &[String]) -> Result<String, String> {
    let Some(path) = args.first() else {
        return Err(format!("sign requires a manifest path\n\n{USAGE}"));
    };
    let encoded = args
        .get(1)
        .cloned()
        .or_else(|| env::var("CROSSH_UPDATE_SIGNING_KEY").ok())
        .ok_or("no signing key: pass it as an argument or set CROSSH_UPDATE_SIGNING_KEY")?;
    let signing_key = parse_signing_key(&encoded)?;

    let bytes =
        fs::read(path).map_err(|error| format!("cannot read {}: {error}", path_display(path)))?;
    let manifest: UpdateManifest = serde_json::from_slice(&bytes)
        .map_err(|error| format!("{} is not a valid manifest: {error}", path_display(path)))?;
    manifest
        .validate()
        .map_err(|error| format!("{} failed validation: {error}", path_display(path)))?;
    let signed = sign_manifest(&manifest, &signing_key)
        .map_err(|error| format!("signing failed: {error}"))?;
    let output = serde_json::to_vec_pretty(&signed)
        .map_err(|error| format!("serializing the signed manifest failed: {error}"))?;
    fs::write(path, output)
        .map_err(|error| format!("cannot write {}: {error}", path_display(path)))?;
    Ok(format!("signed {}", path_display(path)))
}

fn verify(args: &[String]) -> Result<String, String> {
    let Some(path) = args.first() else {
        return Err(format!("verify requires a manifest path\n\n{USAGE}"));
    };
    let bytes =
        fs::read(path).map_err(|error| format!("cannot read {}: {error}", path_display(path)))?;
    let manifest = if let Some(public_key) = args.get(1) {
        let key = parse_verifying_key(public_key)?;
        let manifest: UpdateManifest = serde_json::from_slice(&bytes)
            .map_err(|error| format!("{} is not a valid manifest: {error}", path_display(path)))?;
        manifest
            .validate()
            .map_err(|error| format!("{} failed validation: {error}", path_display(path)))?;
        crossh_update::verify_manifest_signature_with_key(&manifest, &key)
            .map_err(|error| describe_manifest_error(&error))?;
        manifest
    } else {
        // 走与客户端完全相同的解析路径（结构校验 + 固定公钥验签）。
        parse_manifest(&bytes).map_err(|error| describe_manifest_error(&error))?
    };
    Ok(format!(
        "OK: {} is signed for version {}",
        path_display(path),
        manifest.version
    ))
}

fn describe_manifest_error(error: &ManifestError) -> String {
    error.to_string()
}

fn path_display(path: &str) -> String {
    Path::new(path).display().to_string()
}
