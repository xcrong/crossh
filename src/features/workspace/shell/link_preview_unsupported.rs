//! 链接预览的占位实现：本平台不接原生 webview。
//!
//! Linux 并不是能力上做不到：X11 下 wry 的 `build_as_child` 只要一个 Xlib 窗口句柄，
//! 它会用 `gdk_x11_window_foreign_new_for_display` 把外部 X11 窗口包成 GTK toplevel
//! （`wry-0.57.1/src/webkitgtk/mod.rs:125-153`）。代价是必须先 `gtk::init()`、在 GPUI
//! 事件循环里推进 `gtk::main_iteration_do`，并引入 `gtk` / `webkit2gtk` / `soup3`
//! 与系统 WebKitGTK 运行时；Wayland 则只能走 `build_gtk(gtk::Fixed)`，GPUI 没有
//! GTK widget 可用。本仓库不为预览背这些依赖，所以这里只提供与 `link_preview.rs`
//! 同名的空实现，让调用点不必各自 `cfg`：打开预览一律返回 `false`，调用方
//! `open_link` 随即回退系统浏览器（`crates/crossh-terminal-view/src/link_opener.rs`）。
//!
//! 决策背景见 `openspec/changes/link-preview-multi-platform/design.md` 的 D1 / D3。

use gpui::{AnyElement, Context, Window, div};

use super::AppShell;

/// 本平台不创建原生预览；类型只为让 `AppShell` 的字段保持无条件存在。
pub(crate) struct LinkPreview;

impl AppShell {
    pub(crate) fn open_link_preview(
        &mut self,
        _url: &str,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> bool {
        // 本平台没有原生宿主：字段恒为 `None`，读一次以免它被判成死代码。
        let _ = self.link_preview.as_ref();
        false
    }

    pub(crate) fn hide_link_preview(&mut self, _cx: &mut Context<Self>) {}
}

pub(crate) fn render_status_toggle(
    _shell: &AppShell,
    _cx: &mut Context<AppShell>,
) -> Option<AnyElement> {
    None
}

pub(crate) fn render_link_preview(
    _shell: &mut AppShell,
    _window: &Window,
    _cx: &mut Context<AppShell>,
) -> AnyElement {
    div().into_any_element()
}
