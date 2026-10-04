//! 笔记内查找：面板渲染与快捷键处理。
//!
//! 匹配引擎（`TextareaState::search_session`，含 AhoCorasick 匹配与命中高亮）
//! 由 `crossh-editor` 提供；本文件只负责浮层 UI、查询输入和上下一个匹配的分发。

use crossh_core::text_editing::{EditingKeystroke, TextEditingState, handle_text_editing_key};
use crossh_ui::{icons, theme};
use crossh_ui_component::{Button, ButtonSize, ButtonVariant, filter_text_input};
use gpui::{
    AnyElement, ClipboardItem, Context, Focusable, InteractiveElement, IntoElement, KeyDownEvent,
    ParentElement, Styled, Window, div, px,
};

use super::FIND_CONTEXT;
use super::window::{EDITOR_PADDING_X, EDITOR_PADDING_Y, NoteWindow};

impl NoteWindow {
    /// 替换编辑器内容前必须重置查找。
    ///
    /// 匹配是针对旧文本算的，文本一变，旧匹配区间就会落在错误的位置上。
    /// 只清 UI 状态不够——编辑器里的 `search_session.open` 也得关掉，
    /// 否则残留的高亮会被画到新笔记的任意位置上。
    pub(super) fn reset_find(&mut self, cx: &mut Context<Self>) {
        self.find_open = false;
        self.find_query = TextEditingState::new(String::new());
        self.content_state
            .update(cx, |state, cx| state.close_search(cx));
    }

    /// 开/关查找面板。开启时把编辑器里选中的文本预填进查询框（编辑器惯例）。
    pub(super) fn toggle_find(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.find_open = !self.find_open;
        if self.find_open {
            let selected = self.content_state.read(cx).selected_value().to_string();
            if !selected.is_empty() {
                self.find_query = TextEditingState::new(selected);
            }
            // 通知编辑器打开匹配会话（高亮由它绘制）。
            self.content_state
                .update(cx, |state, cx| state.open_search(false, cx));
            self.apply_find_query(cx);
            window.focus(&self.find_focus, cx);
        } else {
            self.reset_find(cx);
            window.focus(&self.content_state.read(cx).focus_handle(cx), cx);
        }
        cx.notify();
    }

    pub(super) fn close_find(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.find_open {
            return;
        }
        self.reset_find(cx);
        window.focus(&self.content_state.read(cx).focus_handle(cx), cx);
        cx.notify();
    }

    /// 把查询文本推给编辑器匹配引擎。每次输入都重算匹配并高亮。
    pub(super) fn apply_find_query(&mut self, cx: &mut Context<Self>) {
        let query = self.find_query.value.clone();
        self.content_state.update(cx, |state, cx| {
            state.set_search_query(query, /* case_insensitive */ true, cx)
        });
    }

    /// 跳到下一个匹配，并把命中处选中。无匹配时不动编辑器。
    pub(super) fn find_next(&mut self, cx: &mut Context<Self>) {
        let range = self
            .content_state
            .update(cx, |state, cx| state.next_search_match(cx));
        if let Some(range) = range {
            self.content_state.update(cx, |state, cx| {
                state.set_selected_range(range, cx);
            });
        }
        cx.notify();
    }

    pub(super) fn find_prev(&mut self, cx: &mut Context<Self>) {
        let range = self
            .content_state
            .update(cx, |state, cx| state.previous_search_match(cx));
        if let Some(range) = range {
            self.content_state.update(cx, |state, cx| {
                state.set_selected_range(range, cx);
            });
        }
        cx.notify();
    }

