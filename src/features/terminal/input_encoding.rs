//! Terminal input, mouse, and OSC52 protocol encoding.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use alacritty_terminal::term::{Osc52, TermMode};
use gpui::{Modifiers, MouseButton};

pub(crate) const MAX_OSC52_CLIPBOARD_BYTES: usize = 1024 * 1024;
pub(crate) const MAX_OSC52_RESPONSE_BYTES: usize = MAX_OSC52_CLIPBOARD_BYTES * 2;
pub(crate) type ProtocolResponseQueue = Arc<Mutex<VecDeque<Vec<u8>>>>;

pub(crate) fn mouse_button_code(btn: MouseButton) -> Option<u8> {
    match btn {
        MouseButton::Left => Some(0),
        MouseButton::Middle => Some(1),
        MouseButton::Right => Some(2),
        MouseButton::Navigate(_) => None,
    }
}

pub(crate) fn mouse_modifier_bits(mods: &Modifiers) -> u8 {
    let mut bits = 0;
    if mods.shift {
        bits |= 4;
    }
    if mods.alt {
        bits |= 8;
    }
    if mods.control {
        bits |= 16;
    }
    bits
}

/// 按当前终端模式生成 SGR、urxvt、UTF-8 扩展或传统 xterm 鼠标序列。
pub(crate) fn encode_mouse_report(
    button: u8,
    col: usize,
    row: usize,
    pressed: bool,
    mods: &Modifiers,
    mode: TermMode,
    urxvt_mouse: bool,
) -> Option<Vec<u8>> {
    let button = if pressed { button } else { 3 };
    let cb = button | mouse_modifier_bits(mods);

    if urxvt_mouse {
        return Some(format!("\x1b[{};{};{}M", cb, col + 1, row + 1).into_bytes());
    }

    if mode.contains(TermMode::SGR_MOUSE) {
        let suffix = if pressed { 'M' } else { 'm' };
        return Some(format!("\x1b[<{};{};{}{}", cb, col + 1, row + 1, suffix).into_bytes());
    }

    encode_normal_mouse(cb, col, row, mode.contains(TermMode::UTF8_MOUSE))
}

/// 传统 `ESC[M` 鼠标协议最多只能表示 223 列/行；UTF-8 变体可扩展到 2015。
pub(crate) fn encode_normal_mouse(
    button: u8,
    col: usize,
    row: usize,
    utf8: bool,
) -> Option<Vec<u8>> {
    let max_point = if utf8 { 2015 } else { 223 };
    if col >= max_point || row >= max_point {
        return None;
    }

    let encode_position = |position: usize| -> Vec<u8> {
        let position = position + 33;
        if utf8 && position >= 128 {
            vec![(0xc0 + position / 64) as u8, (0x80 + (position & 63)) as u8]
        } else {
            vec![position as u8]
        }
    };

    let mut bytes = vec![0x1b, b'[', b'M', 32 + button];
    bytes.extend(encode_position(col));
    bytes.extend(encode_position(row));
    Some(bytes)
}

// ─── 输入编码 ───────────────────────────────────────────────────────────────
pub(crate) fn osc52_text_within_limit(text: &str) -> bool {
    text.len() <= MAX_OSC52_CLIPBOARD_BYTES
}

pub(crate) fn osc52_mode(is_local: bool) -> Osc52 {
    if is_local {
        Osc52::CopyPaste
    } else {
        Osc52::OnlyCopy
    }
}

pub(crate) fn osc52_load_allowed(is_local: bool) -> bool {
    is_local
}

pub(crate) fn take_protocol_responses(queue: &ProtocolResponseQueue) -> Vec<Vec<u8>> {
    match queue.lock() {
        Ok(mut queue) => queue.drain(..).collect::<Vec<_>>(),
        Err(poisoned) => poisoned.into_inner().drain(..).collect::<Vec<_>>(),
    }
}

