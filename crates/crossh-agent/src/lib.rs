//! Vendor-neutral agent messages and wire-protocol adapters.

use crossh_ai_sdk as sdk;
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::fs;
use std::io::{BufRead, BufReader, Read};
use std::path::{Component, Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

#[cfg(unix)]
use std::os::unix::process::CommandExt;

pub mod session;

pub use session::{
    AgentContextFile, AgentPrompt, AgentSession, AgentSessionSummary, AgentSkill, context_prompt,
    create_session, export_markdown, latest_session, list_sessions, load_context_files,
    load_prompts, load_session, load_skills, save_session,
};

fn default_max_tool_rounds() -> u32 {
    200
}

const MAX_TOOL_ARGUMENT_BYTES: usize = 256 * 1024;
const MAX_TOOL_OUTPUT_BYTES: usize = 64 * 1024;
const MAX_FILE_BYTES: u64 = 16 * 1024 * 1024;
const MAX_FILE_SCAN_BYTES: u64 = 64 * 1024 * 1024;
const MAX_LINE_BYTES: usize = 1024 * 1024;
const MAX_DIRECTORY_ENTRIES: usize = 20_000;
const MAX_DISCOVERED_PATHS: usize = 100_000;
const TOOL_TIMEOUT: Duration = Duration::from_secs(120);
const MODEL_REQUEST_TIMEOUT: Duration = Duration::from_secs(10 * 60);
const REVIEWER_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

struct ToolControl<'a> {
    cancel: &'a AtomicBool,
    deadline: Instant,
}

impl<'a> ToolControl<'a> {
    fn new(cancel: &'a AtomicBool) -> Self {
        Self {
            cancel,
            deadline: Instant::now() + TOOL_TIMEOUT,
        }
    }
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

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
pub enum AgentThinkingLevel {
    Off,
    Minimal,
    Low,
    #[default]
    Medium,
    High,
    XHigh,
}

impl AgentThinkingLevel {
    pub const ALL: [Self; 6] = [
        Self::Off,
        Self::Minimal,
        Self::Low,
        Self::Medium,
        Self::High,
        Self::XHigh,
    ];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Minimal => "minimal",
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::XHigh => "xhigh",
        }
    }

    #[cfg(test)]
    fn budget(self, max_tokens: u32) -> u32 {
        let fraction = match self {
            Self::Off => 0.0,
            Self::Minimal => 0.05,
            Self::Low => 0.15,
            Self::Medium => 0.3,
            Self::High => 0.5,
            Self::XHigh => 0.75,
        };
        ((max_tokens as f32 * fraction) as u32)
            .max(1_024)
            .min(max_tokens.saturating_sub(1).max(1))
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

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
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
            providers: Vec::new(),
            active_model: AgentModelRef::default(),
            reviewer_model: AgentModelRef::default(),
            max_tool_rounds: default_max_tool_rounds(),
        }
    }
}

impl AgentSettings {
    pub fn normalized(mut self) -> Self {
        self.active_model.provider = self.active_model.provider.trim().into();
        self.active_model.model = self.active_model.model.trim().into();
        self.reviewer_model.provider = self.reviewer_model.provider.trim().into();
        self.reviewer_model.model = self.reviewer_model.model.trim().into();
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
            if self.active_model == AgentModelRef::default()
                && self.reviewer_model == AgentModelRef::default()
            {
                if !(1..=1000).contains(&self.max_tool_rounds) {
                    return Err("Tool rounds must be between 1 and 1000");
                }
                return Ok(());
            }
            return Err("Model references require a configured provider");
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
                if model.max_tokens >= model.context_window {
                    return Err("Maximum output tokens must be smaller than the context window");
                }
                if model.context_window.saturating_sub(model.max_tokens) < 1_024 {
                    return Err("Model context must leave at least 1024 input tokens");
                }
            }
        }
        let has_models = self
            .providers
            .iter()
            .any(|provider| !provider.models.is_empty());
        if has_models {
            self.resolve(&self.active_model)?;
            self.resolve(&self.reviewer_model)?;
        } else if self.active_model != AgentModelRef::default()
            || self.reviewer_model != AgentModelRef::default()
        {
            return Err("Model references require a configured model");
        }
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
    user_request: &str,
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
                "User request:\n{}\n\nWorkspace: {}\nTool: {}\nArguments: {}",
                user_request,
                workspace.display(),
                call.name,
                call.arguments
            ),
        ),
    ];
    let response = complete_target_with_timeout(
        reviewer,
        api_key,
        &messages,
        false,
        REVIEWER_REQUEST_TIMEOUT,
    )
    .await?;
    Ok(response.text().trim().eq_ignore_ascii_case("ALLOW"))
}

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
            description: "Read a UTF-8 file inside the current workspace; prefer a workspace-relative path such as README.md",
            input_schema: json!({"type":"object","properties":{"path":{"type":"string"},"offset":{"type":["integer","null"],"minimum":1},"limit":{"type":["integer","null"],"minimum":1}},"required":["path","offset","limit"],"additionalProperties":false}),
            requires_approval: false,
        },
        AgentToolDefinition {
            name: "grep",
            description: "Search workspace files for a text or regular expression; prefer a workspace-relative path",
            input_schema: json!({"type":"object","properties":{"pattern":{"type":"string"},"path":{"type":["string","null"]},"limit":{"type":["integer","null"],"minimum":1}},"required":["pattern","path","limit"],"additionalProperties":false}),
            requires_approval: false,
        },
        AgentToolDefinition {
            name: "find",
            description: "Find files and directories in the current workspace; prefer a workspace-relative path",
            input_schema: json!({"type":"object","properties":{"pattern":{"type":["string","null"]},"path":{"type":["string","null"]},"limit":{"type":["integer","null"],"minimum":1}},"required":["pattern","path","limit"],"additionalProperties":false}),
            requires_approval: false,
        },
        AgentToolDefinition {
            name: "ls",
            description: "List entries in a workspace directory; prefer a workspace-relative path such as .",
            input_schema: json!({"type":"object","properties":{"path":{"type":["string","null"]}},"required":["path"],"additionalProperties":false}),
            requires_approval: false,
        },
        AgentToolDefinition {
            name: "write",
            description: "Create or replace a UTF-8 file inside the current workspace; prefer a workspace-relative path",
            input_schema: json!({"type":"object","properties":{"path":{"type":"string"},"content":{"type":"string"}},"required":["path","content"],"additionalProperties":false}),
            requires_approval: true,
        },
        AgentToolDefinition {
            name: "edit",
            description: "Replace one exact text occurrence in a workspace file; prefer a workspace-relative path",
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

pub fn execute_tool(call: &AgentToolCall, workspace: &Path) -> AgentToolResult {
    let cancel = AtomicBool::new(false);
    execute_tool_with_cancel(call, workspace, &cancel)
}

pub fn execute_tool_with_cancel(
    call: &AgentToolCall,
    workspace: &Path,
    cancel: &AtomicBool,
) -> AgentToolResult {
    let control = ToolControl::new(cancel);
    let result = execute_tool_inner(call, workspace, &control);
    AgentToolResult {
        call_id: call.id.clone(),
        is_error: result.is_err(),
        output: truncate_output(&result.unwrap_or_else(|error| error)),
    }
}

fn execute_tool_inner(
    call: &AgentToolCall,
    workspace: &Path,
    control: &ToolControl<'_>,
) -> Result<String, String> {
    if call.arguments.len() > MAX_TOOL_ARGUMENT_BYTES {
        return Err(format!(
            "tool arguments exceed the {} KiB limit",
            MAX_TOOL_ARGUMENT_BYTES / 1024
        ));
    }
    check_cancelled(control)?;
    let args: Value = serde_json::from_str(&call.arguments)
        .map_err(|error| format!("invalid tool arguments: {error}"))?;
    match call.name.as_str() {
        "read" => {
            let path = workspace_path(workspace, required_str(&args, "path")?, false)?;
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
            read_file_lines(&path, offset, limit, control)
        }
        "grep" => execute_grep(&args, workspace, control),
        "find" => execute_find(&args, workspace, control),
        "ls" => execute_ls(&args, workspace, control),
        "write" => {
            let path = workspace_path(workspace, required_str(&args, "path")?, true)?;
            let content = required_str(&args, "content")?;
            if content.len() as u64 > MAX_FILE_BYTES {
                return Err(format!(
                    "file content exceeds the {} MiB limit",
                    MAX_FILE_BYTES / (1024 * 1024)
                ));
            }
            check_cancelled(control)?;
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).map_err(|error| error.to_string())?;
            }
            fs::write(&path, content).map_err(|error| error.to_string())?;
            Ok(format!("wrote {}", path.display()))
        }
        "edit" => {
            let path = workspace_path(workspace, required_str(&args, "path")?, false)?;
            let mut text = read_file_string(&path, control)?;
            let old = required_str(&args, "old_text")?;
            let new = required_str(&args, "new_text")?;
            check_cancelled(control)?;
            if old.is_empty() || text.matches(old).count() != 1 {
                return Err("old_text must match exactly once".into());
            }
            text = text.replacen(old, new, 1);
            fs::write(&path, text).map_err(|error| error.to_string())?;
            Ok(format!("edited {}", path.display()))
        }
        "bash" => {
            let command = required_str(&args, "command")?;
            let mut process = shell_command(command, workspace);
            let output = run_bounded_command(&mut process, control)?;
            Ok(format_command_output(&output))
        }
        _ => Err(format!("unknown tool: {}", call.name)),
    }
}

