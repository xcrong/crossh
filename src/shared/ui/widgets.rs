//! 跨 UI 模块复用的小组件与工具函数。

use gpui::{Keystroke, ParentElement, Render, SharedString, Styled, Window, div, px};

use crate::shared::ui::theme;

/// 简单的纯文本 tooltip：按内容收缩，长文本最多 480px 并自动换行。
pub struct LocalPathTooltip {
    pub path: SharedString,
}

impl Render for LocalPathTooltip {
    fn render(
        &mut self,
        _window: &mut Window,
        _cx: &mut gpui::Context<Self>,
    ) -> impl gpui::IntoElement {
        div()
            .w_auto()
            .max_w(px(480.))
            .px_2()
            .py_1()
            .bg(theme::raised())
            .border_1()
            .border_color(theme::border_strong())
            .text_xs()
            .text_color(theme::text())
            .whitespace_normal()
            .child(self.path.clone())
    }
}

/// Full command preview for quick-command rows.
pub struct CommandTooltip {
    pub command: SharedString,
}

impl Render for CommandTooltip {
    fn render(
        &mut self,
        _window: &mut Window,
        _cx: &mut gpui::Context<Self>,
    ) -> impl gpui::IntoElement {
        div()
            .w_auto()
            .max_w(px(520.))
            .px_3()
            .py_2()
            .bg(theme::canvas())
            .border_1()
            .border_color(theme::border_strong())
            .text_xs()
            .text_color(theme::text())
            .whitespace_normal()
            .child(self.command.clone())
    }
}

/// 键盘事件的可打印字符；带控制/平台修饰键时返回 None。
pub fn printable_char(ks: &Keystroke) -> Option<char> {
    if ks.modifiers.control || ks.modifiers.platform {
        return None;
    }
    ks.key_char.as_ref().and_then(|s| s.chars().next())
}
