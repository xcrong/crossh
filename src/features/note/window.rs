//! Note 窗口 — TextEditingState 多行编辑，修复 Editor 键盘与主题

use crossh_note::{Note, NoteStore};
use crossh_ui::widgets::{ime_input_canvas, marked_text_span, text_caret, text_span, text_width};
use crossh_ui::{icons, theme};
use crossh_ui_component::{BadgeTone, Button, ButtonSize, ButtonVariant, StatusBar, StatusMetric};
use gpui::{
    AnyElement, App, AppContext, Bounds, ClipboardItem, Context, EntityInputHandler, FocusHandle,
    Focusable, InteractiveElement, IntoElement, KeyDownEvent, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, ParentElement, Pixels, Point, Rgba, ScrollHandle, SharedString,
    Size, StatefulInteractiveElement, Styled, TitlebarOptions, UTF16Selection, Window,
    WindowBounds, WindowOptions, div, point, px, size,
};

use super::markdown::render_markdown;
use super::{CloseNoteWindow, DeleteNote, NewNote, SaveNote, TogglePreview};
use crate::shared::input_handler::{
    editing_mark_text, editing_marked_range, editing_replace, editing_selected_range,
    editing_unmark,
};
use crate::shared::text_editing::{
    EditingKeystroke, TextEditingState, byte_index_for_utf16, handle_text_editing_key, utf16_len,
    utf16_offset_for_byte, utf16_slice,
};

const NOTE_WINDOW_CONTEXT: &str = "NoteWindow";
const NOTE_INPUT_PADDING: Pixels = px(12.);
const NOTE_FONT_SIZE: Pixels = px(14.);
const NOTE_LINE_HEIGHT: Pixels = px(18.);
const NOTE_LINE_HEIGHT_F32: f32 = 18.;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ActiveField {
    Search,
    Content,
}

pub struct NoteWindow {
    store: Option<NoteStore>,
    notes: Vec<Note>,
    selected_id: Option<i64>,
    preview: bool,
    search_state: TextEditingState,
    search_focus: FocusHandle,
    content_state: TextEditingState,
    content_focus: FocusHandle,
    content_scroll: ScrollHandle,
    list_scroll: ScrollHandle,
    window_focus: FocusHandle,
    search_bounds: Option<Bounds<Pixels>>,
    content_bounds: Option<Bounds<Pixels>>,
    content_dragging: bool,
}

