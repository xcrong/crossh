use std::borrow::Cow;

use assets::Assets as ZedAssets;
use crossh_assets::AssetStore;
use crossh_assets::load as load_crossh_asset;
use gpui::{App, AssetSource, Result, SharedString};
/// Crossh assets take precedence, with Zed's embedded assets available as the
/// fallback for fonts and other resources consumed by reused Zed components.
pub struct UiAssetSource {
    external: Option<AssetStore>,
}

impl Default for UiAssetSource {
    fn default() -> Self {
        Self {
            external: AssetStore::discover(),
        }
    }
}

impl AssetSource for UiAssetSource {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        if let Some(asset) = self.external.as_ref().and_then(|store| store.load(path)) {
            return Ok(Some(asset));
        }
        if let Some(asset) = load_crossh_asset(path) {
            return Ok(Some(asset));
        }
        ZedAssets.load(path)
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        if let Some(store) = &self.external {
            return Ok(store
                .list(path)
                .into_iter()
                .map(SharedString::from)
                .collect());
        }
        // Crossh single truth: fonts are always bundled (copied from Zed at compile time),
        // not only in debug. Ensures Lilex is resolvable in release without external AssetStore.
        ZedAssets.list(path)
    }
}

pub fn load_fonts(cx: &App) -> Result<()> {
    let font_paths = cx.asset_source().list("fonts")?;
    let mut embedded_fonts = Vec::new();
    for font_path in font_paths {
        if font_path.ends_with(".ttf") {
            let font_bytes = cx
                .asset_source()
                .load(&font_path)?
                .expect("asset source should return listed fonts");
            embedded_fonts.push(font_bytes);
        }
    }
    cx.text_system().add_fonts(embedded_fonts)
}
