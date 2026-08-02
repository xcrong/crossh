use std::borrow::Cow;

use gpui::{AssetSource, Result, SharedString};

/// Embedded UI assets keep the native app self-contained after packaging.
pub(crate) struct UiAssetSource;

impl AssetSource for UiAssetSource {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        let bytes: &'static [u8] = match path {
            "icons/arrow-left.svg" => include_bytes!("../../assets/icons/arrow-left.svg"),
            "icons/arrow-left-right.svg" => {
                include_bytes!("../../assets/icons/arrow-left-right.svg")
            }
            "icons/arrow-up.svg" => include_bytes!("../../assets/icons/arrow-up.svg"),
            "icons/check.svg" => include_bytes!("../../assets/icons/check.svg"),
            "icons/chevron-down.svg" => include_bytes!("../../assets/icons/chevron-down.svg"),
            "icons/chevron-right.svg" => include_bytes!("../../assets/icons/chevron-right.svg"),
            "icons/download.svg" => include_bytes!("../../assets/icons/download.svg"),
            "icons/file-text.svg" => include_bytes!("../../assets/icons/file-text.svg"),
            "icons/folder-open.svg" => include_bytes!("../../assets/icons/folder-open.svg"),
            "icons/folder.svg" => include_bytes!("../../assets/icons/folder.svg"),
            "icons/key-round.svg" => include_bytes!("../../assets/icons/key-round.svg"),
            "icons/link.svg" => include_bytes!("../../assets/icons/link.svg"),
            "icons/pencil.svg" => include_bytes!("../../assets/icons/pencil.svg"),
            "icons/plus.svg" => include_bytes!("../../assets/icons/plus.svg"),
            "icons/refresh-cw.svg" => include_bytes!("../../assets/icons/refresh-cw.svg"),
            "icons/save.svg" => include_bytes!("../../assets/icons/save.svg"),
            "icons/search.svg" => include_bytes!("../../assets/icons/search.svg"),
            "icons/server.svg" => include_bytes!("../../assets/icons/server.svg"),
            "icons/shield-alert.svg" => include_bytes!("../../assets/icons/shield-alert.svg"),
            "icons/terminal.svg" => include_bytes!("../../assets/icons/terminal.svg"),
            "icons/upload.svg" => include_bytes!("../../assets/icons/upload.svg"),
            "icons/x-circle.svg" => include_bytes!("../../assets/icons/x-circle.svg"),
            "icons/x.svg" => include_bytes!("../../assets/icons/x.svg"),
            _ => return Ok(None),
        };
        Ok(Some(Cow::Borrowed(bytes)))
    }

    fn list(&self, _path: &str) -> Result<Vec<SharedString>> {
        Ok(Vec::new())
    }
}
