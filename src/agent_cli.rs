//! Ratatui entry point for Crossh's small Rust-native coding agent.

use std::{
    io::{self, IsTerminal},
    sync::mpsc,
    time::Duration,
};

use crossh_agent::{
    AgentContentBlock, AgentEvent, AgentMessage, AgentResponse, AgentRole, AgentSettings,
    AgentToolCall, AgentToolResult, complete_stream, execute_tool, review_tool,
};
use crossterm::{
    event::{
        self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent, KeyEventKind,
        KeyModifiers, MouseEventKind,
    },
    execute,
};
use ratatui::{
    DefaultTerminal, Frame,
    layout::{Constraint, Layout, Margin},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Padding, Paragraph},
};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

const SYSTEM_PROMPT: &str = "You are Crossh Agent, a concise coding assistant running in the user's terminal. Inspect requests carefully, use the provided workspace tools when needed, and report tool outcomes accurately.";
const SPINNER: [&str; 4] = ["|", "/", "-", "\\"];

enum ModelUpdate {
    Event(AgentEvent),
    Complete(Result<AgentResponse, String>),
}

struct App {
    settings: AgentSettings,
    api_key: Option<String>,
    reviewer_api_key: Option<String>,
    input: String,
    messages: Vec<(Role, String)>,
    scroll: u16,
    max_scroll: u16,
    show_tool_details: bool,
    status: String,
}

#[derive(Clone, Copy)]
enum Role {
    User,
    Reasoning,
    Agent,
    Tool,
    Error,
}

pub(crate) fn run(settings: AgentSettings) -> Result<(), String> {
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        return Err("crossh agent requires an interactive terminal".to_string());
    }
    let settings = settings.normalized();
    settings.validate().map_err(ToString::to_string)?;
    let active = settings
        .resolve(&settings.active_model)
        .map_err(str::to_string)?;
    let reviewer = settings
        .resolve(&settings.reviewer_model)
        .map_err(str::to_string)?;
    let api_key = resolve_key(active.provider)?;
    let reviewer_api_key = resolve_key(reviewer.provider)?;
    let mut app = App {
        settings,
        api_key,
        reviewer_api_key,
        input: String::new(),
        messages: Vec::new(),
        scroll: u16::MAX,
        max_scroll: 0,
        show_tool_details: false,
        status: "Ready  Enter send  Ctrl-T tools  Ctrl-C quit".to_string(),
    };
    execute!(io::stdout(), EnableMouseCapture).map_err(|error| error.to_string())?;
    let result = ratatui::run(|terminal| run_app(terminal, &mut app));
    let cleanup = execute!(io::stdout(), DisableMouseCapture);
    result.and(cleanup).map_err(|error| error.to_string())
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
    match (key.modifiers, key.code) {
        (KeyModifiers::CONTROL, KeyCode::Char('c')) => return Ok(true),
        (_, KeyCode::Esc) => return Ok(true),
        (_, KeyCode::Backspace) => {
            app.input.pop();
        }
        (KeyModifiers::CONTROL, KeyCode::Char('t')) => {
            app.show_tool_details = !app.show_tool_details;
            app.scroll = u16::MAX;
            app.status = if app.show_tool_details {
                "Tool details expanded".into()
            } else {
                "Tool details collapsed".into()
            };
        }
        (_, KeyCode::Up) => scroll_conversation(app, -1),
        (_, KeyCode::Down) => scroll_conversation(app, 1),
        (_, KeyCode::PageUp) => scroll_conversation(app, -10),
        (_, KeyCode::PageDown) => scroll_conversation(app, 10),
        (_, KeyCode::Enter) if matches!(app.input.trim(), "/exit" | "/quit") => return Ok(true),
        (_, KeyCode::Enter) if app.input.trim() == "/clear" => {
            app.input.clear();
            app.messages.clear();
            app.status = "Session cleared".to_string();
        }
        (_, KeyCode::Enter) if !app.input.trim().is_empty() => submit(terminal, app)?,
        (_, KeyCode::Char(character)) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
            app.input.push(character);
        }
        _ => {}
    }
    Ok(false)
}

