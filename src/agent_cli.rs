//! Interactive terminal agent.
//!
//! The agent intentionally stays a normal terminal program. Its state and
//! protocol work live in `crossh-agent`; this module owns the editor, session
//! lifecycle, and the small amount of presentation needed for a focused CLI.

use std::{
    collections::VecDeque,
    io::{self, IsTerminal, Read},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};

use crossh_agent::{
    AgentContentBlock, AgentEvent, AgentMessage, AgentPrompt, AgentRole, AgentSession,
    AgentSessionSummary, AgentSettings, AgentSkill, AgentThinkingLevel, AgentToolCall,
    AgentToolResult, complete_stream_with_options, context_prompt, create_session, export_markdown,
    latest_session, list_sessions, load_context_files, load_prompts, load_session, load_skills,
    review_tool, save_session,
};
use crossh_theme as theme;
use crossterm::{
    event::{
        self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent, KeyEventKind,
        KeyModifiers, MouseEventKind,
    },
    execute,
};
use ratatui::{
    DefaultTerminal, Frame,
    layout::{Constraint, Layout, Margin, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Padding, Paragraph, Wrap},
};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

const SYSTEM_PROMPT: &str = "You are Crossh Agent, a careful coding assistant running in the user's terminal. Inspect the workspace before making claims, use the smallest appropriate tool, keep changes scoped to the request, and report what you changed and how it was verified.";
const SPINNER: [&str; 4] = ["|", "/", "-", "\\"];
const MAX_VISIBLE_INPUT_LINES: usize = 6;
const MAX_FILE_REFERENCE_BYTES: u64 = 32 * 1024;
const MAX_FILE_REFERENCE_TOTAL_BYTES: u64 = 128 * 1024;
const MAX_FILE_REFERENCE_COUNT: usize = 32;

enum ModelUpdate {
    Event(AgentEvent),
    Complete(Result<crossh_agent::AgentResponse, String>),
}

#[derive(Clone, Debug, Default)]
pub(crate) struct AgentOptions {
    pub continue_recent: bool,
    pub resume: Option<String>,
    pub no_session: bool,
    pub model: Option<String>,
    pub thinking: Option<AgentThinkingLevel>,
}

pub(crate) fn parse_options(
    arguments: impl IntoIterator<Item = String>,
) -> Result<AgentOptions, String> {
    let mut options = AgentOptions::default();
    let mut arguments = arguments.into_iter();
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--continue" | "-c" => options.continue_recent = true,
            "--no-session" => options.no_session = true,
            "--resume" | "-r" => {
                options.resume = Some(
                    arguments
                        .next()
                        .ok_or("--resume requires a session id, name, or path")?,
                );
            }
            "--model" | "-m" => {
                options.model = Some(
                    arguments
                        .next()
                        .ok_or("--model requires provider/model or a model id")?,
                );
            }
            "--thinking" => {
                let value = arguments
                    .next()
                    .ok_or("--thinking requires off, minimal, low, medium, high, or xhigh")?;
                options.thinking = Some(
                    parse_thinking(&value)
                        .ok_or_else(|| format!("unknown thinking level: {value}"))?,
                );
            }
            "--help" | "-h" => return Err("help".into()),
            other => return Err(format!("unknown agent option: {other}")),
        }
    }
    if options.no_session && (options.continue_recent || options.resume.is_some()) {
        return Err("--no-session cannot be combined with --continue or --resume".into());
    }
    Ok(options)
}

pub(crate) fn print_help() {
    println!(
        "Usage: crossh agent [OPTIONS]\n\nStart the interactive Crossh coding agent.\n\nOptions:\n  -c, --continue          Continue the most recent project session\n  -r, --resume VALUE      Resume a session by number, id, name, or path\n  -m, --model VALUE       Select provider/model or a model id\n      --thinking LEVEL    Set off, minimal, low, medium, high, or xhigh reasoning\n      --no-session         Do not write a persistent session\n  -h, --help              Print this help\n\nInside the agent, type /help for commands and Ctrl-T/Ctrl-O for display toggles."
    );
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Role {
    User,
    Reasoning,
    Agent,
    Tool,
    Error,
    Notice,
    Queued,
}

struct App {
    settings: AgentSettings,
    api_key: Option<String>,
    workspace: PathBuf,
    context_files: Vec<crossh_agent::AgentContextFile>,
    skills: Vec<AgentSkill>,
    prompts: Vec<AgentPrompt>,
    session_path: Option<PathBuf>,
    session: AgentSession,
    input: String,
    input_cursor: usize,
    history_cursor: Option<usize>,
    queued_inputs: VecDeque<String>,
    messages: Vec<(Role, String)>,
    scroll: u16,
    max_scroll: u16,
    show_tool_details: bool,
    show_reasoning: bool,
    thinking: AgentThinkingLevel,
    thinking_explicit: bool,
    status: String,
    started_at: Instant,
}

pub(crate) fn run_with_options(
    mut settings: AgentSettings,
    options: AgentOptions,
) -> Result<(), String> {
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        return Err("crossh agent requires an interactive terminal".to_string());
    }
    settings = settings.normalized();
    if let Some(selector) = options.model.as_deref() {
        select_model(&mut settings, selector)?;
    }
    settings.validate().map_err(ToString::to_string)?;

    let current_workspace = std::env::current_dir().map_err(|error| error.to_string())?;
    let (session_path, mut session) = open_starting_session(&current_workspace, &options)?;
    let workspace = session
        .cwd
        .canonicalize()
        .unwrap_or_else(|_| current_workspace.clone());
    session.cwd = workspace.clone();
    let api_key = resolve_active_key(&settings)?;
    let context_files = load_context_files(&workspace);
    let skills = load_skills(&workspace);
    let prompts = load_prompts(&workspace);
    let mut app = App {
        settings,
        api_key,
        workspace,
        context_files,
        skills,
        prompts,
        session_path,
        messages: restore_visible_messages(&session.messages),
        session,
        input: String::new(),
        input_cursor: 0,
        history_cursor: None,
        queued_inputs: VecDeque::new(),
        scroll: u16::MAX,
        max_scroll: 0,
        show_tool_details: false,
        show_reasoning: false,
        thinking: options.thinking.unwrap_or(AgentThinkingLevel::Medium),
        thinking_explicit: options.thinking.is_some(),
        status: "Ready  Enter send  Ctrl-T thinking  Ctrl-O tools  Esc quit".into(),
        started_at: Instant::now(),
    };

    execute!(io::stdout(), EnableMouseCapture).map_err(|error| error.to_string())?;
    let result = ratatui::run(|terminal| run_app(terminal, &mut app));
    let cleanup = execute!(io::stdout(), DisableMouseCapture);
    result.and(cleanup).map_err(|error| error.to_string())
}

fn open_starting_session(
    workspace: &Path,
    options: &AgentOptions,
) -> Result<(Option<PathBuf>, AgentSession), String> {
    if options.no_session {
        return Ok((None, AgentSession::new(workspace.to_path_buf())));
    }
    if let Some(selector) = options.resume.as_deref() {
        let summary = find_session(workspace, selector)?
            .ok_or_else(|| format!("session not found: {selector}"))?;
        let session = load_session(&summary.path)?;
        return Ok((Some(summary.path), session));
    }
    if options.continue_recent
        && let Some(summary) = latest_session(workspace)?
    {
        let session = load_session(&summary.path)?;
        return Ok((Some(summary.path), session));
    }
    let (path, session) = create_session(workspace)?;
    Ok((Some(path), session))
}

fn run_app(terminal: &mut DefaultTerminal, app: &mut App) -> io::Result<()> {
    loop {
        terminal.draw(|frame| render(frame, app))?;
        match event::read()? {
            Event::Key(key) if key.kind == KeyEventKind::Press => {
                if handle_key(terminal, app, key)? {
                    return Ok(());
                }
            }
            Event::Mouse(mouse) => match mouse.kind {
                MouseEventKind::ScrollUp => scroll_conversation(app, -3),
                MouseEventKind::ScrollDown => scroll_conversation(app, 3),
                _ => {}
            },
            _ => {}
        }
    }
}

