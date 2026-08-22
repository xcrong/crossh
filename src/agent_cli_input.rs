use super::*;

#[cfg(test)]
pub(super) fn queue_input(app: &mut App) {
    queue_steering(app);
}

pub(super) fn queue_steering(app: &mut App) {
    let input = take_input(app);
    if input.trim().is_empty() {
        return;
    }
    app.queue.push_steering(input.clone());
    app.queued_inputs.push_back(input.clone());
    app.messages.push((
        Role::Queued,
        format!("Queued (steering): {}", one_line(&input)),
    ));
    app.status = format!(
        "Queued steering={} follow_up={} — Alt+Up dequeue",
        app.queue.steering.len(),
        app.queue.follow_up.len()
    );
    app.event_bus
        .emit(crossh_agent::AgentSessionEvent::QueueUpdate {
            steering: app.queue.steering.clone(),
            follow_up: app.queue.follow_up.clone(),
        });
}

pub(super) fn queue_follow_up(app: &mut App) {
    let input = take_input(app);
    if input.trim().is_empty() {
        return;
    }
    app.queue.push_follow_up(input.clone());
    app.queued_inputs.push_back(input.clone());
    app.messages.push((
        Role::Queued,
        format!("Queued (follow-up): {}", one_line(&input)),
    ));
    app.status = format!(
        "Queued steering={} follow_up={} — Alt+Up dequeue",
        app.queue.steering.len(),
        app.queue.follow_up.len()
    );
    app.event_bus
        .emit(crossh_agent::AgentSessionEvent::QueueUpdate {
            steering: app.queue.steering.clone(),
            follow_up: app.queue.follow_up.clone(),
        });
}

/// 对齐 pi 的 `app.message.dequeue`（Alt+Up）：将 queued 消息恢复到输入框并清空队列。
pub(super) fn dequeue_queue(app: &mut App) -> bool {
    if app.queue.is_empty() && app.queued_inputs.is_empty() {
        return false;
    }
    let (steering, follow_up) = app.queue.clear_queue();
    let mut restored = Vec::new();
    restored.extend(steering);
    restored.extend(follow_up);
    // 同步清理显示队列，尽量按恢复顺序移除
    for text in &restored {
        if let Some(pos) = app.queued_inputs.iter().position(|x| x == text) {
            app.queued_inputs.remove(pos);
        }
    }
    if !restored.is_empty() {
        if !app.input.is_empty() {
            app.input.push('\n');
        }
        app.input.push_str(&restored.join("\n"));
        app.input_cursor = app.input.len();
        app.history_cursor = None;
    }
    app.queued_inputs.clear();
    app.event_bus
        .emit(crossh_agent::AgentSessionEvent::QueueUpdate {
            steering: Vec::new(),
            follow_up: Vec::new(),
        });
    app.status = "Dequeued — restored to input".into();
    true
}

pub(super) fn edit_input(app: &mut App, key: KeyEvent) {
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

pub(super) fn is_enter_key(code: KeyCode) -> bool {
    matches!(
        code,
        KeyCode::Enter | KeyCode::Char('\r') | KeyCode::Char('\n')
    )
}

pub(super) fn insert_text(app: &mut App, text: &str) {
    app.input.insert_str(app.input_cursor, text);
    app.input_cursor += text.len();
    app.history_cursor = None;
}

pub(super) fn delete_previous_char(app: &mut App) {
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

pub(super) fn delete_previous_word(app: &mut App) {
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

pub(super) fn move_cursor(app: &mut App, right: bool) {
    app.input_cursor = if right {
        next_boundary(&app.input, app.input_cursor)
    } else {
        previous_boundary(&app.input, app.input_cursor)
    };
}

pub(super) fn move_history(app: &mut App, up: bool) {
    let history = app
        .session
        .messages
        .iter()
        .filter(|message| message.role == MessageRole::User)
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

pub(super) fn clear_input(app: &mut App) {
    app.input.clear();
    app.input_cursor = 0;
    app.history_cursor = None;
}

pub(super) fn take_input(app: &mut App) -> String {
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
