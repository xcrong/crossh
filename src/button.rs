//! 自定义按钮组件 —— 直接基于官方 gpui 实现，不依赖任何第三方组件库。
//!
//! 官方 gpui 没有内置 Button，它只提供 `div()` 元素 + `Styled` / `InteractiveElement`
//! 等 trait。这里把“一个带悬停/按下状态、可点击的盒子”封装成可复用的 Builder，
//! 用法和主流组件库类似：
//!
//! ```ignore
//! Button::new("ok")
//!     .primary()
//!     .label("Let's Go!")
//!     .on_click(|_, _, _| println!("Clicked!"))
//! ```
#![allow(dead_code)]

use gpui::{
    App, ClickEvent, ElementId, InteractiveElement, IntoElement, ParentElement, Rgba, Role,
    SharedString, Stateful, StatefulInteractiveElement, Styled, Window, div, px, rgb,
};

/// 按钮的视觉风格。
#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub enum ButtonStyle {
    /// 主要操作（蓝色高亮），一个视图里通常只有一个。
    Primary,
    /// 普通操作（中性灰）。
    #[default]
    Default,
}

/// 一个可复用的按钮组件。
pub struct Button {
    id: ElementId,
    label: SharedString,
    style: ButtonStyle,
    on_click: Option<Box<dyn Fn(&ClickEvent, &mut Window, &mut App) + 'static>>,
}

impl Button {
    /// 创建一个新按钮。
    ///
    /// `id` 在同一父元素的直接子元素中必须唯一，gpui 依靠它来追踪
    /// 悬停 / 按下 / 焦点等交互状态。
    pub fn new(id: impl Into<ElementId>) -> Self {
        Self {
            id: id.into(),
            label: "".into(),
            style: ButtonStyle::default(),
            on_click: None,
        }
    }

    /// 设置按钮文字。
    pub fn label(mut self, label: impl Into<SharedString>) -> Self {
        self.label = label.into();
        self
    }

    /// 设置按钮视觉风格。
    pub fn style(mut self, style: ButtonStyle) -> Self {
        self.style = style;
        self
    }

    /// 主要按钮，等价于 `.style(ButtonStyle::Primary)`。
    pub fn primary(self) -> Self {
        self.style(ButtonStyle::Primary)
    }

    /// 绑定点击回调。
    pub fn on_click(
        mut self,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_click = Some(Box::new(handler));
        self
    }

    /// 根据风格得到（背景色 / 悬停色 / 文字色）。
    fn palette(&self) -> (Rgba, Rgba, Rgba) {
        match self.style {
            ButtonStyle::Primary => (rgb(0x3b82f6), rgb(0x2563eb), rgb(0xffffff)),
            ButtonStyle::Default => (rgb(0x2a2a2e), rgb(0x3a3a3e), rgb(0xf5f5f7)),
        }
    }
}

impl IntoElement for Button {
    type Element = Stateful<gpui::Div>;

    fn into_element(self) -> Self::Element {
        let (bg, bg_hover, text_color) = self.palette();

        // `.id(...)` 把普通的 Div 变成 Stateful<Div>，只有 Stateful 元素才能
        // 追踪 hover / active 状态并响应 on_click。
        let mut button = div()
            .id(self.id)
            // 无障碍语义：声明这是一个按钮。
            .role(Role::Button)
            .aria_label(self.label.clone())
            // 布局：自身水平居中文字。
            .flex()
            .items_center()
            .justify_center()
            // 尺寸 / 圆角 / 间距。
            .px_4()
            .py_2()
            .rounded_md()
            // 文字样式。
            .text_sm()
            .text_color(text_color)
            // 背景与交互反馈。
            .bg(bg)
            .cursor_pointer()
            .hover(move |s| s.bg(bg_hover))
            .active(move |s| s.opacity(0.85));

        button = button.child(self.label);

        if let Some(handler) = self.on_click {
            button = button.on_click(handler);
        }

        // 让按钮内容有一点呼吸感（label 与点击区域）。
        button.min_h(px(32.))
    }
}
