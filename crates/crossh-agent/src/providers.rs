use super::messages::{
    AgentContentBlock, AgentEvent, AgentMessage, AgentResponse, AgentRole, AgentToolCall,
};
#[cfg(test)]
use super::policy::AgentModel;
use super::policy::{
    AgentProtocol, AgentSettings, AgentThinkingLevel, MODEL_REQUEST_TIMEOUT, ResolvedModel,
};
use super::tools::builtin_tools;
use crossh_ai_sdk as sdk;
#[cfg(test)]
use serde_json::Value;
#[cfg(test)]
use serde_json::json;
use std::time::Duration;

pub(super) async fn complete_target_with_timeout(
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
pub(super) struct Utf8StreamDecoder {
    bytes: Vec<u8>,
}

#[cfg(test)]
impl Utf8StreamDecoder {
    pub(super) fn push(&mut self, chunk: &[u8]) -> String {
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

    pub(super) fn finish(self) -> String {
        String::from_utf8_lossy(&self.bytes).into_owned()
    }
}

fn sdk_tool_definitions() -> Vec<sdk::ToolDefinition> {
    builtin_tools()
        .into_iter()
        .map(|tool| sdk::ToolDefinition::new(tool.name, tool.description, tool.input_schema))
        .collect()
}

#[cfg(test)]
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

#[cfg(test)]
pub(super) fn apply_model_options(body: &mut Value, protocol: AgentProtocol, model: &AgentModel) {
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
pub(super) fn apply_thinking_option(
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

#[cfg(test)]
pub(super) fn wire_messages(protocol: AgentProtocol, messages: &[AgentMessage]) -> Vec<Value> {
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
