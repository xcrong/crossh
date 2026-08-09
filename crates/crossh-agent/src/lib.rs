//! Vendor-neutral agent messages and wire-protocol adapters.

use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::process::Command;

fn default_url() -> String {
    "http://127.0.0.1:11434/v1/chat/completions".into()
}

fn default_model() -> String {
    "qwen3-coder".into()
}
fn default_max_tool_rounds() -> u32 {
    200
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
pub enum AgentProtocol {
    #[default]
    #[serde(rename = "openai-chat")]
    OpenAiChat,
    #[serde(rename = "openai-responses")]
    OpenAiResponses,
    #[serde(rename = "anthropic-messages")]
    AnthropicMessages,
}

impl AgentProtocol {
    pub const ALL: [Self; 3] = [
        Self::OpenAiChat,
        Self::OpenAiResponses,
        Self::AnthropicMessages,
    ];

    pub const fn label(self) -> &'static str {
        match self {
            Self::OpenAiChat => "openai-chat",
            Self::OpenAiResponses => "openai-responses",
            Self::AnthropicMessages => "anthropic-messages",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct AgentProvider {
    pub id: String,
    pub name: String,
    pub protocol: AgentProtocol,
    pub url: String,
    #[serde(default)]
    pub api_key_env: String,
    #[serde(default)]
    pub api_key: String,
    pub models: Vec<AgentModel>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct AgentModel {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub reasoning: bool,
    #[serde(default = "default_context_window")]
    pub context_window: u32,
    #[serde(default = "default_max_tokens")]
    pub max_tokens: u32,
}

fn default_context_window() -> u32 {
    128_000
}
fn default_max_tokens() -> u32 {
    32_000
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct AgentModelRef {
    pub provider: String,
    pub model: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct AgentSettings {
    pub providers: Vec<AgentProvider>,
    pub active_model: AgentModelRef,
    pub reviewer_model: AgentModelRef,
    #[serde(default = "default_max_tool_rounds")]
    pub max_tool_rounds: u32,
}

impl Default for AgentSettings {
    fn default() -> Self {
        Self {
            providers: vec![AgentProvider {
                id: "local".into(),
                name: "Local".into(),
                protocol: AgentProtocol::default(),
                url: default_url(),
                api_key_env: String::new(),
                api_key: String::new(),
                models: vec![AgentModel {
                    id: default_model(),
                    name: default_model(),
                    reasoning: true,
                    context_window: default_context_window(),
                    max_tokens: default_max_tokens(),
                }],
            }],
            active_model: AgentModelRef {
                provider: "local".into(),
                model: default_model(),
            },
            reviewer_model: AgentModelRef {
                provider: "local".into(),
                model: default_model(),
            },
            max_tool_rounds: default_max_tool_rounds(),
        }
    }
}

impl AgentSettings {
    pub fn normalized(mut self) -> Self {
        for provider in &mut self.providers {
            provider.id = provider.id.trim().into();
            provider.name = provider.name.trim().into();
            provider.url = provider.url.trim().into();
            provider.api_key_env = provider.api_key_env.trim().into();
            for model in &mut provider.models {
                model.id = model.id.trim().into();
                model.name = model.name.trim().into();
            }
        }
        self
    }

    pub fn validate(&self) -> Result<(), &'static str> {
        if self.providers.is_empty() {
            return Err("At least one provider is required");
        }
        let mut provider_ids = std::collections::BTreeSet::new();
        for provider in &self.providers {
            if provider.id.is_empty() || provider.name.is_empty() || provider.models.is_empty() {
                return Err("Provider ID, name, and models are required");
            }
            if !provider_ids.insert(provider.id.as_str()) {
                return Err("Provider IDs must be unique");
            }
            if !(provider.url.starts_with("http://") || provider.url.starts_with("https://")) {
                return Err("API URL must start with http:// or https://");
            }
            if !provider.api_key_env.is_empty()
                && !provider.api_key_env.chars().enumerate().all(|(index, ch)| {
                    ch == '_' || ch.is_ascii_alphanumeric() && (index > 0 || !ch.is_ascii_digit())
                })
            {
                return Err("Credential environment variable name is invalid");
            }
            let mut model_ids = std::collections::BTreeSet::new();
            for model in &provider.models {
                if model.id.is_empty() || model.name.is_empty() {
                    return Err("Model ID and name are required");
                }
                if !model_ids.insert(model.id.as_str()) {
                    return Err("Model IDs must be unique within a provider");
                }
                if model.context_window == 0 || model.max_tokens == 0 {
                    return Err("Model token limits must be greater than zero");
                }
            }
        }
        self.resolve(&self.active_model)?;
        self.resolve(&self.reviewer_model)?;
        if !(1..=1000).contains(&self.max_tool_rounds) {
            return Err("Tool rounds must be between 1 and 1000");
        }
        Ok(())
    }

    pub fn resolve(&self, reference: &AgentModelRef) -> Result<ResolvedModel<'_>, &'static str> {
        let provider = self
            .providers
            .iter()
            .find(|p| p.id == reference.provider)
            .ok_or("Provider not found")?;
        let model = provider
            .models
            .iter()
            .find(|m| m.id == reference.model)
            .ok_or("Model not found")?;
        Ok(ResolvedModel { provider, model })
    }
}

pub struct ResolvedModel<'a> {
    pub provider: &'a AgentProvider,
    pub model: &'a AgentModel,
}

pub async fn review_tool(
    settings: &AgentSettings,
    api_key: Option<&str>,
    call: &AgentToolCall,
    workspace: &Path,
) -> Result<bool, String> {
    let reviewer = settings
        .resolve(&settings.reviewer_model)
        .map_err(str::to_string)?;
    let messages = vec![
        AgentMessage::new(
            AgentRole::System,
            "You are a tool execution reviewer. Reply with exactly ALLOW or DENY. Allow only actions that are necessary, scoped to the stated workspace, and consistent with the user's request.",
        ),
        AgentMessage::new(
            AgentRole::User,
            format!(
                "Workspace: {}\nTool: {}\nArguments: {}",
                workspace.display(),
                call.name,
                call.arguments
            ),
        ),
    ];
    let response = complete_target(reviewer, api_key, &messages, false).await?;
    Ok(response.text().trim().eq_ignore_ascii_case("ALLOW"))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AgentRole {
    System,
    User,
    Assistant,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentMessage {
    pub role: AgentRole,
    pub text: String,
    pub tool_calls: Vec<AgentToolCall>,
    pub tool_result: Option<AgentToolResult>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AgentContentBlock {
    Text(String),
    Reasoning(String),
    ToolCall(AgentToolCall),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentToolCall {
    pub id: String,
    pub name: String,
    pub arguments: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
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

#[derive(Clone, Debug, PartialEq)]
pub struct AgentToolDefinition {
    pub name: &'static str,
    pub description: &'static str,
    pub input_schema: Value,
    pub requires_approval: bool,
}

pub fn builtin_tools() -> Vec<AgentToolDefinition> {
    vec![
        AgentToolDefinition {
            name: "read",
            description: "Read a UTF-8 file inside the current workspace",
            input_schema: json!({"type":"object","properties":{"path":{"type":"string"},"offset":{"type":"integer","minimum":1},"limit":{"type":"integer","minimum":1}},"required":["path"],"additionalProperties":false}),
            requires_approval: false,
        },
        AgentToolDefinition {
            name: "write",
            description: "Create or replace a UTF-8 file inside the current workspace",
            input_schema: json!({"type":"object","properties":{"path":{"type":"string"},"content":{"type":"string"}},"required":["path","content"],"additionalProperties":false}),
            requires_approval: true,
        },
        AgentToolDefinition {
            name: "edit",
            description: "Replace one exact text occurrence in a workspace file",
            input_schema: json!({"type":"object","properties":{"path":{"type":"string"},"old_text":{"type":"string"},"new_text":{"type":"string"}},"required":["path","old_text","new_text"],"additionalProperties":false}),
            requires_approval: true,
        },
        AgentToolDefinition {
            name: "bash",
            description: "Run a shell command in the current workspace",
            input_schema: json!({"type":"object","properties":{"command":{"type":"string"}},"required":["command"],"additionalProperties":false}),
            requires_approval: true,
        },
    ]
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentResponse {
    pub content: Vec<AgentContentBlock>,
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
        }
    }

    pub fn assistant_tool_calls(tool_calls: Vec<AgentToolCall>) -> Self {
        Self {
            role: AgentRole::Assistant,
            text: String::new(),
            tool_calls,
            tool_result: None,
        }
    }

    pub fn tool_result(result: AgentToolResult) -> Self {
        Self {
            role: AgentRole::User,
            text: String::new(),
            tool_calls: Vec::new(),
            tool_result: Some(result),
        }
    }
}

pub fn execute_tool(call: &AgentToolCall, workspace: &Path) -> AgentToolResult {
    let result = execute_tool_inner(call, workspace);
    AgentToolResult {
        call_id: call.id.clone(),
        is_error: result.is_err(),
        output: truncate_output(&result.unwrap_or_else(|error| error)),
    }
}

fn execute_tool_inner(call: &AgentToolCall, workspace: &Path) -> Result<String, String> {
    let args: Value = serde_json::from_str(&call.arguments)
        .map_err(|error| format!("invalid tool arguments: {error}"))?;
    match call.name.as_str() {
        "read" => {
            let path = workspace_path(workspace, required_str(&args, "path")?, false)?;
            let text = fs::read_to_string(path).map_err(|error| error.to_string())?;
            let offset = args
                .get("offset")
                .and_then(Value::as_u64)
                .unwrap_or(1)
                .max(1) as usize;
            let limit = args
                .get("limit")
                .and_then(Value::as_u64)
                .unwrap_or(200)
                .min(2000) as usize;
            Ok(text
                .lines()
                .skip(offset - 1)
                .take(limit)
                .enumerate()
                .map(|(index, line)| format!("{}: {line}", offset + index))
                .collect::<Vec<_>>()
                .join("\n"))
        }
        "write" => {
            let path = workspace_path(workspace, required_str(&args, "path")?, true)?;
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).map_err(|error| error.to_string())?;
            }
            fs::write(&path, required_str(&args, "content")?).map_err(|error| error.to_string())?;
            Ok(format!("wrote {}", path.display()))
        }
        "edit" => {
            let path = workspace_path(workspace, required_str(&args, "path")?, false)?;
            let mut text = fs::read_to_string(&path).map_err(|error| error.to_string())?;
            let old = required_str(&args, "old_text")?;
            let new = required_str(&args, "new_text")?;
            if old.is_empty() || text.matches(old).count() != 1 {
                return Err("old_text must match exactly once".into());
            }
            text = text.replacen(old, new, 1);
            fs::write(&path, text).map_err(|error| error.to_string())?;
            Ok(format!("edited {}", path.display()))
        }
        "bash" => {
            let command = required_str(&args, "command")?;
            #[cfg(unix)]
            let output = Command::new("sh")
                .args(["-lc", command])
                .current_dir(workspace)
                .output();
            #[cfg(windows)]
            let output = Command::new("powershell")
                .args(["-NoProfile", "-Command", command])
                .current_dir(workspace)
                .output();
            let output = output.map_err(|error| error.to_string())?;
            let mut text = String::from_utf8_lossy(&output.stdout).into_owned();
            text.push_str(&String::from_utf8_lossy(&output.stderr));
            Ok(format!("exit status: {}\n{text}", output.status))
        }
        _ => Err(format!("unknown tool: {}", call.name)),
    }
}

fn required_str<'a>(args: &'a Value, name: &str) -> Result<&'a str, String> {
    args.get(name)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("missing string argument: {name}"))
}

fn workspace_path(workspace: &Path, value: &str, allow_missing: bool) -> Result<PathBuf, String> {
    let relative = Path::new(value);
    if relative.is_absolute()
        || relative
            .components()
            .any(|part| matches!(part, Component::ParentDir))
    {
        return Err("path must stay inside the current workspace".into());
    }
    let workspace = workspace
        .canonicalize()
        .map_err(|error| error.to_string())?;
    let path = workspace.join(relative);
    if path.exists() {
        let canonical = path.canonicalize().map_err(|error| error.to_string())?;
        if !canonical.starts_with(&workspace) {
            return Err("path escapes the current workspace".into());
        }
        Ok(canonical)
    } else if allow_missing {
        let parent = path.parent().unwrap_or(&workspace);
        let existing = parent
            .ancestors()
            .find(|path| path.exists())
            .ok_or("no existing parent")?;
        if !existing
            .canonicalize()
            .map_err(|error| error.to_string())?
            .starts_with(&workspace)
        {
            return Err("path escapes the current workspace".into());
        }
        Ok(path)
    } else {
        Err("path does not exist".into())
    }
}

fn truncate_output(text: &str) -> String {
    const MAX: usize = 64 * 1024;
    if text.len() <= MAX {
        text.to_string()
    } else {
        format!(
            "{}\n[output truncated]",
            &text[..text.floor_char_boundary(MAX)]
        )
    }
}

/// Protocol-specific HTTP metadata. The transport remains independent of the
/// agent loop and only applies these already-normalized headers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AgentAuthStyle {
    Bearer,
    Anthropic,
}

pub struct AgentWireRequest {
    pub body: Value,
    pub auth_style: AgentAuthStyle,
}

pub async fn complete(
    settings: &AgentSettings,
    api_key: Option<&str>,
    messages: &[AgentMessage],
) -> Result<AgentResponse, String> {
    complete_with_tools(settings, api_key, messages, true).await
}

async fn complete_with_tools(
    settings: &AgentSettings,
    api_key: Option<&str>,
    messages: &[AgentMessage],
    include_tools: bool,
) -> Result<AgentResponse, String> {
    let target = settings
        .resolve(&settings.active_model)
        .map_err(str::to_string)?;
    complete_target(target, api_key, messages, include_tools).await
}

async fn complete_target(
    target: ResolvedModel<'_>,
    api_key: Option<&str>,
    messages: &[AgentMessage],
    include_tools: bool,
) -> Result<AgentResponse, String> {
    let client = reqwest::Client::new();
    let mut wire = encode_request(target.provider.protocol, &target.model.id, messages);
    apply_model_options(&mut wire.body, target.provider.protocol, target.model);
    if !include_tools {
        wire.body.as_object_mut().map(|body| body.remove("tools"));
    }
    let mut request = client.post(&target.provider.url).json(&wire.body);
    if let Some(api_key) = api_key {
        request = match wire.auth_style {
            AgentAuthStyle::Bearer => request.bearer_auth(api_key),
            AgentAuthStyle::Anthropic => request
                .header("x-api-key", api_key)
                .header("anthropic-version", "2023-06-01"),
        };
    }
    let response = request.send().await.map_err(|error| error.to_string())?;
    let status = response.status();
    let body: Value = response.json().await.map_err(|error| error.to_string())?;
    if !status.is_success() {
        let message = body
            .pointer("/error/message")
            .and_then(Value::as_str)
            .unwrap_or("API returned an error");
        return Err(format!("HTTP {status}: {message}"));
    }
    decode_response(target.provider.protocol, &body).map_err(ToString::to_string)
}

pub async fn complete_stream(
    settings: &AgentSettings,
    api_key: Option<&str>,
    messages: &[AgentMessage],
    mut on_event: impl FnMut(&AgentEvent),
) -> Result<AgentResponse, String> {
    let target = settings
        .resolve(&settings.active_model)
        .map_err(str::to_string)?;
    let client = reqwest::Client::new();
    let mut wire = encode_request(target.provider.protocol, &target.model.id, messages);
    apply_model_options(&mut wire.body, target.provider.protocol, target.model);
    wire.body["stream"] = Value::Bool(true);
    let mut request = client.post(&target.provider.url).json(&wire.body);
    if let Some(api_key) = api_key {
        request = match wire.auth_style {
            AgentAuthStyle::Bearer => request.bearer_auth(api_key),
            AgentAuthStyle::Anthropic => request
                .header("x-api-key", api_key)
                .header("anthropic-version", "2023-06-01"),
        };
    }
    let response = request.send().await.map_err(|error| error.to_string())?;
    let status = response.status();
    if !status.is_success() {
        return Err(format!(
            "HTTP {status}: {}",
            response.text().await.unwrap_or_default()
        ));
    }

    let mut bytes = response.bytes_stream();
    let mut pending = String::new();
    let mut accumulator = StreamAccumulator::default();
    while let Some(chunk) = bytes.next().await {
        let chunk = chunk.map_err(|error| error.to_string())?;
        pending.push_str(&String::from_utf8_lossy(&chunk));
        while let Some(newline) = pending.find('\n') {
            let line = pending[..newline].trim_end_matches('\r').to_string();
            pending.drain(..=newline);
            let Some(data) = line.strip_prefix("data:").map(str::trim) else {
                continue;
            };
            if data.is_empty() || data == "[DONE]" {
                continue;
            }
            let value: Value = serde_json::from_str(data).map_err(|error| error.to_string())?;
            for event in decode_stream_event(target.provider.protocol, &value) {
                accumulator.push(&event);
                on_event(&event);
            }
        }
    }
    accumulator.finish()
}

#[derive(Default)]
struct StreamAccumulator {
    text: String,
    reasoning: String,
    tools: BTreeMap<usize, AgentToolCall>,
}

impl StreamAccumulator {
    fn push(&mut self, event: &AgentEvent) {
        match event {
            AgentEvent::TextDelta(delta) => self.text.push_str(delta),
            AgentEvent::ReasoningDelta(delta) => self.reasoning.push_str(delta),
            AgentEvent::ToolCallStart { index, id, name } => {
                self.tools.insert(
                    *index,
                    AgentToolCall {
                        id: id.clone(),
                        name: name.clone(),
                        arguments: String::new(),
                    },
                );
            }
            AgentEvent::ToolCallArgumentsDelta { index, delta } => {
                self.tools
                    .entry(*index)
                    .or_insert_with(|| AgentToolCall {
                        id: String::new(),
                        name: String::new(),
                        arguments: String::new(),
                    })
                    .arguments
                    .push_str(delta);
            }
            AgentEvent::Stop(_) => {}
        }
    }

    fn finish(self) -> Result<AgentResponse, String> {
        let mut content = Vec::new();
        if !self.reasoning.is_empty() {
            content.push(AgentContentBlock::Reasoning(self.reasoning));
        }
        if !self.text.is_empty() {
            content.push(AgentContentBlock::Text(self.text));
        }
        content.extend(self.tools.into_values().map(AgentContentBlock::ToolCall));
        if content.is_empty() {
            return Err("stream completed without content".into());
        }
        Ok(AgentResponse { content })
    }
}

pub fn encode_request(
    protocol: AgentProtocol,
    model: &str,
    messages: &[AgentMessage],
) -> AgentWireRequest {
    match protocol {
        AgentProtocol::OpenAiChat => AgentWireRequest {
            body: json!({
                "model": model,
                "messages": wire_messages(AgentProtocol::OpenAiChat, messages),
                "tools": builtin_tools().iter().map(|tool| json!({"type":"function","function":{"name":tool.name,"description":tool.description,"parameters":tool.input_schema}})).collect::<Vec<_>>(),
                "stream": false
            }),
            auth_style: AgentAuthStyle::Bearer,
        },
        AgentProtocol::OpenAiResponses => AgentWireRequest {
            body: json!({
                "model": model,
                "input": wire_messages(AgentProtocol::OpenAiResponses, messages),
                "tools": builtin_tools().iter().map(|tool| json!({"type":"function","name":tool.name,"description":tool.description,"parameters":tool.input_schema,"strict":true})).collect::<Vec<_>>(),
                "stream": false
            }),
            auth_style: AgentAuthStyle::Bearer,
        },
        AgentProtocol::AnthropicMessages => {
            let system = messages
                .iter()
                .filter(|message| message.role == AgentRole::System)
                .map(|message| message.text.as_str())
                .collect::<Vec<_>>()
                .join("\n\n");
            let regular_messages = messages
                .iter()
                .filter(|message| message.role != AgentRole::System)
                .cloned()
                .collect::<Vec<_>>();
            AgentWireRequest {
                body: json!({
                    "model": model,
                    "system": system,
                    "messages": wire_messages(AgentProtocol::AnthropicMessages, &regular_messages),
                    "tools": builtin_tools().iter().map(|tool| json!({"name":tool.name,"description":tool.description,"input_schema":tool.input_schema})).collect::<Vec<_>>(),
                    "max_tokens": 4096,
                    "stream": false
                }),
                auth_style: AgentAuthStyle::Anthropic,
            }
        }
    }
}

fn apply_model_options(body: &mut Value, protocol: AgentProtocol, model: &AgentModel) {
    let key = match protocol {
        AgentProtocol::OpenAiChat | AgentProtocol::AnthropicMessages => "max_tokens",
        AgentProtocol::OpenAiResponses => "max_output_tokens",
    };
    body[key] = Value::from(model.max_tokens);
}

pub fn decode_response(
    protocol: AgentProtocol,
    body: &Value,
) -> Result<AgentResponse, &'static str> {
    let mut blocks = Vec::new();
    match protocol {
        AgentProtocol::OpenAiChat => {
            let message = body
                .pointer("/choices/0/message")
                .ok_or("Chat response did not contain an assistant message")?;
            push_string(
                &mut blocks,
                AgentContentKind::Reasoning,
                message.get("reasoning_content"),
            );
            push_string(&mut blocks, AgentContentKind::Text, message.get("content"));
            for call in message
                .get("tool_calls")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                push_tool_call(
                    &mut blocks,
                    call.get("id").and_then(Value::as_str),
                    call.pointer("/function/name").and_then(Value::as_str),
                    call.pointer("/function/arguments").and_then(Value::as_str),
                );
            }
        }
        AgentProtocol::OpenAiResponses => {
            for item in body
                .get("output")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                match item.get("type").and_then(Value::as_str) {
                    Some("reasoning") => {
                        let content = item.get("content").and_then(Value::as_array);
                        let summary = item.get("summary").and_then(Value::as_array);
                        let parts = content.filter(|parts| !parts.is_empty()).or(summary);
                        for part in parts.into_iter().flatten() {
                            push_string(&mut blocks, AgentContentKind::Reasoning, part.get("text"));
                        }
                    }
                    Some("message") => {
                        for part in item
                            .get("content")
                            .and_then(Value::as_array)
                            .into_iter()
                            .flatten()
                        {
                            push_string(&mut blocks, AgentContentKind::Text, part.get("text"));
                        }
                    }
                    Some("function_call") => push_tool_call(
                        &mut blocks,
                        item.get("call_id").and_then(Value::as_str),
                        item.get("name").and_then(Value::as_str),
                        item.get("arguments").and_then(Value::as_str),
                    ),
                    _ => {}
                }
            }
        }
        AgentProtocol::AnthropicMessages => {
            for part in body
                .get("content")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                match part.get("type").and_then(Value::as_str) {
                    Some("thinking") => push_string(
                        &mut blocks,
                        AgentContentKind::Reasoning,
                        part.get("thinking"),
                    ),
                    Some("text") => {
                        push_string(&mut blocks, AgentContentKind::Text, part.get("text"))
                    }
                    Some("tool_use") => push_tool_call(
                        &mut blocks,
                        part.get("id").and_then(Value::as_str),
                        part.get("name").and_then(Value::as_str),
                        part.get("input").map(Value::to_string).as_deref(),
                    ),
                    _ => {}
                }
            }
        }
    }
    if blocks.is_empty() {
        return Err("protocol response did not contain text or reasoning content");
    }
    Ok(AgentResponse { content: blocks })
}

