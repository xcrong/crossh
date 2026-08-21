//! Vendor-neutral agent messages and wire-protocol adapters.
//!
//! `crossh-ai-sdk` is the single source of truth for canonical messages, tools,
//! protocols, and thinking levels; this crate re-exports them and consumes them
//! directly (no mirrored types, no conversion glue). Agent-layer semantics such
//! as approval policy, thinking decisions, and settings stay in this crate.

mod config;
mod policy;
mod providers;
mod tools;

pub mod compaction;
pub mod entry;
pub mod event;
pub mod manager;
pub mod runtime;
pub mod session;

pub use config::load as load_agent_settings;

pub use crossh_ai_sdk::{
    ContentBlock, Event, Message, Protocol, Response, Role, ThinkingLevel, ToolCall, ToolResult,
};
pub use policy::{
    ALL_PROTOCOLS, ALL_THINKING_LEVELS, AgentModel, AgentModelRef, AgentProvider,
    AgentReviewResult, AgentSettings, ResolvedModel, review_tool,
};
pub use providers::complete_stream_with_options;
pub use tools::{AgentToolDefinition, builtin_tools, execute_tool_with_cancel};

pub use compaction::{
    CompactionReason, CompactionResult, should_compact, summarize_for_compaction,
};
pub use entry::{CURRENT_SESSION_VERSION as ENTRY_VERSION, SessionEntry, SessionEntryData};
pub use event::{AgentSessionEvent, EventBus, MessageQueue};
pub use manager::{FsSessionManager, InMemorySessionManager, SessionManager};
pub use runtime::{AgentSessionRuntime, AgentSessionServices};
pub use session::{
    AgentContextFile, AgentPrompt, AgentSession, AgentSessionSummary, AgentSkill,
    CURRENT_SESSION_VERSION, context_prompt, create_session, export_markdown, latest_session,
    list_sessions, load_context_files, load_prompts, load_session, load_skills, save_session,
    tree_entries_from_messages,
};

/// Agent-layer label accessor over the SDK [`Protocol`] model.
pub trait ProtocolExt {
    fn label(self) -> &'static str;
}

impl ProtocolExt for Protocol {
    fn label(self) -> &'static str {
        match self {
            Protocol::OpenAiChat => "openai-chat",
            Protocol::OpenAiResponses => "openai-responses",
            Protocol::AnthropicMessages => "anthropic-messages",
        }
    }
}

/// Agent-layer convenience accessors over the SDK [`Response`] model.
pub trait ResponseExt {
    fn text(&self) -> String;
    fn reasoning(&self) -> String;
}

impl ResponseExt for Response {
    fn text(&self) -> String {
        join_blocks(&self.content, |block| match block {
            ContentBlock::Text(text) => Some(text),
            ContentBlock::Reasoning(_) | ContentBlock::ToolCall(_) => None,
        })
    }

    fn reasoning(&self) -> String {
        join_blocks(&self.content, |block| match block {
            ContentBlock::Reasoning(text) => Some(text),
            ContentBlock::Text(_) | ContentBlock::ToolCall(_) => None,
        })
    }
}

/// Agent-layer constructors over the SDK [`Message`] model.
pub trait MessageExt {
    fn assistant_tool_calls(tool_calls: Vec<ToolCall>) -> Message;
    fn tool_result(result: ToolResult) -> Message;
}

impl MessageExt for Message {
    fn assistant_tool_calls(tool_calls: Vec<ToolCall>) -> Message {
        Message {
            role: Role::Assistant,
            text: String::new(),
            tool_calls,
            tool_result: None,
            protocol_items: Vec::new(),
        }
    }

    fn tool_result(result: ToolResult) -> Message {
        Message {
            role: Role::User,
            text: String::new(),
            tool_calls: Vec::new(),
            tool_result: Some(result),
            protocol_items: Vec::new(),
        }
    }
}

fn join_blocks<'a>(
    blocks: &'a [ContentBlock],
    select: impl Fn(&'a ContentBlock) -> Option<&'a String>,
) -> String {
    blocks
        .iter()
        .filter_map(select)
        .map(String::as_str)
        .collect::<Vec<_>>()
        .join("\n\n")
}

#[cfg(test)]
use policy::{MAX_TOOL_OUTPUT_BYTES, parse_review_result};
#[cfg(test)]
use providers::{Utf8StreamDecoder, apply_model_options, apply_thinking_option, wire_messages};
#[cfg(test)]
use serde_json::json;
#[cfg(test)]
use std::fs;
#[cfg(test)]
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
#[cfg(test)]
use std::time::Duration;
#[cfg(test)]
use tools::{ToolControl, grep_without_rg};

#[cfg(test)]
mod tests;
