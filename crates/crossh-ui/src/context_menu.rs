//! 域级菜单动作（由拥有者分发）；通用渲染与类型已抽至 `crossh-ui-component`。
//!
//! `ShellMenuAction` 保留在此以避免 `crossh-ui-component`
//! 依赖业务域；通用 `MenuItem` / `MenuEntry` / `ContextMenuState` /
//! `render_context_menu` 请从 `crossh_ui_component::context_menu` 导入。
//!
//! 工作区身份使用 daemon 的稳定 id（`u64`），菜单层不依赖 `crossh-session`。

use std::path::PathBuf;

/// 外壳级（侧栏/标签条/终端 由各自模块自持）菜单动作。
#[derive(Clone, Debug)]
pub enum ShellMenuAction {
    /// 通过系统目录选择器打开本地项目。
    ChooseLocalProject,
    /// 激活已记住的项目。
    ActivateProject(u64),
    /// 激活项目内的 tab。
    ActivateTab(u64),
    /// 在项目内新建 tab。
    CreateTab(u64),
    /// 关闭 tab（连同其下全部 pane）。
    CloseTab(u64),
    /// 关闭单个 pane。
    ClosePane(u64),
    /// 关闭项目内除 `keep` 外的其他 tab。
    CloseOtherTabs { project_id: u64, keep: u64 },
    /// 停止项目（结束其全部 pane 会话，保留布局）。
    StopProject(u64),
    /// 忘记项目（移除项目及其布局）。
    ForgetProject(u64),
    /// 打开 tab 重命名弹窗。
    RenameTab(u64),
    /// 打开默认命令编辑弹窗。
    EditDefaultCommand(u64),
    /// 重载默认命令到终端。
    ReloadDefaultCommand(u64),
    /// 清除默认命令。
    ClearDefaultCommand(u64),
    /// 重试物化不可用/休眠 pane。
    RetryPaneSession(u64),
    /// 复制任意文本到剪贴板。
    CopyText(String),
    /// 在 Finder 中显示目录。
    RevealInFinder(PathBuf),
    /// 在项目内打开新终端。
    OpenProjectTerminal(u64),
}
