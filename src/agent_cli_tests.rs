use super::*;
use crossh_agent::{AgentModel, AgentModelRef, AgentProtocol, AgentProvider};

fn test_settings() -> AgentSettings {
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

fn app() -> App {
    let session = AgentSession::new("/tmp/project");
    App {
        settings: test_settings(),
        api_key: None,
        workspace: PathBuf::from("/tmp/project"),
        context_files: Vec::new(),
        skills: Vec::new(),
        prompts: Vec::new(),
        session_path: None,
        session,
        input: String::new(),
        input_cursor: 0,
        history_cursor: None,
        queued_inputs: VecDeque::new(),
        messages: Vec::new(),
        scroll: u16::MAX,
        max_scroll: 40,
        show_tool_details: false,
        show_reasoning: false,
        thinking: AgentThinkingLevel::Medium,
        thinking_explicit: false,
        status: String::new(),
        started_at: Instant::now(),
    }
}

#[test]
fn parse_options_supports_session_and_model_controls() {
    let options = parse_options([
        "--continue".into(),
        "--model".into(),
        "local/qwen3-coder".into(),
        "--thinking".into(),
        "high".into(),
    ])
    .unwrap();
    assert!(options.continue_recent);
    assert_eq!(options.model.as_deref(), Some("local/qwen3-coder"));
    assert_eq!(options.thinking, Some(AgentThinkingLevel::High));
}

#[test]
fn editor_handles_unicode_and_word_deletion() {
    let mut app = app();
    insert_text(&mut app, "hello 中😀 world");
    delete_previous_word(&mut app);
    assert_eq!(app.input, "hello 中😀 ");
    delete_previous_char(&mut app);
    assert_eq!(app.input, "hello 中😀");
    move_cursor(&mut app, false);
    assert!(app.input.is_char_boundary(app.input_cursor));
}

#[test]
fn queue_preserves_prompts_in_order() {
    let mut app = app();
    app.input = "one".into();
    app.input_cursor = 3;
    queue_input(&mut app);
    app.input = "two".into();
    app.input_cursor = 3;
    queue_input(&mut app);
    assert_eq!(
        app.queued_inputs.into_iter().collect::<Vec<_>>(),
        ["one", "two"]
    );
}

#[test]
fn markdown_and_wrapping_stay_inside_the_requested_width() {
    assert!(
        wrap_content("abcdefghij\n中中文", 4)
            .iter()
            .all(|line| line.width() <= 4)
    );
    assert!(
        markdown_content("# Title\n\nUse **bold** here.", 10)
            .iter()
            .all(|line| line.width() <= 10)
    );
}

#[test]
fn prompt_templates_expand_arguments() {
    assert_eq!(
        expand_prompt_template("Review this.\n\n$ARGUMENTS", "the diff"),
        "Review this.\n\nthe diff"
    );
    assert_eq!(
        expand_prompt_template("Summarize", "the changes"),
        "Summarize\n\nUser-provided arguments:\nthe changes"
    );
}

#[test]
fn system_prompt_is_capped_to_the_active_input_budget() {
    let mut app = app();
    app.settings.providers[0].models[0].context_window = 2_048;
    app.settings.providers[0].models[0].max_tokens = 512;
    app.context_files.push(crossh_agent::AgentContextFile {
        path: PathBuf::from("/tmp/project/AGENTS.md"),
        content: "项目指令 ".repeat(10_000),
    });

    let prompt = system_prompt(&app);

    assert!(prompt.len() <= system_prompt_limit(&app));
    assert!(prompt.contains("truncated to fit the model context"));
    assert!(prompt.is_char_boundary(prompt.len()));
}

#[test]
fn tree_rewind_keeps_the_selected_turn_complete() {
    let mut app = app();
    app.session.messages = vec![
        AgentMessage::new(AgentRole::User, "first"),
        AgentMessage::new(AgentRole::Assistant, "first answer"),
        AgentMessage::new(AgentRole::User, "second"),
        AgentMessage::new(AgentRole::Assistant, "second answer"),
    ];
    rewind_session(&mut app, 1);
    assert_eq!(app.session.messages.len(), 2);
    assert_eq!(app.session.messages[1].text, "first answer");
}

#[test]
fn input_layout_accounts_for_wrapping_and_wide_characters() {
    assert_eq!(visual_line_count("abcdefgh", 4), 2);
    assert_eq!(visual_line_count("中中文", 4), 2);
    assert_eq!(cursor_position(Rect::new(0, 0, 4, 3), "abcd", 4), (0, 1));
}
