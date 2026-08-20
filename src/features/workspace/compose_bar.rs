use gpui::{
    AnyElement, Context, InteractiveElement, IntoElement, ParentElement, SharedString,
    StatefulInteractiveElement, Styled, Window, div, px,
};

use crossh_ui::icons;
use crossh_ui::theme;
use crossh_ui::widgets::{ime_input_canvas, marked_text_span, text_caret, text_span};
use crossh_ui_component::{Button, ButtonSize, ButtonVariant};

use crate::shared::i18n;

use super::shell::AppShell;

fn line_bounds(text: &str, cursor: usize) -> (usize, usize) {
    let start = text[..cursor].rfind('\n').map(|idx| idx + 1).unwrap_or(0);
    let end = text[cursor..]
        .find('\n')
        .map(|idx| cursor + idx)
        .unwrap_or(text.len());
    (start, end)
}

pub(crate) fn render_compose_bar(
    shell: &mut AppShell,
    window: &Window,
    cx: &mut Context<AppShell>,
) -> AnyElement {
    let focus = shell.compose_focus.clone();
    let value = shell.compose_state.value.clone();
    let ime_marked_text = shell.compose_state.ime_marked_text.clone();
    let ime_replacement = shell.compose_state.ime_replacement;
    let selection = shell.compose_state.selection();
    let cursor = shell.compose_state.cursor;
    let scroll = shell.compose_scroll.clone();
    let focused = focus.is_focused(window);

    // 自动滚动到光标：单行用 x，多行用 y
    scroll.scroll_to_item(1);

    let mut input = div()
        .id("compose-input")
        .flex_1()
        .min_w_0()
        .min_h(px(38.))
        .max_h(px(120.))
        .px_3()
        .py_2()
        .flex()
        .flex_col()
        .gap_1()
        .overflow_y_scroll()
        .overflow_x_scroll()
        .track_scroll(&scroll)
        .bg(theme::canvas())
        .border_1()
        .border_color(theme::border_strong())
        .rounded(px(theme::RADIUS_SM))
        .relative()
        .text_sm()
        .text_color(theme::text())
        .track_focus(&focus)
        .tab_stop(true)
        .focus(|style| style.border_color(theme::focus_ring()))
        .on_click({
            let focus = focus.clone();
            move |_ev, window, cx| window.focus(&focus, cx)
        })
        .on_key_down(cx.listener(AppShell::handle_compose_key));

    if value.is_empty() {
        // 空状态：单行，显示 caret + placeholder 或 IME
        let mut row = div()
            .flex()
            .flex_row()
            .items_center()
            .min_h(px(20.))
            .flex_shrink_0();
        if focused {
            row = row.child(text_caret(px(20.)));
        }
        if ime_marked_text.is_empty() {
            row = row.child(
                div()
                    .flex_shrink_0()
                    .whitespace_nowrap()
                    .text_color(theme::faint_text())
                    .child(SharedString::from(i18n::text("compose.placeholder"))),
            );
        } else {
            row = row.child(marked_text_span(ime_marked_text.clone()));
        }
        input = input.child(row);
    } else {
        // 按 \n 拆行，逐行渲染，正确处理选区/光标跨行
        let lines: Vec<&str> = value.split('\n').collect();
        // 计算每行在原字符串中的字节范围
        let mut line_start = 0usize;
        let mut line_ranges: Vec<(usize, usize)> = Vec::new();
        for line in &lines {
            let start = line_start;
            let end = start + line.len();
            line_ranges.push((start, end));
            line_start = end + 1; // 跳过 \n
        }
        let cursor_line = value[..cursor].chars().filter(|c| *c == '\n').count();
        let (sel_start, sel_end) = selection
            .map(|(a, b)| if a < b { (a, b) } else { (b, a) })
            .unwrap_or((cursor, cursor));
        let has_selection = selection.is_some() && sel_start != sel_end;

        for (idx, line) in lines.iter().enumerate() {
            let (l_start, l_end) = line_ranges[idx];
            // 判断该行是否包含光标
            let is_cursor_line = idx == cursor_line;
            // 该行与选区的重叠区间（行内字节偏移）
            let sel_in_line = if has_selection {
                let overlap_start = sel_start.max(l_start).min(l_end);
                let overlap_end = sel_end.max(l_start).min(l_end).max(overlap_start);
                if overlap_start < overlap_end {
                    Some((overlap_start - l_start, overlap_end - l_start))
                } else {
                    None
                }
            } else {
                None
            };
            // 该行与 IME 替换区间的重叠
            let ime_in_line = if let Some((ime_s, ime_e)) = ime_replacement {
                if !ime_marked_text.is_empty() && ime_s >= l_start && ime_s <= l_end {
                    // IME 替换区在该行内（通常单行）
                    Some((ime_s - l_start, ime_e.min(l_end) - l_start))
                } else {
                    None
                }
            } else {
                None
            };

            let mut row = div()
                .flex()
                .flex_row()
                .items_center()
                .min_h(px(20.))
                .flex_shrink_0();

            if is_cursor_line {
                // 光标所在行的渲染：按选区/IME 拆分
                if let Some((s, e)) = sel_in_line {
                    // 有选区：before + highlighted + after
                    row = row.child(text_span(line[..s].to_string()));
                    row = row.child(
                        div()
                            .flex_shrink_0()
                            .whitespace_nowrap()
                            .bg(theme::accent_soft())
                            .text_color(theme::text())
                            .child(SharedString::from(line[s..e].to_string())),
                    );
                    row = row.child(text_span(line[e..].to_string()));
                    // 选区行不显示 caret
                } else {
                    // 无选区：按光标位置拆分
                    let cursor_col_byte = if cursor >= l_start && cursor <= l_end {
                        cursor - l_start
                    } else {
                        // 光标在行末换行符位置或空行
                        line.len()
                    };
                    // 处理 IME 位于光标处的情况
                    if let Some((ime_s, _)) = ime_in_line {
                        // IME 替换区起点即光标位置
                        let before = &line[..ime_s];
                        let after_start = ime_replacement.unwrap().1.min(l_end) - l_start;
                        let after = &line[after_start..];
                        row = row.child(text_span(before.to_string()));
                        if !ime_marked_text.is_empty() {
                            row = row.child(marked_text_span(ime_marked_text.clone()));
                        }
                        row = row.child(text_span(after.to_string()));
                        if focused && sel_in_line.is_none() {
                            // IME 时 caret 隐藏，待提交后显示
                        }
                    } else {
                        let cursor_byte = cursor_col_byte.min(line.len());
                        row = row.child(text_span(line[..cursor_byte].to_string()));
                        if focused {
                            row = row.child(text_caret(px(20.)));
                        }
                        if !ime_marked_text.is_empty() && is_cursor_line {
                            // 非替换区的 IME（光标处直接插入）
                            row = row.child(marked_text_span(ime_marked_text.clone()));
                        }
                        row = row.child(text_span(line[cursor_byte..].to_string()));
                    }
                }
            } else {
                // 非光标行：仅处理跨行选区高亮
                if let Some((s, e)) = sel_in_line {
                    row = row.child(text_span(line[..s].to_string()));
                    row = row.child(
                        div()
                            .flex_shrink_0()
                            .whitespace_nowrap()
                            .bg(theme::accent_soft())
                            .text_color(theme::text())
                            .child(SharedString::from(line[s..e].to_string())),
                    );
                    row = row.child(text_span(line[e..].to_string()));
                } else {
                    row = row.child(text_span(line.to_string()));
                }
            }
            // 空行保证高度
            if line.is_empty() && !is_cursor_line {
                row = row.child(div().h(px(20.)).w(px(1.)).flex_shrink_0());
            }
            input = input.child(row);
        }
        // 额外处理：值以 \n 结尾时，split 会产生末尾空行，已在上面渲染；
        // 但需确保光标在末尾空行时能显示 caret（已通过 cursor_line 计算覆盖）
        let _ = line_bounds; // 保留 helper 供未来扩展
    }
    input = input.child(ime_input_canvas(focus.clone(), cx.entity()));

    let has_text = !value.trim().is_empty();
    let can_send = has_text && shell.workspace.focused_view().is_some();

    let send_button = Button::new("compose-send")
        .size(ButtonSize::Medium)
        .variant(ButtonVariant::Primary)
        .disabled(!can_send)
        .icon(
            icons::icon(icons::IconName::Play, 13.).text_color(if can_send {
                theme::canvas()
            } else {
                theme::faint_text()
            }),
        )
        .label(i18n::text("compose.send"))
        .tooltip(i18n::text("compose.send_tooltip"))
        .on_click(cx.listener(|this, _ev, window, cx| {
            this.send_compose(cx);
            window.focus(&this.compose_focus, cx);
        }));

    div()
        .id("compose-bar")
        .w_full()
        .flex_shrink_0()
        .flex()
        .flex_row()
        .items_center()
        .gap_2()
        .px_2()
        .py_2()
        .bg(theme::canvas())
        .border_t_1()
        .border_color(theme::border())
        .child(input)
        .child(send_button.into_any_element())
        .into_any_element()
}
