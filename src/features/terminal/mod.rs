//! Terminal session and terminal emulation feature.

use gpui::{App, KeyBinding};
use terminal as zed_terminal;

pub(crate) mod events;
pub(crate) mod settings;
pub(crate) mod view;

pub(crate) use events::{ConnState, TerminalEvent};
pub(crate) use view::TerminalView;

/// Install the terminal-only portion of Zed's default keymap.
///
/// The full Zed keymap belongs to the editor application and is intentionally
/// not a Crossh dependency. These bindings cover actions implemented by the
/// local terminal-view host and are scoped to the `Terminal` key context.
pub(crate) fn init(cx: &mut App) {
    #[cfg(target_os = "macos")]
    cx.bind_keys([
        KeyBinding::new("cmd-c", zed_terminal::Copy, Some("Terminal")),
        KeyBinding::new("cmd-v", zed_terminal::Paste, Some("Terminal")),
        KeyBinding::new("ctrl-cmd-v", zed_terminal::PasteText, Some("Terminal")),
        KeyBinding::new("cmd-a", zed_terminal::SelectAll, Some("Terminal")),
        KeyBinding::new("cmd-k", zed_terminal::Clear, Some("Terminal")),
        KeyBinding::new("shift-pageup", zed_terminal::ScrollPageUp, Some("Terminal")),
        KeyBinding::new("cmd-up", zed_terminal::ScrollPageUp, Some("Terminal")),
        KeyBinding::new(
            "shift-pagedown",
            zed_terminal::ScrollPageDown,
            Some("Terminal"),
        ),
        KeyBinding::new("cmd-down", zed_terminal::ScrollPageDown, Some("Terminal")),
        KeyBinding::new("shift-up", zed_terminal::ScrollLineUp, Some("Terminal")),
        KeyBinding::new("shift-down", zed_terminal::ScrollLineDown, Some("Terminal")),
        KeyBinding::new("shift-home", zed_terminal::ScrollToTop, Some("Terminal")),
        KeyBinding::new("cmd-home", zed_terminal::ScrollToTop, Some("Terminal")),
        KeyBinding::new("shift-end", zed_terminal::ScrollToBottom, Some("Terminal")),
        KeyBinding::new("cmd-end", zed_terminal::ScrollToBottom, Some("Terminal")),
        KeyBinding::new(
            "ctrl-shift-space",
            zed_terminal::ToggleViMode,
            Some("Terminal"),
        ),
    ]);

    #[cfg(not(target_os = "macos"))]
    cx.bind_keys([
        KeyBinding::new("ctrl-shift-c", zed_terminal::Copy, Some("Terminal")),
        KeyBinding::new("ctrl-shift-v", zed_terminal::Paste, Some("Terminal")),
        KeyBinding::new("shift-insert", zed_terminal::Paste, Some("Terminal")),
        KeyBinding::new("ctrl-shift-a", zed_terminal::SelectAll, Some("Terminal")),
        KeyBinding::new("ctrl-shift-l", zed_terminal::Clear, Some("Terminal")),
        KeyBinding::new("shift-pageup", zed_terminal::ScrollPageUp, Some("Terminal")),
        KeyBinding::new(
            "shift-pagedown",
            zed_terminal::ScrollPageDown,
            Some("Terminal"),
        ),
        KeyBinding::new("shift-up", zed_terminal::ScrollLineUp, Some("Terminal")),
        KeyBinding::new("shift-down", zed_terminal::ScrollLineDown, Some("Terminal")),
        KeyBinding::new("shift-home", zed_terminal::ScrollToTop, Some("Terminal")),
        KeyBinding::new("shift-end", zed_terminal::ScrollToBottom, Some("Terminal")),
        KeyBinding::new(
            "ctrl-shift-space",
            zed_terminal::ToggleViMode,
            Some("Terminal"),
        ),
    ]);
}