impl NoteWindow {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let store = NoteStore::open_default().ok();
        let mut this = Self {
            store: None,
            notes: Vec::new(),
            selected_id: None,
            preview: false,
            search_state: TextEditingState::new(String::new()),
            search_focus: cx.focus_handle(),
            content_state: TextEditingState::new(String::new()),
            content_focus: cx.focus_handle(),
            content_scroll: ScrollHandle::new(),
            list_scroll: ScrollHandle::new(),
            window_focus: cx.focus_handle(),
            search_bounds: None,
            content_bounds: None,
            content_dragging: false,
        };
        if let Some(s) = store {
            this.store = Some(s);
            this.reload_notes(cx);
        }
        // 初始聚焦内容区，确保历史笔记加载后即进入编辑态（可复制/IME）。
        // 直接 focus 可能在首帧 track_focus 之前丢失，额外 defer 一次保证。
        cx.defer_in(window, |this, window, cx| {
            window.focus(&this.content_focus, cx);
            cx.notify();
        });
        window.focus(&this.content_focus, cx);
        this
    }

    fn active_field(&self, window: &Window) -> Option<ActiveField> {
        if self.search_focus.is_focused(window) {
            Some(ActiveField::Search)
        } else if self.content_focus.is_focused(window) {
            Some(ActiveField::Content)
        } else {
            None
        }
    }

    fn is_draft_dirty(&self) -> bool {
        !self.content_state.value.trim().is_empty()
    }

    fn reload_notes(&mut self, cx: &mut Context<Self>) {
        let Some(store) = &self.store else { return };
        let query = self.search_state.value.trim().to_string();
        let notes = if query.is_empty() {
            store.list().unwrap_or_default()
        } else {
            store.search(&query).unwrap_or_default()
        };
        let prev_selected = self.selected_id;
        self.notes = notes;
        let still_exists = self
            .selected_id
            .is_some_and(|id| self.notes.iter().any(|n| n.id == id));
        if !still_exists {
            // 保留未保存草稿：若当前编辑器有内容，则不自动切换到首条，避免丢弃用户输入
            let should_preserve_draft = self.is_draft_dirty();
            self.selected_id = None;
            if should_preserve_draft && prev_selected.is_none() {
                // 新建草稿被过滤，保留现状
                cx.notify();
                return;
            }
            if should_preserve_draft && prev_selected.is_some() {
                // 已选笔记被过滤但编辑器为脏，保持草稿不自动选中，待用户显式选择
                // 若后续由保存触发的 reload，则会重新选中
                if !query.is_empty() {
                    cx.notify();
                    return;
                }
            }
        }
        if self.selected_id.is_none() && !self.notes.is_empty() {
            let first = self.notes[0].clone();
            self.select_note(first.id, cx);
        } else if self.selected_id.is_none() {
            self.content_state = TextEditingState::new(String::new());
        }
        cx.notify();
    }

    fn select_note(&mut self, id: i64, cx: &mut Context<Self>) {
        self.selected_id = Some(id);
        if let Some(note) = self.notes.iter().find(|n| n.id == id).cloned() {
            self.content_state = TextEditingState::new(note.content.clone());
            self.preview = false;
            self.content_dragging = false;
        }
        cx.notify();
    }

    fn select_note_focused(&mut self, id: i64, window: &mut Window, cx: &mut Context<Self>) {
        self.select_note(id, cx);
        window.focus(&self.content_focus, cx);
        // 确保选区/IME 坐标失效后重算
        window.invalidate_character_coordinates();
    }

    fn save_current(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(store) = &self.store else { return };
        let content = self.content_state.value.trim().to_string();
        if content.is_empty() {
            return;
        }
        if let Some(id) = self.selected_id {
            if let Some(note) = self.notes.iter().find(|n| n.id == id)
                && note.content == content
            {
                return;
            }
            match store.update(id, &content) {
                Ok(_) => {
                    self.reload_notes(cx);
                    // 保存后保持编辑态，便于继续复制/编辑
                    window.focus(&self.content_focus, cx);
                }
                Err(e) => log::warn!("note update failed: {}", e),
            }
        } else {
            match store.create(&content) {
                Ok(note) => {
                    self.selected_id = Some(note.id);
                    self.reload_notes(cx);
                    window.focus(&self.content_focus, cx);
                }
                Err(e) => log::warn!("note create failed: {}", e),
            }
        }
    }

    fn new_note(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.selected_id = None;
        self.content_state = TextEditingState::new(String::new());
        self.preview = false;
        self.content_dragging = false;
        window.focus(&self.content_focus, cx);
        cx.notify();
    }

    fn delete_current(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(id) = self.selected_id else { return };
        if let Some(store) = &self.store {
            let _ = store.delete(id);
        }
        self.selected_id = None;
        self.reload_notes(cx);
        // 删除后若仍有笔记，自动聚焦内容区
        if !self.notes.is_empty() {
            window.focus(&self.content_focus, cx);
        }
    }

    fn toggle_preview(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.preview = !self.preview;
        if !self.preview {
            window.focus(&self.content_focus, cx);
        }
        cx.notify();
    }

    fn toggle_pin(&mut self, id: i64, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(store) = &self.store
            && let Some(note) = self.notes.iter().find(|n| n.id == id)
        {
            let _ = store.set_pinned(id, !note.pinned);
            self.reload_notes(cx);
            // 置顶切换后保持内容可编辑
            if self.selected_id.is_some() {
                window.focus(&self.content_focus, cx);
            }
        }
    }

    fn is_dirty(&self) -> bool {
        let content = self.content_state.value.trim().to_string();
        if let Some(id) = self.selected_id
            && let Some(note) = self.notes.iter().find(|n| n.id == id)
        {
            return content != note.content;
        }
        !content.is_empty()
    }

    fn is_save_disabled(&self) -> bool {
        let content = self.content_state.value.trim();
        if content.is_empty() {
            return true;
        }
        !self.is_dirty()
    }

    fn status_title(&self) -> String {
        if let Some(id) = self.selected_id
            && let Some(note) = self.notes.iter().find(|n| n.id == id)
        {
            return note
                .content
                .lines()
                .next()
                .unwrap_or("空笔记")
                .chars()
                .take(30)
                .collect();
        }
        if self.is_dirty() {
            return "新建笔记".to_string();
        }
        if self.notes.is_empty() {
            return "无笔记".to_string();
        }
        "未选择".to_string()
    }

    fn render_status_bar(&self, _window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let total = self.notes.len();
        let pinned = self.notes.iter().filter(|n| n.pinned).count();
        let char_count = self.content_state.value.chars().count();
        let dirty = self.is_dirty();
        let save_disabled = self.is_save_disabled();
        let title = self.status_title();

        let mut left = div()
            .min_w_0()
            .flex()
            .items_center()
            .gap_2()
            .child(icons::icon(icons::IconName::FileText, 13.).text_color(theme::accent()))
            .child(
                div()
                    .min_w_0()
                    .max_w(px(240.))
                    .truncate()
                    .text_color(theme::text())
                    .child(SharedString::from(title)),
            );
        if total > 0 {
            left = left.child(StatusMetric::new(format!("{} 条", total)).tone(BadgeTone::Neutral));
        }
        if pinned > 0 {
            left = left.child(StatusMetric::new(format!("📌 {}", pinned)).tone(BadgeTone::Accent));
        }
        if dirty {
            left = left.child(StatusMetric::new("未保存").tone(BadgeTone::Warning));
        }
        if self.preview {
            left = left.child(StatusMetric::new("预览").tone(BadgeTone::Info));
        } else if char_count > 0 {
            left = left
                .child(StatusMetric::new(format!("{} 字", char_count)).tone(BadgeTone::Neutral));
        }

        let actions = div()
            .flex_shrink_0()
            .flex()
            .items_center()
            .gap_1()
            .child(
                Button::new("note-status-new")
                    .size(ButtonSize::Icon(px(22.)))
                    .variant(ButtonVariant::Ghost)
                    .icon(icons::icon(icons::IconName::Plus, 13.).text_color(theme::muted_text()))
                    .tooltip("新建笔记")
                    .on_click(cx.listener(|this, _, window, cx| this.new_note(window, cx))),
            )
            .child(
                Button::new("note-status-save")
                    .size(ButtonSize::Icon(px(22.)))
                    .variant(ButtonVariant::Ghost)
                    .icon(icons::icon(icons::IconName::Save, 13.).text_color(
                        if dirty && !save_disabled {
                            theme::accent()
                        } else {
                            theme::muted_text()
                        },
                    ))
                    .tooltip("保存")
                    .disabled(save_disabled)
                    .on_click(cx.listener(|this, _, window, cx| this.save_current(window, cx))),
            )
            .child(
                Button::new("note-status-delete")
                    .size(ButtonSize::Icon(px(22.)))
                    .variant(ButtonVariant::Ghost)
                    .icon(icons::icon(icons::IconName::Trash, 13.).text_color(theme::muted_text()))
                    .tooltip("删除")
                    .disabled(self.selected_id.is_none())
                    .on_click(cx.listener(|this, _, window, cx| this.delete_current(window, cx))),
            )
            .child(
                Button::new("note-status-preview")
                    .size(ButtonSize::Icon(px(22.)))
                    .variant(ButtonVariant::Ghost)
                    .selected(self.preview)
                    .icon(if self.preview {
                        icons::icon(icons::IconName::Pencil, 13.).text_color(if self.preview {
                            theme::accent()
                        } else {
                            theme::muted_text()
                        })
                    } else {
                        icons::icon(icons::IconName::FileText, 13.).text_color(theme::muted_text())
                    })
                    .tooltip(if self.preview { "编辑" } else { "预览" })
                    .on_click(cx.listener(|this, _, window, cx| this.toggle_preview(window, cx))),
            );

        StatusBar::new("note-status-bar")
            .child(left)
            .child(actions)
            .into_any_element()
    }

    fn handle_search_key(
        &mut self,
        event: &KeyDownEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let ks = event.keystroke.clone();
        let key_lower = ks.key.to_lowercase();
        // 单行搜索框：上下方向键用于列表导航，不应被文本编辑消费
        if matches!(key_lower.as_str(), "up" | "arrowup" | "down" | "arrowdown") {
            if self.notes.is_empty() {
                return;
            }
            let current_idx = self
                .selected_id
                .and_then(|id| self.notes.iter().position(|n| n.id == id))
                .unwrap_or(0);
            let new_idx = if key_lower == "up" || key_lower == "arrowup" {
                current_idx.saturating_sub(1)
            } else {
                (current_idx + 1).min(self.notes.len().saturating_sub(1))
            };
            if let Some(note) = self.notes.get(new_idx).cloned() {
                self.select_note(note.id, cx);
            }
            return;
        }
        let editing_ks = EditingKeystroke {
            key: ks.key.clone(),
            key_char: ks.key_char.clone(),
            control: ks.modifiers.control,
            platform: ks.modifiers.platform,
            shift: ks.modifiers.shift,
        };
        let paste_text = if (editing_ks.control || editing_ks.platform)
            && editing_ks.key.to_lowercase() == "v"
        {
            cx.read_from_clipboard()
                .and_then(|item| item.text().map(|s| s.to_string()))
        } else {
            None
        };
        let result =
            handle_text_editing_key(&mut self.search_state, &editing_ks, paste_text.as_deref());
        if let Some(t) = result.copy_text {
            cx.write_to_clipboard(ClipboardItem::new_string(t));
        }
        if result.handled {
            self.reload_notes(cx);
            cx.notify();
        }
    }

    fn handle_content_key(
        &mut self,
        event: &KeyDownEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let ks = event.keystroke.clone();
        let key_lower = ks.key.to_lowercase();
        let primary = ks.modifiers.control || ks.modifiers.platform;
        // 多行编辑器：回车插入换行，拦截在通用分发之前（避免与提交语义混淆）
        if matches!(key_lower.as_str(), "enter" | "return") {
            if primary {
                // 组合键（如 cmd+enter）保留给未来快捷键，不插入换行
                return;
            }
            self.content_state.clear_composition();
            self.content_state.replace_selection("\n");
            cx.notify();
            return;
        }
        if key_lower == "tab" {
            // 保留焦点遍历：shift+tab / ctrl+tab / cmd+tab 不劫持
            if primary || ks.modifiers.shift {
                return;
            }
            self.content_state.clear_composition();
            self.content_state.replace_selection("\t");
            cx.notify();
            return;
        }
        let editing_ks = EditingKeystroke {
            key: ks.key.clone(),
            key_char: ks.key_char.clone(),
            control: ks.modifiers.control,
            platform: ks.modifiers.platform,
            shift: ks.modifiers.shift,
        };
        let paste_text = if (editing_ks.control || editing_ks.platform)
            && editing_ks.key.to_lowercase() == "v"
        {
            cx.read_from_clipboard()
                .and_then(|item| item.text().map(|s| s.to_string()))
        } else {
            None
        };
        let result =
            handle_text_editing_key(&mut self.content_state, &editing_ks, paste_text.as_deref());
        if let Some(t) = result.copy_text {
            cx.write_to_clipboard(ClipboardItem::new_string(t));
        }
        if result.handled {
            cx.notify();
        }
    }
}

