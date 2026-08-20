//! Workspace-owned settings and their validation rules.

use std::collections::HashSet;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

pub(crate) const DEFAULT_RECENT_DIRS_MAX: usize = 10;
pub(crate) const MIN_RECENT_DIRS_MAX: usize = 1;
pub(crate) const MAX_RECENT_DIRS_MAX: usize = 50;

/// 一条固定本地会话的持久化记录。`pin_id` 是记录内唯一的稳定身份，
/// 分配后不回收、与列表位置解耦；排序由 `Vec` 顺序决定（为拖拽排序预留）。
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub(crate) struct PinnedLocalTab {
    pub(crate) pin_id: u64,
    pub(crate) project_dir: PathBuf,
    pub(crate) cwd: PathBuf,
    #[serde(default)]
    pub(crate) custom_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) default_command: Option<String>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub(crate) struct WorkspaceSettings {
    #[serde(default = "default_show_host_sidebar")]
    pub(crate) show_host_sidebar: bool,
    #[serde(default = "default_show_quick_commands")]
    pub(crate) show_quick_commands: bool,
    #[serde(default, rename = "recent_local_dirs")]
    pub(crate) recent_dirs: Vec<PathBuf>,
    #[serde(default = "default_recent_dirs_max", rename = "recent_local_dirs_max")]
    pub(crate) recent_dirs_max: usize,
    #[serde(default)]
    pub(crate) pinned_local_tabs: Vec<PinnedLocalTab>,
    /// 显式编辑器命令，由设置中的下拉选择框写入（选项来自自动检测结果）；
    /// 设置后状态栏「在编辑器中打开」跳过自动检测。空白归一为 `None`（视为未配置）。
    /// 检测候选列表是代码常量 `editor_launcher::DEFAULT_EDITOR_PRIORITY`，不可配置。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) editor_command: Option<String>,
}

impl Default for WorkspaceSettings {
    fn default() -> Self {
        Self {
            show_host_sidebar: default_show_host_sidebar(),
            show_quick_commands: default_show_quick_commands(),
            recent_dirs: Vec::new(),
            recent_dirs_max: default_recent_dirs_max(),
            pinned_local_tabs: Vec::new(),
            editor_command: None,
        }
    }
}

impl WorkspaceSettings {
    pub(crate) fn normalized(mut self) -> Self {
        self.recent_dirs_max = self
            .recent_dirs_max
            .clamp(MIN_RECENT_DIRS_MAX, MAX_RECENT_DIRS_MAX);
        if self.recent_dirs.len() > self.recent_dirs_max {
            self.recent_dirs.truncate(self.recent_dirs_max);
        }
        // 固定记录按 pin_id 去重（保留首个出现），空白自定义名称归一为 None。
        let mut seen = HashSet::new();
        self.pinned_local_tabs.retain(|tab| seen.insert(tab.pin_id));
        for tab in &mut self.pinned_local_tabs {
            tab.custom_name = tab
                .custom_name
                .take()
                .filter(|name| !name.trim().is_empty())
                .map(|name| name.trim().to_string());
            tab.default_command = tab
                .default_command
                .take()
                .map(|command| command.trim().to_string())
                .filter(|command| !command.is_empty());
        }
        // 编辑器配置：空白命令归一为 None。
        self.editor_command = self
            .editor_command
            .take()
            .map(|command| command.trim().to_string())
            .filter(|command| !command.is_empty());
        self
    }
}

fn default_recent_dirs_max() -> usize {
    DEFAULT_RECENT_DIRS_MAX
}

fn default_show_host_sidebar() -> bool {
    true
}

fn default_show_quick_commands() -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalization_clamps_limits_and_truncates_in_order() {
        let paths = (0..60)
            .map(|index| PathBuf::from(format!("/project-{index}")))
            .collect::<Vec<_>>();
        let low = WorkspaceSettings {
            recent_dirs: paths.clone(),
            recent_dirs_max: 0,
            ..WorkspaceSettings::default()
        }
        .normalized();
        assert_eq!(low.recent_dirs_max, MIN_RECENT_DIRS_MAX);
        assert_eq!(low.recent_dirs, paths[..1]);

        let high = WorkspaceSettings {
            recent_dirs: paths.clone(),
            recent_dirs_max: usize::MAX,
            ..WorkspaceSettings::default()
        }
        .normalized();
        assert_eq!(high.recent_dirs_max, MAX_RECENT_DIRS_MAX);
        assert_eq!(high.recent_dirs, paths[..MAX_RECENT_DIRS_MAX]);
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
    fn spec_20260820_open_project_in_editor_defaults_have_no_configured_command() {
        let defaults = WorkspaceSettings::default();
        assert_eq!(defaults.editor_command, None);
    }

    #[test]
    fn spec_20260818_local_tab_pin_normalization_deduplicates_pin_ids_and_cleans_blank_names() {
        let tabs = vec![
            PinnedLocalTab {
                pin_id: 2,
                project_dir: PathBuf::from("/a"),
                cwd: PathBuf::from("/a"),
                custom_name: Some("   ".into()),
                default_command: Some("   ".into()),
            },
            PinnedLocalTab {
                pin_id: 1,
                project_dir: PathBuf::from("/b"),
                cwd: PathBuf::from("/b"),
                custom_name: Some("work".into()),
                default_command: Some("  opencode  ".into()),
            },
            PinnedLocalTab {
                pin_id: 2,
                project_dir: PathBuf::from("/c"),
                cwd: PathBuf::from("/c"),
                custom_name: None,
                default_command: None,
            },
            PinnedLocalTab {
                pin_id: 3,
                project_dir: PathBuf::from("/d"),
                cwd: PathBuf::from("/d"),
                custom_name: Some("  dev  ".into()),
                default_command: Some("ssh host".into()),
            },
        ];
        let normalized = WorkspaceSettings {
            pinned_local_tabs: tabs,
            ..WorkspaceSettings::default()
        }
        .normalized();
        let ids = normalized
            .pinned_local_tabs
            .iter()
            .map(|tab| tab.pin_id)
            .collect::<Vec<_>>();
        assert_eq!(ids, vec![2, 1, 3], "重复 pin_id 只保留首个且顺序不变");
        assert_eq!(normalized.pinned_local_tabs[0].custom_name, None);
        assert_eq!(normalized.pinned_local_tabs[0].default_command, None);
        assert_eq!(
            normalized.pinned_local_tabs[1].custom_name,
            Some("work".to_string())
        );
        assert_eq!(
            normalized.pinned_local_tabs[1].default_command,
            Some("opencode".to_string())
        );
        assert_eq!(
            normalized.pinned_local_tabs[2].custom_name,
            Some("dev".to_string())
        );
        assert_eq!(
            normalized.pinned_local_tabs[2].default_command,
            Some("ssh host".to_string())
        );
    }
}
