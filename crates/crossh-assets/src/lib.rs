use std::borrow::Cow;
use std::fs;
use std::path::{Path, PathBuf};

use rust_embed::RustEmbed;

#[derive(RustEmbed)]
#[folder = "assets/icons/"]
struct Icons;

pub const LOGO_PATH: &str = "brand/crossh-logo.svg";
const CROSSH_LOGO: &[u8] = include_bytes!("../../../assets/appicon/icon-master.svg");

/// Locate resources shared by all Crossh binaries.
#[derive(Clone, Debug)]
pub struct AssetStore {
    root: PathBuf,
}

impl AssetStore {
    pub fn discover() -> Option<Self> {
        if let Some(path) = std::env::var_os("CROSSH_ASSET_DIR") {
            let store = Self::new(PathBuf::from(path));
            if store.is_valid() {
                return Some(store);
            }
        }

        let executable = std::env::current_exe().ok()?;
        let executable_dir = executable.parent()?;
        Self::candidate_roots(executable_dir)
            .into_iter()
            .map(Self::new)
            .find(|store| store.is_valid())
    }

    /// 按优先级列出共享资源目录：exe 相对路径优先（tarball/AppImage/macOS），
    /// 再回退发行版原生包（.deb/.rpm）的 FHS 系统路径（/usr/local 优先于 /usr）。
    fn candidate_roots(executable_dir: &Path) -> Vec<PathBuf> {
        vec![
            executable_dir.join("../Resources/crossh-assets"),
            executable_dir.join("resources/crossh-assets"),
            PathBuf::from("/usr/local/share/crossh/crossh-assets"),
            PathBuf::from("/usr/share/crossh/crossh-assets"),
        ]
    }

    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    pub fn load(&self, path: &str) -> Option<Cow<'static, [u8]>> {
        let path = safe_relative_path(path)?;
        Some(Cow::Owned(fs::read(self.root.join(path)).ok()?))
    }

    pub fn list(&self, prefix: &str) -> Vec<String> {
        let root = self.root.join(prefix);
        let mut paths = Vec::new();
        collect_files(&root, &self.root, &mut paths);
        paths
    }

    fn is_valid(&self) -> bool {
        self.root.is_dir()
    }
}

fn safe_relative_path(path: &str) -> Option<&Path> {
    let path = Path::new(path);
    if path.is_absolute()
        || path
            .components()
            .any(|component| component == std::path::Component::ParentDir)
    {
        return None;
    }
    Some(path)
}

fn collect_files(path: &Path, root: &Path, output: &mut Vec<String>) {
    let Ok(entries) = fs::read_dir(path) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_files(&path, root, output);
        } else if let Ok(relative) = path.strip_prefix(root) {
            output.push(relative.to_string_lossy().replace('\\', "/"));
        }
    }
}

#[cfg(test)]
mod asset_store_tests {
    use std::fs;

    use super::AssetStore;

    #[test]
    fn external_store_loads_and_lists_relative_files() {
        let directory =
            std::env::temp_dir().join(format!("crossh-assets-test-{}", std::process::id()));
        fs::create_dir_all(directory.join("fonts")).unwrap();
        fs::write(directory.join("fonts/test.ttf"), b"font").unwrap();
        let store = AssetStore::new(directory.clone());

        assert_eq!(store.load("fonts/test.ttf").unwrap().as_ref(), b"font");
        assert_eq!(store.list("fonts"), ["fonts/test.ttf"]);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn external_store_rejects_absolute_and_parent_paths() {
        let store = AssetStore::new(std::env::temp_dir());
        assert!(store.load("/etc/passwd").is_none());
        assert!(store.load("fonts/../secret").is_none());
    }

    #[test]
    fn candidate_roots_prefer_executable_relative_then_system_paths() {
        let executable_dir = std::path::Path::new("/usr/bin");
        let roots = AssetStore::candidate_roots(executable_dir);
        assert_eq!(
            roots,
            vec![
                std::path::PathBuf::from("/usr/bin/../Resources/crossh-assets"),
                std::path::PathBuf::from("/usr/bin/resources/crossh-assets"),
                std::path::PathBuf::from("/usr/local/share/crossh/crossh-assets"),
                std::path::PathBuf::from("/usr/share/crossh/crossh-assets"),
            ]
        );
    }
}

/// Return an embedded Crossh asset by its GPUI-style path.
pub fn load(path: &str) -> Option<Cow<'static, [u8]>> {
    if path == LOGO_PATH {
        return Some(Cow::Borrowed(CROSSH_LOGO));
    }
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
    Activity => "icons/activity.svg",
    Check => "icons/check.svg",
    ChevronDown => "icons/chevron-down.svg",
    ChevronRight => "icons/chevron-right.svg",
    Clock => "icons/clock.svg",
    Columns2 => "icons/columns-2.svg",
    Rows2 => "icons/rows-2.svg",
    Download => "icons/download.svg",
    FileText => "icons/file-text.svg",
    Folder => "icons/folder.svg",
    FolderOpen => "icons/folder-open.svg",
    GitBranch => "icons/git-branch.svg",
    Info => "icons/info.svg",
    KeyRound => "icons/key-round.svg",
    Keyboard => "icons/keyboard.svg",
    Link => "icons/link.svg",
    Pencil => "icons/pencil.svg",
    Pin => "icons/pin.svg",
    PanelLeft => "icons/panel-left.svg",
    PanelRight => "icons/panel-right.svg",
    Plus => "icons/plus.svg",
    Play => "icons/play.svg",
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
    Square => "icons/square.svg",
    SquarePen => "icons/square-pen.svg",
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_declared_and_embedded_icon_is_loadable() {
        let logo = load(LOGO_PATH).expect("brand logo should be embedded");
        assert!(logo.starts_with(b"<svg"));
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