fn execute_grep(
    args: &Value,
    workspace: &Path,
    control: &ToolControl<'_>,
) -> Result<String, String> {
    let pattern = required_str(args, "pattern")?;
    if pattern.is_empty() {
        return Err("pattern must not be empty".into());
    }
    let root = optional_workspace_path(args, "path", workspace)?;
    let limit = args
        .get("limit")
        .and_then(Value::as_u64)
        .unwrap_or(100)
        .clamp(1, 1_000) as usize;

    let mut command = Command::new("rg");
    command
        .args(["--line-number", "--no-heading", "--color", "never"])
        .arg("--glob")
        .arg("!.git/**")
        .arg("--glob")
        .arg("!target/**")
        .arg("--glob")
        .arg("!node_modules/**")
        .arg("--max-count")
        .arg(limit.to_string())
        .arg("--")
        .arg(pattern)
        .arg(&root)
        .current_dir(workspace);
    let output = match run_bounded_command(&mut command, control) {
        Ok(output) => output,
        Err(error) if error.starts_with("failed to spawn command") => {
            return grep_without_rg(pattern, &root, workspace, limit, control);
        }
        Err(error) => return Err(error),
    };
    if !output.status.success() && output.status.code() != Some(1) {
        let error = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(if error.is_empty() {
            format!("rg exited with {}", output.status)
        } else {
            error
        });
    }
    let text = String::from_utf8_lossy(&output.stdout);
    Ok(limit_lines(text.trim(), limit))
}

fn grep_without_rg(
    pattern: &str,
    root: &Path,
    workspace: &Path,
    limit: usize,
    control: &ToolControl<'_>,
) -> Result<String, String> {
    let regex = Regex::new(pattern).map_err(|error| format!("invalid regex: {error}"))?;
    let mut output = String::new();
    let mut match_count = 0;
    for path in walk_paths(root, workspace, control)? {
        check_cancelled(control)?;
        if !path.is_file() {
            continue;
        }
        let Ok(file) = fs::File::open(&path) else {
            continue;
        };
        let mut reader = BufReader::new(file);
        let mut line = String::new();
        let mut line_number = 0;
        let mut scanned = 0_u64;
        loop {
            check_cancelled(control)?;
            line.clear();
            let read = reader
                .by_ref()
                .take((MAX_LINE_BYTES + 1) as u64)
                .read_line(&mut line)
                .map_err(|error| error.to_string())?;
            if read == 0 {
                break;
            }
            if read > MAX_LINE_BYTES {
                return Err(format!(
                    "a searched line exceeds the {} MiB limit",
                    MAX_LINE_BYTES / (1024 * 1024)
                ));
            }
            scanned = scanned.saturating_add(read as u64);
            if scanned > MAX_FILE_SCAN_BYTES {
                break;
            }
            line_number += 1;
            let text = line.trim_end_matches(['\r', '\n']);
            if regex.is_match(text) {
                let formatted = format!(
                    "{}:{}:{}\n",
                    relative_display(workspace, &path),
                    line_number,
                    text
                );
                const TRUNCATION_NOTICE: &str = "\n[output truncated]";
                if output.len().saturating_add(formatted.len()) > MAX_TOOL_OUTPUT_BYTES {
                    let available = MAX_TOOL_OUTPUT_BYTES.saturating_sub(TRUNCATION_NOTICE.len());
                    if output.is_empty() {
                        let end = formatted.floor_char_boundary(available);
                        output.push_str(&formatted[..end]);
                    }
                    output.truncate(output.floor_char_boundary(available));
                    output.push_str(TRUNCATION_NOTICE);
                    return Ok(output);
                }
                output.push_str(&formatted);
                match_count += 1;
                if match_count >= limit {
                    return Ok(output.trim_end_matches('\n').to_string());
                }
            }
        }
    }
    Ok(output.trim_end_matches('\n').to_string())
}

