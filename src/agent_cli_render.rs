use super::*;

pub(super) fn render(frame: &mut Frame, app: &mut App) {
    let input_height = input_height(frame.area(), app);
    let [header, conversation, input, footer] = agent_layout(frame.area(), input_height);
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
            "No messages yet. Ask about this project or type /help (、help also works).",
            Style::new().fg(faint),
        ));
    }
    for (role, content) in &app.messages {
        let (label, color) = match role {
            Role::User => ("you", accent),
            Role::Reasoning => ("thinking", faint),
            Role::Agent => ("agent", tui_color(theme::diff_add_fg())),
            Role::Tool => ("tool", tui_color(theme::warning())),
            Role::Approval => ("approval", tui_color(theme::accent())),
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
    let conversation_block = Block::new()
        .title(" conversation ")
        .borders(Borders::TOP | Borders::BOTTOM)
        .border_style(Style::new().fg(border));
    let viewport_height = conversation_block.inner(conversation).height as usize;
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
            .block(conversation_block),
        conversation,
    );

    let input_title = if app.queued_inputs.is_empty() {
        " prompt "
    } else {
        " queue next prompt "
    };
    let thinking_border = match app.thinking {
        crossh_agent::ThinkingLevel::Off => faint,
        crossh_agent::ThinkingLevel::Minimal => muted,
        crossh_agent::ThinkingLevel::Low => tui_color(theme::info()),
        crossh_agent::ThinkingLevel::Medium => accent,
        crossh_agent::ThinkingLevel::High => tui_color(theme::warning()),
        crossh_agent::ThinkingLevel::XHigh => tui_color(theme::danger()),
        crossh_agent::ThinkingLevel::Max => tui_color(theme::danger()),
    };
    let input_block = Block::new()
        .title(input_title)
        .borders(Borders::ALL)
        .border_style(Style::new().fg(if app.queued_inputs.is_empty() {
            thinking_border
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

    let slash_cands = slash::slash_candidates(app);
    if !slash_cands.is_empty() && is_command_input(&app.input) {
        let selected = app.slash_selected.min(slash_cands.len().saturating_sub(1));
        let mut popup_lines = Vec::new();
        for (idx, cand) in slash_cands.iter().enumerate() {
            let is_sel = idx == selected;
            let name_style = if is_sel {
                Style::new().fg(bg).bg(accent).add_modifier(Modifier::BOLD)
            } else {
                Style::new().fg(accent).add_modifier(Modifier::BOLD)
            };
            let desc_style = if is_sel {
                Style::new().fg(bg).bg(accent)
            } else {
                Style::new().fg(muted)
            };
            popup_lines.push(Line::from(vec![
                Span::styled(format!(" {:<14}", cand.display), name_style),
                Span::styled(format!(" {}", cand.desc), desc_style),
            ]));
        }
        let popup_height = (slash_cands.len() as u16 + 2).clamp(3, 10);
        let popup_width = input.width.saturating_sub(2).clamp(32, 64);
        let popup_x = input.x;
        let popup_y = input.y.saturating_sub(popup_height);
        let mut popup_rect = Rect {
            x: popup_x,
            y: popup_y,
            width: popup_width,
            height: popup_height,
        };
        if popup_rect.y < conversation.y + 1 {
            popup_rect.y = conversation.y + 1;
        }
        if popup_rect.bottom() > input.y {
            popup_rect.y = input.y.saturating_sub(popup_height);
        }
        frame.render_widget(ratatui::widgets::Clear, popup_rect);
        let hint = if slash_cands.len() == 1 {
            " Tab/Enter 补全 "
        } else {
            " ↑↓ 选择 Tab/Enter 补全 "
        };
        frame.render_widget(
            Paragraph::new(popup_lines).block(
                Block::bordered()
                    .border_style(Style::new().fg(accent))
                    .title(hint)
                    .style(Style::new().bg(bg)),
            ),
            popup_rect,
        );
    }

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

pub(super) fn agent_layout(area: Rect, input_height: u16) -> [Rect; 4] {
    Layout::vertical([
        Constraint::Length(HEADER_HEIGHT),
        Constraint::Fill(1),
        Constraint::Length(input_height),
        Constraint::Length(FOOTER_HEIGHT),
    ])
    .areas(area)
}

pub(super) fn input_height(area: Rect, app: &App) -> u16 {
    let width = area.width.saturating_sub(4).max(1) as usize;
    let lines = visual_line_count(&app.input, width);
    let desired = lines.min(MAX_VISIBLE_INPUT_LINES) as u16 + 2;
    let max_height = area
        .height
        .saturating_sub(HEADER_HEIGHT + FOOTER_HEIGHT + MIN_CONVERSATION_HEIGHT)
        .max(1);
    desired.min(max_height)
}

pub(super) fn cursor_position(area: Rect, input: &str, cursor: usize) -> (u16, u16) {
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

pub(super) fn visual_line_count(input: &str, width: usize) -> usize {
    input
        .split('\n')
        .map(|line| {
            let line_width = UnicodeWidthStr::width(line);
            line_width.max(1).div_ceil(width.max(1))
        })
        .sum::<usize>()
        .max(1)
}

pub(super) fn scroll_conversation(app: &mut App, delta: i16) {
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

pub(super) fn session_name(app: &App) -> String {
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

pub(super) fn wrap_content(content: &str, width: usize) -> Vec<Line<'static>> {
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

pub(super) fn markdown_content(content: &str, width: usize) -> Vec<Line<'static>> {
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