fn push_tool_call(
    blocks: &mut Vec<AgentContentBlock>,
    id: Option<&str>,
    name: Option<&str>,
    arguments: Option<&str>,
) {
    let (Some(id), Some(name)) = (id, name) else {
        return;
    };
    blocks.push(AgentContentBlock::ToolCall(AgentToolCall {
        id: id.into(),
        name: name.into(),
        arguments: arguments.unwrap_or("{}").into(),
    }));
}

/// Normalize one decoded SSE `data:` object. Tool argument fragments remain
/// fragments and are assembled by the agent loop using their stable index.
pub fn decode_stream_event(protocol: AgentProtocol, event: &Value) -> Vec<AgentEvent> {
    match protocol {
        AgentProtocol::OpenAiChat => {
            let Some(choice) = event.pointer("/choices/0") else {
                return Vec::new();
            };
            let delta = choice.get("delta").unwrap_or(&Value::Null);
            let mut events = Vec::new();
            push_delta(
                &mut events,
                delta.get("reasoning_content"),
                AgentEvent::ReasoningDelta,
            );
            push_delta(&mut events, delta.get("content"), AgentEvent::TextDelta);
            for call in delta
                .get("tool_calls")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                let index = call.get("index").and_then(Value::as_u64).unwrap_or(0) as usize;
                if let (Some(id), Some(name)) = (
                    call.get("id").and_then(Value::as_str),
                    call.pointer("/function/name").and_then(Value::as_str),
                ) {
                    events.push(AgentEvent::ToolCallStart {
                        index,
                        id: id.into(),
                        name: name.into(),
                    });
                }
                push_tool_delta(&mut events, index, call.pointer("/function/arguments"));
            }
            if choice
                .get("finish_reason")
                .is_some_and(|value| !value.is_null())
            {
                events.push(AgentEvent::Stop(
                    choice
                        .get("finish_reason")
                        .and_then(Value::as_str)
                        .map(str::to_string),
                ));
            }
            events
        }
        AgentProtocol::OpenAiResponses => {
            let index = event
                .get("output_index")
                .and_then(Value::as_u64)
                .unwrap_or(0) as usize;
            match event.get("type").and_then(Value::as_str) {
                Some("response.output_text.delta") => delta_event(event, AgentEvent::TextDelta),
                Some("response.reasoning_text.delta" | "response.reasoning_summary_text.delta") => {
                    delta_event(event, AgentEvent::ReasoningDelta)
                }
                Some("response.output_item.added")
                    if event.pointer("/item/type").and_then(Value::as_str)
                        == Some("function_call") =>
                {
                    vec![AgentEvent::ToolCallStart {
                        index,
                        id: event
                            .pointer("/item/call_id")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .into(),
                        name: event
                            .pointer("/item/name")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .into(),
                    }]
                }
                Some("response.function_call_arguments.delta") => delta_event(event, |delta| {
                    AgentEvent::ToolCallArgumentsDelta { index, delta }
                }),
                Some("response.completed") => vec![AgentEvent::Stop(Some("completed".into()))],
                _ => Vec::new(),
            }
        }
        AgentProtocol::AnthropicMessages => {
            let index = event.get("index").and_then(Value::as_u64).unwrap_or(0) as usize;
            match event.get("type").and_then(Value::as_str) {
                Some("content_block_start")
                    if event.pointer("/content_block/type").and_then(Value::as_str)
                        == Some("tool_use") =>
                {
                    vec![AgentEvent::ToolCallStart {
                        index,
                        id: event
                            .pointer("/content_block/id")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .into(),
                        name: event
                            .pointer("/content_block/name")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .into(),
                    }]
                }
                Some("content_block_delta") => {
                    match event.pointer("/delta/type").and_then(Value::as_str) {
                        Some("text_delta") => {
                            value_event(event.pointer("/delta/text"), AgentEvent::TextDelta)
                        }
                        Some("thinking_delta") => value_event(
                            event.pointer("/delta/thinking"),
                            AgentEvent::ReasoningDelta,
                        ),
                        Some("input_json_delta") => {
                            value_event(event.pointer("/delta/partial_json"), |delta| {
                                AgentEvent::ToolCallArgumentsDelta { index, delta }
                            })
                        }
                        _ => Vec::new(),
                    }
                }
                Some("message_delta") => vec![AgentEvent::Stop(
                    event
                        .pointer("/delta/stop_reason")
                        .and_then(Value::as_str)
                        .map(str::to_string),
                )],
                _ => Vec::new(),
            }
        }
    }
}

