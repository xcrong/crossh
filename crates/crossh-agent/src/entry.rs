//! Tree-shaped session entries, aligned with pi-agent's `SessionEntry`.

use crate::{Message, ThinkingLevel};
use serde::{Deserialize, Serialize};

pub const CURRENT_SESSION_VERSION: u32 = 3;

/// A single node in the session tree.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionEntry {
    pub id: String,
    #[serde(rename = "parentId")]
    pub parent_id: Option<String>,
    pub timestamp: String,
    #[serde(flatten)]
    pub data: SessionEntryData,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum SessionEntryData {
    #[serde(rename = "message")]
    Message { message: Message },
    #[serde(rename = "thinking_level_change")]
    ThinkingLevelChange { thinking_level: String },
    #[serde(rename = "model_change")]
    ModelChange { provider: String, model_id: String },
    #[serde(rename = "compaction")]
    Compaction {
        summary: String,
        first_kept_entry_id: String,
        tokens_before: usize,
    },
    #[serde(rename = "branch_summary")]
    BranchSummary { from_id: String, summary: String },
    #[serde(rename = "session_info")]
    SessionInfo { name: Option<String> },
    #[serde(rename = "custom")]
    Custom { custom_type: String },
}

impl SessionEntry {
    pub fn message(
        id: String,
        parent_id: Option<String>,
        timestamp: String,
        message: Message,
    ) -> Self {
        Self {
            id,
            parent_id,
            timestamp,
            data: SessionEntryData::Message { message },
        }
    }

    pub fn is_message(&self) -> bool {
        matches!(self.data, SessionEntryData::Message { .. })
    }

    pub fn as_message(&self) -> Option<&Message> {
        match &self.data {
            SessionEntryData::Message { message } => Some(message),
            _ => None,
        }
    }
}

pub fn thinking_level_label(level: ThinkingLevel) -> String {
    level.label().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Message, Role};

    #[test]
    fn entry_round_trips_serde() {
        let entry = SessionEntry::message(
            "id1".into(),
            None,
            "2026-08-21T00:00:00Z".into(),
            Message::new(Role::User, "hello"),
        );
        let json = serde_json::to_string(&entry).unwrap();
        let back: SessionEntry = serde_json::from_str(&json).unwrap();
        assert_eq!(entry, back);
    }
}