fn submit(terminal: &mut DefaultTerminal, app: &mut App) -> io::Result<()> {
    let input = std::mem::take(&mut app.input);
    app.messages.push((Role::User, input));
    app.status = "Waiting for model...".to_string();
    app.scroll = u16::MAX;
    terminal.draw(|frame| render(frame, app))?;

    let mut messages = request_messages(&app.messages);
    let settings = app.settings.clone();
    let api_key = app.api_key.clone();
    let reviewer_api_key = app.reviewer_api_key.clone();
    for round in 0..settings.max_tool_rounds {
        let (updates_tx, updates_rx) = mpsc::channel();
        let request_settings = settings.clone();
        let request_messages = messages.clone();
        let request_api_key = api_key.clone();
        crossh_ssh::ssh_runtime().spawn(async move {
            let event_tx = updates_tx.clone();
            let result = complete_stream(
                &request_settings,
                request_api_key.as_deref(),
                &request_messages,
                move |event| {
                    let _ = event_tx.send(ModelUpdate::Event(event.clone()));
                },
            )
            .await;
            let _ = updates_tx.send(ModelUpdate::Complete(result));
        });
        let mut spinner_frame = 0;
        let result = loop {
            match updates_rx.recv_timeout(Duration::from_millis(80)) {
                Ok(ModelUpdate::Event(event)) => match event {
                    AgentEvent::TextDelta(delta) => append_delta(app, Role::Agent, &delta),
                    AgentEvent::ReasoningDelta(delta) => append_delta(app, Role::Reasoning, &delta),
                    AgentEvent::ToolCallStart { name, .. } => {
                        app.status = format!("Tool requested: {name}")
                    }
                    AgentEvent::ToolCallArgumentsDelta { .. } | AgentEvent::Stop(_) => {}
                },
                Ok(ModelUpdate::Complete(result)) => break result,
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    set_spinner_status(app, "Waiting for model", spinner_frame);
                    spinner_frame += 1;
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    break Err("model request task stopped unexpectedly".into());
                }
            }
            app.scroll = u16::MAX;
            terminal.draw(|frame| render(frame, app))?;
        };
        let response = match result {
            Ok(response) => response,
            Err(error) => {
                app.messages.push((Role::Error, error));
                app.status = "Request failed".to_string();
                return Ok(());
            }
        };
        let calls = response
            .content
            .into_iter()
            .filter_map(|block| match block {
                AgentContentBlock::ToolCall(call) => Some(call),
                _ => None,
            })
            .collect::<Vec<_>>();
        if calls.is_empty() {
            app.status = "Ready".to_string();
            return Ok(());
        }
        messages.push(AgentMessage::assistant_tool_calls(calls.clone()));
        let workspace = std::env::current_dir().map_err(io::Error::other)?;
        for call in calls {
            app.messages
                .push((Role::Tool, format!("{} {}", call.name, call.arguments)));
            app.status = format!("Reviewing tool: {}", call.name);
            let approved = review_tool_animated(
                terminal,
                app,
                settings.clone(),
                reviewer_api_key.clone(),
                call.clone(),
                workspace.clone(),
            )?;
            let result = if approved {
                execute_tool_animated(terminal, app, call.clone(), workspace.clone())?
            } else {
                AgentToolResult {
                    call_id: call.id.clone(),
                    output: "Tool execution denied by reviewer model".into(),
                    is_error: true,
                }
            };
            app.messages.push((Role::Tool, result.output.clone()));
            messages.push(AgentMessage::tool_result(result));
        }
        if round + 1 == settings.max_tool_rounds {
            app.messages
                .push((Role::Error, "Tool loop limit reached".into()));
        }
    }
    Ok(())
}