fn execute_find(
    args: &Value,
    workspace: &Path,
    control: &ToolControl<'_>,
) -> Result<String, String> {
    let root = optional_workspace_path(args, "path", workspace)?;
    let pattern = args.get("pattern").and_then(Value::as_str).unwrap_or("");
    let limit = args
        .get("limit")
        .and_then(Value::as_u64)
        .unwrap_or(200)
        .clamp(1, 2_000) as usize;
    let mut results = Vec::new();
    for path in walk_paths(&root, workspace, control)? {
        check_cancelled(control)?;
        let relative = relative_display(workspace, &path);
        let name = path
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or("");
        if pattern.is_empty() || name.contains(pattern) || relative.contains(pattern) {
            results.push(relative);
            if results.len() >= limit {
                break;
            }
        }
    }
    Ok(results.join("\n"))
}

fn execute_ls(args: &Value, workspace: &Path, control: &ToolControl<'_>) -> Result<String, String> {
    let path = optional_workspace_path(args, "path", workspace)?;
    let mut entries = Vec::new();
    for entry in fs::read_dir(&path).map_err(|error| error.to_string())? {
        check_cancelled(control)?;
        let entry = entry.map_err(|error| error.to_string())?;
        entries.push(entry);
        if entries.len() > MAX_DIRECTORY_ENTRIES {
            return Err(format!(
                "directory contains more than {MAX_DIRECTORY_ENTRIES} entries"
            ));
        }
    }
    entries.sort_by_key(|entry| entry.file_name());
    let mut output = Vec::new();
    for entry in entries {
        check_cancelled(control)?;
        let metadata = entry.metadata().map_err(|error| error.to_string())?;
        let kind = if metadata.is_dir() { "dir" } else { "file" };
        output.push(format!(
            "{kind}\t{}\t{}",
            metadata.len(),
            entry.file_name().to_string_lossy()
        ));
    }
    Ok(output.join("\n"))
}

fn optional_workspace_path(args: &Value, name: &str, workspace: &Path) -> Result<PathBuf, String> {
    match args.get(name).and_then(Value::as_str) {
        Some(value) if !value.is_empty() => workspace_path(workspace, value, false),
        _ => workspace.canonicalize().map_err(|error| error.to_string()),
    }
}

fn walk_paths(
    root: &Path,
    workspace: &Path,
    control: &ToolControl<'_>,
) -> Result<Vec<PathBuf>, String> {
    let workspace = workspace
        .canonicalize()
        .map_err(|error| error.to_string())?;
    let root = root.canonicalize().map_err(|error| error.to_string())?;
    if !root.starts_with(&workspace) {
        return Err("path escapes the current workspace".into());
    }
    let mut pending = vec![root.clone()];
    let mut visited = std::collections::BTreeSet::from([root.clone()]);
    let mut result = Vec::new();
    while let Some(path) = pending.pop() {
        check_cancelled(control)?;
        if should_skip_path(&path, &root) {
            continue;
        }
        result.push(path.clone());
        if result.len() > MAX_DISCOVERED_PATHS {
            return Err(format!(
                "workspace traversal exceeded {MAX_DISCOVERED_PATHS} paths"
            ));
        }
        if !path.is_dir() {
            continue;
        }
        let mut children = fs::read_dir(&path)
            .map_err(|error| error.to_string())?
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .collect::<Vec<_>>();
        children.sort();
        for child in children.into_iter().rev() {
            check_cancelled(control)?;
            let Ok(canonical) = child.canonicalize() else {
                continue;
            };
            if canonical.starts_with(&workspace) && visited.insert(canonical.clone()) {
                pending.push(canonical);
            }
        }
    }
    Ok(result)
}

fn should_skip_path(path: &Path, root: &Path) -> bool {
    if path == root {
        return false;
    }
    path.strip_prefix(root)
        .unwrap_or(path)
        .components()
        .any(|component| {
            component
                .as_os_str()
                .to_str()
                .is_some_and(|name| matches!(name, ".git" | "target" | "node_modules"))
        })
}

fn relative_display(workspace: &Path, path: &Path) -> String {
    path.strip_prefix(workspace)
        .unwrap_or(path)
        .display()
        .to_string()
}

fn limit_lines(text: &str, limit: usize) -> String {
    text.lines().take(limit).collect::<Vec<_>>().join("\n")
}

fn required_str<'a>(args: &'a Value, name: &str) -> Result<&'a str, String> {
    args.get(name)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("missing string argument: {name}"))
}

fn check_cancelled(control: &ToolControl<'_>) -> Result<(), String> {
    if control.cancel.load(Ordering::Relaxed) {
        Err("tool execution cancelled".into())
    } else if Instant::now() >= control.deadline {
        Err(format!(
            "tool execution timed out after {} seconds",
            TOOL_TIMEOUT.as_secs()
        ))
    } else {
        Ok(())
    }
}

fn read_file_lines(
    path: &Path,
    offset: usize,
    limit: usize,
    control: &ToolControl<'_>,
) -> Result<String, String> {
    let file = fs::File::open(path).map_err(|error| error.to_string())?;
    let mut reader = BufReader::new(file);
    let mut line = String::new();
    let mut line_number = 0;
    let mut scanned = 0_u64;
    let mut output = String::new();
    let mut returned = 0;
    let end_line = offset.saturating_add(limit.saturating_sub(1));
    while returned < limit {
        check_cancelled(control)?;
        line.clear();
        let read = reader
            .by_ref()
            .take((MAX_LINE_BYTES + 1) as u64)
            .read_line(&mut line)
            .map_err(|error| error.to_string())?;
        if read == 0 {
            break;
        }
        if read > MAX_LINE_BYTES {
            return Err(format!(
                "a read line exceeds the {} MiB limit",
                MAX_LINE_BYTES / (1024 * 1024)
            ));
        }
        scanned = scanned.saturating_add(read as u64);
        if scanned > MAX_FILE_SCAN_BYTES {
            return Err(format!(
                "read scan exceeded the {} MiB limit",
                MAX_FILE_SCAN_BYTES / (1024 * 1024)
            ));
        }
        line_number += 1;
        if line_number < offset {
            continue;
        }
        let text = line.trim_end_matches(['\r', '\n']);
        let formatted = format!("{line_number}: {text}\n");
        if output.len() + formatted.len() > MAX_TOOL_OUTPUT_BYTES {
            return Err("read output exceeded the 64 KiB limit".into());
        }
        output.push_str(&formatted);
        returned += 1;
        if line_number >= end_line {
            break;
        }
    }
    Ok(output.trim_end_matches('\n').to_string())
}

