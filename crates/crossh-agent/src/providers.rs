#[cfg(test)]
use super::policy::AgentModel;
use super::policy::{AgentSettings, MODEL_REQUEST_TIMEOUT, ResolvedModel};
use super::tools::builtin_tools;
use crate::{Event, Message, Response, ThinkingLevel};
use crossh_ai_sdk as sdk;
#[cfg(test)]
use serde_json::Value;
#[cfg(test)]
use serde_json::json;
use std::time::Duration;

pub(super) async fn complete_target_with_timeout(
    target: ResolvedModel<'_>,
    api_key: Option<&str>,
    messages: &[Message],
    include_tools: bool,
    timeout: Duration,
) -> Result<Response, String> {
    let request = sdk_request(target, api_key, messages, include_tools, None, false);
    let adapter = sdk::builtin_adapter(request.protocol);
    let client = sdk::Client::new(timeout).map_err(|error| error.to_string())?;
    client
        .complete(adapter, &request)
        .await
        .map_err(|error| error.to_string())
}

pub async fn complete_stream_with_options(
    settings: &AgentSettings,
    api_key: Option<&str>,
    messages: &[Message],
    thinking: Option<ThinkingLevel>,
    mut on_event: impl FnMut(&Event),
) -> Result<Response, String> {
    let target = settings
        .resolve(&settings.active_model)
        .map_err(str::to_string)?;
    let thinking = thinking.filter(|_| target.model.reasoning);
    let request = sdk_request(target, api_key, messages, true, thinking, true);
    let adapter = sdk::builtin_adapter(request.protocol);
    let client = sdk::Client::new(MODEL_REQUEST_TIMEOUT).map_err(|error| error.to_string())?;
    client
        .stream(adapter, &request, |event| on_event(event))
        .await
        .map_err(|error| error.to_string())
}

fn sdk_request(
    target: ResolvedModel<'_>,
    api_key: Option<&str>,
    messages: &[Message],
    include_tools: bool,
    thinking: Option<ThinkingLevel>,
    stream: bool,
) -> sdk::CompletionRequest {
    let tools = builtin_tools().into_iter().map(|tool| tool.tool).collect();
    let mut request = sdk::CompletionRequest::new(
        target.model.protocol,
        target.model.url.clone(),
        target.model.id.clone(),
        target.model.max_tokens,
        messages.to_vec(),
        tools,
    );
    request.api_key = api_key.map(str::to_owned);
    request.reasoning = target.model.reasoning;
    request.thinking = thinking;
    request.include_tools = include_tools;
    request.stream = stream;
    request
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

#[cfg(test)]
fn sdk_request_for_messages(
    protocol: sdk::Protocol,
    model: &str,
    messages: &[Message],
    max_tokens: u32,
) -> sdk::CompletionRequest {
    let tools = builtin_tools().into_iter().map(|tool| tool.tool).collect();
    sdk::CompletionRequest::new(protocol, "", model, max_tokens, messages.to_vec(), tools)
}

#[cfg(test)]
pub(super) fn apply_model_options(body: &mut Value, protocol: sdk::Protocol, model: &AgentModel) {
    let key = match protocol {
        sdk::Protocol::OpenAiChat | sdk::Protocol::AnthropicMessages => "max_tokens",
        sdk::Protocol::OpenAiResponses => "max_output_tokens",
    };
    body[key] = Value::from(model.max_tokens);
    if protocol == sdk::Protocol::OpenAiResponses && model.reasoning {
        body["reasoning"] = json!({"summary": "auto"});
    }
}

#[cfg(test)]
pub(super) fn apply_thinking_option(
    body: &mut Value,
    protocol: sdk::Protocol,
    model: &AgentModel,
    thinking: ThinkingLevel,
) {
    apply_model_options(body, protocol, model);
    match protocol {
        sdk::Protocol::OpenAiChat => {
            if thinking == ThinkingLevel::Off {
                body.as_object_mut()
                    .map(|body| body.remove("reasoning_effort"));
            } else {
                let effort = match thinking {
                    ThinkingLevel::XHigh => "high",
                    ThinkingLevel::Max => "max",
                    other => other.label(),
                };
                body["reasoning_effort"] = Value::from(effort);
            }
        }
        sdk::Protocol::OpenAiResponses => {
            if thinking == ThinkingLevel::Off {
                body.as_object_mut().map(|body| body.remove("reasoning"));
            } else {
                let effort = match thinking {
                    ThinkingLevel::XHigh => "high",
                    ThinkingLevel::Max => "max",
                    other => other.label(),
                };
                body["reasoning"] = json!({"effort": effort, "summary": "auto"});
            }
        }
        sdk::Protocol::AnthropicMessages => {
            body["thinking"] = if thinking == ThinkingLevel::Off {
                json!({"type":"disabled"})
            } else {
                json!({"type":"enabled","budget_tokens":thinking.budget(model.max_tokens)})
            };
        }
    }
}

#[cfg(test)]
pub(super) fn wire_messages(protocol: sdk::Protocol, messages: &[Message]) -> Vec<Value> {
    let request = sdk_request_for_messages(protocol, "model", messages, 4_096);
    let body = sdk::builtin_adapter(request.protocol)
        .encode_request(&request)
        .expect("built-in adapter request should be valid")
        .body;
    let key = match protocol {
        sdk::Protocol::OpenAiResponses => "input",
        sdk::Protocol::OpenAiChat | sdk::Protocol::AnthropicMessages => "messages",
    };
    body.get(key)
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}
