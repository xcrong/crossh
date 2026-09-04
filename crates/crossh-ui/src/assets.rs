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
        // dev 下 ZedAssets 读的是 crossh 仓库根（见下），缺文件时直接 Err；
        // 先降级为 None 走 checkout 兜底，release 保持原样透传。
        #[cfg(debug_assertions)]
        let loaded = ZedAssets.load(path).unwrap_or(None);
        #[cfg(not(debug_assertions))]
        let loaded = ZedAssets.load(path)?;
        if loaded.is_some() {
            return Ok(loaded);
        }
        #[cfg(debug_assertions)]
        if let Some(bytes) = zed_dev_load(path) {
            return Ok(Some(bytes));
        }
        Ok(None)
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
        let listed = ZedAssets.list(path)?;
        #[cfg(debug_assertions)]
        if listed.is_empty() {
            return Ok(zed_dev_list(path));
        }
        Ok(listed)
    }
}

/// dev 专属：Zed `fs_embed!` 在 dev 下按可执行文件向上找 `.git` 定位资源根，
/// 编进 crossh 后找到的是 crossh 自己的仓库根，其 `assets/` 下没有 Zed 的
/// `fonts/`，于是 `list("fonts")` 为空、`load_fonts` 静默加载 0 个字体，
/// 终端回退到系统字体、格宽错乱（release 走真内嵌，不受影响）。
/// 兜底按 Cargo.toml 锁定的 rev 去 cargo git checkout 里找真正的 Zed `assets/`。
#[cfg(debug_assertions)]
static ZED_DEV_ASSETS_DIR: std::sync::LazyLock<Option<std::path::PathBuf>> =
    std::sync::LazyLock::new(locate_zed_dev_assets_dir);

#[cfg(debug_assertions)]
fn zed_dev_assets_dir() -> Option<std::path::PathBuf> {
    ZED_DEV_ASSETS_DIR.clone()
}

#[cfg(debug_assertions)]
fn locate_zed_dev_assets_dir() -> Option<std::path::PathBuf> {
    use std::path::PathBuf;

    if let Some(dir) = std::env::var_os("CROSSH_ZED_CHECKOUT").map(PathBuf::from) {
        let assets = dir.join("assets");
        if assets.join("fonts").is_dir() {
            return Some(assets);
        }
    }

    let rev = zed_assets_rev()?;
    let cargo_home = std::env::var_os("CARGO_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".cargo")))?;
    // 布局为 `git/checkouts/zed-<hash>/<rev>/assets`：先精确匹配完整 rev，
    // 再退化为 7 位短 rev 前缀匹配。
    let checkouts = cargo_home.join("git/checkouts");
    let Ok(hash_dirs) = std::fs::read_dir(&checkouts) else {
        return None;
    };
    let mut hash_dirs: Vec<_> = hash_dirs
        .flatten()
        .filter(|entry| entry.file_name().to_string_lossy().starts_with("zed-"))
        .collect();
    hash_dirs.sort_by_key(|entry| entry.file_name());
    let short = &rev[..rev.len().min(7)];
    let mut prefix_fallback = None;
    for hash_dir in &hash_dirs {
        let exact = hash_dir.path().join(&rev);
        if exact.join("assets/fonts").is_dir() {
            return Some(exact.join("assets"));
        }
        if prefix_fallback.is_none()
            && let Ok(rev_dirs) = std::fs::read_dir(hash_dir.path())
        {
            for rev_dir in rev_dirs.flatten() {
                let assets = rev_dir.path().join("assets");
                if rev_dir.file_name().to_string_lossy().starts_with(short)
                    && assets.join("fonts").is_dir()
                {
                    prefix_fallback = Some(assets);
                    break;
                }
            }
        }
    }
    prefix_fallback
}

/// 从 crossh 仓库根 Cargo.toml 解析 `assets = { … rev = "…" }`。
#[cfg(debug_assertions)]
fn zed_assets_rev() -> Option<String> {
    let exe = std::env::current_exe().ok();
    let cwd = std::env::current_dir().ok();
    let root = exe
        .iter()
        .chain(cwd.as_ref())
        .flat_map(|start| start.ancestors())
        .find(|dir| dir.join(".git").exists())?;
    let manifest = std::fs::read_to_string(root.join("Cargo.toml")).ok()?;
    let line = manifest
        .lines()
        .find(|line| line.trim_start().starts_with("assets = {"))?;
    let rev = line.split("rev = \"").nth(1)?.split('"').next()?;
    (!rev.is_empty()).then(|| rev.to_string())
}

#[cfg(debug_assertions)]
fn zed_dev_list(path: &str) -> Vec<SharedString> {
    let Some(root) = zed_dev_assets_dir() else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let mut stack = vec![root.join(path)];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let entry_path = entry.path();
            if entry_path.is_dir() {
                stack.push(entry_path);
            } else if let Ok(rel) = entry_path.strip_prefix(&root) {
                out.push(SharedString::from(rel.to_string_lossy().replace('\\', "/")));
            }
        }
    }
    out.sort();
    out
}

#[cfg(debug_assertions)]
fn zed_dev_load(path: &str) -> Option<Cow<'static, [u8]>> {
    // 复用 AssetStore 的防穿越规则：相对路径且不含 `..`。
    let rel = std::path::Path::new(path);
    if rel.is_absolute()
        || rel
            .components()
            .any(|c| c == std::path::Component::ParentDir)
    {
        return None;
    }
    let bytes = std::fs::read(zed_dev_assets_dir()?.join(rel)).ok()?;
    Some(Cow::Owned(bytes))
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

    /// 回归：dev 下 Zed `fs_embed!` 会误定位到 crossh 仓库根，`list("fonts")`
    /// 为空导致终端回退系统字体、字符间距拉宽。兜底必须能列出 Lilex。
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