fn push_delta(
    events: &mut Vec<AgentEvent>,
    value: Option<&Value>,
    make: impl Fn(String) -> AgentEvent,
) {
    events.extend(value_event(value, make));
}
fn push_tool_delta(events: &mut Vec<AgentEvent>, index: usize, value: Option<&Value>) {
    push_delta(events, value, |delta| AgentEvent::ToolCallArgumentsDelta {
        index,
        delta,
    });
}
fn delta_event(event: &Value, make: impl Fn(String) -> AgentEvent) -> Vec<AgentEvent> {
    value_event(event.get("delta"), make)
}
fn value_event(value: Option<&Value>, make: impl Fn(String) -> AgentEvent) -> Vec<AgentEvent> {
    value
        .and_then(Value::as_str)
        .filter(|text| !text.is_empty())
        .map(|text| vec![make(text.into())])
        .unwrap_or_default()
}

#[derive(Clone, Copy)]
enum AgentContentKind {
    Text,
    Reasoning,
}

fn push_string(blocks: &mut Vec<AgentContentBlock>, kind: AgentContentKind, value: Option<&Value>) {
    let Some(text) = value
        .and_then(Value::as_str)
        .filter(|text| !text.is_empty())
    else {
        return;
    };
    blocks.push(match kind {
        AgentContentKind::Text => AgentContentBlock::Text(text.to_string()),
        AgentContentKind::Reasoning => AgentContentBlock::Reasoning(text.to_string()),
    });
}