fn handle_key(terminal: &mut DefaultTerminal, app: &mut App, key: KeyEvent) -> io::Result<bool> {
    if key.modifiers.contains(KeyModifiers::CONTROL) {
        match key.code {
            KeyCode::Char('c') => {
                if app.input.is_empty() {
                    return Ok(true);
                }
                clear_input(app);
                app.status = "Input cleared".into();
                return Ok(false);
            }
            KeyCode::Char('o') => {
                app.show_tool_details = !app.show_tool_details;
                app.scroll = u16::MAX;
                app.status = if app.show_tool_details {
                    "Tool output expanded".into()
                } else {
                    "Tool output collapsed".into()
                };
                return Ok(false);
            }
            KeyCode::Char('t') => {
                app.show_reasoning = !app.show_reasoning;
                app.scroll = u16::MAX;
                app.status = if app.show_reasoning {
                    "Thinking expanded".into()
                } else {
                    "Thinking collapsed".into()
                };
                return Ok(false);
            }
            _ => {}
        }
    }
    if key.code == KeyCode::Esc {
        if app.input.is_empty() {
            return Ok(true);
        }
        clear_input(app);
        app.status = "Input cleared".into();
        return Ok(false);
    }
    if is_enter_key(key.code) {
        if key.modifiers.contains(KeyModifiers::SHIFT) {
            insert_text(app, "\n");
            return Ok(false);
        }
        if app.input.trim().is_empty() {
            return Ok(false);
        }
        if app.input.trim_start().starts_with('/') {
            let command = take_input(app);
            return handle_command(terminal, app, command);
        }
        if let Some(command) = app.input.strip_prefix("!!") {
            let command = command.trim().to_string();
            clear_input(app);
            if !command.is_empty() {
                run_shell_shortcut(terminal, app, command, false)?;
            }
            return Ok(false);
        }
        if let Some(command) = app.input.strip_prefix('!') {
            let command = command.trim().to_string();
            clear_input(app);
            if !command.is_empty() {
                run_shell_shortcut(terminal, app, command, true)?;
            }
            return Ok(false);
        }
        submit(terminal, app)?;
        return Ok(false);
    }
    if matches!(key.code, KeyCode::Up | KeyCode::Down)
        && !app.input.contains('\n')
        && (app.input_cursor == 0 || app.input_cursor == app.input.len())
    {
        move_history(app, key.code == KeyCode::Up);
        return Ok(false);
    }
    edit_input(app, key);
    Ok(false)
}

fn submit(terminal: &mut DefaultTerminal, app: &mut App) -> io::Result<()> {
    let mut prompts = VecDeque::new();
    prompts.push_back(take_input(app));
    while let Some(prompt) = prompts
        .pop_front()
        .or_else(|| app.queued_inputs.pop_front())
    {
        if prompt.trim().is_empty() {
            continue;
        }
        if !process_prompt(terminal, app, prompt)? {
            app.queued_inputs.clear();
            return Ok(());
        }
        if prompts.is_empty() && !app.queued_inputs.is_empty() {
            prompts.push_back(
                app.queued_inputs
                    .pop_front()
                    .expect("queue was checked as non-empty"),
            );
        }
    }
    app.status = "Ready".into();
    Ok(())
}

fn process_prompt(
    terminal: &mut DefaultTerminal,
    app: &mut App,
    prompt: String,
) -> io::Result<bool> {
    let user_request = prompt.clone();
    app.session
        .append(AgentMessage::new(AgentRole::User, prompt.clone()));
    app.messages.push((Role::User, prompt));
    app.scroll = u16::MAX;
    persist_session(app);

    let removed = app.session.compact(compaction_limit(app));
    if removed > 0 {
        app.messages.push((
            Role::Notice,
            format!("Compacted {removed} older messages to stay within the model context."),
        ));
        persist_session(app);
    }

    let mut request_messages = request_messages(app);
    if let Some(message) = request_messages
        .iter_mut()
        .rev()
        .find(|message| message.role == AgentRole::User && !message.text.is_empty())
    {
        message.text = expand_file_references(&app.workspace, &message.text);
    }
    for round in 0..app.settings.max_tool_rounds {
        let settings = app.settings.clone();
        let api_key = app.api_key.clone();
        let request = request_messages.clone();
        let thinking = app.thinking_explicit.then_some(app.thinking);
        let (updates_tx, updates_rx) = mpsc::channel();
        let request_settings = settings.clone();
        let request_api_key = api_key.clone();
        let task = crossh_ssh::ssh_runtime().spawn(async move {
            let event_tx = updates_tx.clone();
            let result = complete_stream_with_options(
                &request_settings,
                request_api_key.as_deref(),
                &request,
                thinking,
                move |event| {
                    let _ = event_tx.send(ModelUpdate::Event(event.clone()));
                },
            )
            .await;
            let _ = updates_tx.send(ModelUpdate::Complete(result));
        });

        let response = match wait_for_model(terminal, app, updates_rx, task)? {
            WaitResult::Cancelled => {
                app.messages
                    .push((Role::Notice, "Request cancelled".into()));
                app.status = "Cancelled".into();
                return Ok(false);
            }
            WaitResult::Complete(result) => match result {
                Ok(response) => response,
                Err(error) => {
                    app.messages.push((Role::Error, error));
                    app.status = "Request failed".into();
                    return Ok(true);
                }
            },
        };

        let text = response.text();
        let protocol_items = response.protocol_items.clone();
        let calls = response
            .content
            .iter()
            .filter_map(|block| match block {
                AgentContentBlock::ToolCall(call) => Some(call.clone()),
                _ => None,
            })
            .collect::<Vec<_>>();
        if calls.is_empty() {
            app.session.append(AgentMessage {
                role: AgentRole::Assistant,
                text,
                tool_calls: Vec::new(),
                tool_result: None,
                protocol_items,
            });
            persist_session(app);
            app.status = "Ready".into();
            return Ok(true);
        }

        let assistant_message = AgentMessage {
            role: AgentRole::Assistant,
            text,
            tool_calls: calls.clone(),
            tool_result: None,
            protocol_items,
        };
        request_messages.push(assistant_message.clone());
        app.session.append(assistant_message);
        for call in calls {
            app.messages.push((Role::Tool, format_tool_call(&call)));
            app.status = format!("Tool requested: {}", call.name);
            let mut approved = true;
            if reviewer_is_distinct(app) {
                approved = match review_tool_animated(terminal, app, &call, &user_request)? {
                    BackgroundResult::Complete(result) => result,
                    BackgroundResult::Cancelled => {
                        app.messages
                            .push((Role::Notice, "Tool review cancelled".into()));
                        app.status = "Cancelled".into();
                        return Ok(false);
                    }
                };
            }
            if approved && tool_requires_approval(&call.name) {
                approved = confirm_tool(terminal, app, &call)?;
            }
            let result = if approved {
                match execute_tool_animated(terminal, app, call.clone(), app.workspace.clone())? {
                    BackgroundResult::Complete(result) => result,
                    BackgroundResult::Cancelled => {
                        app.messages
                            .push((Role::Notice, "Tool execution cancelled".into()));
                        app.status = "Cancelled".into();
                        return Ok(false);
                    }
                }
            } else {
                AgentToolResult {
                    call_id: call.id.clone(),
                    output: "Tool execution denied by the user".into(),
                    is_error: true,
                }
            };
            app.messages.push((Role::Tool, format_tool_result(&result)));
            request_messages.push(AgentMessage::tool_result(result.clone()));
            app.session.append(AgentMessage::tool_result(result));
            persist_session(app);
        }
        app.status = format!("Completed tool round {}", round + 1);
        app.scroll = u16::MAX;
    }
    app.messages
        .push((Role::Error, "Tool loop limit reached".into()));
    app.status = "Tool loop limit reached".into();
    Ok(true)
}

enum WaitResult {
    Complete(Result<crossh_agent::AgentResponse, String>),
    Cancelled,
}