fn line_starts(text: &str) -> Vec<usize> {
    let mut starts = vec![0];
    for (i, ch) in text.char_indices() {
        if ch == '\n' {
            starts.push(i + 1);
        }
    }
    starts
}

fn closest_byte_for_x(window: &Window, text: &str, relative_x: Pixels) -> usize {
    if relative_x <= px(0.) {
        return 0;
    }
    let mut best_byte = text.len();
    let mut best_dist = f32::MAX;
    for (byte_idx, _) in text
        .char_indices()
        .chain(std::iter::once((text.len(), '\0')))
    {
        let w = text_width(window, &text[..byte_idx], NOTE_FONT_SIZE);
        let dist = (w - relative_x).abs().as_f32();
        if dist < best_dist {
            best_dist = dist;
            best_byte = byte_idx;
        }
    }
    best_byte
}

impl Focusable for NoteWindow {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.window_focus.clone()
    }
}

impl EntityInputHandler for NoteWindow {
    fn text_for_range(
        &mut self,
        range: std::ops::Range<usize>,
        _adjusted_range: &mut Option<std::ops::Range<usize>>,
        window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<String> {
        let field = self.active_field(window)?;
        let state = match field {
            ActiveField::Search => &self.search_state,
            ActiveField::Content => &self.content_state,
        };
        Some(utf16_slice(&state.value, range))
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        let field = self.active_field(window)?;
        let state = match field {
            ActiveField::Search => &self.search_state,
            ActiveField::Content => &self.content_state,
        };
        let sel = editing_selected_range(state);
        Some(UTF16Selection {
            range: sel.range,
            reversed: sel.reversed,
        })
    }

    fn marked_text_range(
        &self,
        window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<std::ops::Range<usize>> {
        let field = self.active_field(window)?;
        let state = match field {
            ActiveField::Search => &self.search_state,
            ActiveField::Content => &self.content_state,
        };
        editing_marked_range(state)
    }

    fn unmark_text(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(field) = self.active_field(window) else {
            return;
        };
        match field {
            ActiveField::Search => editing_unmark(&mut self.search_state),
            ActiveField::Content => editing_unmark(&mut self.content_state),
        }
        window.invalidate_character_coordinates();
        cx.notify();
    }

    fn replace_text_in_range(
        &mut self,
        replacement_range: Option<std::ops::Range<usize>>,
        text: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(field) = self.active_field(window) else {
            return;
        };
        let state = match field {
            ActiveField::Search => &mut self.search_state,
            ActiveField::Content => &mut self.content_state,
        };
        editing_replace(state, replacement_range, text);
        if matches!(field, ActiveField::Search) {
            self.reload_notes(cx);
        }
        window.invalidate_character_coordinates();
        cx.notify();
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        _range: Option<std::ops::Range<usize>>,
        new_text: &str,
        _new_selected_range: Option<std::ops::Range<usize>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(field) = self.active_field(window) else {
            return;
        };
        let state = match field {
            ActiveField::Search => &mut self.search_state,
            ActiveField::Content => &mut self.content_state,
        };
        editing_mark_text(state, new_text);
        window.invalidate_character_coordinates();
        cx.notify();
    }

    fn bounds_for_range(
        &mut self,
        range: std::ops::Range<usize>,
        element_bounds: Bounds<Pixels>,
        window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let field = self.active_field(window)?;
        match field {
            ActiveField::Search => {
                self.search_bounds = Some(element_bounds);
                let state = &self.search_state;
                let cursor = byte_index_for_utf16(&state.value, range.start);
                Some(crossh_ui::widgets::ime_caret_bounds(
                    window,
                    element_bounds,
                    &state.value[..cursor],
                    NOTE_FONT_SIZE,
                    NOTE_INPUT_PADDING,
                    px(0.),
                ))
            }
            ActiveField::Content => {
                self.content_bounds = Some(element_bounds);
                let state = &self.content_state;
                let scroll = self.content_scroll.clone();
                let cursor = byte_index_for_utf16(&state.value, range.start);
                let text_before = &state.value[..cursor.min(state.value.len())];
                let line = text_before.chars().filter(|&c| c == '\n').count();
                let line_before = text_before.rsplit('\n').next().unwrap_or("");
                let x = text_width(window, line_before, NOTE_FONT_SIZE);
                let y = NOTE_LINE_HEIGHT * (line as f32);
                Some(Bounds {
                    origin: point(
                        element_bounds.origin.x + NOTE_INPUT_PADDING + x - scroll.offset().x,
                        element_bounds.origin.y + NOTE_INPUT_PADDING + y - scroll.offset().y,
                    ),
                    size: size(px(2.), NOTE_LINE_HEIGHT),
                })
            }
        }
    }

    fn character_index_for_point(
        &mut self,
        point: Point<Pixels>,
        window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        let field = self.active_field(window)?;
        match field {
            ActiveField::Search => {
                let bounds = self.search_bounds?;
                let state = &self.search_state;
                let relative_x = point.x - bounds.origin.x - NOTE_INPUT_PADDING;
                let best_byte = closest_byte_for_x(window, &state.value, relative_x);
                Some(utf16_offset_for_byte(&state.value, best_byte))
            }
            ActiveField::Content => {
                let bounds = self.content_bounds?;
                let state = &self.content_state;
                let scroll = self.content_scroll.offset();
                // y -> 行
                let relative_y = point.y - bounds.origin.y - NOTE_INPUT_PADDING + scroll.y;
                let line_idx = (relative_y.as_f32() / NOTE_LINE_HEIGHT_F32)
                    .floor()
                    .max(0.0) as usize;
                let starts = line_starts(&state.value);
                let line_idx = line_idx.min(starts.len().saturating_sub(1));
                let line_start_byte = starts[line_idx];
                let line_end = starts
                    .get(line_idx + 1)
                    .map(|v| v - 1)
                    .unwrap_or(state.value.len());
                let line = &state.value[line_start_byte..line_end];
                let relative_x = point.x - bounds.origin.x - NOTE_INPUT_PADDING + scroll.x;
                if line.is_empty() {
                    return Some(utf16_offset_for_byte(&state.value, line_start_byte));
                }
                let col_byte = closest_byte_for_x(window, line, relative_x);
                Some(utf16_offset_for_byte(
                    &state.value,
                    line_start_byte + col_byte,
                ))
            }
        }
    }

    fn text_length_utf16(&mut self, window: &mut Window, _cx: &mut Context<Self>) -> Option<usize> {
        let field = self.active_field(window)?;
        let state = match field {
            ActiveField::Search => &self.search_state,
            ActiveField::Content => &self.content_state,
        };
        Some(utf16_len(&state.value))
    }
}

impl gpui::Render for NoteWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let search_state = self.search_state.clone();
        let content_state = self.content_state.clone();
        let notes = self.notes.clone();

        let header = div()
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .p_2()
            .border_b_1()
            .border_color(theme::border())
            .bg(theme::surface())
            .child(render_search_field(
                search_state,
                self.search_focus.clone(),
                window,
                cx,
            ));

        let list = div()
            .w(px(260.))
            .min_w(px(180.))
            .max_w(px(320.))
            .h_full()
            .flex()
            .flex_col()
            .overflow_hidden()
            .border_r_1()
            .border_color(theme::border())
            .bg(theme::surface())
            .child(
                div()
                    .id("note-list")
                    .flex_1()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .p_2()
                    .overflow_y_scroll()
                    .track_scroll(&self.list_scroll)
                    .children(notes.iter().map(|note| {
                        let is_selected = Some(note.id) == self.selected_id;
                        let id = note.id;
                        let pin_label = if note.pinned { "📌 " } else { "" };
                        let preview_text = note
                            .content
                            .lines()
                            .next()
                            .unwrap_or("空笔记")
                            .chars()
                            .take(30)
                            .collect::<String>();
                        div()
                            .id(("note-item", note.id as usize))
                            .w_full()
                            .p_2()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .rounded(px(theme::RADIUS_SM))
                            .bg(if is_selected {
                                theme::accent_soft()
                            } else {
                                Rgba {
                                    r: 0.,
                                    g: 0.,
                                    b: 0.,
                                    a: 0.,
                                }
                            })
                            .border_1()
                            .border_color(if is_selected {
                                theme::accent()
                            } else {
                                Rgba {
                                    r: 0.,
                                    g: 0.,
                                    b: 0.,
                                    a: 0.,
                                }
                            })
                            .cursor_pointer()
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.select_note_focused(id, window, cx)
                            }))
                            .child(
                                div()
                                    .flex()
                                    .flex_row()
                                    .items_center()
                                    .justify_between()
                                    .child(
                                        div()
                                            .text_sm()
                                            .text_color(theme::text())
                                            .child(format!("{}{}", pin_label, preview_text)),
                                    )
                                    .child(
                                        Button::new(("pin", note.id as usize))
                                            .size(ButtonSize::Icon(px(18.)))
                                            .variant(ButtonVariant::Ghost)
                                            .icon(
                                                icons::icon(icons::IconName::Pin, 10.).text_color(
                                                    if note.pinned {
                                                        theme::accent()
                                                    } else {
                                                        theme::muted_text()
                                                    },
                                                ),
                                            )
                                            .on_click(cx.listener(move |this, _, window, cx| {
                                                this.toggle_pin(id, window, cx)
                                            })),
                                    ),
                            )
                    })),
            );

        let right: AnyElement = if self.preview {
            let md = self.content_state.value.clone();
            div()
                .id("note-preview")
                .flex_1()
                .min_h_0()
                .h_full()
                .min_w(px(320.))
                .p_3()
                .bg(theme::canvas())
                .overflow_y_scroll()
                .child(render_markdown(&md))
                .into_any_element()
        } else {
            div()
                .flex_1()
                .min_h_0()
                .h_full()
                .min_w(px(320.))
                .flex()
                .flex_col()
                .overflow_hidden()
                .p_2()
                .child(render_content_editor(
                    content_state,
                    self.content_focus.clone(),
                    self.content_scroll.clone(),
                    window,
                    cx,
                ))
                .into_any_element()
        };

        let body = div()
            .flex_1()
            .min_h_0()
            .w_full()
            .flex()
            .flex_row()
            .overflow_hidden()
            .child(list)
            .child(right);

        let linux_titlebar =
            crossh_ui::linux_titlebar::render_linux_titlebar(window, cx, "Note".into());

        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(theme::canvas())
            .track_focus(&self.window_focus)
            .on_action(cx.listener(|_, _: &CloseNoteWindow, window, _| {
                window.remove_window();
            }))
            .on_action(cx.listener(|this, _: &NewNote, window, cx| this.new_note(window, cx)))
            .on_action(cx.listener(|this, _: &SaveNote, window, cx| this.save_current(window, cx)))
            .on_action(
                cx.listener(|this, _: &TogglePreview, window, cx| this.toggle_preview(window, cx)),
            )
            .on_action(
                cx.listener(|this, _: &DeleteNote, window, cx| this.delete_current(window, cx)),
            )
            .key_context(NOTE_WINDOW_CONTEXT)
            .children(linux_titlebar)
            .child(header)
            .child(body)
            .child(self.render_status_bar(window, cx))
    }
}

