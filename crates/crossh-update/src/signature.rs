//! Ed25519 manifest signature.
//!
//! The signature covers the canonical serialization of the manifest without
//! the `signature` field itself, so semantically identical bytes (whitespace,
//! key order) verify against the same signature. The public key is pinned at
//! compile time (`CROSSH_UPDATE_PUBLIC_KEY` overrides the default constant),
//! making the trust anchor independent of any transport channel.

use base64::Engine;
use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use std::error::Error;

use crate::model::{ManifestError, UpdateManifest};

/// The canonical public key pinned into the client.
/// Generated with `crossh-sign-manifest generate` on 2026-08-18.
pub const DEFAULT_PUBLIC_KEY: &str = "2ruoNty5NOSLRAHeHqchPsXYnCjZ9vfUfyUBZT/kHQs=";

pub(crate) fn canonical_bytes(manifest: &UpdateManifest) -> Vec<u8> {
    let mut copy = manifest.clone();
    copy.signature = None;
    serde_json::to_vec(&copy).expect("manifest serialization cannot fail")
}

pub fn verify_manifest_signature_with_key(
    manifest: &UpdateManifest,
    key: &VerifyingKey,
) -> Result<(), ManifestError> {
    let Some(encoded) = &manifest.signature else {
        return Err(ManifestError::MissingSignature);
    };
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .map_err(|error| {
            ManifestError::InvalidSignature(format!("signature is not valid base64: {error}"))
        })?;
    let bytes: [u8; 64] = bytes.try_into().map_err(|_| {
        ManifestError::InvalidSignature(format!(
            "signature must be 64 bytes, got {}",
            encoded.len()
        ))
    })?;
    let signature = Signature::from_bytes(&bytes);
    key.verify_strict(&canonical_bytes(manifest), &signature)
        .map_err(|error| {
            ManifestError::InvalidSignature(format!("signature verification failed: {error}"))
        })
}

pub(crate) fn pinned_verifying_key() -> Result<VerifyingKey, ManifestError> {
    let encoded = option_env!("CROSSH_UPDATE_PUBLIC_KEY").unwrap_or(DEFAULT_PUBLIC_KEY);
    parse_verifying_key(encoded).map_err(|error| {
        ManifestError::InvalidSignature(format!("pinned public key is unusable: {error}"))
    })
}

pub fn parse_verifying_key(encoded: &str) -> Result<VerifyingKey, String> {
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .map_err(|error| format!("public key is not valid base64: {error}"))?;
    let bytes: [u8; 32] = bytes
        .try_into()
        .map_err(|_| format!("public key must be 32 bytes, got {}", encoded.len()))?;
    VerifyingKey::from_bytes(&bytes)
        .map_err(|error| format!("public key is not a valid Ed25519 key: {error}"))
}

pub fn parse_signing_key(encoded: &str) -> Result<SigningKey, String> {
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .map_err(|error| format!("signing key is not valid base64: {error}"))?;
    let bytes: [u8; 32] = bytes
        .try_into()
        .map_err(|_| format!("signing key must be 32 bytes, got {}", encoded.len()))?;
    Ok(SigningKey::from_bytes(&bytes))
}

pub fn sign_manifest(
    manifest: &UpdateManifest,
    signing_key: &SigningKey,
) -> Result<UpdateManifest, Box<dyn Error>> {
    let mut signed = manifest.clone();
    let signature = signing_key.sign(&canonical_bytes(manifest));
    signed.signature = Some(base64::engine::general_purpose::STANDARD.encode(signature.to_bytes()));
    Ok(signed)
}