pub(crate) fn format_osc52_response(
    formatter: &Arc<dyn Fn(&str) -> String + Sync + Send + 'static>,
    text: &str,
) -> Option<Vec<u8>> {
    if !osc52_text_within_limit(text) {
        return None;
    }
    let response = formatter(text);
    (response.len() <= MAX_OSC52_RESPONSE_BYTES).then(|| response.into_bytes())
}

pub(crate) fn decode_hex_bytes(value: &[u8]) -> Option<Vec<u8>> {
    if !value.len().is_multiple_of(2) {
        return None;
    }
    value
        .chunks_exact(2)
        .map(|pair| Some((hex_value(pair[0])? << 4) | hex_value(pair[1])?))
        .collect()
}

pub(crate) fn encode_hex_bytes(value: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(value.len() * 2);
    for &byte in value {
        encoded.push(HEX[(byte >> 4) as usize] as char);
        encoded.push(HEX[(byte & 0x0f) as usize] as char);
    }
    encoded
}

pub(crate) fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

/// 调试用：把字节流转成可读字符串（控制字符转义，ESC 显示为 \x1b）。
pub(crate) fn debug_bytes(b: &[u8]) -> String {
    let mut out = String::with_capacity(b.len());
    for &byte in b {
        match byte {
            b'\r' => out.push_str("\\r"),
            b'\n' => out.push_str("\\n"),
            b'\t' => out.push_str("\\t"),
            0x20..=0x7e => out.push(byte as char),
            _ => out.push_str(&format!("\\x{:02x}", byte)),
        }
    }
    out
}

#[cfg(test)]
pub(crate) fn encode_keystroke_with_mode(ks: &gpui::Keystroke, mode: TermMode) -> Option<Vec<u8>> {
    encode_keystroke_with_event(ks, mode, 1)
}

#[cfg(test)]
pub(crate) fn encode_keystroke_with_event(
    ks: &gpui::Keystroke,
    mode: TermMode,
    event_type: u8,
) -> Option<Vec<u8>> {
    encode_keystroke_with_options(ks, mode, event_type, 0)
}

pub(crate) fn encode_keystroke_with_options(
    ks: &gpui::Keystroke,
    mode: TermMode,
    event_type: u8,
    modify_other_keys: u8,
) -> Option<Vec<u8>> {
    let m = &ks.modifiers;
    let key = ks.key.as_str();
    let has_modifiers = m.shift || m.alt || m.control || m.platform;

    if let Some(bytes) = encode_kitty_keystroke(ks, mode, event_type) {
        return Some(bytes);
    }

    if let Some(bytes) = encode_modify_other_keys(ks, modify_other_keys) {
        return Some(bytes);
    }

    // Ctrl+letter and the ASCII control punctuation are single-byte controls.
    // Keep Alt+Ctrl as ESC followed by the control byte, which is what shells
    // and most full-screen applications expect for a Meta control key.
    if m.control
        && !m.platform
        && let Some(control) = control_code(key)
    {
        if m.alt {
            return Some(vec![0x1b, control]);
        }
        return Some(vec![control]);
    }

    match key {
        "enter" | "return" => {
            if m.shift && !m.alt && !m.control && !m.platform {
                return Some(vec![b'\n']);
            }
            if m.alt && !m.control && !m.platform {
                return Some(if m.shift {
                    vec![0x1b, b'\n']
                } else {
                    vec![0x1b, b'\r']
                });
            }
            if m.control && !m.alt && !m.platform {
                return Some(vec![b'\n']);
            }
            if !has_modifiers {
                return Some(vec![b'\r']);
            }
        }
        "back" | "backspace" => {
            if m.alt && !m.platform {
                return Some(vec![0x1b, 0x7f]);
            }
            if m.control && !m.platform {
                return Some(vec![0x08]);
            }
            if !m.platform {
                return Some(vec![0x7f]);
            }
        }
        "tab" => {
            if m.shift && !m.alt && !m.control && !m.platform {
                return Some(b"\x1b[Z".to_vec());
            }
            if m.alt && !m.platform {
                return Some(vec![0x1b, b'\t']);
            }
            if !m.control && !m.platform {
                return Some(vec![b'\t']);
            }
        }
        "escape" if !m.platform => {
            return Some(if m.alt { vec![0x1b, 0x1b] } else { vec![0x1b] });
        }
        "space" => {
            if m.control && !m.platform {
                return Some(vec![0]);
            }
            if !m.control && !m.platform {
                return Some(if m.alt { vec![0x1b, b' '] } else { vec![b' '] });
            }
        }
        _ => {}
    }

    if !has_modifiers {
        if let Some(bytes) = keypad_key(key, mode) {
            return Some(bytes);
        }
        // A plain printable key is not a special key. Only return here when
        // the lookup actually matched; otherwise continue to the text path.
        if let Some(bytes) = plain_special_key(key, mode) {
            return Some(bytes);
        }
    } else if let Some(bytes) = modified_special_key(key, m) {
        return Some(bytes);
    }

    // 可打印字符：优先用 key_char（已含 shift/option 组合结果），再回退到
    // ASCII key。Cmd/platform 组合由上层快捷键处理，不发送为普通文本。
    if !m.control && !m.platform {
        let ch = ks
            .key_char
            .as_ref()
            .and_then(|value| value.chars().next())
            .or_else(|| key.chars().next())?;
        let ch = if m.shift && ks.key_char.is_none() {
            shifted_ascii_char(ch)
        } else {
            ch
        };
        if !ch.is_control() {
            let mut text = ch.to_string();
            if m.alt {
                text.insert(0, '\x1b');
            }
            return Some(text.into_bytes());
        }
    }
    None
}

