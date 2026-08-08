use std::borrow::Cow;

use assets::Assets as ZedAssets;
use gpui::{AssetSource, Result, SharedString};

/// Crossh assets take precedence, with Zed's embedded assets available as the
/// fallback for fonts and other resources consumed by reused Zed components.
pub(crate) struct UiAssetSource;

impl AssetSource for UiAssetSource {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        let bytes: &'static [u8] = match path {
            "icons/arrow-left.svg" => include_bytes!("../../../assets/icons/arrow-left.svg"),
            "icons/arrow-left-right.svg" => {
                include_bytes!("../../../assets/icons/arrow-left-right.svg")
            }
            "icons/arrow-up.svg" => include_bytes!("../../../assets/icons/arrow-up.svg"),
            "icons/check.svg" => include_bytes!("../../../assets/icons/check.svg"),
            "icons/chevron-down.svg" => include_bytes!("../../../assets/icons/chevron-down.svg"),
            "icons/chevron-right.svg" => include_bytes!("../../../assets/icons/chevron-right.svg"),
            "icons/download.svg" => include_bytes!("../../../assets/icons/download.svg"),
            "icons/file-text.svg" => include_bytes!("../../../assets/icons/file-text.svg"),
            "icons/folder-open.svg" => include_bytes!("../../../assets/icons/folder-open.svg"),
            "icons/folder.svg" => include_bytes!("../../../assets/icons/folder.svg"),
            "icons/git-branch.svg" => include_bytes!("../../../assets/icons/git-branch.svg"),
            "icons/key-round.svg" => include_bytes!("../../../assets/icons/key-round.svg"),
            "icons/link.svg" => include_bytes!("../../../assets/icons/link.svg"),
            "icons/pencil.svg" => include_bytes!("../../../assets/icons/pencil.svg"),
            "icons/plus.svg" => include_bytes!("../../../assets/icons/plus.svg"),
            "icons/refresh-cw.svg" => include_bytes!("../../../assets/icons/refresh-cw.svg"),
            "icons/save.svg" => include_bytes!("../../../assets/icons/save.svg"),
            "icons/search.svg" => include_bytes!("../../../assets/icons/search.svg"),
            "icons/settings.svg" => include_bytes!("../../../assets/icons/settings.svg"),
            "icons/server.svg" => include_bytes!("../../../assets/icons/server.svg"),
            "icons/shield-alert.svg" => include_bytes!("../../../assets/icons/shield-alert.svg"),
            "icons/terminal.svg" => include_bytes!("../../../assets/icons/terminal.svg"),
            "icons/upload.svg" => include_bytes!("../../../assets/icons/upload.svg"),
            "icons/circle-x.svg" => include_bytes!("../../../assets/icons/circle-x.svg"),
            "icons/x.svg" => include_bytes!("../../../assets/icons/x.svg"),
            "icons/minus.svg" => include_bytes!("../../../assets/icons/minus.svg"),
            _ => return ZedAssets.load(path),
        };
        Ok(Some(Cow::Borrowed(bytes)))
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        ZedAssets.list(path)
    }
}
