//! Workspace-owned settings and their validation rules.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

pub(crate) const DEFAULT_RECENT_DIRS_MAX: usize = 10;
pub(crate) const MIN_RECENT_DIRS_MAX: usize = 1;
pub(crate) const MAX_RECENT_DIRS_MAX: usize = 50;

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub(crate) struct WorkspaceSettings {
    #[serde(default, rename = "recent_local_dirs")]
    pub(crate) recent_dirs: Vec<PathBuf>,
    #[serde(default = "default_recent_dirs_max", rename = "recent_local_dirs_max")]
    pub(crate) recent_dirs_max: usize,
}

impl Default for WorkspaceSettings {
    fn default() -> Self {
        Self {
            recent_dirs: Vec::new(),
            recent_dirs_max: default_recent_dirs_max(),
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
        self
    }
}

fn default_recent_dirs_max() -> usize {
    DEFAULT_RECENT_DIRS_MAX
}