/// Encode xterm's modifyOtherKeys extension. The extension deliberately only
/// handles ordinary keys; arrows, function keys, keypad keys, and the common
/// special keys keep their established encodings. Level 3 also reports
/// unmodified ordinary keys.
pub(crate) fn encode_modify_other_keys(ks: &gpui::Keystroke, level: u8) -> Option<Vec<u8>> {
    if !matches!(level, 1..=3) || ks.modifiers.platform {
        return None;
    }
    let key = ks.key.as_str();
    if is_kitty_functional_key(key)
        || matches!(
            key,
            "enter" | "return" | "tab" | "back" | "backspace" | "escape"
        )
    {
        return None;
    }
    let modifiers = &ks.modifiers;
    let modified = modifiers.shift || modifiers.alt || modifiers.control;
    if level != 3
        && (!modified || (level == 1 && modifiers.alt && !modifiers.shift && !modifiers.control))
    {
        return None;
    }
    if level == 1 && control_code(key).is_some() {
        return None;
    }
    let (code, _) = kitty_text_key_code(ks)?;
    Some(format!("\x1b[27;{};{}~", modifier_code(modifiers), code).into_bytes())
}

/// 生成 Kitty 键盘协议的增强编码。
///
/// 保持 Enter/Tab/Backspace 在 disambiguate 模式下的传统字节，避免应用
/// 崩溃后用户无法在 shell 中输入 `reset`；REPORT_ALL 模式则按协议编码全部键。
pub(crate) fn encode_kitty_keystroke(
    ks: &gpui::Keystroke,
    mode: TermMode,
    event_type: u8,
) -> Option<Vec<u8>> {
    let report_all = mode.contains(TermMode::REPORT_ALL_KEYS_AS_ESC);
    let disambiguate = mode.contains(TermMode::DISAMBIGUATE_ESC_CODES);
    if !report_all && !disambiguate {
        return None;
    }

    let key = ks.key.as_str();
    let modifiers = &ks.modifiers;
    if !report_all && matches!(key, "enter" | "return" | "tab" | "back" | "backspace") {
        return None;
    }

    if report_all || (disambiguate && mode.contains(TermMode::REPORT_EVENT_TYPES)) {
        if let Some(bytes) = encode_kitty_functional_key(key, modifiers, mode, event_type) {
            return Some(bytes);
        }
    } else if let Some(code) = kitty_private_key_code(key) {
        return Some(encode_kitty_u(code, modifiers, mode, event_type, None));
    }

    // In disambiguate mode only Escape and modified text keys switch from the
    // legacy byte encoding to CSI u. REPORT_ALL applies this to plain text too.
    let needs_escape_encoding = report_all
        || key == "escape"
        || (disambiguate && (modifiers.alt || modifiers.control || modifiers.platform));
    if !needs_escape_encoding || is_kitty_functional_key(key) {
        return None;
    }

    let (code, alternate) = kitty_text_key_code(ks)?;
    let key_code = if mode.contains(TermMode::REPORT_ALTERNATE_KEYS) {
        alternate
            .map(|alternate| format!("{code}:{alternate}"))
            .unwrap_or_else(|| code.to_string())
    } else {
        code.to_string()
    };
    let associated_text =
        if report_all && event_type == 1 && mode.contains(TermMode::REPORT_ASSOCIATED_TEXT) {
            associated_text_code(ks)
        } else {
            None
        };
    Some(encode_kitty_u(
        key_code,
        modifiers,
        mode,
        event_type,
        associated_text,
    ))
}

