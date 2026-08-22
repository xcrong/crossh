//! 会话生命周期 — 打开/恢复/新建/分叉/回退会话与可见消息重建。
//!
//! 从 `agent_cli.rs` 拆出以保持单文件 < 2000 行（`scripts/check-architecture.sh`）。

use super::*;

pub(super) fn open_starting_session(
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
pub(super) fn find_session(
    workspace: &Path,
    selector: &str,
) -> Result<Option<AgentSessionSummary>, String> {
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
fn reset_view_state(app: &mut App) {
    app.main_renderer.reset();
    app.screen_renderer.reset();
    app.alt_screen.primary_scroll_view.scroll_to(0);
    app.conversation_lines.clear();
}

pub(super) fn resume_session(app: &mut App, summary: AgentSessionSummary) {
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
            input::clear_input(app);
            reset_view_state(app);
            app.status = format!("Resumed {}", session_name(app));
        }
        Err(error) => push_error(app, error),
    }
}
pub(super) fn new_session(app: &mut App) {
    if app.session_path.is_none() {
        app.session = AgentSession::new(app.workspace.clone());
        app.messages.clear();
        app.queued_inputs.clear();
        app.context_files = load_context_files(&app.workspace);
        app.skills = load_skills(&app.workspace);
        app.prompts = load_prompts(&app.workspace);
        input::clear_input(app);
        reset_view_state(app);
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
            input::clear_input(app);
            reset_view_state(app);
            app.status = "New session".into();
        }
        Err(error) => push_error(app, error),
    }
}
pub(super) fn fork_session(app: &mut App) {
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
pub(super) fn tree_text(app: &App) -> String {
    let mut lines = vec!["Conversation tree (user turns):".to_string()];
    let mut turn = 0;
    for message in &app.session.messages {
        if message.role != MessageRole::User || message.text.is_empty() {
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
pub(super) fn rewind_session(app: &mut App, turn: usize) {
    if turn == 0 {
        app.session.messages.clear();
    } else {
        let user_turns = app
            .session
            .messages
            .iter()
            .enumerate()
            .filter(|(_, message)| message.role == MessageRole::User && !message.text.is_empty())
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
    reset_view_state(app);
    app.status = format!("Rewound to user turn {turn}");
}
pub(super) fn format_session_list(sessions: &[AgentSessionSummary]) -> String {
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
pub(super) fn restore_visible_messages(messages: &[Message]) -> Vec<(Role, String)> {
    let mut visible = Vec::new();
    for message in messages {
        match message.role {
            MessageRole::System => visible.push((Role::Notice, message.text.clone())),
            MessageRole::User => {
                if !message.text.is_empty() {
                    visible.push((Role::User, message.text.clone()));
                }
            }
            MessageRole::Assistant => {
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
