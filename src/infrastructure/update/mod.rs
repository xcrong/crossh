//! Update infrastructure: release transport and process replacement.

pub(crate) mod client;
#[allow(dead_code)]
pub(crate) mod installer;
pub(crate) mod model;

pub(crate) const DEFAULT_MANIFEST_URL: &str =
    "https://github.com/xcrong/crossh/releases/latest/download/stable.json";