pub(crate) fn encode_kitty_functional_key(
    key: &str,
    modifiers: &Modifiers,
    mode: TermMode,
    event_type: u8,
) -> Option<Vec<u8>> {
    let modifier = modifier_code(modifiers);
    let report_event = mode.contains(TermMode::REPORT_EVENT_TYPES);

    let final_byte = match key {
        "up" => Some('A'),
        "down" => Some('B'),
        "right" => Some('C'),
        "left" => Some('D'),
        "home" => Some('H'),
        "end" => Some('F'),
        _ => None,
    };
    if let Some(final_byte) = final_byte {
        let sequence = if modifier == 1 && !report_event {
            format!("\x1b[{final_byte}")
        } else {
            let event = if report_event {
                format!(":{event_type}")
            } else {
                String::new()
            };
            format!("\x1b[1;{modifier}{event}{final_byte}")
        };
        return Some(sequence.into_bytes());
    }

    let tilde_code = match key {
        "insert" => Some(2),
        "delete" => Some(3),
        "pageup" => Some(5),
        "pagedown" => Some(6),
        "f1" => Some(11),
        "f2" => Some(12),
        "f3" => Some(13),
        "f4" => Some(14),
        "f5" => Some(15),
        "f6" => Some(17),
        "f7" => Some(18),
        "f8" => Some(19),
        "f9" => Some(20),
        "f10" => Some(21),
        "f11" => Some(23),
        "f12" => Some(24),
        _ => None,
    };
    if let Some(tilde_code) = tilde_code {
        let sequence = if modifier == 1 && !report_event {
            format!("\x1b[{tilde_code}~")
        } else {
            let event = if report_event {
                format!(":{event_type}")
            } else {
                String::new()
            };
            format!("\x1b[{tilde_code};{modifier}{event}~")
        };
        return Some(sequence.into_bytes());
    }

    let private_code = kitty_private_key_code(key)?;
    Some(encode_kitty_u(
        private_code,
        modifiers,
        mode,
        event_type,
        None,
    ))
}

pub(crate) fn encode_kitty_u(
    key_code: impl std::fmt::Display,
    modifiers: &Modifiers,
    mode: TermMode,
    event_type: u8,
    associated_text: Option<u32>,
) -> Vec<u8> {
    let modifier = modifier_code(modifiers);
    let report_event = mode.contains(TermMode::REPORT_EVENT_TYPES);
    let needs_modifier = modifier != 1 || report_event || associated_text.is_some();
    let mut sequence = format!("\x1b[{}", key_code);
    if needs_modifier {
        sequence.push(';');
        sequence.push_str(&modifier.to_string());
        if report_event {
            sequence.push(':');
            sequence.push_str(&event_type.to_string());
        }
    }
    if let Some(text) = associated_text {
        sequence.push(';');
        sequence.push_str(&text.to_string());
    }
    sequence.push('u');
    sequence.into_bytes()
}

