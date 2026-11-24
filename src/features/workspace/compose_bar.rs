use gpui::{
    AnyElement, Context, Div, InteractiveElement, IntoElement, ParentElement, SharedString,
    StatefulInteractiveElement, Styled, Window, div, px,
};

use crossh_ui::icons;
use crossh_ui::theme;
use crossh_ui::widgets::{ime_input_canvas, marked_text_span, text_caret, text_span};
use crossh_ui_component::{Button, ButtonSize, ButtonVariant};

use crossh_core::i18n;

use super::shell::AppShell;

pub(crate) fn render_compose_bar(
    shell: &mut AppShell,
    window: &Window,
    cx: &mut Context<AppShell>,
) -> AnyElement {
    let focus = shell.compose_focus.clone();
    let scroll = shell.compose_scroll.clone();
    let focused = focus.is_focused(window);
    let compose_state = shell
        .workspace
        .focused_pane()
        .and_then(|pane_id| shell.workspace.compose_state_for(pane_id))
        .cloned()
        .unwrap_or_else(|| crossh_core::text_editing::TextEditingState::new(String::new()));
    let value = compose_state.value.clone();
    let ime_marked_text = compose_state.ime_marked_text.clone();
    let ime_replacement = compose_state.ime_replacement;
    let selection = compose_state.selection();
    let cursor = compose_state.cursor;

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
        input = input.child(render_compose_empty_row(focused, &ime_marked_text));
    } else {
        // 按 \n 拆行，逐行渲染，正确处理选区/光标跨行
        let cursor_line = value[..cursor].chars().filter(|c| *c == '\n').count();
        let (sel_start, sel_end) = selection
            .map(|(a, b)| if a < b { (a, b) } else { (b, a) })
            .unwrap_or((cursor, cursor));
        let ctx = ComposeLineContext {
            cursor_line,
            cursor,
            sel_start,
            sel_end,
            has_selection: selection.is_some() && sel_start != sel_end,
            ime_marked_text: &ime_marked_text,
            ime_replacement,
            focused,
        };
        for row in render_compose_rows(&value, &ctx) {
            input = input.child(row);
        }
    }
    input = input.child(ime_input_canvas(focus.clone(), cx.entity()));

    let has_text = !value.trim().is_empty();
    let can_send = has_text && shell.workspace.focused_pane().is_some();
    let send_button = render_compose_send_button(can_send, cx);

    render_compose_bar_frame(input, send_button.into_any_element())
}

/// 逐行渲染所需的共享状态：光标行、选区、IME 与焦点。
struct ComposeLineContext<'a> {
    cursor_line: usize,
    cursor: usize,
    sel_start: usize,
    sel_end: usize,
    has_selection: bool,
    ime_marked_text: &'a str,
    ime_replacement: Option<(usize, usize)>,
    focused: bool,
}

/// 单行基础容器，空态行与多行行共用同一套布局。
fn compose_row() -> Div {
    div()
        .flex()
        .flex_row()
        .items_center()
        .min_h(px(20.))
        .flex_shrink_0()
}

/// 空值行：caret + placeholder（或 IME 组合文本）。
fn render_compose_empty_row(focused: bool, ime_marked_text: &str) -> Div {
    let mut row = compose_row();
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
        row = row.child(marked_text_span(ime_marked_text));
    }
    row
}

/// 该行与选区的重叠区间（行内字节偏移）。
fn compose_sel_overlap(
    range: (usize, usize),
    ctx: &ComposeLineContext<'_>,
) -> Option<(usize, usize)> {
    if !ctx.has_selection {
        return None;
    }
    let (l_start, l_end) = range;
    let overlap_start = ctx.sel_start.max(l_start).min(l_end);
    let overlap_end = ctx.sel_end.max(l_start).min(l_end).max(overlap_start);
    (overlap_start < overlap_end).then_some((overlap_start - l_start, overlap_end - l_start))
}

/// 该行与 IME 替换区间的重叠（行内字节偏移，通常单行）。
fn compose_ime_overlap(
    range: (usize, usize),
    ctx: &ComposeLineContext<'_>,
) -> Option<(usize, usize)> {
    let (l_start, l_end) = range;
    let (ime_s, ime_e) = ctx.ime_replacement?;
    // ponytail: 补 ime_e 下界校验，否则 `ime_e.min(l_end) - l_start` 在区间
    // 完全位于行首之前时下溢 panic。
    if !ctx.ime_marked_text.is_empty() && ime_s <= l_end && ime_e >= l_start && ime_s <= ime_e {
        let start = ime_s.max(l_start) - l_start;
        let end = ime_e.min(l_end).max(ime_s.max(l_start)) - l_start;
        Some((start, end))
    } else {
        None
    }
}