fn render_search_field(
    state: TextEditingState,
    focus: FocusHandle,
    window: &Window,
    cx: &mut Context<NoteWindow>,
) -> AnyElement {
    let focused = focus.is_focused(window);
    let value = state.value.clone();
    let cursor = state.cursor;
    let anchor = state.anchor.unwrap_or(cursor);
    let ime_marked = state.ime_marked_text.clone();

    let input = div()
        .id("note-search")
        .flex_1()
        .p_2()
        .bg(theme::canvas())
        .border_1()
        .border_color(if focused {
            theme::accent()
        } else {
            theme::border()
        })
        .rounded(px(theme::RADIUS_SM))
        .text_sm()
        .text_color(theme::text())
        .track_focus(&focus)
        .tab_stop(true)
        .cursor_text()
        .on_click({
            let focus = focus.clone();
            move |_, window: &mut Window, cx: &mut App| window.focus(&focus, cx)
        })
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, event: &MouseDownEvent, window, cx| {
                window.focus(&this.search_focus, cx);
                if let Some(bounds) = this.search_bounds {
                    let state = &mut this.search_state;
                    let extend = event.modifiers.shift;
                    if extend && state.anchor.is_none() {
                        state.anchor = Some(state.cursor);
                    } else if !extend {
                        state.anchor = None;
                    }
                    let relative_x = event.position.x - bounds.origin.x - NOTE_INPUT_PADDING;
                    let new_cursor = closest_byte_for_x(window, &state.value, relative_x);
                    state.cursor = new_cursor;
                    if !extend {
                        state.anchor = None;
                    }
                    state.clear_composition();
                    cx.notify();
                    window.invalidate_character_coordinates();
                }
                cx.stop_propagation();
            }),
        )
        .on_key_down(cx.listener(|this, e, window, cx| this.handle_search_key(e, window, cx)));

    let content: AnyElement = if value.is_empty() && ime_marked.is_empty() {
        div()
            .flex()
            .flex_row()
            .items_center()
            .child(if focused {
                text_caret(NOTE_FONT_SIZE).into_any_element()
            } else {
                div().into_any_element()
            })
            .child(div().text_color(theme::muted_text()).child("搜索..."))
            .into_any_element()
    } else {
        let (start, end) = if cursor <= anchor {
            (cursor, anchor)
        } else {
            (anchor, cursor)
        };
        let before = &value[..start.min(value.len())];
        let selected = &value[start.min(value.len())..end.min(value.len())];
        let after = &value[end.min(value.len())..];
        div()
            .flex()
            .flex_row()
            .items_center()
            .child(text_span(before.to_string()))
            .child(if selected.is_empty() && ime_marked.is_empty() {
                div().into_any_element()
            } else if ime_marked.is_empty() {
                div()
                    .bg(theme::accent_soft())
                    .child(text_span(selected.to_string()))
                    .into_any_element()
            } else {
                marked_text_span(ime_marked.clone()).into_any_element()
            })
            .child(if focused {
                text_caret(NOTE_FONT_SIZE).into_any_element()
            } else {
                div().into_any_element()
            })
            .child(text_span(after.to_string()))
            .into_any_element()
    };

    input
        .child(content)
        .child(ime_input_canvas(focus.clone(), cx.entity()))
        .into_any_element()
}

