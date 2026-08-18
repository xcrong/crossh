//! A small provider-neutral SDK for agent protocols.
//!
//! The SDK owns the wire boundary: canonical messages are encoded by a
//! [`ProviderAdapter`], HTTP and SSE are handled by [`Client`], and streamed
//! text, reasoning summaries, and tool calls are normalized into one event
//! model. Agent loops and tool permissions belong in the application crate.

use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::error::Error;
use std::fmt;
use std::time::Duration;

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq, Hash)]
pub enum Protocol {
    #[default]
    #[serde(rename = "openai-chat")]
    OpenAiChat,
    #[serde(rename = "openai-responses")]
    OpenAiResponses,
    #[serde(rename = "anthropic-messages")]
    AnthropicMessages,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
pub enum ThinkingLevel {
    Off,
    Minimal,
    Low,
    #[default]
    Medium,
    High,
    XHigh,
}

impl ThinkingLevel {
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

    pub fn budget(self, max_tokens: u32) -> u32 {
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

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub enum Role {
    System,
    User,
    Assistant,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct Message {
    pub role: Role,
    pub text: String,
    pub tool_calls: Vec<ToolCall>,
    pub tool_result: Option<ToolResult>,
    /// Raw provider output items needed to replay a Responses turn.
    #[serde(default)]
    pub protocol_items: Vec<Value>,
}

impl Message {
    pub fn new(role: Role, text: impl Into<String>) -> Self {
        Self {
            role,
            text: text.into(),
            tool_calls: Vec::new(),
            tool_result: None,
            protocol_items: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct ToolResult {
    pub call_id: String,
    pub output: String,
    pub is_error: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub enum ContentBlock {
    Text(String),
    Reasoning(String),
    ToolCall(ToolCall),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
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
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
    pub requires_approval: bool,
}

impl ToolDefinition {
    pub fn new(
        name: impl Into<String>,
        description: impl Into<String>,
        input_schema: Value,
        requires_approval: bool,
    ) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            input_schema,
            requires_approval,
        }
    }
}

#[derive(Clone, Debug)]
pub struct CompletionRequest {
    pub protocol: Protocol,
    pub url: String,
    pub api_key: Option<String>,
    pub model: String,
    pub max_tokens: u32,
    pub reasoning: bool,
    pub thinking: Option<ThinkingLevel>,
    pub messages: Vec<Message>,
    pub tools: Vec<ToolDefinition>,
    pub include_tools: bool,
    pub stream: bool,
}

impl CompletionRequest {
    pub fn new(
        protocol: Protocol,
        url: impl Into<String>,
        model: impl Into<String>,
        max_tokens: u32,
        messages: Vec<Message>,
        tools: Vec<ToolDefinition>,
    ) -> Self {
        Self {
            protocol,
            url: url.into(),
            api_key: None,
            model: model.into(),
            max_tokens,
            reasoning: false,
            thinking: None,
            messages,
            tools,
            include_tools: true,
            stream: false,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum AuthStyle {
    #[default]
    Bearer,
    Anthropic,
    None,
}

#[derive(Clone, Debug)]
pub struct WireRequest {
    pub body: Value,
    pub auth_style: AuthStyle,
    pub headers: Vec<(String, String)>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Response {
    pub content: Vec<ContentBlock>,
    /// Raw output items are populated for OpenAI Responses when available.
    pub protocol_items: Vec<Value>,
}

#[derive(Debug)]
pub enum SdkError {
    Http(String),
    Api { status: u16, message: String },
    Decode(String),
    Invalid(String),
}

impl fmt::Display for SdkError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Http(message) => formatter.write_str(message),
            Self::Api { status, message } => write!(formatter, "HTTP {status}: {message}"),
            Self::Decode(message) | Self::Invalid(message) => formatter.write_str(message),
        }
    }
}

impl Error for SdkError {}

impl From<serde_json::Error> for SdkError {
    fn from(error: serde_json::Error) -> Self {
        Self::Decode(error.to_string())
    }
}

pub trait ProviderAdapter: Send + Sync {
    fn protocol(&self) -> Protocol;
    fn encode_request(&self, request: &CompletionRequest) -> Result<WireRequest, SdkError>;
    fn decode_response(&self, body: &Value) -> Result<Response, SdkError>;
    fn decode_stream_event(&self, event: &Value) -> Vec<Event>;

    /// Capture provider-specific output that is not represented by a delta,
    /// such as the completed Responses output array or a reasoning summary.
    fn capture_stream_event(&self, _accumulator: &mut StreamAccumulator, _event: &Value) {}
}

pub struct Client {
    http: reqwest::Client,
}

impl Client {
    pub fn new(timeout: Duration) -> Result<Self, SdkError> {
        let http = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .timeout(timeout)
            .build()
            .map_err(|error| SdkError::Http(error.to_string()))?;
        Ok(Self { http })
    }

    pub async fn complete(
        &self,
        adapter: &dyn ProviderAdapter,
        request: &CompletionRequest,
    ) -> Result<Response, SdkError> {
        let wire = adapter.encode_request(request)?;
        let response = self.send(request, &wire).await?;
        let status = response.status();
        let bytes = response
            .bytes()
            .await
            .map_err(|error| SdkError::Http(error.to_string()))?;
        let body = parse_json_or_text(&bytes);
        if !status.is_success() {
            return Err(SdkError::Api {
                status: status.as_u16(),
                message: api_error_message(&body),
            });
        }
        let body = body
            .as_value()
            .ok_or_else(|| SdkError::Decode("API response was not valid JSON".into()))?;
        adapter.decode_response(body)
    }

    pub async fn stream<F>(
        &self,
        adapter: &dyn ProviderAdapter,
        request: &CompletionRequest,
        mut on_event: F,
    ) -> Result<Response, SdkError>
    where
        F: FnMut(&Event),
    {
        let wire = adapter.encode_request(request)?;
        let response = self.send(request, &wire).await?;
        let status = response.status();
        if !status.is_success() {
            let body = response
                .text()
                .await
                .unwrap_or_else(|error| error.to_string());
            return Err(SdkError::Api {
                status: status.as_u16(),
                message: body,
            });
        }

        let is_json = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value.to_ascii_lowercase().contains("json"));
        if is_json {
            let body: Value = response
                .json()
                .await
                .map_err(|error| SdkError::Http(error.to_string()))?;
            return adapter.decode_response(&body);
        }

        let mut bytes = response.bytes_stream();
        let mut pending = String::new();
        let mut utf8 = Utf8StreamDecoder::default();
        let mut accumulator = StreamAccumulator::new(adapter.protocol());
        while let Some(chunk) = bytes.next().await {
            let chunk = chunk.map_err(|error| SdkError::Http(error.to_string()))?;
            pending.push_str(&utf8.push(&chunk));
            while let Some(newline) = pending.find('\n') {
                let line = pending[..newline].trim_end_matches('\r').to_string();
                pending.drain(..=newline);
                consume_sse_line(adapter, &mut accumulator, &line, &mut on_event)?;
            }
        }
        pending.push_str(&utf8.finish());
        if !pending.trim().is_empty() {
            consume_sse_line(
                adapter,
                &mut accumulator,
                pending.trim_end_matches('\r'),
                &mut on_event,
            )?;
        }
        accumulator.finish()
    }

    async fn send(
        &self,
        request: &CompletionRequest,
        wire: &WireRequest,
    ) -> Result<reqwest::Response, SdkError> {
        let mut builder = self.http.post(&request.url).json(&wire.body);
        if let Some(api_key) = request.api_key.as_deref() {
            builder = match wire.auth_style {
                AuthStyle::Bearer => builder.bearer_auth(api_key),
                AuthStyle::Anthropic => builder
                    .header("x-api-key", api_key)
                    .header("anthropic-version", "2023-06-01"),
                AuthStyle::None => builder,
            };
        }
        for (name, value) in &wire.headers {
            builder = builder.header(name, value);
        }
        builder
            .send()
            .await
            .map_err(|error| SdkError::Http(error.to_string()))
    }
}

enum JsonOrText {
    Json(Value),
    Text(String),
}

impl JsonOrText {
    fn as_value(&self) -> Option<&Value> {
        match self {
            Self::Json(value) => Some(value),
            Self::Text(_) => None,
        }
    }
}

fn parse_json_or_text(bytes: &[u8]) -> JsonOrText {
    match serde_json::from_slice(bytes) {
        Ok(value) => JsonOrText::Json(value),
        Err(_) => JsonOrText::Text(String::from_utf8_lossy(bytes).into_owned()),
    }
}

fn api_error_message(body: &JsonOrText) -> String {
    match body {
        JsonOrText::Json(value) => value
            .pointer("/error/message")
            .and_then(Value::as_str)
            .or_else(|| value.get("message").and_then(Value::as_str))
            .unwrap_or("API returned an error")
            .into(),
        JsonOrText::Text(text) if !text.trim().is_empty() => text.clone(),
        JsonOrText::Text(_) => "API returned an error".into(),
    }
}

fn consume_sse_line(
    adapter: &dyn ProviderAdapter,
    accumulator: &mut StreamAccumulator,
    line: &str,
    on_event: &mut impl FnMut(&Event),
) -> Result<(), SdkError> {
    let Some(data) = line.strip_prefix("data:").map(str::trim) else {
        return Ok(());
    };
    if data.is_empty() || data == "[DONE]" {
        return Ok(());
    }
    let value: Value = serde_json::from_str(data)?;
    adapter.capture_stream_event(accumulator, &value);
    for event in adapter.decode_stream_event(&value) {
        accumulator.push(&event);
        on_event(&event);
    }
    Ok(())
}

#[derive(Default)]
struct Utf8StreamDecoder {
    bytes: Vec<u8>,
}

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

#[derive(Default)]
pub struct StreamAccumulator {
    protocol: Protocol,
    text: String,
    reasoning: String,
    tools: BTreeMap<usize, ToolCall>,
    protocol_items: Vec<Value>,
}

impl StreamAccumulator {
    pub fn new(protocol: Protocol) -> Self {
        Self {
            protocol,
            ..Self::default()
        }
    }

    pub fn push(&mut self, event: &Event) {
        match event {
            Event::TextDelta(delta) => self.text.push_str(delta),
            Event::ReasoningDelta(delta) => self.reasoning.push_str(delta),
            Event::ToolCallStart { index, id, name } => {
                self.tools
                    .entry(*index)
                    .and_modify(|call| {
                        if !id.is_empty() {
                            call.id = id.clone();
                        }
                        if !name.is_empty() {
                            call.name = name.clone();
                        }
                    })
                    .or_insert_with(|| ToolCall {
                        id: id.clone(),
                        name: name.clone(),
                        arguments: String::new(),
                    });
            }
            Event::ToolCallArgumentsDelta { index, delta } => {
                self.tools
                    .entry(*index)
                    .or_insert_with(|| ToolCall {
                        id: String::new(),
                        name: String::new(),
                        arguments: String::new(),
                    })
                    .arguments
                    .push_str(delta);
            }
            Event::Stop(_) => {}
        }
    }

    pub fn set_protocol_items(&mut self, items: Vec<Value>) {
        self.protocol_items = items;
    }

    pub fn set_protocol_item(&mut self, index: usize, item: Value) {
        if self.protocol_items.len() <= index {
            self.protocol_items.resize(index + 1, Value::Null);
        }
        self.protocol_items[index] = item;
    }

    pub fn set_reasoning_if_empty(&mut self, text: impl Into<String>) {
        if self.reasoning.is_empty() {
            self.reasoning = text.into();
        }
    }

    pub fn set_tool_call(&mut self, index: usize, call: ToolCall) {
        self.tools.insert(index, call);
    }

    pub fn set_tool_arguments(&mut self, index: usize, arguments: impl Into<String>) {
        if let Some(call) = self.tools.get_mut(&index) {
            call.arguments = arguments.into();
        }
    }

    pub fn finish(self) -> Result<Response, SdkError> {
        let mut content = Vec::new();
        if !self.reasoning.is_empty() {
            content.push(ContentBlock::Reasoning(self.reasoning.clone()));
        }
        if !self.text.is_empty() {
            content.push(ContentBlock::Text(self.text.clone()));
        }
        let tools = self.tools.into_values().collect::<Vec<_>>();
        content.extend(tools.iter().cloned().map(ContentBlock::ToolCall));
        if content.is_empty() {
            return Err(SdkError::Decode("stream completed without content".into()));
        }

        let has_complete_protocol_items = !self.protocol_items.is_empty()
            && self.protocol_items.iter().all(|item| !item.is_null());
        let protocol_items =
            if self.protocol == Protocol::OpenAiResponses && !has_complete_protocol_items {
                let mut items = Vec::new();
                if !self.reasoning.is_empty() {
                    items.push(json!({
                        "type": "reasoning",
                        "summary": [{"type": "summary_text", "text": self.reasoning}]
                    }));
                }
                if !self.text.is_empty() {
                    items.push(json!({
                        "type": "message",
                        "role": "assistant",
                        "content": [{"type": "output_text", "text": self.text}]
                    }));
                }
                items.extend(tools.into_iter().map(|call| {
                    json!({
                        "type": "function_call",
                        "call_id": call.id,
                        "name": call.name,
                        "arguments": call.arguments
                    })
                }));
                items
            } else {
                self.protocol_items
            };
        Ok(Response {
            content,
            protocol_items,
        })
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct OpenAiChatAdapter;

#[derive(Clone, Copy, Debug, Default)]
pub struct OpenAiResponsesAdapter;

#[derive(Clone, Copy, Debug, Default)]
pub struct AnthropicMessagesAdapter;

static OPENAI_CHAT_ADAPTER: OpenAiChatAdapter = OpenAiChatAdapter;
static OPENAI_RESPONSES_ADAPTER: OpenAiResponsesAdapter = OpenAiResponsesAdapter;
static ANTHROPIC_MESSAGES_ADAPTER: AnthropicMessagesAdapter = AnthropicMessagesAdapter;

pub fn builtin_adapter(protocol: Protocol) -> &'static dyn ProviderAdapter {
    match protocol {
        Protocol::OpenAiChat => &OPENAI_CHAT_ADAPTER,
        Protocol::OpenAiResponses => &OPENAI_RESPONSES_ADAPTER,
        Protocol::AnthropicMessages => &ANTHROPIC_MESSAGES_ADAPTER,
    }
}

impl ProviderAdapter for OpenAiChatAdapter {
    fn protocol(&self) -> Protocol {
        Protocol::OpenAiChat
    }

    fn encode_request(&self, request: &CompletionRequest) -> Result<WireRequest, SdkError> {
        let mut body = json!({
            "model": request.model,
            "messages": wire_messages(Protocol::OpenAiChat, &request.messages),
            "stream": request.stream
        });
        if request.include_tools {
            body["tools"] = Value::Array(
                request
                    .tools
                    .iter()
                    .map(|tool| {
                        json!({
                            "type": "function",
                            "function": {
                                "name": tool.name,
                                "description": tool.description,
                                "parameters": tool.input_schema
                            }
                        })
                    })
                    .collect(),
            );
        }
        apply_model_options(&mut body, request);
        Ok(WireRequest {
            body,
            auth_style: AuthStyle::Bearer,
            headers: Vec::new(),
        })
    }

    fn decode_response(&self, body: &Value) -> Result<Response, SdkError> {
        decode_chat_response(body)
    }

    fn decode_stream_event(&self, event: &Value) -> Vec<Event> {
        decode_chat_stream_event(event)
    }
}

impl ProviderAdapter for OpenAiResponsesAdapter {
    fn protocol(&self) -> Protocol {
        Protocol::OpenAiResponses
    }

    fn encode_request(&self, request: &CompletionRequest) -> Result<WireRequest, SdkError> {
        let mut body = json!({
            "model": request.model,
            "input": wire_messages(Protocol::OpenAiResponses, &request.messages),
            "include": ["reasoning.encrypted_content"],
            "stream": request.stream
        });
        if request.include_tools {
            body["tools"] = Value::Array(
                request
                    .tools
                    .iter()
                    .map(|tool| {
                        json!({
                            "type": "function",
                            "name": tool.name,
                            "description": tool.description,
                            "parameters": tool.input_schema,
                            "strict": true
                        })
                    })
                    .collect(),
            );
        }
        apply_model_options(&mut body, request);
        Ok(WireRequest {
            body,
            auth_style: AuthStyle::Bearer,
            headers: Vec::new(),
        })
    }

    fn decode_response(&self, body: &Value) -> Result<Response, SdkError> {
        decode_responses_response(body)
    }

    fn decode_stream_event(&self, event: &Value) -> Vec<Event> {
        decode_responses_stream_event(event)
    }

    fn capture_stream_event(&self, accumulator: &mut StreamAccumulator, event: &Value) {
        capture_responses_stream_event(accumulator, event);
    }
}

impl ProviderAdapter for AnthropicMessagesAdapter {
    fn protocol(&self) -> Protocol {
        Protocol::AnthropicMessages
    }

    fn encode_request(&self, request: &CompletionRequest) -> Result<WireRequest, SdkError> {
        let system = request
            .messages
            .iter()
            .filter(|message| message.role == Role::System)
            .map(|message| message.text.as_str())
            .collect::<Vec<_>>()
            .join("\n\n");
        let messages = request
            .messages
            .iter()
            .filter(|message| message.role != Role::System)
            .cloned()
            .collect::<Vec<_>>();
        let mut body = json!({
            "model": request.model,
            "system": system,
            "messages": wire_messages(Protocol::AnthropicMessages, &messages),
            "max_tokens": request.max_tokens,
            "stream": request.stream
        });
        if request.include_tools {
            body["tools"] = Value::Array(
                request
                    .tools
                    .iter()
                    .map(|tool| {
                        json!({
                            "name": tool.name,
                            "description": tool.description,
                            "input_schema": tool.input_schema
                        })
                    })
                    .collect(),
            );
        }
        apply_model_options(&mut body, request);
        Ok(WireRequest {
            body,
            auth_style: AuthStyle::Anthropic,
            headers: Vec::new(),
        })
    }

    fn decode_response(&self, body: &Value) -> Result<Response, SdkError> {
        decode_anthropic_response(body)
    }

    fn decode_stream_event(&self, event: &Value) -> Vec<Event> {
        decode_anthropic_stream_event(event)
    }
}

fn apply_model_options(body: &mut Value, request: &CompletionRequest) {
    match request.protocol {
        Protocol::OpenAiChat => {
            body["max_tokens"] = Value::from(request.max_tokens);
            if let Some(thinking) = request.thinking {
                apply_openai_thinking(body, thinking);
            }
        }
        Protocol::OpenAiResponses => {
            body["max_output_tokens"] = Value::from(request.max_tokens);
            if request.reasoning {
                body["reasoning"] = json!({"summary": "auto"});
            }
            if let Some(thinking) = request.thinking {
                if thinking == ThinkingLevel::Off {
                    body.as_object_mut().map(|body| body.remove("reasoning"));
                } else {
                    let effort = match thinking {
                        ThinkingLevel::XHigh => "high",
                        other => other.label(),
                    };
                    body["reasoning"] = json!({"effort": effort, "summary": "auto"});
                }
            }
        }
        Protocol::AnthropicMessages => {
            body["max_tokens"] = Value::from(request.max_tokens);
            if let Some(thinking) = request.thinking {
                body["thinking"] = if thinking == ThinkingLevel::Off {
                    json!({"type": "disabled"})
                } else {
                    json!({
                        "type": "enabled",
                        "budget_tokens": thinking.budget(request.max_tokens)
                    })
                };
            }
        }
    }
}

fn apply_openai_thinking(body: &mut Value, thinking: ThinkingLevel) {
    if thinking == ThinkingLevel::Off {
        body.as_object_mut()
            .map(|body| body.remove("reasoning_effort"));
    } else {
        let effort = match thinking {
            ThinkingLevel::XHigh => "high",
            other => other.label(),
        };
        body["reasoning_effort"] = Value::from(effort);
    }
}

fn decode_chat_response(body: &Value) -> Result<Response, SdkError> {
    let message = body.pointer("/choices/0/message").ok_or_else(|| {
        SdkError::Decode("Chat response did not contain an assistant message".into())
    })?;
    let mut blocks = Vec::new();
    push_text(&mut blocks, true, message.get("reasoning_content"));
    push_text(&mut blocks, true, message.get("reasoning"));
    push_text(&mut blocks, false, message.get("content"));
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
    ensure_content(
        blocks,
        "protocol response did not contain text, reasoning, or a tool call",
    )
    .map(|content| Response {
        content,
        protocol_items: Vec::new(),
    })
}

fn decode_responses_response(body: &Value) -> Result<Response, SdkError> {
    let mut blocks = Vec::new();
    for item in body
        .get("output")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        match item.get("type").and_then(Value::as_str) {
            Some("reasoning") => {
                let parts = non_empty_array(item.get("content"))
                    .or_else(|| non_empty_array(item.get("summary")));
                for part in parts.into_iter().flatten() {
                    push_text(&mut blocks, true, part.get("text"));
                }
            }
            Some("message") => {
                for part in item
                    .get("content")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                {
                    push_text(&mut blocks, false, part.get("text"));
                }
            }
            Some("function_call") => push_tool_call(
                &mut blocks,
                item.get("call_id")
                    .or_else(|| item.get("id"))
                    .and_then(Value::as_str),
                item.get("name").and_then(Value::as_str),
                item.get("arguments").and_then(Value::as_str),
            ),
            _ => {}
        }
    }
    ensure_content(
        blocks,
        "protocol response did not contain text, reasoning, or a tool call",
    )
    .map(|content| Response {
        content,
        protocol_items: body
            .get("output")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default(),
    })
}

fn decode_anthropic_response(body: &Value) -> Result<Response, SdkError> {
    let mut blocks = Vec::new();
    for part in body
        .get("content")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        match part.get("type").and_then(Value::as_str) {
            Some("thinking") => push_text(&mut blocks, true, part.get("thinking")),
            Some("text") => push_text(&mut blocks, false, part.get("text")),
            Some("tool_use") => push_tool_call(
                &mut blocks,
                part.get("id").and_then(Value::as_str),
                part.get("name").and_then(Value::as_str),
                part.get("input").map(Value::to_string).as_deref(),
            ),
            _ => {}
        }
    }
    ensure_content(
        blocks,
        "protocol response did not contain text, reasoning, or a tool call",
    )
    .map(|content| Response {
        content,
        protocol_items: Vec::new(),
    })
}

fn ensure_content(
    content: Vec<ContentBlock>,
    message: &str,
) -> Result<Vec<ContentBlock>, SdkError> {
    if content.is_empty() {
        Err(SdkError::Decode(message.into()))
    } else {
        Ok(content)
    }
}

fn non_empty_array(value: Option<&Value>) -> Option<&Vec<Value>> {
    value
        .and_then(Value::as_array)
        .filter(|parts| !parts.is_empty())
}

fn push_text(blocks: &mut Vec<ContentBlock>, reasoning: bool, value: Option<&Value>) {
    match value {
        Some(Value::String(text)) if !text.is_empty() => {
            blocks.push(if reasoning {
                ContentBlock::Reasoning(text.clone())
            } else {
                ContentBlock::Text(text.clone())
            });
        }
        Some(Value::Array(parts)) => {
            for part in parts {
                push_text(blocks, reasoning, part.get("text"));
            }
        }
        _ => {}
    }
}

fn push_tool_call(
    blocks: &mut Vec<ContentBlock>,
    id: Option<&str>,
    name: Option<&str>,
    arguments: Option<&str>,
) {
    let (Some(id), Some(name)) = (id, name) else {
        return;
    };
    blocks.push(ContentBlock::ToolCall(ToolCall {
        id: id.into(),
        name: name.into(),
        arguments: arguments.unwrap_or("{}").into(),
    }));
}

fn decode_chat_stream_event(event: &Value) -> Vec<Event> {
    let Some(choice) = event.pointer("/choices/0") else {
        return Vec::new();
    };
    let delta = choice.get("delta").unwrap_or(&Value::Null);
    let mut events = Vec::new();
    push_delta(
        &mut events,
        delta.get("reasoning_content"),
        Event::ReasoningDelta,
    );
    push_delta(&mut events, delta.get("reasoning"), Event::ReasoningDelta);
    push_delta(&mut events, delta.get("content"), Event::TextDelta);
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
            events.push(Event::ToolCallStart {
                index,
                id: id.into(),
                name: name.into(),
            });
        }
        push_delta(&mut events, call.pointer("/function/arguments"), |delta| {
            Event::ToolCallArgumentsDelta { index, delta }
        });
    }
    if choice
        .get("finish_reason")
        .is_some_and(|value| !value.is_null())
    {
        events.push(Event::Stop(
            choice
                .get("finish_reason")
                .and_then(Value::as_str)
                .map(str::to_string),
        ));
    }
    events
}

fn decode_responses_stream_event(event: &Value) -> Vec<Event> {
    let index = event
        .get("output_index")
        .and_then(Value::as_u64)
        .unwrap_or(0) as usize;
    match event.get("type").and_then(Value::as_str) {
        Some("response.output_text.delta") => delta_event(event, Event::TextDelta),
        Some("response.reasoning_text.delta" | "response.reasoning_summary_text.delta") => {
            delta_event(event, Event::ReasoningDelta)
        }
        Some("response.output_item.added")
            if event.pointer("/item/type").and_then(Value::as_str) == Some("function_call") =>
        {
            vec![Event::ToolCallStart {
                index,
                id: event
                    .pointer("/item/call_id")
                    .or_else(|| event.pointer("/item/id"))
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
            Event::ToolCallArgumentsDelta { index, delta }
        }),
        Some("response.completed") => vec![Event::Stop(Some("completed".into()))],
        _ => Vec::new(),
    }
}

fn capture_responses_stream_event(accumulator: &mut StreamAccumulator, event: &Value) {
    match event.get("type").and_then(Value::as_str) {
        Some("response.completed") => {
            if let Some(items) = event.pointer("/response/output").and_then(Value::as_array) {
                accumulator.set_protocol_items(items.clone());
                if accumulator.reasoning.is_empty() {
                    for item in items {
                        if let Some(text) = response_reasoning_text(item) {
                            accumulator.set_reasoning_if_empty(text);
                            break;
                        }
                    }
                }
            }
        }
        Some("response.output_item.done") => {
            let Some(index) = event.get("output_index").and_then(Value::as_u64) else {
                return;
            };
            let Some(item) = event.get("item") else {
                return;
            };
            let index = index as usize;
            if let Some(text) = response_reasoning_text(item) {
                accumulator.set_reasoning_if_empty(text);
            }
            if item.get("type").and_then(Value::as_str) == Some("function_call")
                && let (Some(id), Some(name)) = (
                    item.get("call_id")
                        .or_else(|| item.get("id"))
                        .and_then(Value::as_str),
                    item.get("name").and_then(Value::as_str),
                )
            {
                accumulator.set_tool_call(
                    index,
                    ToolCall {
                        id: id.into(),
                        name: name.into(),
                        arguments: item
                            .get("arguments")
                            .and_then(Value::as_str)
                            .unwrap_or("{}")
                            .into(),
                    },
                );
            }
            accumulator.set_protocol_item(index, item.clone());
        }
        Some("response.function_call_arguments.done") => {
            if let (Some(index), Some(arguments)) = (
                event.get("output_index").and_then(Value::as_u64),
                event.get("arguments").and_then(Value::as_str),
            ) {
                accumulator.set_tool_arguments(index as usize, arguments);
            }
        }
        Some("response.reasoning_summary_text.done") => {
            if let Some(text) = event.get("text").and_then(Value::as_str) {
                accumulator.set_reasoning_if_empty(text);
            }
        }
        _ => {}
    }
}

fn response_reasoning_text(item: &Value) -> Option<String> {
    if item.get("type").and_then(Value::as_str) != Some("reasoning") {
        return None;
    }
    let parts =
        non_empty_array(item.get("content")).or_else(|| non_empty_array(item.get("summary")));
    let text = parts
        .into_iter()
        .flatten()
        .filter_map(|part| part.get("text").and_then(Value::as_str))
        .collect::<Vec<_>>()
        .join("\n\n");
    (!text.is_empty()).then_some(text)
}

fn push_delta(events: &mut Vec<Event>, value: Option<&Value>, make: impl Fn(String) -> Event) {
    events.extend(value_event(value, make));
}

fn delta_event(event: &Value, make: impl Fn(String) -> Event) -> Vec<Event> {
    value_event(event.get("delta"), make)
}

fn value_event(value: Option<&Value>, make: impl Fn(String) -> Event) -> Vec<Event> {
    value
        .and_then(Value::as_str)
        .filter(|text| !text.is_empty())
        .map(|text| vec![make(text.into())])
        .unwrap_or_default()
}

fn decode_anthropic_stream_event(event: &Value) -> Vec<Event> {
    let index = event.get("index").and_then(Value::as_u64).unwrap_or(0) as usize;
    match event.get("type").and_then(Value::as_str) {
        Some("content_block_start")
            if event.pointer("/content_block/type").and_then(Value::as_str) == Some("tool_use") =>
        {
            vec![Event::ToolCallStart {
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
        Some("content_block_delta") => match event.pointer("/delta/type").and_then(Value::as_str) {
            Some("text_delta") => value_event(event.pointer("/delta/text"), Event::TextDelta),
            Some("thinking_delta") => {
                value_event(event.pointer("/delta/thinking"), Event::ReasoningDelta)
            }
            Some("input_json_delta") => {
                value_event(event.pointer("/delta/partial_json"), |delta| {
                    Event::ToolCallArgumentsDelta { index, delta }
                })
            }
            _ => Vec::new(),
        },
        Some("message_delta") => vec![Event::Stop(
            event
                .pointer("/delta/stop_reason")
                .and_then(Value::as_str)
                .map(str::to_string),
        )],
        _ => Vec::new(),
    }
}

fn wire_messages(protocol: Protocol, messages: &[Message]) -> Vec<Value> {
    let mut result = Vec::new();
    for message in messages {
        if let Some(tool_result) = &message.tool_result {
            result.push(match protocol {
                Protocol::OpenAiChat => json!({
                    "role": "tool",
                    "tool_call_id": tool_result.call_id,
                    "content": tool_result.output
                }),
                Protocol::OpenAiResponses => json!({
                    "type": "function_call_output",
                    "call_id": tool_result.call_id,
                    "output": tool_result.output
                }),
                Protocol::AnthropicMessages => json!({
                    "role": "user",
                    "content": [{
                        "type": "tool_result",
                        "tool_use_id": tool_result.call_id,
                        "content": tool_result.output,
                        "is_error": tool_result.is_error
                    }]
                }),
            });
            continue;
        }
        if protocol == Protocol::OpenAiResponses && !message.protocol_items.is_empty() {
            result.extend(message.protocol_items.iter().cloned());
            continue;
        }
        if !message.tool_calls.is_empty() {
            match protocol {
                Protocol::OpenAiChat => result.push(json!({
                    "role": "assistant",
                    "content": if message.text.is_empty() { Value::Null } else { Value::String(message.text.clone()) },
                    "tool_calls": message.tool_calls.iter().map(|call| json!({
                        "id": call.id,
                        "type": "function",
                        "function": {"name": call.name, "arguments": call.arguments}
                    })).collect::<Vec<_>>()
                })),
                Protocol::OpenAiResponses => {
                    if !message.text.is_empty() {
                        result.push(json!({
                            "type": "message",
                            "role": "assistant",
                            "content": [{"type": "output_text", "text": message.text}]
                        }));
                    }
                    result.extend(message.tool_calls.iter().map(|call| json!({
                        "type": "function_call",
                        "call_id": call.id,
                        "name": call.name,
                        "arguments": call.arguments
                    })));
                }
                Protocol::AnthropicMessages => {
                    let mut content = Vec::new();
                    if !message.text.is_empty() {
                        content.push(json!({"type": "text", "text": message.text}));
                    }
                    content.extend(message.tool_calls.iter().map(|call| json!({
                        "type": "tool_use",
                        "id": call.id,
                        "name": call.name,
                        "input": serde_json::from_str::<Value>(&call.arguments).unwrap_or(Value::Null)
                    })));
                    result.push(json!({"role": "assistant", "content": content}));
                }
            }
            continue;
        }
        result.push(json!({
            "role": match message.role {
                Role::System => "system",
                Role::User => "user",
                Role::Assistant => "assistant"
            },
            "content": message.text
        }));
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(protocol: Protocol) -> CompletionRequest {
        CompletionRequest::new(
            protocol,
            "https://example.test/v1",
            "model",
            4_000,
            vec![Message::new(Role::User, "hello")],
            vec![ToolDefinition::new(
                "read",
                "Read a file",
                json!({"type":"object","properties":{"path":{"type":"string"}},"required":["path"],"additionalProperties":false}),
                false,
            )],
        )
    }

    #[test]
    fn adapters_encode_canonical_messages_and_tools() {
        let chat = OpenAiChatAdapter
            .encode_request(&request(Protocol::OpenAiChat))
            .unwrap();
        assert_eq!(chat.body["messages"][0]["content"], "hello");
        assert_eq!(chat.body["tools"][0]["function"]["name"], "read");

        let responses = OpenAiResponsesAdapter
            .encode_request(&request(Protocol::OpenAiResponses))
            .unwrap();
        assert_eq!(responses.body["input"][0]["content"], "hello");
        assert_eq!(responses.body["tools"][0]["strict"], true);
        assert_eq!(responses.body["max_output_tokens"], 4_000);

        let anthropic = AnthropicMessagesAdapter
            .encode_request(&request(Protocol::AnthropicMessages))
            .unwrap();
        assert_eq!(anthropic.body["messages"][0]["content"], "hello");
        assert_eq!(anthropic.body["max_tokens"], 4_000);
    }

    #[test]
    fn responses_request_asks_for_a_reasoning_summary() {
        let mut request = request(Protocol::OpenAiResponses);
        request.reasoning = true;
        let wire = OpenAiResponsesAdapter.encode_request(&request).unwrap();
        assert_eq!(wire.body["reasoning"]["summary"], "auto");

        request.thinking = Some(ThinkingLevel::High);
        let wire = OpenAiResponsesAdapter.encode_request(&request).unwrap();
        assert_eq!(wire.body["reasoning"]["effort"], "high");
        assert_eq!(wire.body["reasoning"]["summary"], "auto");
    }

    #[test]
    fn responses_reasoning_summary_is_decoded_and_replayed() {
        let body = json!({"output":[
            {"type":"reasoning","summary":[{"type":"summary_text","text":"think"}]},
            {"type":"message","content":[{"type":"output_text","text":"answer"}]}
        ]});
        let response = OpenAiResponsesAdapter.decode_response(&body).unwrap();
        assert_eq!(
            response.content,
            vec![
                ContentBlock::Reasoning("think".into()),
                ContentBlock::Text("answer".into()),
            ]
        );
        assert_eq!(
            response.protocol_items,
            body["output"].as_array().unwrap().clone()
        );
    }

    #[test]
    fn responses_stream_events_capture_summary_and_tool_arguments() {
        let adapter = OpenAiResponsesAdapter;
        let mut accumulator = StreamAccumulator::new(Protocol::OpenAiResponses);
        let events = [
            json!({"type":"response.reasoning_summary_text.delta","delta":"summary"}),
            json!({"type":"response.output_item.added","output_index":1,"item":{"type":"function_call","call_id":"call_1","name":"read"}}),
            json!({"type":"response.function_call_arguments.delta","output_index":1,"delta":"{\"path\":\"README.md\"}"}),
            json!({"type":"response.output_item.done","output_index":1,"item":{"type":"function_call","call_id":"call_1","name":"read","arguments":"{\"path\":\"README.md\"}"}}),
        ];
        for event in events {
            adapter.capture_stream_event(&mut accumulator, &event);
            for decoded in adapter.decode_stream_event(&event) {
                accumulator.push(&decoded);
            }
        }
        let response = accumulator.finish().unwrap();
        assert_eq!(
            response.content[0],
            ContentBlock::Reasoning("summary".into())
        );
        assert_eq!(
            response.content[1],
            ContentBlock::ToolCall(ToolCall {
                id: "call_1".into(),
                name: "read".into(),
                arguments: r#"{"path":"README.md"}"#.into(),
            })
        );
    }

    #[test]
    fn custom_adapter_can_supply_another_wire_protocol() {
        struct CustomAdapter;

        impl ProviderAdapter for CustomAdapter {
            fn protocol(&self) -> Protocol {
                Protocol::OpenAiChat
            }

            fn encode_request(&self, request: &CompletionRequest) -> Result<WireRequest, SdkError> {
                Ok(WireRequest {
                    body: json!({"prompt": request.messages[0].text}),
                    auth_style: AuthStyle::None,
                    headers: vec![("x-provider".into(), "custom".into())],
                })
            }

            fn decode_response(&self, body: &Value) -> Result<Response, SdkError> {
                Ok(Response {
                    content: vec![ContentBlock::Text(
                        body["answer"].as_str().unwrap_or_default().into(),
                    )],
                    protocol_items: Vec::new(),
                })
            }

            fn decode_stream_event(&self, _event: &Value) -> Vec<Event> {
                Vec::new()
            }
        }

        let wire = CustomAdapter
            .encode_request(&request(Protocol::OpenAiChat))
            .unwrap();
        assert_eq!(wire.body["prompt"], "hello");
        assert_eq!(wire.headers[0].0, "x-provider");
    }
}
