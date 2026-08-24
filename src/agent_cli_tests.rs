use super::render::{
    agent_layout, build_editor, build_footer, build_slash_popup, build_transcript, cursor_position,
    input_height, markdown_content, visual_line_count, wrap_content,
};
use super::*;
use crossh_agent::{AgentModel, AgentModelRef, AgentProvider, Protocol};
use crossh_tui::ansi::visible_width;
use crossh_tui::component::Component;
use crossh_tui::layout::Rect as TuiRect;
use unicode_width::UnicodeWidthStr;

fn test_settings() -> AgentSettings {
    let provider = AgentProvider {
        id: "local".into(),
        name: "Local".into(),
        api_key_env: String::new(),
        api_key: String::new(),
        protocol: None,
        url: None,
        models: vec![AgentModel {
            id: "qwen3-coder".into(),
            name: "qwen3-coder".into(),
            protocol: Protocol::OpenAiChat,
            url: "http://127.0.0.1:11434/v1/chat/completions".into(),
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
        slash_selected: 0,
        queued_inputs: VecDeque::new(),
        queue: crossh_agent::MessageQueue::new(),
        messages: Vec::new(),
        alt_screen: crossh_tui::AltScreen::new(
            80,
            24,
            crossh_tui::alt_screen::AltScreenOptions::default(),
        ),
        screen_renderer: crossh_tui::screen::ScreenRenderer::default(),
        main_renderer: crossh_tui::main_screen::MainScreenRenderer::default(),
        fullscreen: false,
        flashes: crossh_tui::screen::FlashContainer::default(),
        conversation_rect: TuiRect::default(),
        conversation_lines: Vec::new(),
        show_tool_details: false,
        show_reasoning: false,
        thinking: ThinkingLevel::Medium,
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
    assert_eq!(options.thinking, Some(ThinkingLevel::High));
}

#[test]
fn chinese_ime_command_prefix_is_normalized_to_a_slash() {
    assert!(is_command_input("、help"));
    assert_eq!(normalize_command_name("、help"), "/help");
    assert_eq!(normalize_command_name("/MODEL"), "/model");
}

#[test]
fn chinese_punctuation_in_a_prompt_is_not_treated_as_a_command() {
    assert!(!is_command_input("请解释一下、这个实现"));
    assert!(!is_command_input("  请解释一下、这个实现"));
}

#[test]
fn mutating_tools_use_the_language_model_reviewer_by_default() {
    let settings = test_settings();
    assert_eq!(
        tool_approval_source(&settings, "bash"),
        ToolApprovalSource::LanguageModel
    );
    assert_eq!(
        tool_approval_source(&settings, "patch"),
        ToolApprovalSource::LanguageModel
    );
    assert_eq!(
        tool_approval_source(&settings, "read"),
        ToolApprovalSource::None
    );

    assert_eq!(
        tool_approval_source(&AgentSettings::default(), "bash"),
        ToolApprovalSource::User
    );
}

#[test]
fn approval_messages_are_added_to_the_visible_stream() {
    let mut app = app();
    push_approval(&mut app, "Language-model approval granted");
    assert_eq!(
        app.messages,
        vec![(Role::Approval, "Language-model approval granted".into())]
    );
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
            .all(|line| crossh_tui::ansi::visible_width(line) <= 10)
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
        Message::new(MessageRole::User, "first"),
        Message::new(MessageRole::Assistant, "first answer"),
        Message::new(MessageRole::User, "second"),
        Message::new(MessageRole::Assistant, "second answer"),
    ];
    rewind_session(&mut app, 1);
    assert_eq!(app.session.messages.len(), 2);
    assert_eq!(app.session.messages[1].text, "first answer");
}

#[test]
fn input_layout_accounts_for_wrapping_and_wide_characters() {
    assert_eq!(visual_line_count("abcdefgh", 4), 2);
    assert_eq!(visual_line_count("中中文", 4), 2);
    assert_eq!(
        cursor_position(
            TuiRect {
                x: 0,
                y: 0,
                width: 4,
                height: 3
            },
            "abcd",
            4
        ),
        (0, 1)
    );
}

#[test]
fn agent_layout_keeps_prompt_below_the_conversation() {
    let mut app = app();
    app.input = "one\ntwo\nthree\nfour\nfive\nsix\nseven".into();
    let area = TuiRect {
        x: 0,
        y: 0,
        width: 80,
        height: 16,
    };
    let [header, conversation, input, footer] = agent_layout(area, input_height(area, &app));
    assert_eq!(header.bottom(), conversation.y);
    assert_eq!(conversation.bottom(), input.y);
    assert_eq!(input.bottom(), footer.y);
    assert_eq!(footer.bottom(), area.bottom());
    assert!(conversation.height >= 5);
}

#[test]
fn user_message_background_aligns_flush_left_like_agent_messages() {
    let mut app = app();
    app.messages = vec![(Role::User, "hi".into()), (Role::Agent, "reply".into())];
    let width = 40;
    let lines = build_transcript(&mut app, width);
    // 用户消息行：背景序列从第 0 列开始（无前导空格），可见宽度铺满整行
    let user_lines: Vec<&String> = lines
        .iter()
        .filter(|l| l.contains("48;2;52;53;65"))
        .collect();
    assert!(user_lines.len() >= 2, "label 行与正文行都应带背景");
    for l in &user_lines {
        assert!(
            l.starts_with("\x1b[48;"),
            "用户行应以背景序列开头（左侧对齐）: {l:?}"
        );
        assert_eq!(visible_width(l), width, "背景应铺满整行: {l:?}");
    }
    // agent 正文行从第 0 列开始，与用户行左对齐
    let agent_line = lines
        .iter()
        .find(|l| strip(l).contains("reply"))
        .expect("agent line");
    assert!(
        !strip(agent_line).starts_with(' '),
        "agent 行不应有前导空格"
    );

    fn strip(s: &str) -> String {
        crossh_tui::ansi::strip_terminal_sequences(s)
    }
}

#[test]
fn slash_popup_height_is_stable_across_candidate_count_changes() {
    // 回归：候选数变化（8→2→1…）不得改变 popup 高度。
    // 旧实现 popup 随候选数伸缩，dock 高度随之变化，
    // 触发整屏 \x1b[2J 重绘，打字时页面闪烁。
    let mut app = app();
    // 追加第二个模型，使 /model 参数候选数量能真实跨 2→1 变化
    app.settings.providers[0].models.push(AgentModel {
        id: "zephyr-7b".into(),
        name: "zephyr-7b".into(),
        protocol: Protocol::OpenAiChat,
        url: "http://127.0.0.1:11434/v1/chat/completions".into(),
        reasoning: false,
        context_window: 32_000,
        max_tokens: 8_000,
    });
    let width = 80;
    for input in [
        "/",
        "/mod",
        "/model",
        "/model q",
        "/model z",
        "/model local/zephyr-7b",
    ] {
        app.input = input.into();
        app.input_cursor = input.len();
        let popup = build_slash_popup(&app, width).unwrap_or_default();
        assert_eq!(
            popup.len(),
            2 + 8,
            "popup 高度应恒为 2 边框 + 8 候选视口行 (input={input:?}): {popup:?}"
        );
    }
}

#[test]
fn slash_popup_changes_do_not_trigger_full_screen_repaint() {
    use crossh_tui::main_screen::MainScreenRenderer;
    let mut app = app();
    let width = 80;
    let height = 24;

    fn dock_lines(app: &mut App, width: usize) -> Vec<String> {
        let mut editor = build_editor(app);
        editor.max_visible_lines = 8;
        let mut lines = Vec::new();
        if let Some(popup) = build_slash_popup(app, width) {
            lines.extend(popup);
        }
        lines.extend(editor.render(width));
        lines.extend(build_footer(app, width));
        lines
    }

    let mut renderer = MainScreenRenderer::default();
    app.settings.providers[0].models.push(AgentModel {
        id: "zephyr-7b".into(),
        name: "zephyr-7b".into(),
        protocol: Protocol::OpenAiChat,
        url: "http://127.0.0.1:11434/v1/chat/completions".into(),
        reasoning: false,
        context_window: 32_000,
        max_tokens: 8_000,
    });
    app.input = "/model".into();
    app.input_cursor = app.input.len();
    let transcript = build_transcript(&mut app, width);
    let first =
        renderer.render_frame_regular(transcript, dock_lines(&mut app, width), width, height);
    assert!(first.contains("\x1b[2J"), "首帧整屏重绘是预期行为");

    // 候选数量/内容变化（/model → /model q → /model z → 完全匹配）：
    // 只应原位重绘，不得 2J
    for input in [
        "/model q",
        "/model z",
        "/model qw",
        "/model local/zephyr-7b",
    ] {
        app.input = input.into();
        app.input_cursor = app.input.len();
        let transcript = build_transcript(&mut app, width);
        let frame =
            renderer.render_frame_regular(transcript, dock_lines(&mut app, width), width, height);
        assert!(
            !frame.contains("\x1b[2J"),
            "候选变化不应整屏重绘 (input={input:?}): {frame:?}"
        );
    }
}