fn read_file_string(path: &Path, control: &ToolControl<'_>) -> Result<String, String> {
    let metadata = fs::metadata(path).map_err(|error| error.to_string())?;
    if metadata.len() > MAX_FILE_BYTES {
        return Err(format!(
            "file exceeds the {} MiB limit",
            MAX_FILE_BYTES / (1024 * 1024)
        ));
    }
    let mut file = fs::File::open(path).map_err(|error| error.to_string())?;
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    let mut chunk = [0_u8; 32 * 1024];
    loop {
        check_cancelled(control)?;
        let read = file.read(&mut chunk).map_err(|error| error.to_string())?;
        if read == 0 {
            break;
        }
        if bytes.len().saturating_add(read) as u64 > MAX_FILE_BYTES {
            return Err(format!(
                "file exceeds the {} MiB limit",
                MAX_FILE_BYTES / (1024 * 1024)
            ));
        }
        bytes.extend_from_slice(&chunk[..read]);
    }
    String::from_utf8(bytes).map_err(|error| format!("file is not valid UTF-8: {error}"))
}

struct CommandOutput {
    status: ExitStatus,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

fn format_command_output(output: &CommandOutput) -> String {
    let mut text = String::from_utf8_lossy(&output.stdout).into_owned();
    if !output.stderr.is_empty() {
        if !text.is_empty() {
            text.push('\n');
        }
        text.push_str(&String::from_utf8_lossy(&output.stderr));
    }
    format!("exit status: {}\n{text}", output.status)
}

fn shell_command(command: &str, workspace: &Path) -> Command {
    let mut process = if cfg!(windows) {
        let mut process = Command::new("powershell");
        process.args(["-NoProfile", "-Command", command]);
        process
    } else {
        let mut process = Command::new("sh");
        process.args(["-lc", command]);
        process
    };
    process.current_dir(workspace);
    process
}

fn run_bounded_command(
    process: &mut Command,
    control: &ToolControl<'_>,
) -> Result<CommandOutput, String> {
    process
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(unix)]
    process.process_group(0);
    let mut child = process
        .spawn()
        .map_err(|error| format!("failed to spawn command: {error}"))?;
    let stdout = match child.stdout.take() {
        Some(stdout) => stdout,
        None => {
            kill_child(&mut child);
            let _ = child.wait();
            return Err("command stdout was not captured".into());
        }
    };
    let stderr = match child.stderr.take() {
        Some(stderr) => stderr,
        None => {
            kill_child(&mut child);
            let _ = child.wait();
            return Err("command stderr was not captured".into());
        }
    };
    let buffer = Arc::new(Mutex::new(CommandOutputBuffer::default()));
    let bytes = Arc::new(AtomicUsize::new(0));
    let exceeded = Arc::new(AtomicBool::new(false));
    let stdout_thread = spawn_output_reader(
        stdout,
        OutputStream::Stdout,
        buffer.clone(),
        bytes.clone(),
        exceeded.clone(),
    );
    let stderr_thread = spawn_output_reader(
        stderr,
        OutputStream::Stderr,
        buffer.clone(),
        bytes.clone(),
        exceeded.clone(),
    );
    let status = loop {
        if control.cancel.load(Ordering::Relaxed) {
            kill_child(&mut child);
            let _ = child.wait();
            join_output_reader(stdout_thread);
            join_output_reader(stderr_thread);
            return Err("tool execution cancelled".into());
        }
        if exceeded.load(Ordering::Relaxed) {
            kill_child(&mut child);
            let _ = child.wait();
            join_output_reader(stdout_thread);
            join_output_reader(stderr_thread);
            return Err(format!(
                "command output exceeded the {} KiB limit",
                MAX_TOOL_OUTPUT_BYTES / 1024
            ));
        }
        if Instant::now() >= control.deadline {
            kill_child(&mut child);
            let _ = child.wait();
            join_output_reader(stdout_thread);
            join_output_reader(stderr_thread);
            return Err(format!(
                "command timed out after {} seconds",
                TOOL_TIMEOUT.as_secs()
            ));
        }
        match child.try_wait() {
            Ok(Some(status)) => {
                // A shell can exit while a background descendant still owns
                // stdout or stderr. Close that process group before joining
                // the readers so the tool cannot wait forever for EOF.
                kill_child(&mut child);
                break status;
            }
            Ok(None) => {}
            Err(error) => {
                kill_child(&mut child);
                let _ = child.wait();
                join_output_reader(stdout_thread);
                join_output_reader(stderr_thread);
                return Err(error.to_string());
            }
        }
        thread::sleep(Duration::from_millis(20));
    };
    join_output_reader(stdout_thread);
    join_output_reader(stderr_thread);
    let output = buffer
        .lock()
        .map_err(|_| "command output lock was poisoned".to_string())?
        .clone();
    Ok(CommandOutput {
        status,
        stdout: output.stdout,
        stderr: output.stderr,
    })
}

#[derive(Clone, Default)]
struct CommandOutputBuffer {
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

#[derive(Clone, Copy)]
enum OutputStream {
    Stdout,
    Stderr,
}

fn spawn_output_reader<R: Read + Send + 'static>(
    reader: R,
    stream: OutputStream,
    buffer: Arc<Mutex<CommandOutputBuffer>>,
    bytes: Arc<AtomicUsize>,
    exceeded: Arc<AtomicBool>,
) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        let mut reader = reader;
        let mut chunk = [0_u8; 8 * 1024];
        while let Ok(read) = reader.read(&mut chunk) {
            if read == 0 {
                break;
            }
            let start = bytes.fetch_add(read, Ordering::Relaxed);
            if start >= MAX_TOOL_OUTPUT_BYTES {
                exceeded.store(true, Ordering::Relaxed);
                continue;
            }
            let keep = read.min(MAX_TOOL_OUTPUT_BYTES - start);
            if keep < read {
                exceeded.store(true, Ordering::Relaxed);
            }
            if let Ok(mut output) = buffer.lock() {
                match stream {
                    OutputStream::Stdout => output.stdout.extend_from_slice(&chunk[..keep]),
                    OutputStream::Stderr => output.stderr.extend_from_slice(&chunk[..keep]),
                }
            }
        }
    })
}

fn join_output_reader(thread: thread::JoinHandle<()>) {
    let _ = thread.join();
}

fn kill_child(child: &mut Child) {
    #[cfg(unix)]
    {
        let process_group = -(child.id() as i32);
        // The child is placed in its own process group before spawn so shell
        // descendants are terminated together with the command wrapper.
        unsafe {
            libc::kill(process_group, libc::SIGKILL);
        }
    }
    let _ = child.kill();
}

