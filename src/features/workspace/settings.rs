//! Workspace-owned settings and their validation rules.
//!
//! 最近目录与固定标签已迁入 daemon 的工作区模型；本结构只保留展示与
//! 终端交互设置。

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub(crate) struct WorkspaceSettings {
    #[serde(default = "default_show_host_sidebar")]
    pub(crate) show_host_sidebar: bool,
    /// 显式编辑器命令，由设置中的下拉选择框写入（选项来自自动检测结果）；
    /// 设置后状态栏「在编辑器中打开」跳过自动检测。空白归一为 `None`（视为未配置）。
    /// 检测候选列表是代码常量 `editor_launcher::DEFAULT_EDITOR_PRIORITY`，不可配置。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) editor_command: Option<String>,
    /// 焦点跟随鼠标：分屏时鼠标悬停到哪个窗格，焦点（含 accent 边框）就跟到哪。
    /// 仅在分屏布局内生效，不跨标签页切换（横扫标签条会导致视图乱跳）。
    #[serde(default = "default_focus_follows_mouse")]
    pub(crate) focus_follows_mouse: bool,
}

impl Default for WorkspaceSettings {
    fn default() -> Self {
        Self {
            show_host_sidebar: default_show_host_sidebar(),
            editor_command: None,
            focus_follows_mouse: default_focus_follows_mouse(),
        }
    }
}

impl WorkspaceSettings {
    pub(crate) fn normalized(mut self) -> Self {
        // 编辑器配置：空白命令归一为 None。
        self.editor_command = self
            .editor_command
            .take()
            .map(|command| command.trim().to_string())
            .filter(|command| !command.is_empty());
        self
    }
}

fn default_show_host_sidebar() -> bool {
    true
}

/// 焦点跟随鼠标默认开启：分屏越多，鼠标定位比键盘导航更省事。
fn default_focus_follows_mouse() -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn focus_follows_mouse_defaults_on_for_legacy_and_explicit_configs() {
        // 老版本配置文件不含该键，反序列化必须回落到默认开启而非 false。
        assert!(
            WorkspaceSettings::default().focus_follows_mouse,
            "默认应开启"
        );

        let legacy = toml::from_str::<WorkspaceSettings>("").unwrap();
        assert!(legacy.focus_follows_mouse, "缺失键应回落到默认值 true");

        let off = toml::from_str::<WorkspaceSettings>("focus_follows_mouse = false").unwrap();
        assert!(!off.focus_follows_mouse, "显式关闭必须被尊重");

        let on = toml::from_str::<WorkspaceSettings>("focus_follows_mouse = true").unwrap();
        assert!(on.focus_follows_mouse, "显式开启必须被尊重");
    }

    #[test]
    fn spec_20260820_open_project_in_editor_normalization_cleans_editor_settings() {
        let settings = WorkspaceSettings {
            editor_command: Some("  ".into()),
            ..WorkspaceSettings::default()
        }
        .normalized();
        assert_eq!(settings.editor_command, None, "空白命令归一为未配置");
    }

    #[test]
    fn spec_20260820_open_project_in_editor_normalization_is_idempotent() {
        let once = WorkspaceSettings {
            editor_command: Some(" zed ".into()),
            ..WorkspaceSettings::default()
        }
        .normalized();
        assert_eq!(once.clone().normalized(), once);
    }

    #[test]
    fn legacy_workspace_keys_are_ignored() {
        let settings: WorkspaceSettings = toml::from_str(
            "recent_local_dirs = [\"/a\"]\nrecent_local_dirs_max = 10\npinned_local_tabs = []\n",
        )
        .expect("legacy settings should deserialize");
        assert_eq!(settings, WorkspaceSettings::default());
    }
}