pub(crate) fn kitty_text_key_code(ks: &gpui::Keystroke) -> Option<(u32, Option<u32>)> {
    let key = ks.key.as_str();
    let base = match key {
        "escape" => 27,
        "enter" | "return" => 13,
        "tab" => 9,
        "back" | "backspace" => 127,
        "space" => 32,
        _ if is_kitty_functional_key(key) => return None,
        _ => {
            let ch = key.chars().next()?;
            if ch.is_control() {
                return None;
            }
            ch as u32
        }
    };
    let base = if base <= 0x7f {
        (base as u8 as char).to_ascii_lowercase() as u32
    } else {
        base
    };

    let alternate = ks
        .key_char
        .as_ref()
        .and_then(|value| value.chars().next())
        .or_else(|| {
            ks.modifiers
                .shift
                .then(|| key.chars().next())
                .flatten()
                .map(shifted_ascii_char)
        })
        .filter(|_| ks.modifiers.shift)
        .filter(|ch| !ch.is_control())
        .map(|ch| ch as u32)
        .filter(|alternate| *alternate != base);
    Some((base, alternate))
}

pub(crate) fn associated_text_code(ks: &gpui::Keystroke) -> Option<u32> {
    if ks.modifiers.control || ks.modifiers.platform {
        return None;
    }
    let ch = ks
        .key_char
        .as_ref()
        .and_then(|value| value.chars().next())
        .or_else(|| ks.key.chars().next())
        .map(|ch| {
            if ks.modifiers.shift && ks.key_char.is_none() {
                shifted_ascii_char(ch)
            } else {
                ch
            }
        })?;
    if ch.is_control() || (0x80..=0x9f).contains(&(ch as u32)) {
        None
    } else {
        Some(ch as u32)
    }
}

/// Encode the xterm application keypad. GPUI uses slightly different key
/// names across desktop backends, so accept the common aliases here.
pub(crate) fn keypad_key(key: &str, mode: TermMode) -> Option<Vec<u8>> {
    let (normal, application) = match key {
        "kp0" | "numpad0" | "num0" => (b'0', "\x1bOp"),
        "kp1" | "numpad1" | "num1" => (b'1', "\x1bOq"),
        "kp2" | "numpad2" | "num2" => (b'2', "\x1bOr"),
        "kp3" | "numpad3" | "num3" => (b'3', "\x1bOs"),
        "kp4" | "numpad4" | "num4" => (b'4', "\x1bOt"),
        "kp5" | "numpad5" | "num5" => (b'5', "\x1bOu"),
        "kp6" | "numpad6" | "num6" => (b'6', "\x1bOv"),
        "kp7" | "numpad7" | "num7" => (b'7', "\x1bOw"),
        "kp8" | "numpad8" | "num8" => (b'8', "\x1bOx"),
        "kp9" | "numpad9" | "num9" => (b'9', "\x1bOy"),
        "kpdecimal" | "numpaddecimal" => (b'.', "\x1bOn"),
        "kpcomma" | "numpadcomma" => (b',', "\x1bOl"),
        "kpminus" | "numpadminus" => (b'-', "\x1bOm"),
        "kpplus" | "numpadplus" => (b'+', "\x1bOk"),
        "kpmultiply" | "numpadmultiply" => (b'*', "\x1bOj"),
        "kpdivide" | "numpaddivide" => (b'/', "\x1bOo"),
        "kpenter" | "numpadenter" => (b'\r', "\x1bOM"),
        _ => return None,
    };
    if mode.contains(TermMode::APP_KEYPAD) {
        Some(application.as_bytes().to_vec())
    } else {
        Some(vec![normal])
    }
}