fn openai_message(message: &AgentMessage) -> Value {
    json!({
        "role": match message.role {
            AgentRole::System => "system",
            AgentRole::User => "user",
            AgentRole::Assistant => "assistant",
        },
        "content": message.text
    })
}

fn wire_messages(protocol: AgentProtocol, messages: &[AgentMessage]) -> Vec<Value> {
    let mut result = Vec::new();
    for message in messages {
        if let Some(tool_result) = &message.tool_result {
            result.push(match protocol {
                AgentProtocol::OpenAiChat => json!({"role":"tool","tool_call_id":tool_result.call_id,"content":tool_result.output}),
                AgentProtocol::OpenAiResponses => json!({"type":"function_call_output","call_id":tool_result.call_id,"output":tool_result.output}),
                AgentProtocol::AnthropicMessages => json!({"role":"user","content":[{"type":"tool_result","tool_use_id":tool_result.call_id,"content":tool_result.output,"is_error":tool_result.is_error}]}),
            });
            continue;
        }
        if !message.tool_calls.is_empty() {
            result.push(match protocol {
                AgentProtocol::OpenAiChat => json!({"role":"assistant","content":null,"tool_calls":message.tool_calls.iter().map(|call| json!({"id":call.id,"type":"function","function":{"name":call.name,"arguments":call.arguments}})).collect::<Vec<_>>()}),
                AgentProtocol::OpenAiResponses => {
                    for call in &message.tool_calls { result.push(json!({"type":"function_call","call_id":call.id,"name":call.name,"arguments":call.arguments})); }
                    continue;
                }
                AgentProtocol::AnthropicMessages => json!({"role":"assistant","content":message.tool_calls.iter().map(|call| json!({"type":"tool_use","id":call.id,"name":call.name,"input":serde_json::from_str::<Value>(&call.arguments).unwrap_or(Value::Null)})).collect::<Vec<_>>()}),
            });
            continue;
        }
        result.push(openai_message(message));
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn messages() -> Vec<AgentMessage> {
        vec![
            AgentMessage::new(AgentRole::System, "be useful"),
            AgentMessage::new(AgentRole::User, "hello"),
        ]
    }

    #[test]
    fn protocol_ids_match_the_public_api_format_names() {
        assert_eq!(
            serde_json::to_string(&AgentProtocol::OpenAiChat).unwrap(),
            r#""openai-chat""#
        );
        assert_eq!(
            serde_json::to_string(&AgentProtocol::OpenAiResponses).unwrap(),
            r#""openai-responses""#
        );
        assert_eq!(
            serde_json::to_string(&AgentProtocol::AnthropicMessages).unwrap(),
            r#""anthropic-messages""#
        );
    }

    #[test]
    fn multi_provider_models_resolve_independently() {
        let mut settings = AgentSettings::default();
        settings.providers.push(AgentProvider {
            id: "reviewer".into(),
            name: "Reviewer".into(),
            protocol: AgentProtocol::AnthropicMessages,
            url: "https://example.test/messages".into(),
            api_key_env: "REVIEWER_API_KEY".into(),
            api_key: String::new(),
            models: vec![AgentModel {
                id: "review-model".into(),
                name: "Review Model".into(),
                reasoning: true,
                context_window: 200_000,
                max_tokens: 8_000,
            }],
        });
        settings.reviewer_model = AgentModelRef {
            provider: "reviewer".into(),
            model: "review-model".into(),
        };

        settings.validate().unwrap();
        let active = settings.resolve(&settings.active_model).unwrap();
        let reviewer = settings.resolve(&settings.reviewer_model).unwrap();
        assert_eq!(active.provider.id, "local");
        assert_eq!(reviewer.provider.id, "reviewer");
        assert_eq!(reviewer.model.max_tokens, 8_000);
    }

    #[test]
    fn provider_and_model_ids_must_be_unique() {
        let mut settings = AgentSettings::default();
        settings.providers.push(settings.providers[0].clone());
        assert_eq!(settings.validate(), Err("Provider IDs must be unique"));

        let mut settings = AgentSettings::default();
        let duplicate = settings.providers[0].models[0].clone();
        settings.providers[0].models.push(duplicate);
        assert_eq!(
            settings.validate(),
            Err("Model IDs must be unique within a provider")
        );
    }

    #[test]
    fn model_output_limit_maps_to_each_protocol() {
        let model = AgentModel {
            id: "m".into(),
            name: "M".into(),
            reasoning: false,
            context_window: 10_000,
            max_tokens: 1_234,
        };
        for (protocol, key) in [
            (AgentProtocol::OpenAiChat, "max_tokens"),
            (AgentProtocol::OpenAiResponses, "max_output_tokens"),
            (AgentProtocol::AnthropicMessages, "max_tokens"),
        ] {
            let mut wire = encode_request(protocol, &model.id, &messages());
            apply_model_options(&mut wire.body, protocol, &model);
            assert_eq!(wire.body[key], 1_234);
        }
    }

    #[test]
    fn adapters_encode_the_same_canonical_messages() {
        let chat = encode_request(AgentProtocol::OpenAiChat, "model", &messages());
        let responses = encode_request(AgentProtocol::OpenAiResponses, "model", &messages());
        let anthropic = encode_request(AgentProtocol::AnthropicMessages, "model", &messages());
        assert_eq!(chat.body["messages"][1]["content"], "hello");
        assert_eq!(responses.body["input"][1]["content"], "hello");
        assert_eq!(anthropic.body["system"], "be useful");
        assert_eq!(anthropic.body["messages"][0]["content"], "hello");
    }

    #[test]
    fn adapters_decode_protocol_responses() {
        assert_eq!(
            decode_response(
                AgentProtocol::OpenAiChat,
                &json!({"choices":[{"message":{"content":"a"}}]})
            ),
            Ok(AgentResponse {
                content: vec![AgentContentBlock::Text("a".into())]
            })
        );
        assert_eq!(
            decode_response(
                AgentProtocol::OpenAiResponses,
                &json!({"output":[
                    {"type":"reasoning","summary":[{"type":"summary_text","text":"think b"}]},
                    {"type":"message","content":[{"type":"output_text","text":"b"}]}
                ]})
            ),
            Ok(AgentResponse {
                content: vec![
                    AgentContentBlock::Reasoning("think b".into()),
                    AgentContentBlock::Text("b".into())
                ]
            })
        );
        assert_eq!(
            decode_response(
                AgentProtocol::AnthropicMessages,
                &json!({"content":[
                    {"type":"thinking","thinking":"think c","signature":"sig"},
                    {"type":"text","text":"c"}
                ]})
            ),
            Ok(AgentResponse {
                content: vec![
                    AgentContentBlock::Reasoning("think c".into()),
                    AgentContentBlock::Text("c".into())
                ]
            })
        );
    }

    #[test]
    fn chat_reasoning_content_is_separate_from_visible_text() {
        let response = decode_response(
            AgentProtocol::OpenAiChat,
            &json!({"choices":[{"message":{"reasoning_content":"think a","content":"a"}}]}),
        )
        .unwrap();
        assert_eq!(response.reasoning(), "think a");
        assert_eq!(response.text(), "a");
    }

    #[test]
    fn stream_events_normalize_text_reasoning_and_tool_arguments() {
        assert_eq!(
            decode_stream_event(
                AgentProtocol::OpenAiChat,
                &json!({"choices":[{"delta":{"reasoning_content":"think","content":"answer","tool_calls":[{"index":0,"id":"call_1","function":{"name":"read","arguments":"{\"path\":"}}]}}]})
            ),
            vec![
                AgentEvent::ReasoningDelta("think".into()),
                AgentEvent::TextDelta("answer".into()),
                AgentEvent::ToolCallStart {
                    index: 0,
                    id: "call_1".into(),
                    name: "read".into()
                },
                AgentEvent::ToolCallArgumentsDelta {
                    index: 0,
                    delta: "{\"path\":".into()
                },
            ]
        );
        assert_eq!(
            decode_stream_event(
                AgentProtocol::OpenAiResponses,
                &json!({"type":"response.reasoning_summary_text.delta","delta":"summary","output_index":0})
            ),
            vec![AgentEvent::ReasoningDelta("summary".into())]
        );
        assert_eq!(
            decode_stream_event(
                AgentProtocol::AnthropicMessages,
                &json!({"type":"content_block_delta","index":2,"delta":{"type":"input_json_delta","partial_json":"{\"path\":"}})
            ),
            vec![AgentEvent::ToolCallArgumentsDelta {
                index: 2,
                delta: "{\"path\":".into()
            }]
        );
    }

    #[test]
    fn tool_results_encode_for_each_protocol() {
        let message = AgentMessage::tool_result(AgentToolResult {
            call_id: "call_1".into(),
            output: "done".into(),
            is_error: false,
        });
        assert_eq!(
            wire_messages(AgentProtocol::OpenAiChat, std::slice::from_ref(&message))[0]["role"],
            "tool"
        );
        assert_eq!(
            wire_messages(
                AgentProtocol::OpenAiResponses,
                std::slice::from_ref(&message)
            )[0]["type"],
            "function_call_output"
        );
        assert_eq!(
            wire_messages(AgentProtocol::AnthropicMessages, &[message])[0]["content"][0]["type"],
            "tool_result"
        );
    }

    #[test]
    fn read_tool_is_workspace_scoped() {
        let workspace = tempfile::tempdir().unwrap();
        fs::write(workspace.path().join("notes.txt"), "one\ntwo\nthree\n").unwrap();
        let result = execute_tool(
            &AgentToolCall {
                id: "1".into(),
                name: "read".into(),
                arguments: r#"{"path":"notes.txt","offset":2,"limit":1}"#.into(),
            },
            workspace.path(),
        );
        assert!(!result.is_error);
        assert_eq!(result.output, "2: two");

        let escaped = execute_tool(
            &AgentToolCall {
                id: "2".into(),
                name: "read".into(),
                arguments: r#"{"path":"../secret"}"#.into(),
            },
            workspace.path(),
        );
        assert!(escaped.is_error);
    }
}