fn workspace_path(workspace: &Path, value: &str, allow_missing: bool) -> Result<PathBuf, String> {
    let input = Path::new(value);
    let workspace = workspace
        .canonicalize()
        .map_err(|error| error.to_string())?;
    if !input.is_absolute()
        && input
            .components()
            .any(|part| matches!(part, Component::ParentDir))
    {
        return Err(
            "path must stay inside the current workspace; use a workspace-relative path".into(),
        );
    }
    let path = if input.is_absolute() {
        input.to_path_buf()
    } else {
        workspace.join(input)
    };
    if path.exists() {
        let canonical = path.canonicalize().map_err(|error| error.to_string())?;
        if !canonical.starts_with(&workspace) {
            return Err(
                "path must stay inside the current workspace; use a workspace-relative path".into(),
            );
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
            return Err(
                "path must stay inside the current workspace; use a workspace-relative path".into(),
            );
        }
        Ok(path)
    } else {
        Err("path does not exist".into())
    }
}

fn truncate_output(text: &str) -> String {
    if text.len() <= MAX_TOOL_OUTPUT_BYTES {
        text.to_string()
    } else {
        format!(
            "{}\n[output truncated]",
            &text[..text.floor_char_boundary(MAX_TOOL_OUTPUT_BYTES)]
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
    complete_target_with_timeout(
        target,
        api_key,
        messages,
        include_tools,
        MODEL_REQUEST_TIMEOUT,
    )
    .await
}

async fn complete_target_with_timeout(
    target: ResolvedModel<'_>,
    api_key: Option<&str>,
    messages: &[AgentMessage],
    include_tools: bool,
    timeout: Duration,
) -> Result<AgentResponse, String> {
    let request = sdk_request(target, api_key, messages, include_tools, None, false);
    let adapter = sdk::builtin_adapter(request.protocol);
    let client = sdk::Client::new(timeout).map_err(|error| error.to_string())?;
    let response = client
        .complete(adapter, &request)
        .await
        .map_err(|error| error.to_string())?;
    Ok(from_sdk_response(response))
}

pub async fn complete_stream(
    settings: &AgentSettings,
    api_key: Option<&str>,
    messages: &[AgentMessage],
    on_event: impl FnMut(&AgentEvent),
) -> Result<AgentResponse, String> {
    complete_stream_with_options(settings, api_key, messages, None, on_event).await
}

pub async fn complete_stream_with_options(
    settings: &AgentSettings,
    api_key: Option<&str>,
    messages: &[AgentMessage],
    thinking: Option<AgentThinkingLevel>,
    mut on_event: impl FnMut(&AgentEvent),
) -> Result<AgentResponse, String> {
    let target = settings
        .resolve(&settings.active_model)
        .map_err(str::to_string)?;
    let thinking = thinking.filter(|_| target.model.reasoning);
    let request = sdk_request(target, api_key, messages, true, thinking, true);
    let adapter = sdk::builtin_adapter(request.protocol);
    let client = sdk::Client::new(MODEL_REQUEST_TIMEOUT).map_err(|error| error.to_string())?;
    let response = client
        .stream(adapter, &request, |event| {
            let event = from_sdk_event(event);
            on_event(&event);
        })
        .await
        .map_err(|error| error.to_string())?;
    Ok(from_sdk_response(response))
}

fn sdk_request(
    target: ResolvedModel<'_>,
    api_key: Option<&str>,
    messages: &[AgentMessage],
    include_tools: bool,
    thinking: Option<AgentThinkingLevel>,
    stream: bool,
) -> sdk::CompletionRequest {
    let protocol = to_sdk_protocol(target.provider.protocol);
    let tools = sdk_tool_definitions();
    let mut request = sdk::CompletionRequest::new(
        protocol,
        target.provider.url.clone(),
        target.model.id.clone(),
        target.model.max_tokens,
        messages.iter().map(to_sdk_message).collect(),
        tools,
    );
    request.api_key = api_key.map(str::to_owned);
    request.reasoning = target.model.reasoning;
    request.thinking = thinking.map(to_sdk_thinking);
    request.include_tools = include_tools;
    request.stream = stream;
    request
}

fn to_sdk_protocol(protocol: AgentProtocol) -> sdk::Protocol {
    match protocol {
        AgentProtocol::OpenAiChat => sdk::Protocol::OpenAiChat,
        AgentProtocol::OpenAiResponses => sdk::Protocol::OpenAiResponses,
        AgentProtocol::AnthropicMessages => sdk::Protocol::AnthropicMessages,
    }
}

fn to_sdk_thinking(thinking: AgentThinkingLevel) -> sdk::ThinkingLevel {
    match thinking {
        AgentThinkingLevel::Off => sdk::ThinkingLevel::Off,
        AgentThinkingLevel::Minimal => sdk::ThinkingLevel::Minimal,
        AgentThinkingLevel::Low => sdk::ThinkingLevel::Low,
        AgentThinkingLevel::Medium => sdk::ThinkingLevel::Medium,
        AgentThinkingLevel::High => sdk::ThinkingLevel::High,
        AgentThinkingLevel::XHigh => sdk::ThinkingLevel::XHigh,
    }
}

fn to_sdk_message(message: &AgentMessage) -> sdk::Message {
    sdk::Message {
        role: match message.role {
            AgentRole::System => sdk::Role::System,
            AgentRole::User => sdk::Role::User,
            AgentRole::Assistant => sdk::Role::Assistant,
        },
        text: message.text.clone(),
        tool_calls: message
            .tool_calls
            .iter()
            .map(|call| sdk::ToolCall {
                id: call.id.clone(),
                name: call.name.clone(),
                arguments: call.arguments.clone(),
            })
            .collect(),
        tool_result: message.tool_result.as_ref().map(|result| sdk::ToolResult {
            call_id: result.call_id.clone(),
            output: result.output.clone(),
            is_error: result.is_error,
        }),
        protocol_items: message.protocol_items.clone(),
    }
}

fn from_sdk_response(response: sdk::Response) -> AgentResponse {
    AgentResponse {
        content: response
            .content
            .into_iter()
            .map(|block| match block {
                sdk::ContentBlock::Text(text) => AgentContentBlock::Text(text),
                sdk::ContentBlock::Reasoning(text) => AgentContentBlock::Reasoning(text),
                sdk::ContentBlock::ToolCall(call) => AgentContentBlock::ToolCall(AgentToolCall {
                    id: call.id,
                    name: call.name,
                    arguments: call.arguments,
                }),
            })
            .collect(),
        protocol_items: response.protocol_items,
    }
}

fn from_sdk_event(event: &sdk::Event) -> AgentEvent {
    match event {
        sdk::Event::TextDelta(delta) => AgentEvent::TextDelta(delta.clone()),
        sdk::Event::ReasoningDelta(delta) => AgentEvent::ReasoningDelta(delta.clone()),
        sdk::Event::ToolCallStart { index, id, name } => AgentEvent::ToolCallStart {
            index: *index,
            id: id.clone(),
            name: name.clone(),
        },
        sdk::Event::ToolCallArgumentsDelta { index, delta } => AgentEvent::ToolCallArgumentsDelta {
            index: *index,
            delta: delta.clone(),
        },
        sdk::Event::Stop(reason) => AgentEvent::Stop(reason.clone()),
    }
}

#[cfg(test)]
#[derive(Default)]
struct Utf8StreamDecoder {
    bytes: Vec<u8>,
}

#[cfg(test)]
impl Utf8StreamDecoder {
    fn push(&mut self, chunk: &[u8]) -> String {
        self.bytes.extend_from_slice(chunk);
        match std::str::from_utf8(&self.bytes) {
            Ok(text) => {
                let text = text.to_string();
                self.bytes.clear();
                text
            }
            Err(error) if error.error_len().is_none() => {
                let valid = error.valid_up_to();
                let text = String::from_utf8_lossy(&self.bytes[..valid]).into_owned();
                self.bytes.drain(..valid);
                text
            }
            Err(_) => {
                let text = String::from_utf8_lossy(&self.bytes).into_owned();
                self.bytes.clear();
                text
            }
        }
    }

    fn finish(self) -> String {
        String::from_utf8_lossy(&self.bytes).into_owned()
    }
}

/// Compatibility accumulator retained for callers of the original
/// `crossh-agent` protocol helpers. The implementation lives in the SDK.
#[cfg(test)]
#[derive(Default)]
struct StreamAccumulator {
    inner: sdk::StreamAccumulator,
    protocol: AgentProtocol,
}

#[cfg(test)]
impl StreamAccumulator {
    fn set_protocol(&mut self, protocol: AgentProtocol) {
        if self.protocol != protocol {
            self.protocol = protocol;
            self.inner.set_protocol(to_sdk_protocol(protocol));
        }
    }

    fn capture_protocol_event(&mut self, protocol: AgentProtocol, event: &Value) {
        self.set_protocol(protocol);
        sdk::builtin_adapter(to_sdk_protocol(protocol))
            .capture_stream_event(&mut self.inner, event);
    }

    fn push(&mut self, event: &AgentEvent) {
        self.inner.push(&to_sdk_event(event));
    }

    fn finish(mut self, protocol: AgentProtocol) -> Result<AgentResponse, String> {
        self.set_protocol(protocol);
        Ok(from_sdk_response(
            self.inner.finish().map_err(|error| error.to_string())?,
        ))
    }
}

#[cfg(test)]
fn to_sdk_event(event: &AgentEvent) -> sdk::Event {
    match event {
        AgentEvent::TextDelta(delta) => sdk::Event::TextDelta(delta.clone()),
        AgentEvent::ReasoningDelta(delta) => sdk::Event::ReasoningDelta(delta.clone()),
        AgentEvent::ToolCallStart { index, id, name } => sdk::Event::ToolCallStart {
            index: *index,
            id: id.clone(),
            name: name.clone(),
        },
        AgentEvent::ToolCallArgumentsDelta { index, delta } => sdk::Event::ToolCallArgumentsDelta {
            index: *index,
            delta: delta.clone(),
        },
        AgentEvent::Stop(reason) => sdk::Event::Stop(reason.clone()),
    }
}

fn sdk_tool_definitions() -> Vec<sdk::ToolDefinition> {
    builtin_tools()
        .into_iter()
        .map(|tool| {
            sdk::ToolDefinition::new(
                tool.name,
                tool.description,
                tool.input_schema,
                tool.requires_approval,
            )
        })
        .collect()
}

fn sdk_request_for_messages(
    protocol: AgentProtocol,
    model: &str,
    messages: &[AgentMessage],
    max_tokens: u32,
) -> sdk::CompletionRequest {
    sdk::CompletionRequest::new(
        to_sdk_protocol(protocol),
        "",
        model,
        max_tokens,
        messages.iter().map(to_sdk_message).collect(),
        sdk_tool_definitions(),
    )
}

pub fn encode_request(
    protocol: AgentProtocol,
    model: &str,
    messages: &[AgentMessage],
) -> AgentWireRequest {
    let request = sdk_request_for_messages(protocol, model, messages, 4_096);
    let wire = sdk::builtin_adapter(request.protocol)
        .encode_request(&request)
        .expect("built-in adapter request should be valid");
    AgentWireRequest {
        body: wire.body,
        auth_style: match wire.auth_style {
            sdk::AuthStyle::Anthropic => AgentAuthStyle::Anthropic,
            sdk::AuthStyle::Bearer | sdk::AuthStyle::None => AgentAuthStyle::Bearer,
        },
    }
}

#[cfg(test)]
fn apply_model_options(body: &mut Value, protocol: AgentProtocol, model: &AgentModel) {
    let key = match protocol {
        AgentProtocol::OpenAiChat | AgentProtocol::AnthropicMessages => "max_tokens",
        AgentProtocol::OpenAiResponses => "max_output_tokens",
    };
    body[key] = Value::from(model.max_tokens);
    if protocol == AgentProtocol::OpenAiResponses && model.reasoning {
        body["reasoning"] = json!({"summary": "auto"});
    }
}

#[cfg(test)]
fn apply_thinking_option(
    body: &mut Value,
    protocol: AgentProtocol,
    model: &AgentModel,
    thinking: AgentThinkingLevel,
) {
    apply_model_options(body, protocol, model);
    match protocol {
        AgentProtocol::OpenAiChat => {
            if thinking == AgentThinkingLevel::Off {
                body.as_object_mut()
                    .map(|body| body.remove("reasoning_effort"));
            } else {
                let effort = match thinking {
                    AgentThinkingLevel::XHigh => "high",
                    other => other.label(),
                };
                body["reasoning_effort"] = Value::from(effort);
            }
        }
        AgentProtocol::OpenAiResponses => {
            if thinking == AgentThinkingLevel::Off {
                body.as_object_mut().map(|body| body.remove("reasoning"));
            } else {
                let effort = match thinking {
                    AgentThinkingLevel::XHigh => "high",
                    other => other.label(),
                };
                body["reasoning"] = json!({"effort": effort, "summary": "auto"});
            }
        }
        AgentProtocol::AnthropicMessages => {
            body["thinking"] = if thinking == AgentThinkingLevel::Off {
                json!({"type":"disabled"})
            } else {
                json!({"type":"enabled","budget_tokens":thinking.budget(model.max_tokens)})
            };
        }
    }
}

pub fn decode_response(
    protocol: AgentProtocol,
    body: &Value,
) -> Result<AgentResponse, &'static str> {
    let response = sdk::builtin_adapter(to_sdk_protocol(protocol))
        .decode_response(body)
        .map_err(|_| "protocol response did not contain text or reasoning content")?;
    Ok(from_sdk_response(response))
}

/// Normalize one decoded SSE `data:` object. Tool argument fragments remain
/// fragments and are assembled by the agent loop using their stable index.
pub fn decode_stream_event(protocol: AgentProtocol, event: &Value) -> Vec<AgentEvent> {
    sdk::builtin_adapter(to_sdk_protocol(protocol))
        .decode_stream_event(event)
        .iter()
        .map(from_sdk_event)
        .collect()
}

#[cfg(test)]
fn wire_messages(protocol: AgentProtocol, messages: &[AgentMessage]) -> Vec<Value> {
    let request = sdk_request_for_messages(protocol, "model", messages, 4_096);
    let body = sdk::builtin_adapter(request.protocol)
        .encode_request(&request)
        .expect("built-in adapter request should be valid")
        .body;
    let key = match protocol {
        AgentProtocol::OpenAiResponses => "input",
        AgentProtocol::OpenAiChat | AgentProtocol::AnthropicMessages => "messages",
    };
    body.get(key)
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn configured_settings() -> AgentSettings {
        let provider = AgentProvider {
            id: "local".into(),
            name: "Local".into(),
            protocol: AgentProtocol::OpenAiChat,
            url: "http://127.0.0.1:11434/v1/chat/completions".into(),
            api_key_env: String::new(),
            api_key: String::new(),
            models: vec![AgentModel {
                id: "qwen3-coder".into(),
                name: "qwen3-coder".into(),
                reasoning: true,
                context_window: 128_000,
                max_tokens: 32_000,
            }],
        };
        AgentSettings {
            providers: vec![provider],
            active_model: AgentModelRef {
                provider: "local".into(),
                model: "qwen3-coder".into(),
            },
            reviewer_model: AgentModelRef {
                provider: "local".into(),
                model: "qwen3-coder".into(),
            },
            ..AgentSettings::default()
        }
    }

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
    fn empty_default_settings_are_valid_without_model_references() {
        let settings = AgentSettings::default();
        assert!(settings.providers.is_empty());
        assert_eq!(settings.active_model, AgentModelRef::default());
        assert_eq!(settings.reviewer_model, AgentModelRef::default());
        assert_eq!(settings.validate(), Ok(()));
        assert!(matches!(
            settings.resolve(&settings.active_model),
            Err("Provider not found")
        ));
    }

    #[test]
    fn multi_provider_models_resolve_independently() {
        let mut settings = configured_settings();
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
        let mut settings = configured_settings();
        settings.providers.push(settings.providers[0].clone());
        assert_eq!(settings.validate(), Err("Provider IDs must be unique"));

        let mut settings = configured_settings();
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
    fn strict_tool_schemas_require_every_declared_property() {
        for tool in builtin_tools() {
            let properties = tool.input_schema["properties"]
                .as_object()
                .expect("tool schema properties");
            let required = tool.input_schema["required"]
                .as_array()
                .expect("strict tool schema required list");
            assert_eq!(required.len(), properties.len(), "{}", tool.name);
            assert_eq!(tool.input_schema["additionalProperties"], false);
        }
    }

    #[test]
    fn responses_replay_original_output_items() {
        let raw = vec![
            json!({"type":"reasoning","summary":[{"type":"summary_text","text":"think"}],"id":"rs_1"}),
            json!({"type":"function_call","call_id":"call_1","name":"read","arguments":"{\"path\":\"README.md\"}"}),
        ];
        let message = AgentMessage {
            role: AgentRole::Assistant,
            text: "".into(),
            tool_calls: vec![],
            tool_result: None,
            protocol_items: raw.clone(),
        };
        assert_eq!(
            wire_messages(AgentProtocol::OpenAiResponses, &[message]),
            raw
        );
    }

    #[test]
    fn streamed_responses_capture_completed_output_items() {
        let mut accumulator = StreamAccumulator::default();
        let item = json!({
            "id":"rs_1",
            "type":"reasoning",
            "summary":[{"type":"summary_text","text":"think"}]
        });
        accumulator.capture_protocol_event(
            AgentProtocol::OpenAiResponses,
            &json!({"type":"response.output_item.done","output_index":0,"item":item}),
        );
        accumulator.push(&AgentEvent::ReasoningDelta("think".into()));
        let response = accumulator.finish(AgentProtocol::OpenAiResponses).unwrap();
        assert_eq!(response.protocol_items, vec![item]);
    }

    #[test]
    fn responses_tool_events_keep_the_final_call_and_arguments() {
        let mut accumulator = StreamAccumulator::default();
        let events = [
            json!({
                "type":"response.output_item.added",
                "output_index":0,
                "item":{
                    "type":"function_call",
                    "id":"fc_1",
                    "call_id":"call_1",
                    "name":"read",
                    "arguments":""
                }
            }),
            json!({
                "type":"response.function_call_arguments.delta",
                "output_index":0,
                "delta":"{\"path\":\"README.md\"}"
            }),
            json!({
                "type":"response.function_call_arguments.done",
                "output_index":0,
                "arguments":"{\"path\":\"README.md\"}"
            }),
            json!({
                "type":"response.output_item.done",
                "output_index":0,
                "item":{
                    "type":"function_call",
                    "id":"fc_1",
                    "call_id":"call_1",
                    "name":"read",
                    "arguments":"{\"path\":\"README.md\"}"
                }
            }),
        ];
        for event in events {
            accumulator.capture_protocol_event(AgentProtocol::OpenAiResponses, &event);
            for decoded in decode_stream_event(AgentProtocol::OpenAiResponses, &event) {
                accumulator.push(&decoded);
            }
        }

        let response = accumulator.finish(AgentProtocol::OpenAiResponses).unwrap();
        assert_eq!(
            response.content,
            vec![AgentContentBlock::ToolCall(AgentToolCall {
                id: "call_1".into(),
                name: "read".into(),
                arguments: r#"{"path":"README.md"}"#.into(),
            })]
        );
    }

    #[test]
    fn responses_output_item_done_can_create_a_tool_call() {
        let mut accumulator = StreamAccumulator::default();
        let event = json!({
            "type":"response.output_item.done",
            "output_index":2,
            "item":{
                "type":"function_call",
                "id":"fc_2",
                "call_id":"call_2",
                "name":"ls",
                "arguments":"{\"path\":null}"
            }
        });
        accumulator.capture_protocol_event(AgentProtocol::OpenAiResponses, &event);

        let response = accumulator.finish(AgentProtocol::OpenAiResponses).unwrap();
        assert_eq!(
            response.content,
            vec![AgentContentBlock::ToolCall(AgentToolCall {
                id: "call_2".into(),
                name: "ls".into(),
                arguments: r#"{"path":null}"#.into(),
            })]
        );
    }

    #[test]
    fn adapters_decode_protocol_responses() {
        assert_eq!(
            decode_response(
                AgentProtocol::OpenAiChat,
                &json!({"choices":[{"message":{"content":"a"}}]})
            ),
            Ok(AgentResponse {
                content: vec![AgentContentBlock::Text("a".into())],
                protocol_items: Vec::new()
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
                ],
                protocol_items: vec![
                    json!({"type":"reasoning","summary":[{"type":"summary_text","text":"think b"}]}),
                    json!({"type":"message","content":[{"type":"output_text","text":"b"}]})
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
                ],
                protocol_items: Vec::new()
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

        let absolute = execute_tool(
            &AgentToolCall {
                id: "absolute".into(),
                name: "read".into(),
                arguments: json!({
                    "path": workspace.path().join("notes.txt").to_string_lossy()
                })
                .to_string(),
            },
            workspace.path(),
        );
        assert!(!absolute.is_error);
        assert_eq!(absolute.output, "1: one\n2: two\n3: three");

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

    #[test]
    fn discovery_tools_search_and_list_without_leaving_workspace() {
        let workspace = tempfile::tempdir().unwrap();
        fs::create_dir(workspace.path().join("src")).unwrap();
        fs::write(
            workspace.path().join("src/main.rs"),
            "fn main() {\n    println!(\"needle\");\n}\n",
        )
        .unwrap();
        fs::write(workspace.path().join("README.md"), "needle\n").unwrap();

        let grep = execute_tool(
            &AgentToolCall {
                id: "grep".into(),
                name: "grep".into(),
                arguments: r#"{"pattern":"needle"}"#.into(),
            },
            workspace.path(),
        );
        assert!(!grep.is_error);
        assert!(grep.output.contains("README.md"));
        assert!(grep.output.contains("main.rs"));

        let find = execute_tool(
            &AgentToolCall {
                id: "find".into(),
                name: "find".into(),
                arguments: r#"{"pattern":"main"}"#.into(),
            },
            workspace.path(),
        );
        assert!(!find.is_error);
        assert!(find.output.contains("main.rs"));

        let listing = execute_tool(
            &AgentToolCall {
                id: "ls".into(),
                name: "ls".into(),
                arguments: r#"{"path":"src"}"#.into(),
            },
            workspace.path(),
        );
        assert!(!listing.is_error);
        assert!(listing.output.contains("main.rs"));
    }

    #[test]
    fn explicit_thinking_options_map_to_provider_wire_fields() {
        let model = AgentModel {
            id: "reasoning".into(),
            name: "Reasoning".into(),
            reasoning: true,
            context_window: 10_000,
            max_tokens: 4_000,
        };
        let mut chat = encode_request(AgentProtocol::OpenAiChat, &model.id, &messages()).body;
        apply_thinking_option(
            &mut chat,
            AgentProtocol::OpenAiChat,
            &model,
            AgentThinkingLevel::High,
        );
        assert_eq!(chat["reasoning_effort"], "high");

        let mut responses =
            encode_request(AgentProtocol::OpenAiResponses, &model.id, &messages()).body;
        apply_thinking_option(
            &mut responses,
            AgentProtocol::OpenAiResponses,
            &model,
            AgentThinkingLevel::High,
        );
        assert_eq!(responses["reasoning"]["effort"], "high");
        assert!(responses.get("reasoning_effort").is_none());

        let mut anthropic =
            encode_request(AgentProtocol::AnthropicMessages, &model.id, &messages()).body;
        apply_thinking_option(
            &mut anthropic,
            AgentProtocol::AnthropicMessages,
            &model,
            AgentThinkingLevel::Low,
        );
        assert_eq!(anthropic["thinking"]["type"], "enabled");
        assert!(
            anthropic["thinking"]["budget_tokens"]
                .as_u64()
                .is_some_and(|budget| budget < model.max_tokens as u64)
        );
    }

    #[test]
    fn utf8_stream_decoder_waits_for_split_codepoints() {
        let mut decoder = Utf8StreamDecoder::default();
        let bytes = "中".as_bytes();
        assert_eq!(decoder.push(&bytes[..1]), "");
        assert_eq!(decoder.push(&bytes[1..]), "中");
        assert_eq!(decoder.finish(), "");
    }

    #[test]
    fn fallback_grep_uses_regular_expressions() {
        let workspace = tempfile::tempdir().unwrap();
        fs::write(
            workspace.path().join("notes.txt"),
            "foo and bar\nfoo only\n",
        )
        .unwrap();
        fs::write(
            workspace.path().join("zz-large.txt"),
            format!("foo{}bar\n", "x".repeat(MAX_TOOL_OUTPUT_BYTES)),
        )
        .unwrap();
        let cancel = AtomicBool::new(false);
        let control = ToolControl::new(&cancel);
        let result =
            grep_without_rg("foo.*bar", workspace.path(), workspace.path(), 10, &control).unwrap();
        assert!(result.contains("foo and bar"));
        assert!(!result.contains("foo only"));
        assert!(result.len() <= MAX_TOOL_OUTPUT_BYTES);
        assert!(result.contains("output truncated"));
    }

    #[test]
    fn cancelled_tool_does_not_start_a_shell() {
        let workspace = tempfile::tempdir().unwrap();
        let cancel = AtomicBool::new(true);
        let result = execute_tool_with_cancel(
            &AgentToolCall {
                id: "1".into(),
                name: "bash".into(),
                arguments: r#"{"command":"sleep 60"}"#.into(),
            },
            workspace.path(),
            &cancel,
        );
        assert!(result.is_error);
        assert!(result.output.contains("cancelled"));
    }

    #[cfg(unix)]
    #[test]
    fn cancelling_running_shell_terminates_the_process_group() {
        let workspace = tempfile::tempdir().unwrap();
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = cancel.clone();
        let worker = std::thread::spawn(move || {
            execute_tool_with_cancel(
                &AgentToolCall {
                    id: "1".into(),
                    name: "bash".into(),
                    arguments: r#"{"command":"sleep 60"}"#.into(),
                },
                workspace.path(),
                &worker_cancel,
            )
        });
        std::thread::sleep(Duration::from_millis(100));
        cancel.store(true, Ordering::Relaxed);
        let result = worker.join().unwrap();
        assert!(result.is_error);
        assert!(result.output.contains("cancelled"));
    }

    #[cfg(unix)]
    #[test]
    fn discovery_does_not_follow_symlink_cycles() {
        use std::os::unix::fs::symlink;
        let workspace = tempfile::tempdir().unwrap();
        fs::write(workspace.path().join("file.txt"), "content").unwrap();
        symlink(".", workspace.path().join("loop")).unwrap();
        let result = execute_tool(
            &AgentToolCall {
                id: "find".into(),
                name: "find".into(),
                arguments: r#"{"pattern":"file"}"#.into(),
            },
            workspace.path(),
        );
        assert!(!result.is_error);
        assert!(result.output.contains("file.txt"));
    }
}
