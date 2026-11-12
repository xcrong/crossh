//! Shell 装配：共享契约下沉到 `crossh-core`（tasks 2.1），本模块只保留
//! 终端本地的任务概念并 re-export 共享类型，避免 5974 行文件的调用点扩散。
//!
//! `Shell` / `ShellKind` / `get_system_shell` / `ShellBuilder` /
//! `insert_zed_terminal_env` 的 canonical 定义见
//! `crossh_core::terminal::shell_select`。

pub use crossh_core::terminal::{
    Shell, ShellBuilder, ShellKind, get_system_shell, get_windows_system_shell,
    insert_zed_terminal_env,
};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// What to do with the terminal pane and tab, after the command has finished.
#[derive(Default, Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum HideStrategy {
    /// Do nothing when the command finishes.
    #[default]
    Never,
    /// Always hide the terminal tab, hide the pane also if it was the last tab in it.
    Always,
    /// Hide the terminal tab on task success only, otherwise behaves similar to `Always`.
    OnSuccess,
}

/// Trimmed task-spawn descriptor: only the fields this fork reads when
/// rendering task terminals (titles, summary lines, hide behavior). Upstream
/// carries the whole task system (ids, cwd, env, reveal/save strategies,
/// proto round-trips); none of it is reachable from here.
#[derive(Default, Debug, Clone, PartialEq, Eq)]
pub struct SpawnInTerminal {
    /// Human readable name of the terminal tab.
    pub label: String,
    /// Full unshortened form of `label`.
    pub full_label: String,
    /// A human-readable label, containing command and all of its arguments, joined and substituted.
    pub command_label: String,
    /// Whether to show the task summary line in the task output (success/failure).
    pub show_summary: bool,
    /// Whether to show the command line in the task output.
    pub show_command: bool,
    /// What to do with the terminal pane and tab, after the command had finished.
    pub hide: HideStrategy,
}
