//! Git domain types.

use crate::git_status::GitStatus;

pub const MAX_DIFF_BYTES: u64 = 2 * 1024 * 1024;
pub const MAX_DIFF_LINES: usize = 10_000;
#[derive(Debug, thiserror::Error)]
pub enum GitError {
    #[error("无法执行 git：{0}")]
    Spawn(#[from] std::io::Error),
    #[error("{0}")]
    CommandFailed(String),
    #[error("差异内容超过 {MAX_DIFF_BYTES} 字节，未加载以保持界面可用")]
    DiffTooLarge,
}

/// 单个文件在索引（已暂存）或工作区（未暂存）中的变更状态。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChangeStatus {
    Modified,
    Added,
    Deleted,
    Renamed,
    Conflict,
    Untracked,
}

impl ChangeStatus {
    /// 组内排序权重：冲突/新增优先于修改，便于稳定排序。
    pub fn rank(self) -> u8 {
        match self {
            Self::Conflict => 0,
            Self::Added | Self::Renamed => 1,
            Self::Deleted => 2,
            Self::Modified => 3,
            Self::Untracked => 4,
        }
    }

    /// 列表行首的状态字形（VS Code 风格）。
    pub fn glyph(self) -> &'static str {
        match self {
            Self::Modified => "M",
            Self::Added => "A",
            Self::Deleted => "D",
            Self::Renamed => "R",
            Self::Conflict => "!",
            Self::Untracked => "?",
        }
    }
}

/// 一条（路径，暂存态）上的改动。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileChange {
    /// 当前磁盘上的路径（重命名的目标路径）。
    pub path: String,
    /// 重命名/复制的源路径。
    pub orig_path: Option<String>,
    pub status: ChangeStatus,
    /// true = 已暂存（相对 HEAD），false = 工作区未暂存（相对索引）。
    pub staged: bool,
    pub insertions: usize,
    pub deletions: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiffLineKind {
    /// `@@ -a,b +c,d @@` 的 hunk 头。
    Hunk,
    Context,
    Added,
    Removed,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiffLine {
    pub kind: DiffLineKind,
    /// 所属 Hunk 的索引；未跟踪文件的合成新增行没有 Hunk。
    pub hunk_index: Option<usize>,
    /// 旧文件行号（`-` / 上下文行有值，新增行为 None）。
    pub old_ln: Option<u32>,
    /// 新文件行号（`+` / 上下文行有值，删除行为 None）。
    pub new_ln: Option<u32>,
    pub text: String,
}

/// 一个文件的完整 diff 内容（选中行渲染用）。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FileDiff {
    pub old_path: Option<String>,
    pub new_path: Option<String>,
    pub lines: Vec<DiffLine>,
    /// 二进制文件：无文本 hunk。
    pub binary: bool,
}

/// 一次工作区扫描的变更列表与分支状态。
///
/// 两者来自同一份 porcelain 输出，避免界面层为状态栏额外启动一次 `git status`。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ChangeScan {
    pub changes: Vec<FileChange>,
    pub status: Option<GitStatus>,
}