/// 选区高亮段：before + highlighted + after。
fn compose_selection_spans(mut row: Div, line: &str, (s, e): (usize, usize)) -> Div {
    // ponytail: `get` 回退空串，陈旧选区区间不再 panic。
    let s = s.min(line.len());
    let e = e.min(line.len()).max(s);
    row = row.child(text_span(line.get(..s).unwrap_or("").to_string()));
    row = row.child(
        div()
            .flex_shrink_0()
            .whitespace_nowrap()
            .bg(theme::accent_soft())
            .text_color(theme::text())
            .child(SharedString::from(line.get(s..e).unwrap_or("").to_string())),
    );
    row.child(text_span(line.get(e..).unwrap_or("").to_string()))
}

/// 渲染一行：光标行按选区/IME/普通拆分，非光标行仅处理跨行选区高亮。
fn render_compose_line_row(
    line: &str,
    range: (usize, usize),
    idx: usize,
    ctx: &ComposeLineContext<'_>,
) -> Div {
    let (l_start, l_end) = range;
    // 判断该行是否包含光标
    let is_cursor_line = idx == ctx.cursor_line;
    let sel_in_line = compose_sel_overlap(range, ctx);
    let ime_in_line = compose_ime_overlap(range, ctx);

    let mut row = compose_row();
    if is_cursor_line {
        // 光标所在行的渲染：按选区/IME 拆分
        if let Some((s, e)) = sel_in_line {
            // 有选区：before + highlighted + after
            row = compose_selection_spans(row, line, (s, e));
            // 选区行不显示 caret
        } else {
            // 无选区：按光标位置拆分
            let cursor_col_byte = if ctx.cursor >= l_start && ctx.cursor <= l_end {
                ctx.cursor - l_start
            } else {
                // 光标在行末换行符位置或空行
                line.len()
            };
            // 处理 IME 位于光标处的情况
            if let Some((ime_s, _)) = ime_in_line {
                // IME 替换区起点即光标位置
                // ponytail: `get` 回退空串，陈旧 IME 区间不再 panic。
                let before = line.get(..ime_s).unwrap_or("");
                let after_start = ctx.ime_replacement.unwrap().1.min(l_end) - l_start;
                let after = line.get(after_start.min(line.len())..).unwrap_or("");
                row = row.child(text_span(before.to_string()));
                if !ctx.ime_marked_text.is_empty() {
                    row = row.child(marked_text_span(ctx.ime_marked_text));
                }
                row = row.child(text_span(after.to_string()));
                if ctx.focused && sel_in_line.is_none() {
                    // IME 时 caret 隐藏，待提交后显示
                }
            } else {
                // ponytail: 退到上一个字符边界，陈旧光标不再 panic。
                let mut cursor_byte = cursor_col_byte.min(line.len());
                while !line.is_char_boundary(cursor_byte) {
                    cursor_byte -= 1;
                }
                row = row.child(text_span(line.get(..cursor_byte).unwrap_or("").to_string()));
                if ctx.focused {
                    row = row.child(text_caret(px(20.)));
                }
                if !ctx.ime_marked_text.is_empty() && is_cursor_line {
                    // 非替换区的 IME（光标处直接插入）
                    row = row.child(marked_text_span(ctx.ime_marked_text));
                }
                row = row.child(text_span(line.get(cursor_byte..).unwrap_or("").to_string()));
            }
        }
    } else if let Some((s, e)) = sel_in_line {
        // 非光标行：仅处理跨行选区高亮
        row = compose_selection_spans(row, line, (s, e));
    } else {
        row = row.child(text_span(line.to_string()));
    }
    // 空行保证高度
    if line.is_empty() && !is_cursor_line {
        row = row.child(div().h(px(20.)).w(px(1.)).flex_shrink_0());
    }
    row
}

/// 按 `\n` 拆行并逐行渲染；末尾空行由 `split` 自然产生。
fn render_compose_rows(value: &str, ctx: &ComposeLineContext<'_>) -> Vec<Div> {
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
    lines
        .iter()
        .enumerate()
        .map(|(idx, line)| render_compose_line_row(line, line_ranges[idx], idx, ctx))
        .collect()
}

/// 发送按钮：图标颜色、禁用态、文案与快捷键行为。
fn render_compose_send_button(can_send: bool, cx: &mut Context<AppShell>) -> impl IntoElement {
    Button::new("compose-send")
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
        }))
}

/// 最外层 `compose-bar` 容器。
fn render_compose_bar_frame(input: impl IntoElement, send_button: AnyElement) -> AnyElement {
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
        .child(send_button)
        .into_any_element()
}