fn render_content_editor(
    state: TextEditingState,
    focus: FocusHandle,
    scroll: ScrollHandle,
    window: &Window,
    cx: &mut Context<NoteWindow>,
) -> AnyElement {
    let focused = focus.is_focused(window);
    let value = state.value.clone();
    let cursor = state.cursor;
    let ime_marked = state.ime_marked_text.clone();
    let selection = state.selection();

    let mut input = div()
        .id("note-content")
        .flex_1()
        .min_h_0()
        .h_full()
        .w_full()
        .p_2()
        .flex()
        .flex_col()
        .gap_1()
        .overflow_y_scroll()
        .track_scroll(&scroll)
        .bg(theme::canvas())
        .border_1()
        .border_color(if focused {
            theme::accent()
        } else {
            theme::border()
        })
        .rounded(px(theme::RADIUS_SM))
        .text_sm()
        .text_color(theme::text())
        .track_focus(&focus)
        .tab_stop(true)
        .cursor_text()
        .on_click({
            let focus = focus.clone();
            move |_, window: &mut Window, cx: &mut App| window.focus(&focus, cx)
        })
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, event: &MouseDownEvent, window, cx| {
                window.focus(&this.content_focus, cx);
                let Some(bounds) = this.content_bounds else {
                    // 首次点击仅聚焦，待 IME bounds 就绪后下次点击可定位
                    cx.notify();
                    window.invalidate_character_coordinates();
                    return;
                };
                let scroll = this.content_scroll.offset();
                let relative_y = event.position.y - bounds.origin.y - NOTE_INPUT_PADDING + scroll.y;
                let line_idx = (relative_y.as_f32() / NOTE_LINE_HEIGHT_F32)
                    .floor()
                    .max(0.0) as usize;
                let starts = line_starts(&this.content_state.value);
                let line_idx = line_idx.min(starts.len().saturating_sub(1));
                let line_start_byte = starts[line_idx];
                let line_end = starts
                    .get(line_idx + 1)
                    .map(|v| v - 1)
                    .unwrap_or(this.content_state.value.len());
                let line = &this.content_state.value[line_start_byte..line_end];
                let relative_x = event.position.x - bounds.origin.x - NOTE_INPUT_PADDING + scroll.x;
                let col_byte = closest_byte_for_x(window, line, relative_x);
                let best_byte = (line_start_byte + col_byte).min(this.content_state.value.len());
                let extend = event.modifiers.shift;
                this.content_dragging = true;
                if extend {
                    if this.content_state.anchor.is_none() {
                        this.content_state.anchor = Some(this.content_state.cursor);
                    }
                    this.content_state.cursor = best_byte;
                } else {
                    // 非 shift：以点击处为锚点，支持拖选；单点时 anchor==cursor 无选区
                    this.content_state.cursor = best_byte;
                    this.content_state.anchor = Some(best_byte);
                }
                this.content_state.clear_composition();
                cx.notify();
                window.invalidate_character_coordinates();
                cx.stop_propagation();
            }),
        )
        .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, window, cx| {
            if !this.content_dragging || !event.dragging() {
                return;
            }
            let Some(bounds) = this.content_bounds else {
                return;
            };
            let scroll = this.content_scroll.offset();
            let relative_y = event.position.y - bounds.origin.y - NOTE_INPUT_PADDING + scroll.y;
            let line_idx = (relative_y.as_f32() / NOTE_LINE_HEIGHT_F32)
                .floor()
                .max(0.0) as usize;
            let starts = line_starts(&this.content_state.value);
            let line_idx = line_idx.min(starts.len().saturating_sub(1));
            let line_start_byte = starts[line_idx];
            let line_end = starts
                .get(line_idx + 1)
                .map(|v| v - 1)
                .unwrap_or(this.content_state.value.len());
            let line = &this.content_state.value[line_start_byte..line_end];
            let relative_x = event.position.x - bounds.origin.x - NOTE_INPUT_PADDING + scroll.x;
            let col_byte = closest_byte_for_x(window, line, relative_x);
            let best_byte = (line_start_byte + col_byte).min(this.content_state.value.len());
            // 拖选时保持 anchor，更新 cursor
            if this.content_state.anchor.is_none() {
                this.content_state.anchor = Some(this.content_state.cursor);
            }
            this.content_state.cursor = best_byte;
            this.content_state.clear_composition();
            cx.notify();
            window.invalidate_character_coordinates();
        }))
        .on_mouse_up(
            MouseButton::Left,
            cx.listener(|this, _: &MouseUpEvent, window, cx| {
                if !this.content_dragging {
                    return;
                }
                this.content_dragging = false;
                // 单点点击（未拖动）时 anchor==cursor，清除幽灵选区
                if this.content_state.anchor == Some(this.content_state.cursor) {
                    this.content_state.anchor = None;
                }
                cx.notify();
                window.invalidate_character_coordinates();
            }),
        )
        .on_key_down(cx.listener(|this, e, window, cx| this.handle_content_key(e, window, cx)));

    if value.is_empty() && ime_marked.is_empty() {
        let placeholder = div()
            .text_color(theme::muted_text())
            .child(if focused {
                SharedString::from("")
            } else {
                SharedString::from("输入笔记内容... (支持 Markdown)")
            })
            .into_any_element();
        if focused {
            input = input.child(
                div()
                    .flex()
                    .flex_row()
                    .child(text_caret(NOTE_FONT_SIZE).into_any_element())
                    .child(placeholder),
            );
        } else {
            input = input.child(placeholder);
        }
        return input
            .child(ime_input_canvas(focus.clone(), cx.entity()))
            .into_any_element();
    }

    // 多行：按 \n 分行，支持选区高亮与 caret/IME
    let lines: Vec<&str> = value.split('\n').collect();
    let cursor_line = value[..cursor.min(value.len())]
        .chars()
        .filter(|&c| c == '\n')
        .count();
    let (sel_start, sel_end) = if let Some((s, e)) = selection {
        (s, e)
    } else {
        (cursor, cursor)
    };
    let has_selection = selection.is_some();

    let starts = line_starts(&value);
    let mut line_elements: Vec<AnyElement> = Vec::new();
    for (idx, line) in lines.iter().enumerate() {
        let line_start_byte = starts[idx];
        let line_end_byte = line_start_byte + line.len();
        let is_cursor_line = idx == cursor_line;
        let overlap_start = sel_start.max(line_start_byte);
        let overlap_end = sel_end.min(line_end_byte);
        let has_overlap = has_selection && overlap_start < overlap_end;
        let line_el: AnyElement = if has_overlap {
            let before_len = overlap_start - line_start_byte;
            let sel_len = overlap_end - overlap_start;
            let before = &line[..before_len.min(line.len())];
            let selected =
                &line[before_len.min(line.len())..(before_len + sel_len).min(line.len())];
            let after = &line[(before_len + sel_len).min(line.len())..];
            // 光标在该行的字节偏移
            let cursor_in_line = if line_start_byte <= cursor && cursor <= line_end_byte {
                Some(cursor - line_start_byte)
            } else {
                None
            };
            let mut row = div()
                .flex()
                .flex_row()
                .items_center()
                .min_h(NOTE_LINE_HEIGHT);
            if let Some(cur_off) = cursor_in_line {
                let cur_off = cur_off.min(line.len());
                let cur_off = if line.is_char_boundary(cur_off) {
                    cur_off
                } else {
                    let mut b = cur_off;
                    while b > 0 && !line.is_char_boundary(b) {
                        b -= 1;
                    }
                    b
                };
                if cur_off <= before_len {
                    let (b1, b2) = before.split_at(cur_off.min(before.len()));
                    row = row.child(text_span(b1.to_string()));
                    if !ime_marked.is_empty() && is_cursor_line {
                        row = row.child(marked_text_span(ime_marked.clone()));
                    }
                    if focused {
                        row = row.child(text_caret(NOTE_FONT_SIZE).into_any_element());
                    }
                    row = row.child(text_span(b2.to_string()));
                    row = row.child(
                        div()
                            .bg(theme::accent_soft())
                            .child(text_span(selected.to_string()))
                            .into_any_element(),
                    );
                    row = row.child(text_span(after.to_string()));
                } else if cur_off <= before_len + sel_len {
                    let sel_cur = cur_off - before_len;
                    let sel_cur = sel_cur.min(selected.len());
                    let sel_cur = if selected.is_char_boundary(sel_cur) {
                        sel_cur
                    } else {
                        let mut b = sel_cur;
                        while b > 0 && !selected.is_char_boundary(b) {
                            b -= 1;
                        }
                        b
                    };
                    let (s1, s2) = selected.split_at(sel_cur);
                    row = row.child(text_span(before.to_string()));
                    if !s1.is_empty() {
                        row = row.child(
                            div()
                                .bg(theme::accent_soft())
                                .child(text_span(s1.to_string()))
                                .into_any_element(),
                        );
                    }
                    if !ime_marked.is_empty() && is_cursor_line {
                        row = row.child(marked_text_span(ime_marked.clone()));
                    }
                    if focused {
                        row = row.child(text_caret(NOTE_FONT_SIZE).into_any_element());
                    }
                    if !s2.is_empty() {
                        row = row.child(
                            div()
                                .bg(theme::accent_soft())
                                .child(text_span(s2.to_string()))
                                .into_any_element(),
                        );
                    }
                    row = row.child(text_span(after.to_string()));
                } else {
                    let after_cur = cur_off - before_len - sel_len;
                    let after_cur = after_cur.min(after.len());
                    let after_cur = if after.is_char_boundary(after_cur) {
                        after_cur
                    } else {
                        let mut b = after_cur;
                        while b > 0 && !after.is_char_boundary(b) {
                            b -= 1;
                        }
                        b
                    };
                    let (a1, a2) = after.split_at(after_cur);
                    row = row.child(text_span(before.to_string()));
                    row = row.child(
                        div()
                            .bg(theme::accent_soft())
                            .child(text_span(selected.to_string()))
                            .into_any_element(),
                    );
                    row = row.child(text_span(a1.to_string()));
                    if !ime_marked.is_empty() && is_cursor_line {
                        row = row.child(marked_text_span(ime_marked.clone()));
                    }
                    if focused {
                        row = row.child(text_caret(NOTE_FONT_SIZE).into_any_element());
                    }
                    row = row.child(text_span(a2.to_string()));
                }
            } else {
                row = row.child(text_span(before.to_string()));
                row = row.child(
                    div()
                        .bg(theme::accent_soft())
                        .child(text_span(selected.to_string()))
                        .into_any_element(),
                );
                row = row.child(text_span(after.to_string()));
            }
            row.into_any_element()
        } else if is_cursor_line {
            // 无选区重叠但为光标行：原逻辑
            let col_byte = cursor.saturating_sub(line_start_byte).min(line.len());
            let col_byte = if line.is_char_boundary(col_byte) {
                col_byte
            } else {
                let mut b = col_byte;
                while b > 0 && !line.is_char_boundary(b) {
                    b -= 1;
                }
                b
            };
            let before = &line[..col_byte];
            let after = &line[col_byte..];
            let mut row = div()
                .flex()
                .flex_row()
                .items_center()
                .min_h(NOTE_LINE_HEIGHT);
            row = row.child(text_span(before.to_string()));
            if !ime_marked.is_empty() {
                row = row.child(marked_text_span(ime_marked.clone()));
            }
            if focused {
                row = row.child(text_caret(NOTE_FONT_SIZE).into_any_element());
            }
            row = row.child(text_span(after.to_string()));
            row.into_any_element()
        } else if has_selection {
            // 有选区但该行无重叠且非光标行：整行未选中，按普通文本渲染
            // 实际上整行被选中时 has_overlap 为 true，已在上分支处理；此处为未选中行
            div()
                .min_h(NOTE_LINE_HEIGHT)
                .child(text_span(line.to_string()))
                .into_any_element()
        } else {
            div()
                .min_h(NOTE_LINE_HEIGHT)
                .child(text_span(line.to_string()))
                .into_any_element()
        };
        line_elements.push(line_el);
    }

    input
        .children(line_elements)
        .child(ime_input_canvas(focus.clone(), cx.entity()))
        .into_any_element()
}

