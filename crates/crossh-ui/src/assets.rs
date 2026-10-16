use std::borrow::Cow;

use crossh_assets::AssetStore;
use crossh_assets::{embedded_font_paths, load as load_crossh_asset};
use gpui::{App, AssetSource, Result, SharedString};

/// Crossh assets take precedence; the fallback is what `crossh-assets`
/// embeds (OFL fonts vendored from Zed, Lucide icons).
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
        Ok(load_crossh_asset(path))
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        if let Some(store) = &self.external {
            return Ok(store
                .list(path)
                .into_iter()
                .map(SharedString::from)
                .collect());
        }
        // Crossh single truth: fonts are always bundled in `crossh-assets`,
        // so Lilex resolves in every build without an external AssetStore.
        // (`list` is only ever called with `"fonts"`; icons resolve via `load`.)
        Ok(embedded_font_paths()
            .into_iter()
            .filter(|font| font.starts_with(path))
            .map(SharedString::from)
            .collect())
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

#[cfg(test)]
mod tests {
    use super::*;

    /// 回归：`list("fonts")` 必须能列出内嵌 Lilex，否则终端回退系统字体、
    /// 字符间距拉宽。
    #[test]
    fn lists_bundled_lilex_without_external_store() {
        let source = UiAssetSource { external: None };
        let fonts = source.list("fonts").unwrap();
        assert!(
            fonts
                .iter()
                .any(|path| path.ends_with("fonts/lilex/Lilex-Regular.ttf")),
            "expected bundled Lilex, got: {fonts:?}"
        );
    }
}
