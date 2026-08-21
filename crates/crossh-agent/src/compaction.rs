//! Compaction with threshold/overflow, aligned with pi's `compaction/`.
//!
//! Threshold: proactive when context usage > 75%. Overflow: hard limit.

use crate::Message;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CompactionReason {
    Threshold,
    Overflow,
    Manual,
}

impl CompactionReason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Threshold => "threshold",
            Self::Overflow => "overflow",
            Self::Manual => "manual",
        }
    }
}

#[derive(Clone, Debug)]
pub struct CompactionDecision {
    pub reason: Option<CompactionReason>,
    pub tokens_before: usize,
}

pub fn should_compact(tokens_used: usize, context_limit: usize) -> Option<CompactionReason> {
    if context_limit == 0 {
        return None;
    }
    if tokens_used >= context_limit {
        Some(CompactionReason::Overflow)
    } else if tokens_used * 100 / context_limit >= 75 {
        Some(CompactionReason::Threshold)
    } else {
        None
    }
}

#[derive(Clone, Debug)]
pub struct CompactionResult {
    pub reason: CompactionReason,
    pub summary: String,
    pub first_kept_entry_id: String,
    pub tokens_before: usize,
    pub removed: usize,
}

pub fn summarize_for_compaction(
    session: &crate::session::AgentSession,
    keep_recent_tokens: usize,
    reason: CompactionReason,
) -> CompactionResult {
    let messages = &session.messages;
    let total: usize = messages.iter().map(|m| m.text.len() / 4).sum();
    let mut kept = 0;
    let mut first_kept = 0;
    for (i, m) in messages.iter().enumerate().rev() {
        kept += m.text.len() / 4;
        if kept >= keep_recent_tokens {
            first_kept = i;
            break;
        }
    }
    let removed = first_kept;
    let summary = if removed == 0 {
        String::new()
    } else {
        format!(
            "Earlier context was summarized. {removed} messages were summarized; rely on summary and recent history."
        )
    };
    // Use the real tree entry id for the first kept message, not a synthetic id.
    let first_kept_entry_id = if removed == 0 || messages.is_empty() {
        String::new()
    } else {
        let entries = crate::session::tree_entries_from_messages(session);
        entries
            .get(first_kept)
            .map(|e| e.id.clone())
            .unwrap_or_else(|| format!("entry-{first_kept}"))
    };
    CompactionResult {
        reason,
        summary,
        first_kept_entry_id,
        tokens_before: total,
        removed,
    }
}
/// Legacy helper for tests that only have a slice of messages.
/// Derive a temporary session to compute the entry id correctly.
pub fn summarize_for_compaction_messages(
    messages: &[Message],
    keep_recent_tokens: usize,
    reason: CompactionReason,
) -> CompactionResult {
    let mut tmp = crate::session::AgentSession::new("/tmp");
    tmp.messages = messages.to_vec();
    summarize_for_compaction(&tmp, keep_recent_tokens, reason)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Message, Role};

    #[test]
    fn spec_20260821_agent_runtime_threshold_triggers_at_75_percent() {
        assert_eq!(should_compact(750, 1000), Some(CompactionReason::Threshold));
        assert_eq!(should_compact(749, 1000), None);
        assert_eq!(should_compact(1000, 1000), Some(CompactionReason::Overflow));
    }

    #[test]
    fn spec_20260821_agent_runtime_summarize_keeps_recent() {
        let msgs = vec![
            Message::new(Role::User, "a".repeat(4000)),
            Message::new(Role::User, "b".repeat(4000)),
            Message::new(Role::User, "c".repeat(4000)),
        ];
        let r = summarize_for_compaction_messages(&msgs, 1000, CompactionReason::Threshold);
        assert!(r.removed > 0);
        assert!(!r.summary.is_empty());
        assert!(r.first_kept_entry_id.contains("-m"));
        assert_eq!(r.reason, CompactionReason::Threshold);
    }
}