fn wait_for_model(
    terminal: &mut DefaultTerminal,
    app: &mut App,
    receiver: mpsc::Receiver<ModelUpdate>,
    task: tokio::task::JoinHandle<()>,
) -> io::Result<WaitResult> {
    let mut spinner_frame = 0;
    loop {
        while let Ok(update) = receiver.try_recv() {
            match update {
                ModelUpdate::Event(event) => match event {
                    AgentEvent::TextDelta(delta) => append_delta(app, Role::Agent, &delta),
                    AgentEvent::ReasoningDelta(delta) => append_delta(app, Role::Reasoning, &delta),
                    AgentEvent::ToolCallStart { name, .. } => {
                        app.status = format!("Preparing tool: {name}")
                    }
                    AgentEvent::ToolCallArgumentsDelta { .. } | AgentEvent::Stop(_) => {}
                },
                ModelUpdate::Complete(result) => return Ok(WaitResult::Complete(result)),
            }
        }
        if event::poll(Duration::from_millis(80))? {
            match event::read()? {
                Event::Key(key) if key.kind == KeyEventKind::Press => {
                    if key.code == KeyCode::Esc
                        || (key.modifiers.contains(KeyModifiers::CONTROL)
                            && key.code == KeyCode::Char('c'))
                    {
                        task.abort();
                        return Ok(WaitResult::Cancelled);
                    }
                    if is_enter_key(key.code) && !key.modifiers.contains(KeyModifiers::SHIFT) {
                        queue_input(app);
                    } else if key.modifiers.contains(KeyModifiers::CONTROL)
                        && key.code == KeyCode::Char('o')
                    {
                        app.show_tool_details = !app.show_tool_details;
                    } else if key.modifiers.contains(KeyModifiers::CONTROL)
                        && key.code == KeyCode::Char('t')
                    {
                        app.show_reasoning = !app.show_reasoning;
                    } else {
                        edit_input(app, key);
                    }
                }
                Event::Mouse(mouse) => match mouse.kind {
                    MouseEventKind::ScrollUp => scroll_conversation(app, -3),
                    MouseEventKind::ScrollDown => scroll_conversation(app, 3),
                    _ => {}
                },
                _ => {}
            }
        }
        set_spinner_status(app, "Working", spinner_frame);
        spinner_frame += 1;
        app.scroll = u16::MAX;
        terminal.draw(|frame| render(frame, app))?;
    }
}

fn confirm_tool(
    terminal: &mut DefaultTerminal,
    app: &mut App,
    call: &AgentToolCall,
) -> io::Result<bool> {
    let previous_details = app.show_tool_details;
    app.show_tool_details = true;
    app.messages.push((
        Role::Notice,
        format!(
            "Approval required for tool execution.\n\nTool: {}\nArguments:\n{}",
            call.name,
            pretty_tool_arguments(&call.arguments)
        ),
    ));
    app.status = format!("Allow {}?  y/Enter allow  n/Esc deny", call.name);
    let decision = loop {
        terminal.draw(|frame| render(frame, app))?;
        if !event::poll(Duration::from_millis(100))? {
            continue;
        }
        let Event::Key(key) = event::read()? else {
            continue;
        };
        if key.kind != KeyEventKind::Press {
            continue;
        }
        match key.code {
            KeyCode::Char('y') | KeyCode::Enter | KeyCode::Char('\r') | KeyCode::Char('\n') => {
                break true;
            }
            KeyCode::Char('n') | KeyCode::Esc => break false,
            _ => {}
        }
    };
    app.show_tool_details = previous_details;
    Ok(decision)
}

fn reviewer_is_distinct(app: &App) -> bool {
    app.settings.reviewer_model != app.settings.active_model
}

fn review_tool_animated(
    terminal: &mut DefaultTerminal,
    app: &mut App,
    call: &AgentToolCall,
    user_request: &str,
) -> io::Result<BackgroundResult<bool>> {
    let settings = app.settings.clone();
    let key = match resolve_model_key(&settings, &settings.reviewer_model) {
        Ok(key) => key,
        Err(error) => {
            app.status = format!("Reviewer unavailable: {error}");
            return Ok(BackgroundResult::Complete(false));
        }
    };
    let tool_name = call.name.clone();
    let call = call.clone();
    let workspace = app.workspace.clone();
    let user_request = user_request.to_string();
    let (tx, rx) = mpsc::channel();
    let task = crossh_ssh::ssh_runtime().spawn(async move {
        let result = review_tool(&settings, key.as_deref(), &call, &workspace, &user_request)
            .await
            .unwrap_or(false);
        let _ = tx.send(result);
    });
    app.status = format!("Reviewing {tool_name} with the secondary model");
    wait_for_background(terminal, app, "Reviewing tool", rx, move || task.abort())
}

fn run_shell_shortcut(
    terminal: &mut DefaultTerminal,
    app: &mut App,
    command: String,
    send_to_agent: bool,
) -> io::Result<()> {
    let call = AgentToolCall {
        id: format!("shell-{}", app.session.messages.len()),
        name: "bash".into(),
        arguments: serde_json::json!({"command": command}).to_string(),
    };
    app.messages.push((Role::Tool, format_tool_call(&call)));
    let result = match execute_tool_animated(terminal, app, call, app.workspace.clone())? {
        BackgroundResult::Complete(result) => result,
        BackgroundResult::Cancelled => {
            app.status = "Cancelled".into();
            return Ok(());
        }
    };
    app.messages.push((Role::Tool, format_tool_result(&result)));
    if send_to_agent {
        let prompt = format!(
            "The user ran this shell command in the workspace:\n\n`{command}`\n\nCommand output:\n\n```text\n{}\n```\n\nUse this result to continue.",
            result.output
        );
        process_prompt(terminal, app, prompt)?;
    } else {
        app.status = "Command finished without sending output to the model".into();
    }
    Ok(())
}

fn execute_tool_animated(
    terminal: &mut DefaultTerminal,
    app: &mut App,
    call: AgentToolCall,
    workspace: PathBuf,
) -> io::Result<BackgroundResult<AgentToolResult>> {
    let label = call.name.clone();
    let (tx, rx) = mpsc::channel();
    let cancel = Arc::new(AtomicBool::new(false));
    let worker_cancel = cancel.clone();
    std::thread::spawn(move || {
        let _ = tx.send(crossh_agent::execute_tool_with_cancel(
            &call,
            &workspace,
            &worker_cancel,
        ));
    });
    wait_for_background(terminal, app, &format!("Running {label}"), rx, move || {
        cancel.store(true, Ordering::Relaxed)
    })
}

enum BackgroundResult<T> {
    Complete(T),
    Cancelled,
}

fn wait_for_background<T>(
    terminal: &mut DefaultTerminal,
    app: &mut App,
    label: &str,
    receiver: mpsc::Receiver<T>,
    cancel: impl FnOnce(),
) -> io::Result<BackgroundResult<T>> {
    let mut frame = 0;
    loop {
        match receiver.try_recv() {
            Ok(result) => return Ok(BackgroundResult::Complete(result)),
            Err(mpsc::TryRecvError::Disconnected) => {
                return Err(io::Error::other("background task stopped unexpectedly"));
            }
            Err(mpsc::TryRecvError::Empty) => {}
        }
        set_spinner_status(app, label, frame);
        frame += 1;
        terminal.draw(|frame| render(frame, app))?;
        if event::poll(Duration::from_millis(80))?
            && matches!(event::read()?, Event::Key(key) if key.kind == KeyEventKind::Press && (key.code == KeyCode::Esc || key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL)))
        {
            cancel();
            app.status = format!("{label} cancelled");
            return Ok(BackgroundResult::Cancelled);
        }
    }
}