fn review_tool_animated(
    terminal: &mut DefaultTerminal,
    app: &mut App,
    settings: AgentSettings,
    api_key: Option<String>,
    call: AgentToolCall,
    workspace: std::path::PathBuf,
) -> io::Result<bool> {
    let tool_name = call.name.clone();
    let (tx, rx) = mpsc::channel();
    crossh_ssh::ssh_runtime().spawn(async move {
        let result = review_tool(&settings, api_key.as_deref(), &call, &workspace)
            .await
            .unwrap_or(false);
        let _ = tx.send(result);
    });
    wait_for_background(terminal, app, &format!("Reviewing {tool_name}"), rx)
}

fn execute_tool_animated(
    terminal: &mut DefaultTerminal,
    app: &mut App,
    call: AgentToolCall,
    workspace: std::path::PathBuf,
) -> io::Result<AgentToolResult> {
    let tool_name = call.name.clone();
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(execute_tool(&call, &workspace));
    });
    wait_for_background(terminal, app, &format!("Running {tool_name}"), rx)
}

fn wait_for_background<T>(
    terminal: &mut DefaultTerminal,
    app: &mut App,
    label: &str,
    receiver: mpsc::Receiver<T>,
) -> io::Result<T> {
    let mut frame = 0;
    loop {
        match receiver.recv_timeout(Duration::from_millis(80)) {
            Ok(result) => return Ok(result),
            Err(mpsc::RecvTimeoutError::Timeout) => {
                set_spinner_status(app, label, frame);
                frame += 1;
                terminal.draw(|frame| render(frame, app))?;
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                return Err(io::Error::other("background task stopped unexpectedly"));
            }
        }
    }
}

fn set_spinner_status(app: &mut App, label: &str, frame: usize) {
    app.status = format!("{label} {}", SPINNER[frame % SPINNER.len()]);
}

fn resolve_key(provider: &crossh_agent::AgentProvider) -> Result<Option<String>, String> {
    if !provider.api_key.is_empty() {
        Ok(Some(provider.api_key.clone()))
    } else if provider.api_key_env.is_empty() {
        Ok(None)
    } else {
        std::env::var(&provider.api_key_env)
            .map(Some)
            .map_err(|_| {
                format!(
                    "{} is not set; provide a key in Agent settings or export it before starting crossh agent",
                    provider.api_key_env
                )
            })
    }
}

fn append_delta(app: &mut App, role: Role, delta: &str) {
    if let Some((last_role, text)) = app.messages.last_mut()
        && std::mem::discriminant(last_role) == std::mem::discriminant(&role)
    {
        text.push_str(delta);
    } else {
        app.messages.push((role, delta.to_string()));
    }
}

fn request_messages(messages: &[(Role, String)]) -> Vec<AgentMessage> {
    let mut result = vec![AgentMessage::new(AgentRole::System, SYSTEM_PROMPT)];
    result.extend(messages.iter().filter_map(|(role, content)| match role {
        Role::User => Some(AgentMessage::new(AgentRole::User, content)),
        Role::Agent => Some(AgentMessage::new(AgentRole::Assistant, content)),
        Role::Reasoning | Role::Tool | Role::Error => None,
    }));
    result
}

