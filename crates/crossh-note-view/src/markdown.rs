//! Markdown 预览：直接复用 crossh-editor 的 `TextView`。
//!
//! 旧实现自己跑了一遍 pulldown-cmark 事件流，把 `**`/`*`/`#1 ` 当字面文本
//! push 进缓冲区再 `join("")` 拼成一段字符串渲染，于是预览里直接显示 Markdown
//! 源码符号，粗体/斜体/删除线/标题层级全部失效。`TextView` 已经把这些都做好了
//! （标题缩放、粗体、表格、代码块、链接、可选中、可滚动），故预览直接用它。

use crossh_editor::TextView;
use gpui::{AnyElement, IntoElement, ParentElement, Styled, div};

/// 渲染 Markdown 预览。
///
/// 无状态用法：每次调用按当前内容新建 `TextView`。`TextView` 内部按文本内容
/// 缓存解析结果（见 `TextView::markdown` 的 Element 生命周期），切笔记不会串内容。
/// 预览容器负责滚动（`.overflow_y_scroll()`），故这里不设 `scrollable(true)`，
/// 避免 TextView 的内部滚动与外层容器滚动叠加。
pub fn render_markdown(md: &str) -> AnyElement {
    if md.trim().is_empty() {
        return div()
            .text_sm()
            .text_color(crossh_ui::theme::muted_text())
            .child("预览为空")
            .into_any_element();
    }
    TextView::markdown("note-preview-text", md.to_string()).into_any_element()
}
