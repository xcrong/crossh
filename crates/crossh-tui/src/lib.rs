//! `crossh-tui` — Rust 1:1 移植 `pi-tui` 的完整渲染管线
//!
//! 对应 `pi 0.84.2`：
//! - `ansi.rs`        ← utils.js（ANSI 提取/宽度/换行/切片）
//! - `component.rs`   ← tui.js 的 Container + text/box/spacer 组件
//! - `markdown.rs`    ← components/markdown.js
//! - `editor.rs`      ← components/editor.js（边框 + 假光标 + 滚动）
//! - `layout.rs`      ← layout.js（vstack/hstack/scroll 节点 + scrollbar 几何 + hit-test）
//! - `screen.rs`      ← TuiBase.doRender（diff 渲染 + 选区高亮 + flash + 光标标记）
//! - `scroll_view.rs` ← components/scroll-view.js
//! - `terminal.rs`    ← 终端序列常量
//!
//! 行为合约见 `docs/specs/20260822-agent-tui-pi-parity.md`

pub mod alt_screen;
pub mod ansi;
pub mod component;
pub mod editor;
pub mod layout;
pub mod main_screen;
pub mod markdown;
pub mod screen;
pub mod scroll_view;
pub mod selection;
pub mod terminal;

pub use alt_screen::{AltScreen, AltScreenOptions};
pub use ansi::CURSOR_MARKER;
pub use component::TuiBox;
pub use component::{Component, Container, Spacer, Text};
pub use editor::Editor;
pub use markdown::Markdown;
pub use screen::ScreenRenderer;
