use std::borrow::Cow;

use assets::Assets as ZedAssets;
use crossh_assets::load as load_crossh_asset;
use gpui::{AssetSource, Result, SharedString};

/// Crossh assets take precedence, with Zed's embedded assets available as the
/// fallback for fonts and other resources consumed by reused Zed components.
pub struct UiAssetSource;

impl AssetSource for UiAssetSource {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        if let Some(asset) = load_crossh_asset(path) {
            return Ok(Some(asset));
        }
        ZedAssets.load(path)
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        ZedAssets.list(path)
    }
}
