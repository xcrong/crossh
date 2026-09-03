//! Terminal session and terminal emulation feature.

use gpui::{App, KeyBinding};
use terminal as zed_terminal;

use crate::features::terminal::view::{SendKeystroke, SendText};

pub(crate) mod view;

pub(crate) use crossh_terminal::{ConnState, TerminalEvent};
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
        KeyBinding::new("cmd-shift-k", zed_terminal::Clear, Some("Terminal")),
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
        // Line-editing conveniences from Zed's macOS keymap: word navigation,
        // word deletion, and clear-line operations.
        KeyBinding::new(
            "cmd-backspace",
            SendKeystroke("ctrl-u".to_string()),
            Some("Terminal"),
        ),
        KeyBinding::new(
            "cmd-delete",
            SendKeystroke("ctrl-k".to_string()),
            Some("Terminal"),
        ),
        KeyBinding::new(
            "cmd-left",
            SendKeystroke("ctrl-a".to_string()),
            Some("Terminal"),
        ),
        KeyBinding::new(
            "cmd-right",
            SendKeystroke("ctrl-e".to_string()),
            Some("Terminal"),
        ),
        KeyBinding::new(
            "alt-left",
            SendText("\u{1b}b".to_string()),
            Some("Terminal"),
        ),
        KeyBinding::new(
            "alt-right",
            SendText("\u{1b}f".to_string()),
            Some("Terminal"),
        ),
        KeyBinding::new("alt-b", SendText("\u{1b}b".to_string()), Some("Terminal")),
        KeyBinding::new("alt-f", SendText("\u{1b}f".to_string()), Some("Terminal")),
        KeyBinding::new(
            "alt-delete",
            SendText("\u{1b}d".to_string()),
            Some("Terminal"),
        ),
        KeyBinding::new(
            "ctrl-delete",
            SendText("\u{1b}[3;5~".to_string()),
            Some("Terminal"),
        ),
        KeyBinding::new(
            "ctrl-backspace",
            SendKeystroke("ctrl-w".to_string()),
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
        // Word navigation and deletion conveniences from Zed's Linux keymap.
        KeyBinding::new("alt-b", SendText("\u{1b}b".to_string()), Some("Terminal")),
        KeyBinding::new("alt-f", SendText("\u{1b}f".to_string()), Some("Terminal")),
        KeyBinding::new("alt-.", SendText("\u{1b}.".to_string()), Some("Terminal")),
        KeyBinding::new(
            "ctrl-delete",
            SendText("\u{1b}[3;5~".to_string()),
            Some("Terminal"),
        ),
        KeyBinding::new(
            "ctrl-backspace",
            SendKeystroke("ctrl-w".to_string()),
            Some("Terminal"),
        ),
    ]);
}