    /// 查找框的键盘处理：Enter/Shift+Enter 翻匹配由 FIND_CONTEXT 的键位处理；
    /// 这里只接管文本编辑（输入、删除、Esc 冒泡截断），语义与列表搜索框一致。
    pub(super) fn handle_find_key(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let ks = &event.keystroke;
        if ks.key == "escape" {
            // Escape 关面板而不是关窗：查找框是 NOTE_WINDOW_CONTEXT/Input 的
            // 兄弟上下文，Esc 冒泡到窗口会被当作关窗请求。
            self.close_find(window, cx);
            cx.stop_propagation();
            return;
        }
        let primary = ks.modifiers.control || ks.modifiers.platform;
        let paste_text = if primary && ks.key == "v" {
            cx.read_from_clipboard()
                .and_then(|item| item.text().map(|s| s.to_string()))
        } else {
            None
        };
        let editing_ks = EditingKeystroke {
            key: ks.key.clone(),
            key_char: ks.key_char.clone(),
            control: ks.modifiers.control,
            platform: ks.modifiers.platform,
            shift: ks.modifiers.shift,
        };
        let result =
            handle_text_editing_key(&mut self.find_query, &editing_ks, paste_text.as_deref());
        if let Some(text) = result.copy_text {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
        }
        if result.handled {
            self.apply_find_query(cx);
            cx.notify();
            cx.stop_propagation();
        }
    }

    /// 查找面板浮层：查询框 + 匹配计数 + 上一个/下一个/关闭。
    pub(super) fn render_find_panel(
        &self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let session = self.content_state.read(cx).search_session();
        let total = session.matcher.matched_ranges().len();
        let current = session.matcher.current_match_index() + 1;
        let counter = if total == 0 {
            if self.find_query.value.is_empty() {
                String::new()
            } else {
                "无匹配".to_string()
            }
        } else {
            format!("{current}/{total}")
        };

        div()
            .id("note-find-panel")
            .absolute()
            .top(px(EDITOR_PADDING_Y))
            .right(px(EDITOR_PADDING_X))
            .w(px(320.))
            .flex()
            .flex_col()
            .gap_2()
            .p_2()
            .rounded(px(theme::RADIUS_SM))
            .bg(theme::raised())
            .border_1()
            .border_color(theme::border_strong())
            .shadow_md()
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .child(
                        filter_text_input(
                            "note-find-input",
                            self.find_focus.clone(),
                            self.find_query.value.clone(),
                            "查找...",
                            self.find_query.ime_marked_text.clone(),
                            self.find_query.selection(),
                            self.find_query.cursor,
                        )
                        .entity(cx.entity())
                        .key_context(FIND_CONTEXT)
                        .on_key_down(cx.listener(Self::handle_find_key)),
                    )
                    .child(
                        div()
                            .flex_shrink_0()
                            .text_xs()
                            .text_color(if total == 0 && !self.find_query.value.is_empty() {
                                theme::danger()
                            } else {
                                theme::muted_text()
                            })
                            .child(counter),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_end()
                    .gap_1()
                    .child(
                        Button::new("note-find-prev")
                            .size(ButtonSize::Icon(px(22.)))
                            .variant(ButtonVariant::Ghost)
                            .icon(
                                icons::icon(icons::IconName::ArrowLeft, 12.)
                                    .text_color(theme::muted_text()),
                            )
                            .tooltip("上一个匹配")
                            .disabled(total == 0)
                            .on_click(cx.listener(|this, _, _, cx| this.find_prev(cx))),
                    )
                    .child(
                        Button::new("note-find-next")
                            .size(ButtonSize::Icon(px(22.)))
                            .variant(ButtonVariant::Ghost)
                            .icon(
                                icons::icon(icons::IconName::ArrowUp, 12.)
                                    .text_color(theme::muted_text()),
                            )
                            .tooltip("下一个匹配")
                            .disabled(total == 0)
                            .on_click(cx.listener(|this, _, _, cx| this.find_next(cx))),
                    )
                    .child(
                        Button::new("note-find-close")
                            .size(ButtonSize::Icon(px(22.)))
                            .variant(ButtonVariant::Ghost)
                            .icon(
                                icons::icon(icons::IconName::X, 12.)
                                    .text_color(theme::muted_text()),
                            )
                            .tooltip("关闭查找")
                            .on_click(
                                cx.listener(|this, _, window, cx| this.close_find(window, cx)),
                            ),
                    ),
            )
            .into_any_element()
    }
}
