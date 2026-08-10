use std::borrow::Cow;

use rust_embed::RustEmbed;

#[derive(RustEmbed)]
#[folder = "assets/icons/"]
struct Icons;

/// Return an embedded Crossh asset by its GPUI-style path.
pub fn load(path: &str) -> Option<Cow<'static, [u8]>> {
    let icon_path = path.strip_prefix("icons/")?;
    Icons::get(icon_path).map(|asset| asset.data)
}

/// Icon identifiers shared by UI consumers and the embedded asset package.
macro_rules! define_icons {
    ($( $variant:ident => $path:literal ),+ $(,)?) => {
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        pub enum IconName {
            $( $variant ),+
        }

        impl IconName {
            pub const ALL: &'static [Self] = &[
                $( Self::$variant ),+
            ];

            pub const fn path(self) -> &'static str {
                match self {
                    $( Self::$variant => $path ),+
                }
            }
        }
    };
}

define_icons! {
    ArrowLeft => "icons/arrow-left.svg",
    ArrowLeftRight => "icons/arrow-left-right.svg",
    ArrowUp => "icons/arrow-up.svg",
    Bot => "icons/bot.svg",
    Check => "icons/check.svg",
    ChevronDown => "icons/chevron-down.svg",
    ChevronRight => "icons/chevron-right.svg",
    Download => "icons/download.svg",
    Eye => "icons/eye.svg",
    EyeOff => "icons/eye-off.svg",
    FileText => "icons/file-text.svg",
    Folder => "icons/folder.svg",
    FolderOpen => "icons/folder-open.svg",
    GitBranch => "icons/git-branch.svg",
    Info => "icons/info.svg",
    KeyRound => "icons/key-round.svg",
    Link => "icons/link.svg",
    Pencil => "icons/pencil.svg",
    Plus => "icons/plus.svg",
    RefreshCw => "icons/refresh-cw.svg",
    Save => "icons/save.svg",
    Search => "icons/search.svg",
    Settings => "icons/settings.svg",
    Server => "icons/server.svg",
    ShieldAlert => "icons/shield-alert.svg",
    Terminal => "icons/terminal.svg",
    Trash => "icons/trash.svg",
    Upload => "icons/upload.svg",
    X => "icons/x.svg",
    CircleX => "icons/circle-x.svg",
    Minus => "icons/minus.svg",
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_declared_and_embedded_icon_is_loadable() {
        for icon in IconName::ALL {
            let data = load(icon.path()).expect("declared icon should be embedded");
            assert!(
                data.starts_with(b"<svg"),
                "{} is not an SVG asset",
                icon.path()
            );
        }
        let mut count = 0;
        for icon in Icons::iter() {
            count += 1;
            let icon = icon.as_ref();
            let path = format!("icons/{icon}");
            let data = load(&path).expect("icon should be embedded");
            assert!(data.starts_with(b"<svg"), "{path} is not an SVG asset");
        }
        assert!(count > 0);
    }
}