fn render(frame: &mut Frame, app: &mut App) {
    let model_label = app
        .settings
        .resolve(&app.settings.active_model)
        .map(|target| format!("{}/{}", target.provider.name, target.model.name))
        .unwrap_or_else(|_| "unconfigured".into());
    let [header, conversation, input, footer] = Layout::vertical([
        Constraint::Length(3),
        Constraint::Min(4),
        Constraint::Length(3),
        Constraint::Length(1),
    ])
    .areas(frame.area());

    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                " CROSSH AGENT ",
                Style::new().fg(Color::Black).bg(Color::Cyan).bold(),
            ),
            Span::raw("  "),
            Span::styled(model_label, Style::new().add_modifier(Modifier::DIM)),
        ]))
        .block(
            Block::new()
                .borders(Borders::BOTTOM)
                .padding(Padding::top(1)),
        ),
        header,
    );

    let mut lines = Vec::new();
    if app.messages.is_empty() {
        lines.push(Line::styled(
            "Ask about the current project. Tool execution is gated by the reviewer model.",
            Style::new().fg(Color::DarkGray),
        ));
    }
    let content_width = conversation.width.saturating_sub(2).max(1) as usize;
    for (role, content) in &app.messages {
        let (label, color) = match role {
            Role::User => ("you", Color::Cyan),
            Role::Reasoning => ("reasoning", Color::DarkGray),
            Role::Agent => ("agent", Color::Green),
            Role::Tool => ("tool", Color::Yellow),
            Role::Error => ("error", Color::Red),
        };
        lines.push(Line::styled(label, Style::new().fg(color).bold()));
        if matches!(role, Role::Tool) && !app.show_tool_details {
            lines.push(Line::from(collapsed_tool_summary(content, content_width)));
        } else if matches!(role, Role::Agent) {
            lines.extend(markdown_content(content, content_width));
        } else {
            lines.extend(wrap_content(content, content_width));
        }
        lines.push(Line::default());
    }
    let conversation_area = conversation.inner(Margin::new(1, 0));
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
    frame.render_widget(Paragraph::new(lines).scroll((scroll, 0)), conversation_area);

    frame.render_widget(
        Paragraph::new(app.input.as_str())
            .style(Style::new().fg(Color::White))
            .block(
                Block::new()
                    .title(" prompt ")
                    .borders(Borders::ALL)
                    .border_style(Style::new().fg(Color::DarkGray))
                    .padding(Padding::horizontal(1)),
            ),
        input,
    );
    let cursor_x = input
        .x
        .saturating_add(2)
        .saturating_add(UnicodeWidthStr::width(app.input.as_str()) as u16);
    frame.set_cursor_position((cursor_x.min(input.right().saturating_sub(2)), input.y + 1));
    frame.render_widget(
        Paragraph::new(app.status.as_str()).style(Style::new().fg(Color::DarkGray)),
        footer,
    );
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

fn collapsed_tool_summary(content: &str, width: usize) -> String {
    let compact = content.split_whitespace().collect::<Vec<_>>().join(" ");
    let limit = width.saturating_sub(4).max(8);
    if UnicodeWidthStr::width(compact.as_str()) <= limit {
        format!("[+] {compact}")
    } else {
        let mut result = String::from("[+] ");
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
mod tests {
    use super::*;

    fn app_with_scroll(scroll: u16, max_scroll: u16) -> App {
        App {
            settings: AgentSettings::default(),
            api_key: None,
            reviewer_api_key: None,
            input: String::new(),
            messages: Vec::new(),
            scroll,
            max_scroll,
            show_tool_details: false,
            status: String::new(),
        }
    }

    #[test]
    fn wrapped_content_counts_display_rows() {
        let lines = wrap_content("abcdefghij\n中中文", 4);
        assert_eq!(lines.len(), 5);
        assert_eq!(lines[0].width(), 4);
        assert_eq!(lines[4].width(), 2);
    }

    #[test]
    fn scrolling_up_from_follow_mode_starts_at_the_real_bottom() {
        let mut app = app_with_scroll(u16::MAX, 40);
        scroll_conversation(&mut app, -1);
        assert_eq!(app.scroll, 39);
        scroll_conversation(&mut app, 1);
        assert_eq!(app.scroll, u16::MAX);
    }

    #[test]
    fn tool_summary_is_single_line_and_bounded() {
        let summary = collapsed_tool_summary("bash {\n  very long arguments here\n}", 18);
        assert!(!summary.contains('\n'));
        assert!(UnicodeWidthStr::width(summary.as_str()) <= 18);
    }

    #[test]
    fn markdown_content_preserves_formatting_and_wraps() {
        let lines = markdown_content("# Title\n\nUse **bold text** here.", 10);
        assert!(lines.len() >= 4);
        assert!(lines.iter().flat_map(|line| &line.spans).any(|span| {
            span.style
                .add_modifier
                .contains(ratatui::style::Modifier::BOLD)
        }));
        assert!(lines.iter().all(|line| line.width() <= 10));
    }
}
