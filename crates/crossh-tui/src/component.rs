//! component — pi-tui 的组件模型（Container/Text/Box/Spacer）
//!
//! 行是带 ANSI 的 String，与 pi 完全一致

use crate::ansi::{normalize_terminal_output, visible_width, wrap_text_with_ansi};

pub trait Component {
    /// 渲染为若干行（可含 ANSI），宽度不超过 width 可见列
    fn render(&mut self, width: usize) -> Vec<String>;
    fn invalidate(&mut self) {}
}

/// Container — 顺序拼接子组件的行（pi 的 Container）
#[derive(Default)]
pub struct Container {
    pub children: Vec<Box<dyn Component>>,
}

impl Container {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn add_child(&mut self, child: Box<dyn Component>) {
        self.children.push(child);
    }
    pub fn clear(&mut self) {
        self.children.clear();
    }
    pub fn is_empty(&self) -> bool {
        self.children.is_empty()
    }
}

impl Component for Container {
    fn render(&mut self, width: usize) -> Vec<String> {
        let mut lines = Vec::new();
        for child in &mut self.children {
            lines.extend(child.render(width));
        }
        lines
    }
    fn invalidate(&mut self) {
        for child in &mut self.children {
            child.invalidate();
        }
    }
}

/// Text — 多行文本 + padding + 自动换行（pi 的 Text）
pub struct Text {
    pub text: String,
    pub padding_x: usize,
    pub padding_y: usize,
}

impl Text {
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            padding_x: 1,
            padding_y: 1,
        }
    }
    pub fn with_padding(text: impl Into<String>, padding_x: usize, padding_y: usize) -> Self {
        Self {
            text: text.into(),
            padding_x,
            padding_y,
        }
    }
    pub fn set_text(&mut self, text: impl Into<String>) {
        self.text = text.into();
    }
}

impl Component for Text {
    fn render(&mut self, width: usize) -> Vec<String> {
        if self.text.trim().is_empty() {
            return Vec::new();
        }
        let normalized = normalize_terminal_output(&self.text);
        let content_width = width.saturating_sub(self.padding_x * 2).max(1);
        let wrapped = if self.text.contains('\x1b') {
            // 已含样式：直接按可见宽度硬换
            crate::ansi::wrap_text_with_ansi(&normalized, content_width)
        } else {
            wrap_text_with_ansi(&normalized, content_width)
        };
        let left = " ".repeat(self.padding_x);
        let right = " ".repeat(self.padding_x);
        let mut result = Vec::new();
        for _ in 0..self.padding_y {
            result.push(" ".repeat(width));
        }
        for line in wrapped {
            let with_margins = format!("{}{}{}", left, line, right);
            let visible = visible_width(&with_margins);
            let pad = " ".repeat(width.saturating_sub(visible));
            result.push(format!("{}{}", with_margins, pad));
        }
        for _ in 0..self.padding_y {
            result.push(" ".repeat(width));
        }
        result
    }
}

/// Box — 给所有子行加 padding 与背景的容器（pi 的 Box）
pub struct TuiBox {
    pub children: Vec<Box<dyn Component>>,
    pub padding_x: usize,
    pub padding_y: usize,
    pub bg_fn: Option<fn(&str) -> String>,
}

impl TuiBox {
    pub fn new(padding_x: usize, padding_y: usize) -> Self {
        Self {
            children: Vec::new(),
            padding_x,
            padding_y,
            bg_fn: None,
        }
    }
    pub fn with_bg(padding_x: usize, padding_y: usize, bg_fn: fn(&str) -> String) -> Self {
        Self {
            children: Vec::new(),
            padding_x,
            padding_y,
            bg_fn: Some(bg_fn),
        }
    }
    pub fn add_child(&mut self, child: Box<dyn Component>) {
        self.children.push(child);
    }
    fn apply_bg(&self, line: &str, width: usize) -> String {
        let visible = visible_width(line);
        let padded = format!("{}{}", line, " ".repeat(width.saturating_sub(visible)));
        match self.bg_fn {
            Some(bg) => bg(&padded),
            None => padded,
        }
    }
}

impl Component for TuiBox {
    fn render(&mut self, width: usize) -> Vec<String> {
        if self.children.is_empty() {
            return Vec::new();
        }
        let content_width = width.saturating_sub(self.padding_x * 2).max(1);
        let left_pad = " ".repeat(self.padding_x);
        // 先收集子行，避免迭代借用与 apply_bg 的不可变借用冲突
        let mut child_lines = Vec::new();
        for child in &mut self.children {
            child_lines.extend(child.render(content_width));
        }
        let mut result = Vec::new();
        for _ in 0..self.padding_y {
            result.push(self.apply_bg("", width));
        }
        for line in child_lines {
            let with_pad = format!("{}{}", left_pad, line);
            result.push(self.apply_bg(&with_pad, width));
        }
        for _ in 0..self.padding_y {
            result.push(self.apply_bg("", width));
        }
        result
    }
    fn invalidate(&mut self) {
        for child in &mut self.children {
            child.invalidate();
        }
    }
}

/// Spacer — N 个空行（pi 的 Spacer）
pub struct Spacer(pub usize);

impl Component for Spacer {
    fn render(&mut self, _width: usize) -> Vec<String> {
        vec![String::new(); self.0]
    }
}
