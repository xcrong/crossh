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

#[path = "agent_cli_render.rs"]
mod render;
use render::{render, scroll_conversation, session_name};

const SYSTEM_PROMPT: &str = "You are Crossh Agent, a careful coding assistant running in the user's terminal. Inspect the workspace before making claims, use the smallest appropriate tool, keep changes scoped to the request, and report what you changed and how it was verified. For multi-line changes, prefer the patch tool with a unified diff; use edit only for a short exact replacement. For file and directory tool arguments, always generate workspace-relative paths such as `.` or `README.md`. Do not generate absolute paths; the executor tolerates an in-workspace absolute path only for compatibility. Never use paths outside the workspace.";
const SPINNER: [&str; 4] = ["|", "/", "-", "\\"];
const MAX_VISIBLE_INPUT_LINES: usize = 6;
const HEADER_HEIGHT: u16 = 3;
const FOOTER_HEIGHT: u16 = 1;
const MIN_CONVERSATION_HEIGHT: u16 = 5;
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
    print_help_for("crossh-agent");
}

fn print_help_for(command: &str) {
    println!(
        "Usage: {command} [OPTIONS]\n\nStart the interactive Crossh coding agent.\n\nOptions:\n  -c, --continue          Continue the most recent project session\n  -r, --resume VALUE      Resume a session by number, id, name, or path\n  -m, --model VALUE       Select provider/model or a model id\n      --thinking LEVEL    Set off, minimal, low, medium, high, or xhigh reasoning\n      --no-session         Do not write a persistent session\n  -h, --help              Print this help\n\nInside the agent, type /help for commands and Ctrl-T/Ctrl-O for display toggles."
    );
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Role {
    User,
    Reasoning,
    Agent,
    Tool,
    Approval,
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
    if settings.providers.is_empty() {
        return Err("No agent provider configured; add one in Settings first.".into());
    }

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
        if is_command_input(&app.input) {
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
        let visible_response_start = app.messages.len();
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

        append_response_block_if_missing(
            app,
            visible_response_start,
            Role::Reasoning,
            response.reasoning(),
        );
        append_response_block_if_missing(app, visible_response_start, Role::Agent, response.text());
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
            let approval_source = tool_approval_source(&app.settings, &call.name);
            if approval_source == ToolApprovalSource::LanguageModel {
                push_approval(
                    app,
                    format!(
                        "Language-model approval requested.\n\nTool: {}\nArguments:\n{}",
                        call.name,
                        pretty_tool_arguments(&call.arguments)
                    ),
                );
            }
            let mut reviewer_denial_reason = None;
            let approved = match approval_source {
                ToolApprovalSource::None => true,
                ToolApprovalSource::LanguageModel => {
                    match review_tool_animated(terminal, app, &call, &user_request)? {
                        BackgroundResult::Complete(ReviewDecision::Approved(reason)) => {
                            push_approval(
                                app,
                                format!(
                                    "Language-model approval granted.\n\nTool: {}\nReason: {}",
                                    call.name, reason
                                ),
                            );
                            true
                        }
                        BackgroundResult::Complete(ReviewDecision::Denied(reason)) => {
                            push_approval(
                                app,
                                format!(
                                    "Language-model approval denied.\n\nTool: {}\nReason: {}",
                                    call.name, reason
                                ),
                            );
                            reviewer_denial_reason = Some(reason);
                            false
                        }
                        BackgroundResult::Complete(ReviewDecision::Unavailable(error)) => {
                            push_approval(
                                app,
                                format!(
                                    "Language-model approval unavailable: {error}\nFalling back to local confirmation for {}.",
                                    call.name
                                ),
                            );
                            confirm_tool(terminal, app, &call)?
                        }
                        BackgroundResult::Cancelled => {
                            push_approval(app, "Language-model approval cancelled");
                            app.status = "Cancelled".into();
                            return Ok(false);
                        }
                    }
                }
                ToolApprovalSource::User => confirm_tool(terminal, app, &call)?,
            };
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
                    output: if let Some(reason) = reviewer_denial_reason {
                        format!("Tool execution denied by the language-model reviewer: {reason}")
                    } else {
                        "Tool execution denied by the user".into()
                    },
                    is_error: true,
                }
            };
            app.messages.push((Role::Tool, format_tool_result(&result)));
            request_messages.push(AgentMessage::tool_result(result.clone()));
            app.session.append(AgentMessage::tool_result(result));
            persist_session(app);
        }
        app.status = format!("Completed tool round {}", round + 1);
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
    push_approval(
        app,
        format!(
            "Local confirmation required for tool execution.\n\nTool: {}\nArguments:\n{}",
            call.name,
            pretty_tool_arguments(&call.arguments)
        ),
    );
    app.status = format!("Allow {}?  y/Enter allow  n/Esc deny", call.name);
    let decision = loop {
        terminal.draw(|frame| render(frame, app))?;
        if !event::poll(Duration::from_millis(100))? {
            continue;
        }
        match event::read()? {
            Event::Key(key) if key.kind == KeyEventKind::Press => match key.code {
                KeyCode::Char('y') | KeyCode::Enter | KeyCode::Char('\r') | KeyCode::Char('\n') => {
                    break true;
                }
                KeyCode::Char('n') | KeyCode::Esc => break false,
                _ => {}
            },
            Event::Mouse(mouse) => match mouse.kind {
                MouseEventKind::ScrollUp => scroll_conversation(app, -3),
                MouseEventKind::ScrollDown => scroll_conversation(app, 3),
                _ => {}
            },
            _ => {}
        }
    };
    app.show_tool_details = previous_details;
    Ok(decision)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ToolApprovalSource {
    None,
    LanguageModel,
    User,
}

