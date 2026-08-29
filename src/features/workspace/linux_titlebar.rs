//! 渲染 Linux 客户端装饰（CSD）时的自定义标题栏。
//!
//! 平台隔离约定：本文件的全部实现仅在 `target_os = "linux"` 下编译；
//! 其他平台使用文件尾的同签名空桩（恒返回 `None`），使 Linux 特定代码
//! 与其依赖的 GPUI API 不进入其它平台的编译单元——调用点无需任何门控。

use crate::features::workspace::shell::AppShell;

use gpui::{Context, Window};

/// 主窗口专用入口：标题固定为 `crossh`，内部委托给 `crossh_ui::linux_titlebar` 的通用实现。
#[cfg(target_os = "linux")]
pub fn render_linux_titlebar(
    window: &mut Window,
    cx: &mut Context<AppShell>,
) -> Option<gpui::AnyElement> {
    crossh_ui::linux_titlebar::render_linux_titlebar(window, cx, "crossh".into())
}

/// 非 Linux 平台桩：CSD 标题栏不存在，恒返回 `None`。
#[cfg(not(target_os = "linux"))]
pub fn render_linux_titlebar(
    _window: &mut Window,
    _cx: &mut Context<AppShell>,
) -> Option<gpui::AnyElement> {
    None
}
