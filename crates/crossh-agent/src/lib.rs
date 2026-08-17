//! Vendor-neutral agent messages and wire-protocol adapters.

mod config;
mod messages;
mod policy;
mod providers;
mod tools;

pub mod session;

pub use config::load as load_agent_settings;

pub use messages::{
    AgentContentBlock, AgentEvent, AgentMessage, AgentResponse, AgentRole, AgentToolCall,
    AgentToolResult,
};
pub use policy::{
    AgentModel, AgentModelRef, AgentProtocol, AgentProvider, AgentReviewResult, AgentSettings,
    AgentThinkingLevel, ResolvedModel, review_tool,
};
pub use providers::{complete, complete_stream_with_options};
pub use tools::{AgentToolDefinition, builtin_tools, execute_tool_with_cancel};

pub use session::{
    AgentContextFile, AgentPrompt, AgentSession, AgentSessionSummary, AgentSkill, context_prompt,
    create_session, export_markdown, latest_session, list_sessions, load_context_files,
    load_prompts, load_session, load_skills, save_session,
};

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
