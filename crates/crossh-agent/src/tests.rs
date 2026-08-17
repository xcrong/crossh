use super::*;
use crossh_ai_sdk as sdk;

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

fn sdk_messages() -> Vec<sdk::Message> {
    vec![
        sdk::Message::new(sdk::Role::System, "be useful"),
        sdk::Message::new(sdk::Role::User, "hello"),
    ]
}

fn to_sdk_protocol(protocol: AgentProtocol) -> sdk::Protocol {
    match protocol {
        AgentProtocol::OpenAiChat => sdk::Protocol::OpenAiChat,
        AgentProtocol::OpenAiResponses => sdk::Protocol::OpenAiResponses,
        AgentProtocol::AnthropicMessages => sdk::Protocol::AnthropicMessages,
    }
}

#[test]
fn reviewer_denials_preserve_a_reason() {
    assert_eq!(
        parse_review_result(r#"{"decision":"DENY","reason":"shell command deletes data"}"#),
        AgentReviewResult {
            approved: false,
            reason: "shell command deletes data".into(),
        }
    );
    assert_eq!(
        parse_review_result("DENY: path is outside the workspace"),
        AgentReviewResult {
            approved: false,
            reason: "path is outside the workspace".into(),
        }
    );
    assert!(parse_review_result(r#"{"decision":"ALLOW","reason":"scoped read"}"#).approved);
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
fn reviewer_may_reuse_the_active_model() {
    // 设计意图：评审模型与主模型是否分离是产品层面的可选增强（多模型用户
    // 可另配评审模型以隔离仓库注入内容），而不是配置硬约束。默认配置只有
    // 一个模型时，评审复用主模型或回退到人工审批，绝不能因此拒绝启动。
    // 2026-08 曾把“评审模型必须与主模型不同”误当作校验规则加入 validate()，
    // 导致只有单模型配额的用户在 deb731f 之后启动即失败；此测试锁定
    // “二者相等合法”的行为，防止同类约束再次混入校验层。
    let settings = configured_settings();
    assert_eq!(settings.active_model, settings.reviewer_model);
    assert_eq!(settings.validate(), Ok(()));
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
        let request = sdk::CompletionRequest::new(
            to_sdk_protocol(protocol),
            "",
            &model.id,
            4_096,
            Vec::new(),
            Vec::new(),
        );
        let mut wire = sdk::builtin_adapter(to_sdk_protocol(protocol))
            .encode_request(&request)
            .expect("built-in adapter request should be valid");
        apply_model_options(&mut wire.body, protocol, &model);
        assert_eq!(wire.body[key], 1_234);
    }
}

#[test]
fn adapters_encode_the_same_canonical_messages() {
    let request = |protocol| {
        sdk::CompletionRequest::new(protocol, "", "model", 4_096, sdk_messages(), Vec::new())
    };
    let chat = sdk::builtin_adapter(sdk::Protocol::OpenAiChat)
        .encode_request(&request(sdk::Protocol::OpenAiChat))
        .expect("built-in adapter request should be valid");
    let responses = sdk::builtin_adapter(sdk::Protocol::OpenAiResponses)
        .encode_request(&request(sdk::Protocol::OpenAiResponses))
        .expect("built-in adapter request should be valid");
    let anthropic = sdk::builtin_adapter(sdk::Protocol::AnthropicMessages)
        .encode_request(&request(sdk::Protocol::AnthropicMessages))
        .expect("built-in adapter request should be valid");
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
    let adapter = sdk::builtin_adapter(sdk::Protocol::OpenAiResponses);
    let mut accumulator = sdk::StreamAccumulator::new(sdk::Protocol::OpenAiResponses);
    let item = json!({
        "id":"rs_1",
        "type":"reasoning",
        "summary":[{"type":"summary_text","text":"think"}]
    });
    adapter.capture_stream_event(
        &mut accumulator,
        &json!({"type":"response.output_item.done","output_index":0,"item":item}),
    );
    accumulator.push(&sdk::Event::ReasoningDelta("think".into()));
    let response = accumulator.finish().unwrap();
    assert_eq!(response.protocol_items, vec![item]);
}

#[test]
fn responses_tool_events_keep_the_final_call_and_arguments() {
    let adapter = sdk::builtin_adapter(sdk::Protocol::OpenAiResponses);
    let mut accumulator = sdk::StreamAccumulator::new(sdk::Protocol::OpenAiResponses);
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
        adapter.capture_stream_event(&mut accumulator, &event);
        for decoded in adapter.decode_stream_event(&event) {
            accumulator.push(&decoded);
        }
    }

    let response = accumulator.finish().unwrap();
    assert_eq!(
        response.content,
        vec![sdk::ContentBlock::ToolCall(sdk::ToolCall {
            id: "call_1".into(),
            name: "read".into(),
            arguments: r#"{"path":"README.md"}"#.into(),
        })]
    );
}

#[test]
fn responses_output_item_done_can_create_a_tool_call() {
    let adapter = sdk::builtin_adapter(sdk::Protocol::OpenAiResponses);
    let mut accumulator = sdk::StreamAccumulator::new(sdk::Protocol::OpenAiResponses);
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
    adapter.capture_stream_event(&mut accumulator, &event);

    let response = accumulator.finish().unwrap();
    assert_eq!(
        response.content,
        vec![sdk::ContentBlock::ToolCall(sdk::ToolCall {
            id: "call_2".into(),
            name: "ls".into(),
            arguments: r#"{"path":null}"#.into(),
        })]
    );
}

#[test]
fn adapters_decode_protocol_responses() {
    assert_eq!(
        sdk::builtin_adapter(sdk::Protocol::OpenAiChat)
            .decode_response(&json!({"choices":[{"message":{"content":"a"}}]}))
            .unwrap(),
        sdk::Response {
            content: vec![sdk::ContentBlock::Text("a".into())],
            protocol_items: Vec::new()
        }
    );
    assert_eq!(
        sdk::builtin_adapter(sdk::Protocol::OpenAiResponses)
            .decode_response(&json!({"output":[
                {"type":"reasoning","summary":[{"type":"summary_text","text":"think b"}]},
                {"type":"message","content":[{"type":"output_text","text":"b"}]}
            ]}))
            .unwrap(),
        sdk::Response {
            content: vec![
                sdk::ContentBlock::Reasoning("think b".into()),
                sdk::ContentBlock::Text("b".into())
            ],
            protocol_items: vec![
                json!({"type":"reasoning","summary":[{"type":"summary_text","text":"think b"}]}),
                json!({"type":"message","content":[{"type":"output_text","text":"b"}]})
            ]
        }
    );
    assert_eq!(
        sdk::builtin_adapter(sdk::Protocol::AnthropicMessages)
            .decode_response(&json!({"content":[
                {"type":"thinking","thinking":"think c","signature":"sig"},
                {"type":"text","text":"c"}
            ]}))
            .unwrap(),
        sdk::Response {
            content: vec![
                sdk::ContentBlock::Reasoning("think c".into()),
                sdk::ContentBlock::Text("c".into())
            ],
            protocol_items: Vec::new()
        }
    );
}

#[test]
fn chat_reasoning_content_is_separate_from_visible_text() {
    let response = sdk::builtin_adapter(sdk::Protocol::OpenAiChat)
        .decode_response(
            &json!({"choices":[{"message":{"reasoning_content":"think a","content":"a"}}]}),
        )
        .unwrap();
    assert_eq!(response.reasoning(), "think a");
    assert_eq!(response.text(), "a");
}

#[test]
fn stream_events_normalize_text_reasoning_and_tool_arguments() {
    assert_eq!(
        sdk::builtin_adapter(sdk::Protocol::OpenAiChat)
            .decode_stream_event(&json!({"choices":[{"delta":{"reasoning_content":"think","content":"answer","tool_calls":[{"index":0,"id":"call_1","function":{"name":"read","arguments":"{\"path\":"}}]}}]})),
        vec![
            sdk::Event::ReasoningDelta("think".into()),
            sdk::Event::TextDelta("answer".into()),
            sdk::Event::ToolCallStart {
                index: 0,
                id: "call_1".into(),
                name: "read".into()
            },
            sdk::Event::ToolCallArgumentsDelta {
                index: 0,
                delta: "{\"path\":".into()
            },
        ]
    );
    assert_eq!(
        sdk::builtin_adapter(sdk::Protocol::OpenAiResponses)
            .decode_stream_event(&json!({"type":"response.reasoning_summary_text.delta","delta":"summary","output_index":0})),
        vec![sdk::Event::ReasoningDelta("summary".into())]
    );
    assert_eq!(
        sdk::builtin_adapter(sdk::Protocol::AnthropicMessages)
            .decode_stream_event(&json!({"type":"content_block_delta","index":2,"delta":{"type":"input_json_delta","partial_json":"{\"path\":"}})),
        vec![sdk::Event::ToolCallArgumentsDelta {
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
fn patch_tool_applies_unified_hunks_and_keeps_failed_patches_atomic() {
    let workspace = tempfile::tempdir().unwrap();
    let path = workspace.path().join("notes.txt");
    fs::write(&path, "one\ntwo\nthree\n").unwrap();
    let patch = json!({
        "path": "notes.txt",
        "patch": "--- a/notes.txt\n+++ b/notes.txt\n@@ -1,3 +1,3 @@\n one\n-two\n+updated\n three\n"
    });
    let result = execute_tool(
        &AgentToolCall {
            id: "patch".into(),
            name: "patch".into(),
            arguments: patch.to_string(),
        },
        workspace.path(),
    );
    assert!(!result.is_error);
    assert_eq!(fs::read_to_string(&path).unwrap(), "one\nupdated\nthree\n");

    let failed_patch = json!({
        "path": "notes.txt",
        "patch": "@@ -1,3 +1,3 @@\n one\n-missing\n+broken\n three\n"
    });
    let result = execute_tool(
        &AgentToolCall {
            id: "patch-failed".into(),
            name: "patch".into(),
            arguments: failed_patch.to_string(),
        },
        workspace.path(),
    );
    assert!(result.is_error);
    assert!(result.output.contains("patch context mismatch"));
    assert_eq!(fs::read_to_string(&path).unwrap(), "one\nupdated\nthree\n");
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
    let request = |protocol| {
        sdk::CompletionRequest::new(protocol, "", &model.id, 4_096, Vec::new(), Vec::new())
    };
    let mut chat = sdk::builtin_adapter(sdk::Protocol::OpenAiChat)
        .encode_request(&request(sdk::Protocol::OpenAiChat))
        .expect("built-in adapter request should be valid")
        .body;
    apply_thinking_option(
        &mut chat,
        AgentProtocol::OpenAiChat,
        &model,
        AgentThinkingLevel::High,
    );
    assert_eq!(chat["reasoning_effort"], "high");

    let mut responses = sdk::builtin_adapter(sdk::Protocol::OpenAiResponses)
        .encode_request(&request(sdk::Protocol::OpenAiResponses))
        .expect("built-in adapter request should be valid")
        .body;
    apply_thinking_option(
        &mut responses,
        AgentProtocol::OpenAiResponses,
        &model,
        AgentThinkingLevel::High,
    );
    assert_eq!(responses["reasoning"]["effort"], "high");
    assert!(responses.get("reasoning_effort").is_none());

    let mut anthropic = sdk::builtin_adapter(sdk::Protocol::AnthropicMessages)
        .encode_request(&request(sdk::Protocol::AnthropicMessages))
        .expect("built-in adapter request should be valid")
        .body;
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

#[cfg(unix)]
#[test]
fn write_rejects_dangling_symlink_escape() {
    use std::os::unix::fs::symlink;
    let root = tempfile::tempdir().unwrap();
    let workspace = root.path().join("ws");
    let outside = root.path().join("outside.txt");
    fs::create_dir(&workspace).unwrap();
    symlink(&outside, workspace.join("escape.txt")).unwrap();

    let result = execute_tool(
        &AgentToolCall {
            id: "write".into(),
            name: "write".into(),
            arguments: json!({"path": "escape.txt", "content": "pwned"}).to_string(),
        },
        &workspace,
    );
    assert!(
        result.is_error,
        "write through a dangling symlink must be rejected, got {result:?}"
    );
    assert!(
        !outside.exists(),
        "write must not create a file outside the workspace via a symlink"
    );
}

#[cfg(unix)]
#[test]
fn write_rejects_symlink_pointing_outside_workspace() {
    use std::os::unix::fs::symlink;
    let root = tempfile::tempdir().unwrap();
    let workspace = root.path().join("ws");
    let outside = root.path().join("outside.txt");
    fs::create_dir(&workspace).unwrap();
    symlink(&outside, workspace.join("escape.txt")).unwrap();
    fs::write(&outside, "original").unwrap();

    let result = execute_tool(
        &AgentToolCall {
            id: "write".into(),
            name: "write".into(),
            arguments: json!({"path": "escape.txt", "content": "pwned"}).to_string(),
        },
        &workspace,
    );
    assert!(
        result.is_error,
        "write through an escaping symlink must be rejected"
    );
    assert_eq!(
        fs::read_to_string(&outside).unwrap(),
        "original",
        "the target outside the workspace must stay untouched"
    );
}

#[cfg(unix)]
#[test]
fn write_rejects_dangling_two_hop_symlink_chain() {
    use std::os::unix::fs::symlink;
    let root = tempfile::tempdir().unwrap();
    let workspace = root.path().join("ws");
    let outside = root.path().join("outside.txt");
    fs::create_dir(&workspace).unwrap();
    // hop1 → "hop2"（工作区内相对链接），hop2 → 工作区外（悬空）。
    // 写入 "hop1/x" 时 hop1 自身指向的工作区内，但链条终点在区外，
    // 必须逐跳解析而不是只看第一跳。
    symlink("hop2", workspace.join("hop1")).unwrap();
    symlink(&outside, workspace.join("hop2")).unwrap();

    let result = execute_tool(
        &AgentToolCall {
            id: "write".into(),
            name: "write".into(),
            arguments: json!({"path": "hop1/x", "content": "pwned"}).to_string(),
        },
        &workspace,
    );
    assert!(
        result.is_error,
        "write through a two-hop dangling chain must be rejected, got {result:?}"
    );
    assert!(
        !outside.exists(),
        "write must not create a file outside the workspace via a symlink chain"
    );
}

#[cfg(unix)]
#[test]
fn write_rejects_two_hop_symlink_chain_to_existing_outside_dir() {
    use std::os::unix::fs::symlink;
    let root = tempfile::tempdir().unwrap();
    let workspace = root.path().join("ws");
    let outside_dir = root.path().join("outside-dir");
    fs::create_dir(&workspace).unwrap();
    fs::create_dir(&outside_dir).unwrap();
    // hop1 → "hop2"（工作区内相对链接），hop2 → 工作区外已存在的目录。
    // 区外的目录存在、被写入的文件不存在，是更隐蔽的逃逸形态。
    symlink("hop2", workspace.join("hop1")).unwrap();
    symlink(&outside_dir, workspace.join("hop2")).unwrap();

    let result = execute_tool(
        &AgentToolCall {
            id: "write".into(),
            name: "write".into(),
            arguments: json!({"path": "hop1/x", "content": "pwned"}).to_string(),
        },
        &workspace,
    );
    assert!(
        result.is_error,
        "write through a two-hop chain to an existing outside dir must be rejected, got {result:?}"
    );
    assert!(
        !outside_dir.join("x").exists(),
        "write must not create a file outside the workspace via a symlink chain"
    );
}

#[cfg(unix)]
#[test]
fn write_resolves_symlink_inside_workspace() {
    use std::os::unix::fs::symlink;
    let workspace = tempfile::tempdir().unwrap();
    symlink("real.txt", workspace.path().join("alias.txt")).unwrap();

    let result = execute_tool(
        &AgentToolCall {
            id: "write".into(),
            name: "write".into(),
            arguments: json!({"path": "alias.txt", "content": "hello"}).to_string(),
        },
        workspace.path(),
    );
    assert!(
        !result.is_error,
        "write through an inner symlink must work, got {result:?}"
    );
    assert_eq!(
        fs::read_to_string(workspace.path().join("real.txt")).unwrap(),
        "hello"
    );
}