pub(crate) fn shifted_ascii_char(ch: char) -> char {
    match ch {
        'a'..='z' => ch.to_ascii_uppercase(),
        '1' => '!',
        '2' => '@',
        '3' => '#',
        '4' => '$',
        '5' => '%',
        '6' => '^',
        '7' => '&',
        '8' => '*',
        '9' => '(',
        '0' => ')',
        '-' => '_',
        '=' => '+',
        '[' => '{',
        ']' => '}',
        '\\' => '|',
        ';' => ':',
        '\'' => '"',
        ',' => '<',
        '.' => '>',
        '/' => '?',
        '`' => '~',
        _ => ch,
    }
}

pub(crate) fn is_kitty_functional_key(key: &str) -> bool {
    matches!(
        key,
        "up" | "down"
            | "left"
            | "right"
            | "home"
            | "end"
            | "insert"
            | "delete"
            | "pageup"
            | "pagedown"
            | "f1"
            | "f2"
            | "f3"
            | "f4"
            | "f5"
            | "f6"
            | "f7"
            | "f8"
            | "f9"
            | "f10"
            | "f11"
            | "f12"
            | "f13"
            | "f14"
            | "f15"
            | "f16"
            | "f17"
            | "f18"
            | "f19"
            | "f20"
            | "f21"
            | "f22"
            | "f23"
            | "f24"
            | "f25"
            | "f26"
            | "f27"
            | "f28"
            | "f29"
            | "f30"
            | "f31"
            | "f32"
            | "f33"
            | "f34"
            | "f35"
    )
}

pub(crate) fn kitty_private_key_code(key: &str) -> Option<u32> {
    let function = key.strip_prefix('f')?.parse::<u32>().ok()?;
    if (13..=35).contains(&function) {
        Some(57376 + function - 13)
    } else {
        None
    }
}

pub(crate) fn control_code(key: &str) -> Option<u8> {
    if key.chars().count() != 1 {
        return None;
    }
    let ch = key.chars().next()?.to_ascii_lowercase();
    match ch {
        'a'..='z' => Some(ch as u8 - b'a' + 1),
        '@' => Some(0),
        '[' => Some(0x1b),
        '\\' => Some(0x1c),
        ']' => Some(0x1d),
        '^' => Some(0x1e),
        '_' => Some(0x1f),
        '?' => Some(0x7f),
        _ => None,
    }
}

pub(crate) fn plain_special_key(key: &str, mode: TermMode) -> Option<Vec<u8>> {
    let app_cursor = mode.contains(TermMode::APP_CURSOR);
    let sequence = match key {
        "up" => {
            if app_cursor {
                "\x1bOA"
            } else {
                "\x1b[A"
            }
        }
        "down" => {
            if app_cursor {
                "\x1bOB"
            } else {
                "\x1b[B"
            }
        }
        "right" => {
            if app_cursor {
                "\x1bOC"
            } else {
                "\x1b[C"
            }
        }
        "left" => {
            if app_cursor {
                "\x1bOD"
            } else {
                "\x1b[D"
            }
        }
        "home" => {
            if app_cursor {
                "\x1bOH"
            } else {
                "\x1b[H"
            }
        }
        "end" => {
            if app_cursor {
                "\x1bOF"
            } else {
                "\x1b[F"
            }
        }
        "insert" => "\x1b[2~",
        "delete" => "\x1b[3~",
        "pageup" => "\x1b[5~",
        "pagedown" => "\x1b[6~",
        "f1" => "\x1bOP",
        "f2" => "\x1bOQ",
        "f3" => "\x1bOR",
        "f4" => "\x1bOS",
        "f5" => "\x1b[15~",
        "f6" => "\x1b[17~",
        "f7" => "\x1b[18~",
        "f8" => "\x1b[19~",
        "f9" => "\x1b[20~",
        "f10" => "\x1b[21~",
        "f11" => "\x1b[23~",
        "f12" => "\x1b[24~",
        "f13" => "\x1b[25~",
        "f14" => "\x1b[26~",
        "f15" => "\x1b[28~",
        "f16" => "\x1b[29~",
        "f17" => "\x1b[31~",
        "f18" => "\x1b[32~",
        "f19" => "\x1b[33~",
        "f20" => "\x1b[34~",
        "f21" => "\x1b[38~",
        "f22" => "\x1b[39~",
        "f23" => "\x1b[40~",
        "f24" => "\x1b[41~",
        "f25" => "\x1b[42~",
        "f26" => "\x1b[43~",
        "f27" => "\x1b[44~",
        "f28" => "\x1b[45~",
        "f29" => "\x1b[46~",
        "f30" => "\x1b[47~",
        "f31" => "\x1b[48~",
        "f32" => "\x1b[49~",
        "f33" => "\x1b[50~",
        "f34" => "\x1b[51~",
        "f35" => "\x1b[52~",
        _ => return None,
    };
    Some(sequence.as_bytes().to_vec())
}

