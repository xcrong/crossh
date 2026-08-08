use gpui::{SharedString, Styled, Svg, px, svg};

#[derive(Clone, Copy)]
pub(crate) enum IconName {
    ArrowLeft,
    ArrowLeftRight,
    ArrowUp,
    Check,
    ChevronDown,
    ChevronRight,
    Download,
    FileText,
    Folder,
    FolderOpen,
    GitBranch,
    KeyRound,
    Link,
    Pencil,
    Plus,
    RefreshCw,
    Save,
    Search,
    Settings,
    Server,
    ShieldAlert,
    Terminal,
    Trash,
    Upload,
    X,
    CircleX,
    Minus,
}

impl IconName {
    fn path(self) -> &'static str {
        match self {
            Self::ArrowLeft => "icons/arrow-left.svg",
            Self::ArrowLeftRight => "icons/arrow-left-right.svg",
            Self::ArrowUp => "icons/arrow-up.svg",
            Self::Check => "icons/check.svg",
            Self::ChevronDown => "icons/chevron-down.svg",
            Self::ChevronRight => "icons/chevron-right.svg",
            Self::Download => "icons/download.svg",
            Self::FileText => "icons/file-text.svg",
            Self::Folder => "icons/folder.svg",
            Self::FolderOpen => "icons/folder-open.svg",
            Self::GitBranch => "icons/git-branch.svg",
            Self::KeyRound => "icons/key-round.svg",
            Self::Link => "icons/link.svg",
            Self::Pencil => "icons/pencil.svg",
            Self::Plus => "icons/plus.svg",
            Self::RefreshCw => "icons/refresh-cw.svg",
            Self::Save => "icons/save.svg",
            Self::Search => "icons/search.svg",
            Self::Settings => "icons/settings.svg",
            Self::Server => "icons/server.svg",
            Self::ShieldAlert => "icons/shield-alert.svg",
            Self::Terminal => "icons/terminal.svg",
            Self::Trash => "icons/trash.svg",
            Self::Upload => "icons/upload.svg",
            Self::X => "icons/x.svg",
            Self::CircleX => "icons/circle-x.svg",
            Self::Minus => "icons/minus.svg",
        }
    }
}

pub(crate) fn icon(name: IconName, size: f32) -> Svg {
    svg().path(SharedString::from(name.path())).size(px(size))
}
