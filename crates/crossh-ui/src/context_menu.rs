//! 域级菜单动作（由拥有者分发）；通用渲染与类型已抽至 `crossh-ui-component`。
//!
//! `ShellMenuAction` 保留在此以避免 `crossh-ui-component`
//! 依赖业务域；通用 `MenuItem` / `MenuEntry` / `ContextMenuState` /
//! `render_context_menu` 请从 `crossh_ui_component::context_menu` 导入。

use std::path::PathBuf;

/// 外壳级（侧栏/标签条/终端 由各自模块自持）菜单动作。
#[derive(Clone, Debug)]
pub enum ShellMenuAction {
    /// 通过系统目录选择器打开本地项目。
    ChooseLocalProject,
    /// 恢复或切换到记住的本地项目。
    ActivateLocalProject(PathBuf),
    /// 复制任意文本到剪贴板。
    CopyText(String),
    /// 在 Finder 中显示本地目录。
    RevealInFinder(PathBuf),
    /// 从「最近本地目录」移除。
    ForgetLocalDir(PathBuf),
    /// 停止本地项目（关闭该目录下全部本地会话，保留 recent/pinned）。
    StopLocalProject(PathBuf),
    /// 在本地目录打开终端。
    OpenLocalTerminal(PathBuf),
    /// 切换到本地会话。
    SelectLocalSession(u64),
    /// 固定本地会话（分配持久化 pin_id）。
    PinLocalSession(u64),
    /// 取消固定本地会话（移除持久化记录）。
    UnpinLocalSession(u64),
    /// 打开本地会话重命名弹窗。
    RenameLocalSession(u64),
    /// 打开默认命令编辑弹窗。
    EditDefaultCommand(u64),
    /// 重载默认命令到终端。
    ReloadDefaultCommand(u64),
    /// 清除默认命令。
    ClearDefaultCommand(u64),
    /// 关闭本地会话。
    CloseLocalSession(u64),
    /// 关闭同目录下的其他本地会话。
    CloseOtherLocalSessions(u64),
    /// Execute a cwd-bound quick command in the active terminal or in a task.
    RunQuickCommand {
        scope: String,
        command: String,
        background: bool,
    },
    /// Open the command editor.
    EditQuickCommand { scope: String, command: String },
    /// Toggle whether a command is shown in the collapsed rail.
    ToggleQuickCommandPin { scope: String, command: String },
    /// Remove a command from the aggregate history.
    DeleteQuickCommand { scope: String, command: String },
    /// Exclude a command from cwd-bound history.
    IgnoreQuickCommand { scope: String, command: String },
    /// Stop one background command task.
    StopBackgroundTask(u64),
    /// Stop one running background command and start it again in the background.
    RestartBackgroundTask(u64),
}
