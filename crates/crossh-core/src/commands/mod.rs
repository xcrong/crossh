//! Working-directory-bound command history and background command execution.

pub mod background;
pub mod history;

pub use background::{
    BackgroundTask, BackgroundTaskEvent, BackgroundTaskManager, BackgroundTaskStatus,
};
pub use history::{
    CommandHistory, CommandRecord, DISPLAY_LIMIT, MAX_HISTORY_ENTRIES, command_history_cache_path,
    local_scope, quick_commands_config_path, remote_scope,
};