fn handle_command(
    terminal: &mut DefaultTerminal,
    app: &mut App,
    input: String,
) -> io::Result<bool> {
    let mut parts = input.trim().splitn(2, char::is_whitespace);
    let command = parts.next().unwrap_or_default().to_ascii_lowercase();
    let argument = parts.next().unwrap_or_default().trim();
    match command.as_str() {
        "/quit" | "/exit" => return Ok(true),
        "/help" | "/hotkeys" => push_notice(
            app,
            format!(
                "{}\n\nTree navigation:\n  /tree                 List previous user turns\n  /tree NUMBER          Rewind to a turn before continuing\n\nShell shortcuts:\n  !command              Run and send output to the model\n  !!command             Run without sending output",
                help_text()
            ),
        ),
        "/tools" => push_notice(app, tools_text()),
        "/skills" => push_notice(app, skills_text(app)),
        "/prompts" => push_notice(app, prompts_text(app)),
        "/skill" => run_skill_command(terminal, app, argument)?,
        "/prompt" => run_prompt_command(terminal, app, argument)?,
        "/model" => {
            if argument.is_empty() {
                push_notice(app, model_options_text(app));
            } else if let Err(error) = switch_model(app, argument) {
                push_error(app, error);
            }
        }
        "/thinking" => {
            if argument.is_empty() {
                push_notice(
                    app,
                    format!(
                        "Thinking: {}\nAvailable: {}",
                        app.thinking.label(),
                        AgentThinkingLevel::ALL
                            .iter()
                            .map(|level| level.label())
                            .collect::<Vec<_>>()
                            .join(", ")
                    ),
                );
            } else if let Some(level) = parse_thinking(argument) {
                app.thinking = level;
                app.thinking_explicit = true;
                app.status = format!("Thinking set to {}", level.label());
            } else {
                push_error(app, format!("Unknown thinking level: {argument}"));
            }
        }
        "/new" | "/clear" => new_session(app),
        "/continue" => {
            if app.session_path.is_none() {
                push_error(app, "Session persistence is disabled by --no-session.");
            } else {
                match latest_session(&app.workspace) {
                    Ok(Some(summary)) => resume_session(app, summary),
                    Ok(None) => push_notice(app, "No saved sessions for this project."),
                    Err(error) => push_error(app, error),
                }
            }
        }
        "/resume" => {
            if app.session_path.is_none() {
                push_error(app, "Session persistence is disabled by --no-session.");
            } else if argument.is_empty() {
                match list_sessions(&app.workspace) {
                    Ok(sessions) => push_notice(app, format_session_list(&sessions)),
                    Err(error) => push_error(app, error),
                }
            } else {
                match find_session(&app.workspace, argument) {
                    Ok(Some(summary)) => resume_session(app, summary),
                    Ok(None) => push_error(app, format!("Session not found: {argument}")),
                    Err(error) => push_error(app, error),
                }
            }
        }
        "/tree" => {
            if argument.is_empty() {
                push_notice(app, tree_text(app));
            } else if let Ok(index) = argument.parse::<usize>() {
                rewind_session(app, index);
            } else {
                push_error(app, "Tree position must be a number.");
            }
        }
        "/fork" | "/clone" => fork_session(app),
        "/name" => {
            if argument.is_empty() {
                push_notice(app, format!("Session name: {}", session_name(app)));
            } else {
                app.session.set_name(Some(argument.to_string()));
                persist_session(app);
                app.status = format!("Session named {argument}");
            }
        }
        "/session" | "/stats" => push_notice(app, session_info(app)),
        "/compact" => {
            let removed = app.session.compact(compaction_limit(app));
            persist_session(app);
            app.messages.push((
                Role::Notice,
                if removed == 0 {
                    "Context is already compact.".into()
                } else {
                    format!("Compacted {removed} older messages.")
                },
            ));
        }
        "/reload" => {
            app.context_files = load_context_files(&app.workspace);
            app.skills = load_skills(&app.workspace);
            app.prompts = load_prompts(&app.workspace);
            push_notice(
                app,
                format!(
                    "Reloaded {} context files, {} skills, and {} prompts.",
                    app.context_files.len(),
                    app.skills.len(),
                    app.prompts.len()
                ),
            );
        }
        "/export" => {
            let path = if argument.is_empty() {
                app.workspace
                    .join(format!("crossh-session-{}.md", app.session.id))
            } else {
                app.workspace.join(argument)
            };
            match export_markdown(&app.session, &path) {
                Ok(()) => push_notice(app, format!("Exported session to {}", path.display())),
                Err(error) => push_error(app, error),
            }
        }
        "" => {}
        _ => push_error(app, format!("Unknown command: {command}. Try /help.")),
    }
    app.scroll = u16::MAX;
    Ok(false)
}

fn help_text() -> String {
    "Commands:\n  /help, /hotkeys       Show commands and shortcuts\n  /model [value]        List or switch provider/model\n  /thinking [level]     Set reasoning level\n  /tools                Show available tools\n  /skills               List project skills\n  /skill NAME [request] Apply a skill to a request\n  /prompts              List prompt templates\n  /prompt NAME [args]   Run a prompt template\n  /new, /clear          Start a fresh session\n  /continue             Resume the most recent session\n  /resume [value]       List or resume a saved session\n  /fork, /clone         Branch the current conversation\n  /name [value]         Set or show the session name\n  /session, /stats      Show session and context details\n  /compact              Compact older conversation context\n  /reload               Reload project instructions and resources\n  /export [path]        Export the session as Markdown\n  /quit, /exit          Quit\n\nShortcuts:\n  Enter                 Send prompt\n  Shift+Enter           Insert a new line\n  Escape                Clear input, then quit\n  Ctrl+C                Clear input, then quit\n  Ctrl+T                Expand or collapse thinking\n  Ctrl+O                Expand or collapse tool output\n  Up/Down               Browse prompt history\n  While working, Enter  Queue a follow-up prompt"
        .into()
}

