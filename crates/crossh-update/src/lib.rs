//! Release metadata, download verification, and standalone installation.
//!
//! The UI owns update state and presentation, while this crate owns all
//! network, archive, checksum, signature, and process-replacement behavior.

mod client;
mod installer;
mod model;
pub mod signature;

pub const DEFAULT_MANIFEST_URL: &str =
    "https://github.com/xcrong/crossh/releases/latest/download/stable.json";

pub use client::{UpdateError, download_artifact, fetch_manifest};
pub use installer::{InstallerError, run_from_args, spawn_updater};
pub use model::{
    ArtifactFormat, ManifestError, UpdateArtifact, UpdateCandidate, UpdateManifest, UpdateResult,
    UpdateTarget, parse_manifest, record_update_result, take_update_result,
};
pub use signature::{
    DEFAULT_PUBLIC_KEY, parse_signing_key, parse_verifying_key, sign_manifest,
    verify_manifest_signature, verify_manifest_signature_with_key,
};