pub(crate) fn modified_special_key(key: &str, modifiers: &Modifiers) -> Option<Vec<u8>> {
    let code = modifier_code(modifiers);
    let arrow = match key {
        "up" => Some('A'),
        "down" => Some('B'),
        "right" => Some('C'),
        "left" => Some('D'),
        "home" => Some('H'),
        "end" => Some('F'),
        "f1" => Some('P'),
        "f2" => Some('Q'),
        "f3" => Some('R'),
        "f4" => Some('S'),
        _ => None,
    };
    if let Some(final_byte) = arrow {
        return Some(format!("\x1b[1;{}{}", code, final_byte).into_bytes());
    }

    let tilde_code = match key {
        "insert" => Some(2),
        "delete" => Some(3),
        "pageup" => Some(5),
        "pagedown" => Some(6),
        "f5" => Some(15),
        "f6" => Some(17),
        "f7" => Some(18),
        "f8" => Some(19),
        "f9" => Some(20),
        "f10" => Some(21),
        "f11" => Some(23),
        "f12" => Some(24),
        "f13" => Some(25),
        "f14" => Some(26),
        "f15" => Some(28),
        "f16" => Some(29),
        "f17" => Some(31),
        "f18" => Some(32),
        "f19" => Some(33),
        "f20" => Some(34),
        "f21" => Some(38),
        "f22" => Some(39),
        "f23" => Some(40),
        "f24" => Some(41),
        "f25" => Some(42),
        "f26" => Some(43),
        "f27" => Some(44),
        "f28" => Some(45),
        "f29" => Some(46),
        "f30" => Some(47),
        "f31" => Some(48),
        "f32" => Some(49),
        "f33" => Some(50),
        "f34" => Some(51),
        "f35" => Some(52),
        _ => None,
    }?;
    Some(format!("\x1b[{};{}~", tilde_code, code).into_bytes())
}

pub(crate) fn is_shell_shortcut(ks: &gpui::Keystroke) -> bool {
    if !(ks.modifiers.platform || ks.modifiers.control) {
        return false;
    }
    matches!(
        ks.key.as_str(),
        "w" | "t" | "tab" | "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9"
    )
}

pub(crate) fn is_low_latency_shell_passthrough_key(ks: &gpui::Keystroke) -> bool {
    ks.modifiers.control && !ks.modifiers.alt && !ks.modifiers.platform && ks.key == "l"
}

/// 计算带修饰键时的 CSI 修饰码（shift=1, alt=2, control=4, meta=8，再 +1）。
pub(crate) fn modifier_code(m: &Modifiers) -> u8 {
    let mut code = 1;
    if m.shift {
        code += 1;
    }
    if m.alt {
        code += 2;
    }
    if m.control {
        code += 4;
    }
    if m.platform {
        code += 8;
    }
    code
}
