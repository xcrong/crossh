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
        // ponytail: 与 `load` 两级回退对齐——外部缺字体时仍列出内嵌字体，
        // 否则 `load_fonts` 看到空列表直接零字体启动。
        let mut paths: Vec<String> = self
            .external
            .as_ref()
            .map(|store| store.list(path))
            .unwrap_or_default();
        paths.extend(
            embedded_font_paths()
                .into_iter()
                .filter(|font| font.starts_with(path)),
        );
        paths.sort();
        paths.dedup();
        Ok(paths.into_iter().map(SharedString::from).collect())
    }
}

pub fn load_fonts(cx: &App) -> Result<()> {
    let font_paths = cx.asset_source().list("fonts")?;
    let mut embedded_fonts = Vec::new();
    for font_path in font_paths {
        if font_path.ends_with(".ttf") {
            match cx.asset_source().load(&font_path)? {
                Some(font_bytes) => embedded_fonts.push(font_bytes),
                // ponytail: 单个字体缺失只记日志回退系统字体，不 panic；
                // 上层调用点同样只记日志（见 main.rs / crossh-git.rs / crossh-note.rs）。
                None => log::warn!("font listed but not loadable, skipped: {font_path}"),
            }
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
