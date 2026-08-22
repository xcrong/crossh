//! 终端转义序列常量，对齐 `pi-tui/dist/tui-alt-screen.js` 与 `terminal.js`

pub const ENTER_ALT_SCREEN: &str = "\x1b[?1049h";
pub const EXIT_ALT_SCREEN: &str = "\x1b[?1049l";
pub const DISABLE_AUTOWRAP: &str = "\x1b[?7l";
pub const ENABLE_AUTOWRAP: &str = "\x1b[?7h";
pub const ENABLE_BUTTON_MOTION_MOUSE: &str = "\x1b[?1002h\x1b[?1006h";
pub const ENABLE_ALL_MOTION_MOUSE: &str = "\x1b[?1003h\x1b[?1006h";
pub const DISABLE_MOUSE: &str = "\x1b[?1002l\x1b[?1003l\x1b[?1006l";
pub const BEGIN_SYNCHRONIZED_OUTPUT: &str = "\x1b[?2026h";
pub const END_SYNCHRONIZED_OUTPUT: &str = "\x1b[?2026l";
pub const CLEAR_SCREEN: &str = "\x1b[2J";
pub const HOME_CURSOR: &str = "\x1b[H";
pub const HIDE_CURSOR: &str = "\x1b[?25l";
pub const SHOW_CURSOR: &str = "\x1b[?25h";

/// 根据是否在复用器（tmux/zellij/STY）选择鼠标序列，对齐 pi 的 beforeTerminalStart
pub fn mouse_sequence_for_env(env: &std::collections::HashMap<String, String>) -> &'static str {
    let term = env
        .get("TERM")
        .map(|s| s.to_ascii_lowercase())
        .unwrap_or_default();
    let in_multiplexer = env.contains_key("TMUX")
        || env.contains_key("ZELLIJ")
        || env.contains_key("STY")
        || term.starts_with("tmux")
        || term.starts_with("screen");
    if in_multiplexer {
        ENABLE_BUTTON_MOTION_MOUSE
    } else {
        ENABLE_ALL_MOTION_MOUSE
    }
}

#[cfg(test)]
mod tests {
    #![allow(non_snake_case)]

    use super::*;
    use std::collections::HashMap;

    #[test]
    fn spec_20260822_agent_tui_pi_parity__mouse_sequence_multiplexer_uses_button_motion() {
        let mut env = HashMap::new();
        env.insert("TMUX".into(), "1".into());
        assert_eq!(mouse_sequence_for_env(&env), ENABLE_BUTTON_MOTION_MOUSE);
        env.clear();
        env.insert("TERM".into(), "tmux-256color".into());
        assert_eq!(mouse_sequence_for_env(&env), ENABLE_BUTTON_MOTION_MOUSE);
        env.clear();
        assert_eq!(mouse_sequence_for_env(&env), ENABLE_ALL_MOTION_MOUSE);
    }
}