enum ReviewDecision {
    Approved(String),
    Denied(String),
    Unavailable(String),
}

fn tool_approval_source(settings: &AgentSettings, tool_name: &str) -> ToolApprovalSource {
    if !tool_requires_approval(tool_name) {
        return ToolApprovalSource::None;
    }
    if settings.resolve(&settings.reviewer_model).is_ok() {
        ToolApprovalSource::LanguageModel
    } else {
        ToolApprovalSource::User
    }
}

fn review_tool_animated(
    terminal: &mut DefaultTerminal,
    app: &mut App,
    call: &AgentToolCall,
    user_request: &str,
) -> io::Result<BackgroundResult<ReviewDecision>> {
    let settings = app.settings.clone();
    let key = match resolve_model_key(&settings, &settings.reviewer_model) {
        Ok(key) => key,
        Err(error) => {
            app.status = format!("Reviewer unavailable: {error}");
            return Ok(BackgroundResult::Complete(ReviewDecision::Unavailable(
                error,
            )));
        }
    };
    let tool_name = call.name.clone();
    let call = call.clone();
    let workspace = app.workspace.clone();
    let user_request = user_request.to_string();
    let (tx, rx) = mpsc::channel();
    let task = crossh_ssh::ssh_runtime().spawn(async move {
        let result =
            match review_tool(&settings, key.as_deref(), &call, &workspace, &user_request).await {
                Ok(review) if review.approved => ReviewDecision::Approved(review.reason),
                Ok(review) => ReviewDecision::Denied(review.reason),
                Err(error) => ReviewDecision::Unavailable(error),
            };
        let _ = tx.send(result);
    });
    app.status = format!("Reviewing {tool_name} with the language model");
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
        if event::poll(Duration::from_millis(80))? {
            match event::read()? {
                Event::Key(key)
                    if key.kind == KeyEventKind::Press
                        && (key.code == KeyCode::Esc
                            || key.code == KeyCode::Char('c')
                                && key.modifiers.contains(KeyModifiers::CONTROL)) =>
                {
                    cancel();
                    app.status = format!("{label} cancelled");
                    return Ok(BackgroundResult::Cancelled);
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
}

fn handle_command(
    terminal: &mut DefaultTerminal,
    app: &mut App,
    input: String,
) -> io::Result<bool> {
    let mut parts = input.trim().splitn(2, char::is_whitespace);
    let command = normalize_command_name(parts.next().unwrap_or_default());
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

fn is_command_input(input: &str) -> bool {
    matches!(input.trim_start().chars().next(), Some('/' | '、'))
}

fn normalize_command_name(command: &str) -> String {
    match command.strip_prefix('、') {
        Some(rest) => format!("/{rest}").to_ascii_lowercase(),
        None => command.to_ascii_lowercase(),
    }
}

fn help_text() -> String {
    "Commands (start with / or 、):\n  /help, /hotkeys       Show commands and shortcuts\n  /model [value]        List or switch provider/model\n  /thinking [level]     Set reasoning level\n  /tools                Show available tools\n  /skills               List project skills\n  /skill NAME [request] Apply a skill to a request\n  /prompts              List prompt templates\n  /prompt NAME [args]   Run a prompt template\n  /new, /clear          Start a fresh session\n  /continue             Resume the most recent session\n  /resume [value]       List or resume a saved session\n  /fork, /clone         Branch the current conversation\n  /name [value]         Set or show the session name\n  /session, /stats      Show session and context details\n  /compact              Compact older conversation context\n  /reload               Reload project instructions and resources\n  /export [path]        Export the session as Markdown\n  /quit, /exit          Quit\n\nShortcuts:\n  Enter                 Send prompt\n  Shift+Enter           Insert a new line\n  Escape                Clear input, then quit\n  Ctrl+C                Clear input, then quit\n  Ctrl+T                Expand or collapse thinking\n  Ctrl+O                Expand or collapse tool output\n  Up/Down               Browse prompt history\n  While working, Enter  Queue a follow-up prompt"
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
        "{SYSTEM_PROMPT}\n\nWorkspace root: . (the current workspace; use relative paths)\nThinking preference: {}\nAvailable tools: read, grep, find, ls, patch, edit, write, bash.",
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

fn append_response_block_if_missing(app: &mut App, start: usize, role: Role, content: String) {
    if content.is_empty()
        || app.messages[start..]
            .iter()
            .any(|(existing_role, _)| *existing_role == role)
    {
        return;
    }
    app.messages.push((role, content));
}

fn append_notice(app: &mut App, text: impl Into<String>, role: Role) {
    app.messages.push((role, text.into()));
    app.scroll = u16::MAX;
}

fn push_notice(app: &mut App, text: impl Into<String>) {
    append_notice(app, text, Role::Notice);
}

fn push_approval(app: &mut App, text: impl Into<String>) {
    append_notice(app, text, Role::Approval);
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

#[cfg(test)]
#[path = "agent_cli_tests.rs"]
mod tests;