pub fn open_note_window(cx: &mut App) {
    if let Some(window) = cx.windows().iter().find_map(|h| h.downcast::<NoteWindow>()) {
        let _ = window.update(cx, |note, window, cx| {
            window.activate_window();
            // 重新聚焦内容区，确保从主窗口唤起时即可编辑/复制
            window.focus(&note.content_focus, cx);
        });
        return;
    }
    if cx.windows().is_empty() {
        create_note_window(cx);
    } else {
        cx.defer(create_note_window);
    }
}

fn create_note_window(cx: &mut App) {
    let bounds = Bounds::centered(
        None,
        Size {
            width: px(900.),
            height: px(600.),
        },
        cx,
    );
    let handle = cx
        .open_window(
            WindowOptions {
                titlebar: Some(TitlebarOptions {
                    title: Some("Note".into()),
                    ..Default::default()
                }),
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                window_min_size: Some(Size {
                    width: px(640.),
                    height: px(400.),
                }),
                ..Default::default()
            },
            |window, cx| cx.new(|cx| NoteWindow::new(window, cx)),
        )
        .expect("Note window should open");
    // 确保新建窗口后内容区获得焦点（defer 兜底 + 直接聚焦）
    let _ = handle.update(cx, |note, window, cx| {
        window.focus(&note.content_focus, cx);
    });
    cx.activate(true);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shared::text_editing::{EditingKeystroke, handle_text_editing_key};
    use crossh_note::Note;
    use gpui::TestAppContext;

    #[gpui::test]
    fn note_window_historic_note_loads_in_edit_state(cx: &mut TestAppContext) {
        // 不依赖真实 DB，直接注入历史笔记模拟 reload 后的状态
        let window = cx.add_window(NoteWindow::new);
        cx.run_until_parked();

        window
            .update(cx, |note_window, window, cx| {
                let historic = Note {
                    id: 1,
                    content: "hello 历史笔记".to_string(),
                    pinned: false,
                    created_at: 0,
                    updated_at: 0,
                };
                note_window.notes = vec![historic.clone()];
                // 模拟 reload 自动选中的路径
                note_window.select_note_focused(historic.id, window, cx);
                assert_eq!(note_window.notes.len(), 1);
                assert_eq!(note_window.selected_id, Some(historic.id));
                assert_eq!(note_window.content_state.value, "hello 历史笔记");
                assert!(!note_window.preview);
                // 初始应聚焦内容区，允许 cmd+a / 复制
                assert!(
                    note_window.content_focus.is_focused(window),
                    "content should be focused after historic load"
                );
                // 模拟全选与复制的纯逻辑路径
                let mut state = note_window.content_state.clone();
                let select_all = EditingKeystroke {
                    key: "a".to_string(),
                    key_char: None,
                    control: false,
                    platform: true,
                    shift: false,
                };
                let r = handle_text_editing_key(&mut state, &select_all, None);
                assert!(r.handled);
                assert_eq!(state.selection(), Some((0, state.value.len())));
                let copy = EditingKeystroke {
                    key: "c".to_string(),
                    key_char: None,
                    control: false,
                    platform: true,
                    shift: false,
                };
                let r2 = handle_text_editing_key(&mut state, &copy, None);
                assert!(r2.handled);
                assert_eq!(r2.copy_text.as_deref(), Some("hello 历史笔记"));
            })
            .unwrap();
    }

    #[gpui::test]
    fn note_window_select_via_list_focuses_content(cx: &mut TestAppContext) {
        let window = cx.add_window(NoteWindow::new);
        cx.run_until_parked();

        window
            .update(cx, |note_window, window, cx| {
                let n1 = Note {
                    id: 1,
                    content: "first".to_string(),
                    pinned: false,
                    created_at: 0,
                    updated_at: 0,
                };
                let n2 = Note {
                    id: 2,
                    content: "second".to_string(),
                    pinned: false,
                    created_at: 0,
                    updated_at: 1,
                };
                note_window.notes = vec![n2.clone(), n1.clone()];
                note_window.selected_id = Some(n2.id);
                note_window.content_state = TextEditingState::new(n2.content.clone());
                // 模拟用户点击列表第二条，期望进入编辑态
                note_window.select_note_focused(n1.id, window, cx);
                assert_eq!(note_window.selected_id, Some(n1.id));
                assert_eq!(note_window.content_state.value, "first");
                assert!(note_window.content_focus.is_focused(window));
                assert!(!note_window.preview);
                // 拖选逻辑：首次点击 anchor==cursor，随后 mouse_move 应扩展选区
                note_window.content_state.cursor = 0;
                note_window.content_state.anchor = Some(0);
                note_window.content_dragging = true;
                note_window.content_state.cursor = note_window.content_state.value.len();
                assert_eq!(
                    note_window.content_state.selection(),
                    Some((0, "first".len()))
                );
                note_window.content_dragging = false;
                if note_window.content_state.anchor == Some(note_window.content_state.cursor) {
                    note_window.content_state.anchor = None;
                }
                let mut st = note_window.content_state.clone();
                st.select_all();
                let copy = EditingKeystroke {
                    key: "c".to_string(),
                    key_char: None,
                    control: false,
                    platform: true,
                    shift: false,
                };
                let r = handle_text_editing_key(&mut st, &copy, None);
                assert_eq!(r.copy_text.as_deref(), Some("first"));
                assert_ne!(n1.id, n2.id);
            })
            .unwrap();
    }

    #[gpui::test]
    fn note_status_dirty_and_save_disabled(cx: &mut TestAppContext) {
        let window = cx.add_window(NoteWindow::new);
        cx.run_until_parked();
        window
            .update(cx, |note_window, _window, _cx| {
                // 空态：无选中、无内容 => 不脏、保存禁用
                note_window.notes = Vec::new();
                note_window.selected_id = None;
                note_window.content_state = TextEditingState::new(String::new());
                assert!(!note_window.is_dirty());
                assert!(note_window.is_save_disabled());
                assert_eq!(note_window.status_title(), "无笔记");

                // 新建草稿：有内容 => 脏、可保存
                note_window.content_state = TextEditingState::new("hello".to_string());
                assert!(note_window.is_dirty());
                assert!(!note_window.is_save_disabled());
                assert_eq!(note_window.status_title(), "新建笔记");

                // 选中态未修改 => 不脏、保存禁用
                let note = Note {
                    id: 1,
                    content: "hello".to_string(),
                    pinned: false,
                    created_at: 0,
                    updated_at: 0,
                };
                note_window.notes = vec![note.clone()];
                note_window.selected_id = Some(note.id);
                note_window.content_state = TextEditingState::new(note.content.clone());
                assert!(!note_window.is_dirty());
                assert!(note_window.is_save_disabled());
                assert_eq!(note_window.status_title(), "hello");

                // 修改内容 => 脏
                note_window.content_state = TextEditingState::new("hello world".to_string());
                assert!(note_window.is_dirty());
                assert!(!note_window.is_save_disabled());
            })
            .unwrap();
    }

    #[gpui::test]
    fn note_status_title_fallbacks(cx: &mut TestAppContext) {
        let window = cx.add_window(NoteWindow::new);
        cx.run_until_parked();
        window
            .update(cx, |note_window, _window, _cx| {
                note_window.notes = vec![Note {
                    id: 1,
                    content: "".to_string(),
                    pinned: false,
                    created_at: 0,
                    updated_at: 0,
                }];
                note_window.selected_id = None;
                note_window.content_state = TextEditingState::new(String::new());
                assert_eq!(note_window.status_title(), "未选择");
            })
            .unwrap();
    }
}