fn tools_text() -> String {
    crossh_agent::builtin_tools()
        .iter()
        .map(|tool| {
            format!(
                "{}  {}{}",
                tool.name,
                tool.description,
                if tool.requires_approval {
                    "  [approval]"
                } else {
                    ""
                }
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn skills_text(app: &App) -> String {
    if app.skills.is_empty() {
        return "No project skills found. Add .agents/skills/<name>/SKILL.md or .pi/skills/<name>/SKILL.md.".into();
    }
    let mut lines = vec!["Project skills:".to_string()];
    for skill in &app.skills {
        lines.push(format!(
            "  {:<18} {}  ({})",
            skill.name,
            skill.description(),
            skill.path.display()
        ));
    }
    lines.join("\n")
}

fn prompts_text(app: &App) -> String {
    if app.prompts.is_empty() {
        return "No prompt templates found. Add .pi/prompts/<name>.md or prompts/<name>.md.".into();
    }
    let mut lines = vec!["Prompt templates:".to_string()];
    for prompt in &app.prompts {
        lines.push(format!(
            "  {:<18} {}  ({})",
            prompt.name,
            prompt.description(),
            prompt.path.display()
        ));
    }
    lines.join("\n")
}

fn run_skill_command(
    terminal: &mut DefaultTerminal,
    app: &mut App,
    argument: &str,
) -> io::Result<()> {
    let mut parts = argument.splitn(2, char::is_whitespace);
    let name = parts.next().unwrap_or_default().trim();
    if name.is_empty() {
        push_notice(app, skills_text(app));
        return Ok(());
    }
    let Some(skill) = app
        .skills
        .iter()
        .find(|skill| skill.name.eq_ignore_ascii_case(name))
        .cloned()
    else {
        push_error(app, format!("Skill not found: {name}"));
        return Ok(());
    };
    let request = parts.next().unwrap_or_default().trim();
    let prompt = if request.is_empty() {
        format!(
            "Apply this project skill to the current task.\n\nSkill: {}\nSource: {}\n\n{}",
            skill.name,
            skill.path.display(),
            skill.content.trim()
        )
    } else {
        format!(
            "Apply this project skill while handling the user's request.\n\nSkill: {}\nSource: {}\n\nSkill instructions:\n{}\n\nUser request:\n{}",
            skill.name,
            skill.path.display(),
            skill.content.trim(),
            request
        )
    };
    process_prompt(terminal, app, prompt)?;
    Ok(())
}

fn run_prompt_command(
    terminal: &mut DefaultTerminal,
    app: &mut App,
    argument: &str,
) -> io::Result<()> {
    let mut parts = argument.splitn(2, char::is_whitespace);
    let name = parts.next().unwrap_or_default().trim();
    if name.is_empty() {
        push_notice(app, prompts_text(app));
        return Ok(());
    }
    let Some(prompt) = app
        .prompts
        .iter()
        .find(|prompt| prompt.name.eq_ignore_ascii_case(name))
        .cloned()
    else {
        push_error(app, format!("Prompt template not found: {name}"));
        return Ok(());
    };
    let arguments = parts.next().unwrap_or_default().trim();
    let expanded = expand_prompt_template(&prompt.content, arguments);
    process_prompt(terminal, app, expanded)?;
    Ok(())
}

fn expand_prompt_template(template: &str, arguments: &str) -> String {
    let mut expanded = template.replace("$ARGUMENTS", arguments);
    expanded = expanded.replace("{{args}}", arguments);
    if !arguments.is_empty() && expanded == template {
        expanded.push_str("\n\nUser-provided arguments:\n");
        expanded.push_str(arguments);
    }
    expanded.trim().to_string()
}

fn model_options_text(app: &App) -> String {
    let mut lines = vec!["Models:".to_string()];
    for provider in &app.settings.providers {
        for model in &provider.models {
            let active = app.settings.active_model.provider == provider.id
                && app.settings.active_model.model == model.id;
            lines.push(format!(
                "{} {}/{}{}",
                if active { "*" } else { " " },
                provider.id,
                model.id,
                if model.reasoning { "  reasoning" } else { "" }
            ));
        }
    }
    lines.join("\n")
}

fn session_info(app: &App) -> String {
    let token_estimate = estimate_tokens(&app.session.messages);
    format!(
        "Session: {}\nId: {}\nWorking directory: {}\nMessages: {}\nContext estimate: {} tokens / {}\nContext files: {}\nSkills: {}\nPrompts: {}\nThinking: {}\nUptime: {}s",
        session_name(app),
        app.session.id,
        app.workspace.display(),
        app.session.messages.len(),
        token_estimate,
        active_context_limit(app),
        app.context_files.len(),
        app.skills.len(),
        app.prompts.len(),
        app.thinking.label(),
        app.started_at.elapsed().as_secs()
    )
}

fn format_session_list(sessions: &[AgentSessionSummary]) -> String {
    if sessions.is_empty() {
        return "No saved sessions for this project.".into();
    }
    let mut lines = vec!["Saved sessions:".to_string()];
    for (index, session) in sessions.iter().take(20).enumerate() {
        lines.push(format!(
            "{:>2}. {}  {} messages  {}",
            index + 1,
            session.label(),
            session.message_count,
            session.cwd.display()
        ));
    }
    lines.join("\n")
}

fn switch_model(app: &mut App, selector: &str) -> Result<(), String> {
    let mut settings = app.settings.clone();
    select_model(&mut settings, selector)?;
    let api_key = resolve_active_key(&settings)?;
    app.settings = settings;
    app.api_key = api_key;
    app.status = format!("Model set to {}", active_model_label(app));
    Ok(())
}

fn select_model(settings: &mut AgentSettings, selector: &str) -> Result<(), String> {
    let selector = selector.trim();
    let (provider_id, model_id) = if let Some((provider_id, model_id)) = selector.split_once('/') {
        let provider = settings
            .providers
            .iter()
            .find(|provider| {
                provider.id.eq_ignore_ascii_case(provider_id)
                    || provider.name.eq_ignore_ascii_case(provider_id)
            })
            .ok_or_else(|| format!("provider not found: {provider_id}"))?;
        let model = provider
            .models
            .iter()
            .find(|model| {
                model.id.eq_ignore_ascii_case(model_id) || model.name.eq_ignore_ascii_case(model_id)
            })
            .ok_or_else(|| format!("model not found: {model_id}"))?;
        (provider.id.clone(), model.id.clone())
    } else {
        let mut matches = settings.providers.iter().flat_map(|provider| {
            provider
                .models
                .iter()
                .filter(move |model| {
                    model.id.eq_ignore_ascii_case(selector)
                        || model.name.eq_ignore_ascii_case(selector)
                })
                .map(move |model| (provider.id.clone(), model.id.clone()))
        });
        let selected = matches
            .next()
            .ok_or_else(|| format!("model not found: {selector}"))?;
        if matches.next().is_some() {
            return Err(format!(
                "model is ambiguous; use provider/model: {selector}"
            ));
        }
        selected
    };
    settings.active_model.provider = provider_id;
    settings.active_model.model = model_id;
    Ok(())
}

fn find_session(workspace: &Path, selector: &str) -> Result<Option<AgentSessionSummary>, String> {
    if Path::new(selector).is_file() {
        let session = load_session(Path::new(selector))?;
        return Ok(Some(AgentSessionSummary {
            path: PathBuf::from(selector),
            id: session.id,
            name: session.name,
            cwd: session.cwd,
            updated_at: session.updated_at,
            message_count: session.messages.len(),
        }));
    }
    let sessions = list_sessions(workspace)?;
    if let Ok(index) = selector.parse::<usize>() {
        return Ok(index
            .checked_sub(1)
            .and_then(|index| sessions.get(index).cloned()));
    }
    Ok(sessions.into_iter().find(|session| {
        session.id.starts_with(selector)
            || session
                .name
                .as_deref()
                .is_some_and(|name| name.eq_ignore_ascii_case(selector))
    }))
}

fn resume_session(app: &mut App, summary: AgentSessionSummary) {
    if app.session_path.is_none() {
        push_error(app, "Session persistence is disabled by --no-session.");
        return;
    }
    match load_session(&summary.path) {
        Ok(session) => {
            app.workspace = session
                .cwd
                .canonicalize()
                .unwrap_or_else(|_| app.workspace.clone());
            app.context_files = load_context_files(&app.workspace);
            app.skills = load_skills(&app.workspace);
            app.prompts = load_prompts(&app.workspace);
            app.session_path = Some(summary.path);
            app.session = session;
            app.messages = restore_visible_messages(&app.session.messages);
            clear_input(app);
            app.scroll = u16::MAX;
            app.status = format!("Resumed {}", session_name(app));
        }
        Err(error) => push_error(app, error),
    }
}

fn new_session(app: &mut App) {
    if app.session_path.is_none() {
        app.session = AgentSession::new(app.workspace.clone());
        app.messages.clear();
        app.queued_inputs.clear();
        app.context_files = load_context_files(&app.workspace);
        app.skills = load_skills(&app.workspace);
        app.prompts = load_prompts(&app.workspace);
        clear_input(app);
        app.status = "New in-memory session".into();
        return;
    }
    match create_session(&app.workspace) {
        Ok((path, session)) => {
            app.session_path = Some(path);
            app.session = session;
            app.messages.clear();
            app.queued_inputs.clear();
            app.context_files = load_context_files(&app.workspace);
            app.skills = load_skills(&app.workspace);
            app.prompts = load_prompts(&app.workspace);
            clear_input(app);
            app.status = "New session".into();
        }
        Err(error) => push_error(app, error),
    }
}

fn fork_session(app: &mut App) {
    if app.session_path.is_none() {
        let mut session = AgentSession::new(app.workspace.clone());
        session.messages = app.session.messages.clone();
        session.name = app.session.name.as_ref().map(|name| format!("{name} fork"));
        app.session = session;
        app.status = "Forked in-memory session".into();
        return;
    }
    match create_session(&app.workspace) {
        Ok((path, mut session)) => {
            session.messages = app.session.messages.clone();
            session.name = app.session.name.as_ref().map(|name| format!("{name} fork"));
            if let Err(error) = save_session(&path, &session) {
                push_error(app, error);
                return;
            }
            app.session_path = Some(path);
            app.session = session;
            app.status = "Forked current session".into();
        }
        Err(error) => push_error(app, error),
    }
}

fn tree_text(app: &App) -> String {
    let mut lines = vec!["Conversation tree (user turns):".to_string()];
    let mut turn = 0;
    for message in &app.session.messages {
        if message.role != AgentRole::User || message.text.is_empty() {
            continue;
        }
        turn += 1;
        lines.push(format!("{:>2}. {}", turn, one_line(&message.text)));
    }
    if turn == 0 {
        lines.push("No user turns yet.".into());
    } else {
        lines.push("Use /tree NUMBER to rewind this session.".into());
    }
    lines.join("\n")
}

fn rewind_session(app: &mut App, turn: usize) {
    if turn == 0 {
        app.session.messages.clear();
    } else {
        let user_turns = app
            .session
            .messages
            .iter()
            .enumerate()
            .filter(|(_, message)| message.role == AgentRole::User && !message.text.is_empty())
            .map(|(index, _)| index)
            .collect::<Vec<_>>();
        let Some(message_index) = user_turns.get(turn.saturating_sub(1)).copied() else {
            push_error(app, format!("User turn not found: {turn}"));
            return;
        };
        let end = user_turns
            .get(turn)
            .copied()
            .unwrap_or(app.session.messages.len());
        debug_assert!(message_index < end);
        app.session.messages.truncate(end);
    }
    app.messages = restore_visible_messages(&app.session.messages);
    persist_session(app);
    app.scroll = u16::MAX;
    app.status = format!("Rewound to user turn {turn}");
}

fn request_messages(app: &App) -> Vec<AgentMessage> {
    let mut messages = vec![AgentMessage::new(AgentRole::System, system_prompt(app))];
    messages.extend(app.session.messages.iter().cloned());
    messages
}

fn system_prompt(app: &App) -> String {
    let context = context_prompt(&app.context_files);
    let mut system = format!(
        "{SYSTEM_PROMPT}\n\nWorkspace: {}\nThinking preference: {}\nAvailable tools: read, grep, find, ls, edit, write, bash.",
        app.workspace.display(),
        app.thinking.label()
    );
    if !context.is_empty() {
        system.push_str("\n\nProject instructions:\n");
        system.push_str(&context);
    }
    if !app.skills.is_empty() {
        system.push_str("\n\nProject skills available through /skill NAME:\n");
        system.push_str(
            &app.skills
                .iter()
                .map(|skill| format!("- {}: {}", skill.name, skill.description()))
                .collect::<Vec<_>>()
                .join("\n"),
        );
    }
    let limit = system_prompt_limit(app);
    if system.len() > limit {
        const TRUNCATION_NOTICE: &str =
            "\n\n[Project instructions truncated to fit the model context.]";
        let content_limit = limit.saturating_sub(TRUNCATION_NOTICE.len());
        let end = system.floor_char_boundary(content_limit.min(system.len()));
        system.truncate(end);
        system.push_str(TRUNCATION_NOTICE);
    }
    system
}

fn expand_file_references(workspace: &Path, prompt: &str) -> String {
    let mut references = Vec::new();
    let mut referenced_bytes = 0_u64;
    for token in prompt.split_whitespace() {
        if references.len() >= MAX_FILE_REFERENCE_COUNT {
            references.push(format!(
                "Referenced file expansion stopped after {MAX_FILE_REFERENCE_COUNT} files; inspect additional files with the read tool."
            ));
            break;
        }
        let Some(value) = token.strip_prefix('@').filter(|value| !value.is_empty()) else {
            continue;
        };
        let path = Path::new(value);
        if path.is_absolute()
            || path
                .components()
                .any(|component| matches!(component, std::path::Component::ParentDir))
        {
            continue;
        }
        let Ok(path) = workspace.join(path).canonicalize() else {
            continue;
        };
        if !path.starts_with(workspace) || !path.is_file() {
            continue;
        }
        let Ok(metadata) = std::fs::metadata(&path) else {
            continue;
        };
        if metadata.len() > MAX_FILE_REFERENCE_BYTES {
            references.push(format!(
                "Referenced file {} is larger than 32 KiB; inspect it with the read tool.",
                path.display()
            ));
        } else {
            if referenced_bytes.saturating_add(metadata.len()) > MAX_FILE_REFERENCE_TOTAL_BYTES {
                references.push(format!(
                    "Referenced file expansion stopped after {} KiB; inspect additional files with the read tool.",
                    MAX_FILE_REFERENCE_TOTAL_BYTES / 1024
                ));
                break;
            }
            let Ok(file) = std::fs::File::open(&path) else {
                continue;
            };
            let mut content = String::new();
            if file
                .take(MAX_FILE_REFERENCE_BYTES.saturating_add(1))
                .read_to_string(&mut content)
                .is_err()
            {
                continue;
            }
            if content.len() as u64 > MAX_FILE_REFERENCE_BYTES {
                references.push(format!(
                    "Referenced file {} is larger than 32 KiB; inspect it with the read tool.",
                    path.display()
                ));
                continue;
            }
            referenced_bytes = referenced_bytes.saturating_add(metadata.len());
            references.push(format!(
                "Referenced file: {}\n```text\n{}\n```",
                path.display(),
                content
            ));
        }
    }
    if references.is_empty() {
        prompt.to_string()
    } else {
        format!("{prompt}\n\n{}", references.join("\n\n"))
    }
}

fn restore_visible_messages(messages: &[AgentMessage]) -> Vec<(Role, String)> {
    let mut visible = Vec::new();
    for message in messages {
        match message.role {
            AgentRole::System => visible.push((Role::Notice, message.text.clone())),
            AgentRole::User => {
                if !message.text.is_empty() {
                    visible.push((Role::User, message.text.clone()));
                }
            }
            AgentRole::Assistant => {
                if !message.text.is_empty() {
                    visible.push((Role::Agent, message.text.clone()));
                }
                for call in &message.tool_calls {
                    visible.push((Role::Tool, format_tool_call(call)));
                }
            }
        }
        if let Some(result) = &message.tool_result {
            visible.push((Role::Tool, format_tool_result(result)));
        }
    }
    visible
}

fn persist_session(app: &mut App) {
    let Some(path) = &app.session_path else {
        return;
    };
    if let Err(error) = save_session(path, &app.session) {
        app.status = format!("Session save failed: {error}");
    }
}

fn resolve_active_key(settings: &AgentSettings) -> Result<Option<String>, String> {
    resolve_model_key(settings, &settings.active_model)
}

fn resolve_model_key(
    settings: &AgentSettings,
    reference: &crossh_agent::AgentModelRef,
) -> Result<Option<String>, String> {
    let provider = settings
        .resolve(reference)
        .map_err(ToString::to_string)?
        .provider;
    if !provider.api_key.is_empty() {
        return Ok(Some(provider.api_key.clone()));
    }
    if provider.api_key_env.is_empty() {
        return Ok(None);
    }
    std::env::var(&provider.api_key_env)
        .map(Some)
        .map_err(|_| format!("{} is not set", provider.api_key_env))
}

fn append_delta(app: &mut App, role: Role, delta: &str) {
    if let Some((last_role, text)) = app.messages.last_mut()
        && *last_role == role
    {
        text.push_str(delta);
    } else {
        app.messages.push((role, delta.to_string()));
    }
}

fn append_notice(app: &mut App, text: impl Into<String>, role: Role) {
    app.messages.push((role, text.into()));
    app.scroll = u16::MAX;
}

fn push_notice(app: &mut App, text: impl Into<String>) {
    append_notice(app, text, Role::Notice);
}

fn push_error(app: &mut App, text: impl Into<String>) {
    append_notice(app, text, Role::Error);
    app.status = "Command failed".into();
}

fn set_spinner_status(app: &mut App, label: &str, frame: usize) {
    app.status = format!(
        "{label} {}{}",
        SPINNER[frame % SPINNER.len()],
        if app.queued_inputs.is_empty() {
            String::new()
        } else {
            format!("  queued {}", app.queued_inputs.len())
        }
    );
}

fn queue_input(app: &mut App) {
    let input = take_input(app);
    if input.trim().is_empty() {
        return;
    }
    app.queued_inputs.push_back(input.clone());
    app.messages
        .push((Role::Queued, format!("Queued: {}", one_line(&input))));
    app.status = format!("Queued {} prompt(s)", app.queued_inputs.len());
}

fn edit_input(app: &mut App, key: KeyEvent) {
    let control = key.modifiers.contains(KeyModifiers::CONTROL);
    if is_enter_key(key.code) && key.modifiers.contains(KeyModifiers::SHIFT) {
        insert_text(app, "\n");
        return;
    }
    match key.code {
        KeyCode::Backspace => delete_previous_char(app),
        KeyCode::Delete => delete_next_char(app),
        KeyCode::Left => move_cursor(app, false),
        KeyCode::Right => move_cursor(app, true),
        KeyCode::Home => app.input_cursor = line_start(&app.input, app.input_cursor),
        KeyCode::End => app.input_cursor = line_end(&app.input, app.input_cursor),
        KeyCode::Char('w') if control => delete_previous_word(app),
        KeyCode::Char('u') if control => {
            let start = line_start(&app.input, app.input_cursor);
            app.input.replace_range(start..app.input_cursor, "");
            app.input_cursor = start;
        }
        KeyCode::Char('k') if control => {
            let end = line_end(&app.input, app.input_cursor);
            app.input.replace_range(app.input_cursor..end, "");
        }
        KeyCode::Char(character) if !control && !key.modifiers.contains(KeyModifiers::ALT) => {
            insert_text(app, &character.to_string())
        }
        _ => {}
    }
}

fn is_enter_key(code: KeyCode) -> bool {
    matches!(
        code,
        KeyCode::Enter | KeyCode::Char('\r') | KeyCode::Char('\n')
    )
}

fn insert_text(app: &mut App, text: &str) {
    app.input.insert_str(app.input_cursor, text);
    app.input_cursor += text.len();
    app.history_cursor = None;
}

fn delete_previous_char(app: &mut App) {
    if app.input_cursor == 0 {
        return;
    }
    let start = previous_boundary(&app.input, app.input_cursor);
    app.input.replace_range(start..app.input_cursor, "");
    app.input_cursor = start;
}

fn delete_next_char(app: &mut App) {
    if app.input_cursor >= app.input.len() {
        return;
    }
    let end = next_boundary(&app.input, app.input_cursor);
    app.input.replace_range(app.input_cursor..end, "");
}

fn delete_previous_word(app: &mut App) {
    let mut start = app.input_cursor;
    while start > 0 && app.input.as_bytes()[start - 1].is_ascii_whitespace() {
        start = previous_boundary(&app.input, start);
    }
    while start > 0 && !app.input.as_bytes()[start - 1].is_ascii_whitespace() {
        start = previous_boundary(&app.input, start);
    }
    app.input.replace_range(start..app.input_cursor, "");
    app.input_cursor = start;
}

fn move_cursor(app: &mut App, right: bool) {
    app.input_cursor = if right {
        next_boundary(&app.input, app.input_cursor)
    } else {
        previous_boundary(&app.input, app.input_cursor)
    };
}

fn move_history(app: &mut App, up: bool) {
    let history = app
        .session
        .messages
        .iter()
        .filter(|message| message.role == AgentRole::User)
        .filter_map(|message| (!message.text.is_empty()).then_some(message.text.clone()))
        .collect::<Vec<_>>();
    if history.is_empty() {
        return;
    }
    let next = match (app.history_cursor, up) {
        (None, true) => history.len().saturating_sub(1),
        (None, false) => return,
        (Some(index), true) => index.saturating_sub(1),
        (Some(index), false) if index + 1 < history.len() => index + 1,
        (Some(_), false) => {
            clear_input(app);
            return;
        }
    };
    app.history_cursor = Some(next);
    app.input = history[next].clone();
    app.input_cursor = app.input.len();
}

fn clear_input(app: &mut App) {
    app.input.clear();
    app.input_cursor = 0;
    app.history_cursor = None;
}

fn take_input(app: &mut App) -> String {
    let input = std::mem::take(&mut app.input);
    app.input_cursor = 0;
    app.history_cursor = None;
    input
}

fn previous_boundary(text: &str, index: usize) -> usize {
    text[..index]
        .char_indices()
        .next_back()
        .map_or(0, |(start, _)| start)
}

fn next_boundary(text: &str, index: usize) -> usize {
    text[index..]
        .char_indices()
        .nth(1)
        .map_or(text.len(), |(offset, _)| index + offset)
}

fn line_start(text: &str, index: usize) -> usize {
    text[..index].rfind('\n').map_or(0, |offset| offset + 1)
}

fn line_end(text: &str, index: usize) -> usize {
    text[index..]
        .find('\n')
        .map_or(text.len(), |offset| index + offset)
}

fn parse_thinking(value: &str) -> Option<AgentThinkingLevel> {
    AgentThinkingLevel::ALL
        .into_iter()
        .find(|level| level.label().eq_ignore_ascii_case(value.trim()))
}

fn active_model_label(app: &App) -> String {
    app.settings
        .resolve(&app.settings.active_model)
        .map(|target| format!("{}/{}", target.provider.name, target.model.name))
        .unwrap_or_else(|_| "unconfigured".into())
}

fn active_context_limit(app: &App) -> usize {
    app.settings
        .resolve(&app.settings.active_model)
        .map(|target| target.model.context_window as usize)
        .unwrap_or(128_000)
        .max(1_024)
}

fn active_input_limit(app: &App) -> usize {
    let output_reserve = app
        .settings
        .resolve(&app.settings.active_model)
        .map(|target| target.model.max_tokens as usize)
        .unwrap_or(4_096);
    active_context_limit(app)
        .saturating_sub(output_reserve)
        .max(1_024)
}

fn system_prompt_limit(app: &App) -> usize {
    active_input_limit(app).saturating_mul(2)
}

fn compaction_limit(app: &App) -> usize {
    let input_tokens = active_input_limit(app);
    input_tokens
        .saturating_mul(4)
        .saturating_sub(system_prompt(app).len())
        .max(1_024)
}

fn estimate_tokens(messages: &[AgentMessage]) -> usize {
    messages
        .iter()
        .map(|message| {
            let tool_bytes = message
                .tool_calls
                .iter()
                .map(|call| call.name.len() + call.arguments.len())
                .sum::<usize>();
            let result_bytes = message
                .tool_result
                .as_ref()
                .map_or(0, |result| result.output.len());
            (message.text.len() + tool_bytes + result_bytes).div_ceil(4)
        })
        .sum()
}

fn tool_requires_approval(name: &str) -> bool {
    crossh_agent::builtin_tools()
        .into_iter()
        .find(|tool| tool.name == name)
        .is_some_and(|tool| tool.requires_approval)
}

fn format_tool_call(call: &AgentToolCall) -> String {
    format!("$ {} {}", call.name, compact_json(&call.arguments))
}

fn format_tool_result(result: &AgentToolResult) -> String {
    let prefix = if result.is_error { "x" } else { "ok" };
    format!("[{prefix}] {}", result.output)
}

fn compact_json(value: &str) -> String {
    serde_json::from_str::<serde_json::Value>(value)
        .ok()
        .map(|value| value.to_string())
        .unwrap_or_else(|| one_line(value))
}

fn pretty_tool_arguments(value: &str) -> String {
    serde_json::from_str::<serde_json::Value>(value)
        .ok()
        .and_then(|value| serde_json::to_string_pretty(&value).ok())
        .unwrap_or_else(|| value.to_string())
}

fn one_line(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn tui_color(value: theme::Rgb) -> Color {
    let (red, green, blue) = value.channels();
    Color::Rgb(red, green, blue)
}

fn render(frame: &mut Frame, app: &mut App) {
    let input_height = input_height(frame.area(), app);
    let [header, conversation, input, footer] = Layout::vertical([
        Constraint::Length(3),
        Constraint::Min(5),
        Constraint::Length(input_height),
        Constraint::Length(1),
    ])
    .areas(frame.area());
    let accent = tui_color(theme::accent());
    let bg = tui_color(theme::canvas());
    let surface = tui_color(theme::surface());
    let border = tui_color(theme::border_strong());
    let text = tui_color(theme::text());
    let muted = tui_color(theme::muted_text());
    let faint = tui_color(theme::faint_text());

    let header_line = Line::from(vec![
        Span::styled(
            " CROSSH ",
            Style::new().fg(bg).bg(accent).add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            " AGENT ",
            Style::new().fg(accent).add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!(
                "  {}  {}  {}",
                active_model_label(app),
                app.thinking.label(),
                session_name(app)
            ),
            Style::new().fg(muted),
        ),
    ]);
    frame.render_widget(
        Paragraph::new(header_line)
            .style(Style::new().bg(bg))
            .block(
                Block::new()
                    .borders(Borders::BOTTOM)
                    .border_style(Style::new().fg(border))
                    .padding(Padding::top(1)),
            ),
        header,
    );

    let content_width = conversation.width.saturating_sub(4).max(1) as usize;
    let mut lines = Vec::new();
    if app.messages.is_empty() {
        lines.push(Line::styled(
            "No messages yet. Ask about this project or type /help.",
            Style::new().fg(faint),
        ));
    }
    for (role, content) in &app.messages {
        let (label, color) = match role {
            Role::User => ("you", accent),
            Role::Reasoning => ("thinking", faint),
            Role::Agent => ("agent", tui_color(theme::diff_add_fg())),
            Role::Tool => ("tool", tui_color(theme::warning())),
            Role::Error => ("error", tui_color(theme::danger())),
            Role::Notice => ("note", tui_color(theme::info())),
            Role::Queued => ("queued", tui_color(theme::accent_hover())),
        };
        lines.push(Line::styled(
            format!("[{label}]"),
            Style::new().fg(color).add_modifier(Modifier::BOLD),
        ));
        if matches!(role, Role::Reasoning) && !app.show_reasoning {
            lines.push(Line::styled(
                format!("  [thinking hidden: {} chars]", content.len()),
                Style::new().fg(faint).add_modifier(Modifier::ITALIC),
            ));
        } else if matches!(role, Role::Agent) {
            lines.extend(markdown_content(content, content_width));
        } else if matches!(role, Role::Tool) && !app.show_tool_details {
            lines.push(Line::styled(
                format!(
                    "  {}",
                    collapsed_tool_summary(content, content_width.saturating_sub(2))
                ),
                Style::new().fg(muted),
            ));
        } else {
            lines.extend(wrap_content(content, content_width));
        }
        lines.push(Line::default());
    }
    let conversation_area = conversation.inner(Margin::new(2, 0));
    let viewport_height = conversation_area.height as usize;
    app.max_scroll = lines
        .len()
        .saturating_sub(viewport_height)
        .min(u16::MAX as usize) as u16;
    let scroll = if app.scroll == u16::MAX {
        app.max_scroll
    } else {
        app.scroll.min(app.max_scroll)
    };
    if app.scroll != u16::MAX {
        app.scroll = scroll;
    }
    frame.render_widget(
        Paragraph::new(lines)
            .style(Style::new().fg(text).bg(bg))
            .scroll((scroll, 0))
            .block(
                Block::new()
                    .title(" conversation ")
                    .borders(Borders::TOP | Borders::BOTTOM)
                    .border_style(Style::new().fg(border)),
            ),
        conversation,
    );

    let input_title = if app.queued_inputs.is_empty() {
        " prompt "
    } else {
        " queue next prompt "
    };
    let input_block = Block::new()
        .title(input_title)
        .borders(Borders::ALL)
        .border_style(Style::new().fg(if app.queued_inputs.is_empty() {
            border
        } else {
            accent
        }))
        .padding(Padding::horizontal(1));
    let input_area = input.inner(Margin::new(2, 1));
    let input_text = if app.input.is_empty() {
        "Ask about the project...".to_string()
    } else {
        app.input.clone()
    };
    frame.render_widget(
        Paragraph::new(input_text)
            .style(Style::new().fg(if app.input.is_empty() { faint } else { text }))
            .wrap(Wrap { trim: false })
            .block(input_block),
        input,
    );
    let (cursor_x, cursor_y) = cursor_position(input_area, &app.input, app.input_cursor);
    frame.set_cursor_position((cursor_x, cursor_y));

    let footer_line = Line::from(vec![
        Span::styled(format!(" {}", app.status), Style::new().fg(muted)),
        Span::styled(
            format!(
                "    {}  {} msgs  ~{} tokens  {} ctx  {} skills  {} prompts",
                app.workspace.display(),
                app.session.messages.len(),
                estimate_tokens(&app.session.messages),
                app.context_files.len(),
                app.skills.len(),
                app.prompts.len()
            ),
            Style::new().fg(faint),
        ),
    ]);
    frame.render_widget(
        Paragraph::new(footer_line).style(Style::new().bg(surface)),
        footer,
    );
}

fn input_height(area: Rect, app: &App) -> u16 {
    let width = area.width.saturating_sub(4).max(1) as usize;
    let lines = visual_line_count(&app.input, width);
    (lines.min(MAX_VISIBLE_INPUT_LINES) as u16 + 2).min(area.height.saturating_sub(8).max(3))
}

fn cursor_position(area: Rect, input: &str, cursor: usize) -> (u16, u16) {
    let prefix = &input[..cursor.min(input.len())];
    let width = area.width.max(1) as usize;
    let mut column = 0;
    let mut row = 0;
    for character in prefix.chars() {
        if character == '\n' {
            row += 1;
            column = 0;
            continue;
        }
        let character_width = UnicodeWidthChar::width(character).unwrap_or(0);
        if column > 0 && column + character_width > width {
            row += 1;
            column = 0;
        }
        column += character_width;
    }
    if column >= width {
        row += column / width;
        column %= width;
    }
    let x = area
        .x
        .saturating_add(column.min(width.saturating_sub(1)) as u16);
    let y = area
        .y
        .saturating_add(row.min(u16::MAX as usize) as u16)
        .min(area.bottom().saturating_sub(1));
    (x, y)
}

fn visual_line_count(input: &str, width: usize) -> usize {
    input
        .split('\n')
        .map(|line| {
            let line_width = UnicodeWidthStr::width(line);
            line_width.max(1).div_ceil(width.max(1))
        })
        .sum::<usize>()
        .max(1)
}

fn scroll_conversation(app: &mut App, delta: i16) {
    let current = if app.scroll == u16::MAX {
        app.max_scroll
    } else {
        app.scroll.min(app.max_scroll)
    };
    let next = if delta < 0 {
        current.saturating_sub(delta.unsigned_abs())
    } else {
        current.saturating_add(delta as u16).min(app.max_scroll)
    };
    app.scroll = if next == app.max_scroll {
        u16::MAX
    } else {
        next
    };
}

fn session_name(app: &App) -> String {
    app.session.name.clone().unwrap_or_else(|| {
        format!(
            "session {}",
            app.session.id.get(..8).unwrap_or(&app.session.id)
        )
    })
}

fn collapsed_tool_summary(content: &str, width: usize) -> String {
    let compact = one_line(content);
    let limit = width.saturating_sub(4).max(8);
    if UnicodeWidthStr::width(compact.as_str()) <= limit {
        return compact;
    }
    let mut result = String::new();
    let mut used = 0;
    for character in compact.chars() {
        let character_width = UnicodeWidthChar::width(character).unwrap_or(0);
        if used + character_width + 3 > limit {
            break;
        }
        result.push(character);
        used += character_width;
    }
    result.push_str("...");
    result
}

fn wrap_content(content: &str, width: usize) -> Vec<Line<'static>> {
    let width = width.max(1);
    let mut lines = Vec::new();
    for source_line in content.split('\n') {
        if source_line.is_empty() {
            lines.push(Line::default());
            continue;
        }
        let mut current = String::new();
        let mut current_width = 0;
        for character in source_line.chars() {
            let character_width = UnicodeWidthChar::width(character).unwrap_or(0);
            if !current.is_empty() && current_width + character_width > width {
                lines.push(Line::from(std::mem::take(&mut current)));
                current_width = 0;
            }
            current.push(character);
            current_width += character_width;
        }
        lines.push(Line::from(current));
    }
    lines
}

fn markdown_content(content: &str, width: usize) -> Vec<Line<'static>> {
    let markdown = tui_markdown::from_str(content);
    wrap_styled_lines(markdown.lines, width)
}

fn wrap_styled_lines(lines: Vec<Line<'_>>, width: usize) -> Vec<Line<'static>> {
    let width = width.max(1);
    let mut wrapped = Vec::new();
    for line in lines {
        if line.spans.is_empty() {
            wrapped.push(Line::default());
            continue;
        }
        let mut spans = Vec::new();
        let mut line_width = 0;
        for span in line.spans {
            let style = span.style;
            let mut chunk = String::new();
            for character in span.content.chars() {
                let character_width = UnicodeWidthChar::width(character).unwrap_or(0);
                if line_width > 0 && line_width + character_width > width {
                    if !chunk.is_empty() {
                        spans.push(Span::styled(std::mem::take(&mut chunk), style));
                    }
                    wrapped.push(Line::from(std::mem::take(&mut spans)));
                    line_width = 0;
                }
                chunk.push(character);
                line_width += character_width;
            }
            if !chunk.is_empty() {
                spans.push(Span::styled(chunk, style));
            }
        }
        wrapped.push(Line::from(spans));
    }
    wrapped
}

#[cfg(test)]
#[path = "agent_cli_tests.rs"]
mod tests;
