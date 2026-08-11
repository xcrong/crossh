use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub enum AgentRole {
    System,
    User,
    Assistant,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct AgentMessage {
    pub role: AgentRole,
    pub text: String,
    pub tool_calls: Vec<AgentToolCall>,
    pub tool_result: Option<AgentToolResult>,
    /// Original provider output items needed to replay an OpenAI Responses turn.
    #[serde(default)]
    pub protocol_items: Vec<Value>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub enum AgentContentBlock {
    Text(String),
    Reasoning(String),
    ToolCall(AgentToolCall),
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct AgentToolCall {
    pub id: String,
    pub name: String,
    pub arguments: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct AgentToolResult {
    pub call_id: String,
    pub output: String,
    pub is_error: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AgentEvent {
    TextDelta(String),
    ReasoningDelta(String),
    ToolCallStart {
        index: usize,
        id: String,
        name: String,
    },
    ToolCallArgumentsDelta {
        index: usize,
        delta: String,
    },
    Stop(Option<String>),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentResponse {
    pub content: Vec<AgentContentBlock>,
    /// Raw output items are populated for OpenAI Responses when available.
    pub protocol_items: Vec<Value>,
}

impl AgentResponse {
    pub fn text(&self) -> String {
        join_blocks(&self.content, |block| match block {
            AgentContentBlock::Text(text) => Some(text),
            AgentContentBlock::Reasoning(_) => None,
            AgentContentBlock::ToolCall(_) => None,
        })
    }

    pub fn reasoning(&self) -> String {
        join_blocks(&self.content, |block| match block {
            AgentContentBlock::Reasoning(text) => Some(text),
            AgentContentBlock::Text(_) => None,
            AgentContentBlock::ToolCall(_) => None,
        })
    }
}

fn join_blocks<'a>(
    blocks: &'a [AgentContentBlock],
    select: impl Fn(&'a AgentContentBlock) -> Option<&'a String>,
) -> String {
    blocks
        .iter()
        .filter_map(select)
        .map(String::as_str)
        .collect::<Vec<_>>()
        .join("\n\n")
}

impl AgentMessage {
    pub fn new(role: AgentRole, text: impl Into<String>) -> Self {
        Self {
            role,
            text: text.into(),
            tool_calls: Vec::new(),
            tool_result: None,
            protocol_items: Vec::new(),
        }
    }

    pub fn assistant_tool_calls(tool_calls: Vec<AgentToolCall>) -> Self {
        Self {
            role: AgentRole::Assistant,
            text: String::new(),
            tool_calls,
            tool_result: None,
            protocol_items: Vec::new(),
        }
    }

    pub fn tool_result(result: AgentToolResult) -> Self {
        Self {
            role: AgentRole::User,
            text: String::new(),
            tool_calls: Vec::new(),
            tool_result: Some(result),
            protocol_items: Vec::new(),
        }
    }
}
