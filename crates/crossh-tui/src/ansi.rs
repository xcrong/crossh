//! ansi — 1:1 移植 pi-tui 的 utils.js 核心能力
//!
//! 覆盖：extractAnsiCode / stripTerminalSequences / visibleWidth /
//! sliceByColumn / truncateToWidth / wrapTextWithAnsi(含 AnsiCodeTracker) /
//! normalizeTerminalOutput（tab→3 空格）

use unicode_segmentation::UnicodeSegmentation;

/// CURSOR_MARKER — pi 的 APC 序列，零宽，用于硬件光标定位
pub const CURSOR_MARKER: &str = "\x1b_pi:c\x07";
/// SEGMENT_RESET — pi 在行尾追加的重置序列
pub const SEGMENT_RESET: &str = "\x1b[0m\x1b]8;;\x07";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnsiCode {
    pub code: String,
    pub length: usize,
}

/// 提取 pos 处的 ANSI/OSC/APC 序列，与 pi 的 extractAnsiCode 1:1
pub fn extract_ansi_code(s: &str, pos: usize) -> Option<AnsiCode> {
    let bytes = s.as_bytes();
    if pos >= s.len() || bytes[pos] != 0x1b {
        return None;
    }
    let next = *bytes.get(pos + 1)?;
    match next {
        b'[' => {
            // CSI: ESC [ ... 终结于 m/G/K/H/J
            let mut j = pos + 2;
            while j < s.len() && !matches!(bytes[j], b'm' | b'G' | b'K' | b'H' | b'J') {
                j += 1;
            }
            if j < s.len() {
                Some(AnsiCode {
                    code: s[pos..=j].to_string(),
                    length: j + 1 - pos,
                })
            } else {
                None
            }
        }
        b']' => {
            // OSC: ESC ] ... BEL 或 ESC \
            let mut j = pos + 2;
            while j < s.len() {
                if bytes[j] == 0x07 {
                    return Some(AnsiCode {
                        code: s[pos..=j].to_string(),
                        length: j + 1 - pos,
                    });
                }
                if bytes[j] == 0x1b && bytes.get(j + 1) == Some(&b'\\') {
                    return Some(AnsiCode {
                        code: s[pos..j + 2].to_string(),
                        length: j + 2 - pos,
                    });
                }
                j += 1;
            }
            None
        }
        b'_' => {
            // APC: ESC _ ... BEL 或 ESC \
            let mut j = pos + 2;
            while j < s.len() {
                if bytes[j] == 0x07 {
                    return Some(AnsiCode {
                        code: s[pos..=j].to_string(),
                        length: j + 1 - pos,
                    });
                }
                if bytes[j] == 0x1b && bytes.get(j + 1) == Some(&b'\\') {
                    return Some(AnsiCode {
                        code: s[pos..j + 2].to_string(),
                        length: j + 2 - pos,
                    });
                }
                j += 1;
            }
            None
        }
        _ => None,
    }
}

/// 移除 ANSI/OSC/APC 序列，保留可见文本
pub fn strip_terminal_sequences(s: &str) -> String {
    if !s.contains('\x1b') {
        return s.to_string();
    }
    let mut result = String::new();
    let mut i = 0;
    while i < s.len() {
        match extract_ansi_code(s, i) {
            Some(ansi) => i += ansi.length,
            None => {
                let ch = s[i..].chars().next().unwrap();
                result.push(ch);
                i += ch.len_utf8();
            }
        }
    }
    result
}

fn is_printable_ascii(s: &str) -> bool {
    s.bytes().all(|b| (0x20..=0x7e).contains(&b))
}

/// grapheme 宽度：对齐 pi 的 graphemeWidth（tab=3、emoji=2、CJK 全角）
fn grapheme_width(segment: &str) -> usize {
    use unicode_width::UnicodeWidthChar;
    if segment == "\t" {
        return 3;
    }
    let mut width = 0usize;
    for ch in segment.chars() {
        let cp = ch as u32;
        // 控制字符零宽
        if cp < 0x20 || (0x7f..0xa0).contains(&cp) {
            continue;
        }
        // 组合附加符号零宽
        if unicode_width::UnicodeWidthStr::width(ch.to_string().as_str()) == 0 {
            continue;
        }
        width += UnicodeWidthChar::width(ch).unwrap_or(0);
    }
    width
}

/// 可见宽度（忽略 ANSI/tab），与 pi 的 visibleWidth 对齐
pub fn visible_width(s: &str) -> usize {
    if s.is_empty() {
        return 0;
    }
    if is_printable_ascii(s) {
        return s.len();
    }
    let clean = strip_terminal_sequences(s);
    clean
        .graphemes(true)
        .map(|g| if g == "\t" { 3 } else { grapheme_width(g) })
        .sum()
}

/// 按可见列切片，与 pi 的 sliceByColumn(strict) 语义一致：
/// ANSI 状态跨切片保留；strict 时排除跨界宽字符
pub fn slice_by_column(line: &str, start_col: usize, length: usize, strict: bool) -> String {
    slice_with_width(line, start_col, length, strict).text
}

/// 同上但附带实际宽度（pi 的 sliceWithWidth）
pub fn slice_with_width(line: &str, start_col: usize, length: usize, strict: bool) -> SliceResult {
    let mut result = SliceResult::default();
    if length == 0 {
        return result;
    }
    let end_col = start_col + length;
    let mut text = String::new();
    let mut width = 0usize;
    let mut current_col = 0usize;
    let mut pending_ansi = String::new();
    let mut i = 0usize;
    while i < line.len() {
        if let Some(ansi) = extract_ansi_code(line, i) {
            if current_col >= start_col && current_col < end_col {
                text.push_str(&ansi.code);
            } else if current_col < start_col {
                pending_ansi.push_str(&ansi.code);
            }
            i += ansi.length;
            continue;
        }
        // 到下一个转义序列（或行尾）之前的文本段按 grapheme 切，
        // 转义序列的参数字节不计入可见列
        let next_escape = line[i..]
            .find('\x1b')
            .map(|offset| i + offset)
            .unwrap_or(line.len());
        for g in line[i..next_escape].graphemes(true) {
            let w = if g == "\t" { 3 } else { grapheme_width(g) };
            let in_range = current_col >= start_col && current_col < end_col;
            let fits = !strict || current_col + w <= end_col;
            if in_range && fits {
                if !pending_ansi.is_empty() {
                    text.push_str(&pending_ansi);
                    pending_ansi.clear();
                }
                text.push_str(g);
                width += w;
            }
            current_col += w;
            if current_col >= end_col {
                result = SliceResult { text, width };
                return result;
            }
        }
        i = next_escape;
    }
    SliceResult { text, width }
}

#[derive(Debug, Default, Clone)]
pub struct SliceResult {
    pub text: String,
    pub width: usize,
}

/// 截断到最大可见宽度，可加省略号与填充，与 pi 的 truncateToWidth 对齐
pub fn truncate_to_width(text: &str, max_width: usize, ellipsis: &str, pad: bool) -> String {
    if max_width == 0 {
        return String::new();
    }
    if text.is_empty() {
        return if pad {
            " ".repeat(max_width)
        } else {
            String::new()
        };
    }
    let ellipsis_width = visible_width(ellipsis);
    let total = visible_width(text);
    if total <= max_width {
        return if pad {
            format!("{}{}", text, " ".repeat(max_width - total))
        } else {
            text.to_string()
        };
    }
    if ellipsis_width >= max_width {
        let clipped = slice_by_column(ellipsis, 0, max_width, false);
        let w = visible_width(&clipped);
        return if pad {
            format!("{}{}", clipped, " ".repeat(max_width.saturating_sub(w)))
        } else {
            clipped
        };
    }
    let target = max_width - ellipsis_width;
    let prefix = slice_by_column(text, 0, target, true);
    let prefix_w = visible_width(&prefix);
    let result = format!("{}{}\x1b[0m{}", prefix, ellipsis, "\x1b[0m");
    let visible = prefix_w + ellipsis_width;
    if pad {
        format!(
            "{}{}",
            result,
            " ".repeat(max_width.saturating_sub(visible))
        )
    } else {
        result
    }
}

// ── AnsiCodeTracker：SGR 状态跟踪，跨行/换行恢复样式 ──

#[derive(Debug, Default, Clone)]
struct SgrState {
    bold: bool,
    dim: bool,
    italic: bool,
    underline: bool,
    inverse: bool,
    strikethrough: bool,
    fg: Option<String>,
    bg: Option<String>,
}

impl SgrState {
    fn reset(&mut self) {
        *self = SgrState::default();
    }
    fn process(&mut self, ansi_code: &str) {
        let Some(inner) = ansi_code
            .strip_prefix("\x1b[")
            .and_then(|s| s.strip_suffix('m'))
        else {
            return;
        };
        if inner.is_empty() || inner == "0" {
            self.reset();
            return;
        }
        let parts: Vec<&str> = inner.split(';').collect();
        let mut i = 0;
        while i < parts.len() {
            let code: u32 = parts[i].parse().unwrap_or(u32::MAX);
            match code {
                38 | 48 => {
                    if parts.get(i + 1) == Some(&"5") && parts.get(i + 2).is_some() {
                        let color = format!("{};{};{}", parts[i], parts[i + 1], parts[i + 2]);
                        if code == 38 {
                            self.fg = Some(color);
                        } else {
                            self.bg = Some(color);
                        }
                        i += 3;
                        continue;
                    } else if parts.get(i + 1) == Some(&"2") && parts.get(i + 4).is_some() {
                        let color = format!(
                            "{};{};{};{};{}",
                            parts[i],
                            parts[i + 1],
                            parts[i + 2],
                            parts[i + 3],
                            parts[i + 4]
                        );
                        if code == 38 {
                            self.fg = Some(color);
                        } else {
                            self.bg = Some(color);
                        }
                        i += 5;
                        continue;
                    }
                }
                0 => self.reset(),
                1 => self.bold = true,
                2 => self.dim = true,
                3 => self.italic = true,
                4 => self.underline = true,
                7 => self.inverse = true,
                9 => self.strikethrough = true,
                21 => self.bold = false,
                22 => {
                    self.bold = false;
                    self.dim = false;
                }
                23 => self.italic = false,
                24 => self.underline = false,
                27 => self.inverse = false,
                29 => self.strikethrough = false,
                39 => self.fg = None,
                49 => self.bg = None,
                c if (30..=37).contains(&c) || (90..=97).contains(&c) => {
                    self.fg = Some(c.to_string());
                }
                c if (40..=47).contains(&c) || (100..=107).contains(&c) => {
                    self.bg = Some(c.to_string());
                }
                _ => {}
            }
            i += 1;
        }
    }
    fn active_codes(&self) -> String {
        let mut codes = Vec::new();
        if self.bold {
            codes.push("1");
        }
        if self.dim {
            codes.push("2");
        }
        if self.italic {
            codes.push("3");
        }
        if self.underline {
            codes.push("4");
        }
        if self.inverse {
            codes.push("7");
        }
        if self.strikethrough {
            codes.push("9");
        }
        if let Some(fg) = &self.fg {
            codes.push(fg.as_str());
        }
        if let Some(bg) = &self.bg {
            codes.push(bg.as_str());
        }
        if codes.is_empty() {
            String::new()
        } else {
            format!("\x1b[{}m", codes.join(";"))
        }
    }
    fn line_end_reset(&self) -> String {
        if self.underline {
            "\x1b[24m".into()
        } else {
            String::new()
        }
    }
}

fn update_tracker_from_text(text: &str, tracker: &mut SgrState) {
    let mut i = 0;
    while i < text.len() {
        match extract_ansi_code(text, i) {
            Some(a) => {
                tracker.process(&a.code);
                i += a.length;
            }
            None => {
                let ch = text[i..].chars().next().unwrap();
                i += ch.len_utf8();
            }
        }
    }
}

const CJK_BREAK: fn(char) -> bool = |c| {
    matches!(c as u32,
        0x4E00..=0x9FFF   // CJK Unified
        | 0x3400..=0x4DBF // CJK Ext A
        | 0x3040..=0x30FF // Hiragana/Katakana
        | 0xAC00..=0xD7AF // Hangul
        | 0x3105..=0x312F // Bopomofo
        | 0xF900..=0xFAFF // CJK Compat
    )
};

/// 把一行切成 token（word/space/CJK 单字），ANSI 挂在后续可见 token 前
fn split_into_tokens_with_ansi(line: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut pending_ansi = String::new();
    let mut current_kind: Option<u8> = None; // 0=space 1=word 2=cjk
    for g in line.graphemes(true) {
        if g.chars().all(extract_ansi_code_char) {
            // 不可能：grapheme 含 ESC 时 extract_ansi 已在外层处理；
            // 这里保守把 ESC 开头的 grapheme 视作文本
        }
        let _ = extract_ansi_code_char;
        if g.starts_with('\x1b') {
            // 整段 ANSI（grapheme 化不会合并 ESC），逐字符提取
            pending_ansi.push_str(g);
            continue;
        }
        let kind = if g == " " {
            0u8
        } else if g.chars().next().map(CJK_BREAK).unwrap_or(false) {
            2u8
        } else {
            1u8
        };
        if kind == 2 {
            if !current.is_empty() {
                tokens.push(std::mem::take(&mut current));
                current_kind = None;
            }
            let mut token = std::mem::take(&mut pending_ansi);
            token.push_str(g);
            tokens.push(token);
            continue;
        }
        if !current.is_empty() && current_kind != Some(kind) {
            tokens.push(std::mem::take(&mut current));
        }
        if !pending_ansi.is_empty() {
            current.push_str(&pending_ansi);
            pending_ansi.clear();
        }
        current_kind = Some(kind);
        current.push_str(g);
    }
    if !pending_ansi.is_empty() {
        if !current.is_empty() {
            current.push_str(&pending_ansi);
        } else if let Some(last) = tokens.last_mut() {
            last.push_str(&pending_ansi);
        } else {
            current = pending_ansi;
        }
    }
    if !current.is_empty() {
        tokens.push(current);
    }
    tokens
}

fn extract_ansi_code_char(_c: char) -> bool {
    false
}

/// 断开超长单词（与 pi 的 breakLongWord 一致）
fn break_long_word(word: &str, width: usize, tracker: &SgrState) -> Vec<String> {
    let mut lines = Vec::new();
    let mut current_line = tracker.active_codes();
    let mut current_width = 0usize;
    for g in word.graphemes(true) {
        if g.starts_with('\x1b') {
            // ANSI grapheme 直接附加（宽度为零）
            current_line.push_str(g);
            continue;
        }
        let gw = if g == "\t" { 3 } else { grapheme_width(g) };
        if current_width + gw > width {
            current_line.push_str(&tracker.line_end_reset());
            lines.push(std::mem::take(&mut current_line));
            current_line = tracker.active_codes();
            current_width = 0;
        }
        current_line.push_str(g);
        current_width += gw;
    }
    if !current_line.is_empty() {
        lines.push(current_line);
    }
    if lines.is_empty() {
        lines.push(String::new());
    }
    lines
}

fn wrap_single_line(line: &str, width: usize) -> Vec<String> {
    if line.is_empty() {
        return vec![String::new()];
    }
    if visible_width(line) <= width {
        return vec![line.to_string()];
    }
    let mut wrapped = Vec::new();
    let mut tracker = SgrState::default();
    let tokens = split_into_tokens_with_ansi(line);
    let mut current_line = String::new();
    let mut current_visible = 0usize;
    for token in &tokens {
        let token_visible = visible_width(token);
        let is_whitespace = strip_terminal_sequences(token).trim().is_empty();
        if token_visible > width && !is_whitespace {
            if !current_line.is_empty() {
                let reset = tracker.line_end_reset();
                current_line.push_str(&reset);
                wrapped.push(std::mem::take(&mut current_line));
            }
            let broken = break_long_word(token, width, &tracker);
            for b in &broken[..broken.len() - 1] {
                wrapped.push(b.clone());
            }
            update_tracker_from_text(token, &mut tracker);
            // 断行余段成为新行，其真实宽度作为后续 token 累计基准
            current_line = broken[broken.len() - 1].clone();
            current_visible = strip_terminal_sequences(&current_line)
                .graphemes(true)
                .map(|g| if g == "\t" { 3 } else { grapheme_width(g) })
                .sum();
            continue;
        }
        if current_visible + token_visible > width && current_visible > 0 {
            let mut line_out = current_line.trim_end().to_string();
            line_out.push_str(&tracker.line_end_reset());
            wrapped.push(line_out);
            current_line.clear();
            if is_whitespace {
                current_line.push_str(&tracker.active_codes());
                #[allow(unused_assignments)]
                {
                    current_visible = 0;
                }
            } else {
                current_line.push_str(&tracker.active_codes());
                current_line.push_str(token);
                current_visible = token_visible;
            }
        } else {
            current_line.push_str(token);
            current_visible += token_visible;
        }
        update_tracker_from_text(token, &mut tracker);
    }
    if !current_line.is_empty() {
        wrapped.push(current_line);
    }
    if wrapped.is_empty() {
        return vec![String::new()];
    }
    wrapped
        .into_iter()
        .map(|l| l.trim_end().to_string())
        .collect()
}

/// ANSI 感知的自动换行，与 pi 的 wrapTextWithAnsi 1:1
/// （样式跨行延续、CJK 逐字断行、超长单词强制断）
pub fn wrap_text_with_ansi(text: &str, width: usize) -> Vec<String> {
    if text.is_empty() {
        return vec![String::new()];
    }
    let input_lines: Vec<&str> = text.split(['\n', '\r']).collect();
    let mut result = Vec::new();
    let mut tracker = SgrState::default();
    for input_line in input_lines {
        let prefix = if result.is_empty() {
            String::new()
        } else {
            tracker.active_codes()
        };
        let combined = format!("{}{}", prefix, input_line);
        for wrapped in wrap_single_line(&combined, width) {
            result.push(wrapped);
        }
        update_tracker_from_text(input_line, &mut tracker);
    }
    if result.is_empty() {
        result.push(String::new());
    }
    result
}

/// tab → 3 空格归一化（pi 的 normalizeTerminalOutput 的 tab 部分）
pub fn normalize_terminal_output(s: &str) -> String {
    if !s.contains('\t') {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < s.len() {
        match extract_ansi_code(s, i) {
            Some(a) => {
                out.push_str(&a.code);
                i += a.length;
            }
            None => {
                if s.as_bytes()[i] == b'\t' {
                    out.push_str("   ");
                    i += 1;
                } else {
                    let ch = s[i..].chars().next().unwrap();
                    out.push(ch);
                    i += ch.len_utf8();
                }
            }
        }
    }
    out
}

/// 列级合成（pi 的 compositeTuiLine 1:1）：
/// 把 overlay 以 `start_col` 为起点、占 `overlay_width` 列合成到 base 行，
/// 前段原样保留、后段继承边界处 SGR 状态并补齐到 totalWidth
pub fn composite_tui_line(
    base: &str,
    overlay: &str,
    start_col: usize,
    overlay_width: usize,
    total_width: usize,
) -> String {
    let after_start = start_col + overlay_width;
    // 解析 base 为 ANSI/字符序列
    let mut items: Vec<(bool, String)> = Vec::new();
    {
        let mut i = 0usize;
        while i < base.len() {
            match extract_ansi_code(base, i) {
                Some(a) => {
                    items.push((true, a.code));
                    i += a.length;
                }
                None => {
                    let ch = base[i..].chars().next().unwrap();
                    items.push((false, ch.to_string()));
                    i += ch.len_utf8();
                }
            }
        }
    }
    let mut before = String::new();
    let mut after = String::new();
    let mut after_style = String::new();
    let mut tracker = SgrState::default();
    let mut col = 0usize;
    for (is_ansi, text) in &items {
        if *is_ansi {
            tracker.process(text);
            if col < start_col {
                before.push_str(text);
            } else if !after_style.is_empty() || col >= after_start {
                after.push_str(text);
            }
            continue;
        }
        let w = if text == "\t" {
            3
        } else {
            grapheme_width(text)
        };
        if col < start_col {
            before.push_str(text);
        } else if col >= after_start {
            if after_style.is_empty() {
                after_style = tracker.active_codes();
            }
            after.push_str(text);
        }
        col += w;
    }
    let before_w = visible_width(&before);
    let before_pad = " ".repeat(start_col.saturating_sub(before_w));
    let overlay_clipped = slice_by_column(overlay, 0, overlay_width, true);
    let overlay_w = visible_width(&overlay_clipped);
    let overlay_pad = " ".repeat(overlay_width.saturating_sub(overlay_w));
    let after_w = visible_width(&after);
    let after_pad = " ".repeat(
        total_width
            .saturating_sub(start_col + overlay_width)
            .saturating_sub(after_w),
    );
    format!(
        "{}{}{}{}{}{}{}{}{}",
        before,
        before_pad,
        SEGMENT_RESET,
        overlay_clipped,
        overlay_pad,
        SEGMENT_RESET,
        after_style,
        after,
        after_pad
    )
}

/// 列级选区高亮（pi 的 applySelection + getSelectionColumns 1:1）：
/// 只把 `[start_col, end_col)` 可见列包 `\x1b[7m…\x1b[27m`，
/// 前后段原样保留；after 段继承边界处的 SGR 状态（pi 的 extractSegments）
pub fn style_visible_range(line: &str, start_col: usize, end_col: usize) -> String {
    if start_col >= end_col || line.is_empty() {
        return line.to_string();
    }
    #[derive(PartialEq, Clone, Copy)]
    enum Seg {
        Before,
        Sel,
        After,
    }
    let seg_of = |col: usize| -> Seg {
        if col < start_col {
            Seg::Before
        } else if col < end_col {
            Seg::Sel
        } else {
            Seg::After
        }
    };

    // 先解析为 (is_ansi, text) 序列，再按列归属分段
    let mut items: Vec<(bool, String)> = Vec::new();
    {
        let mut i = 0usize;
        while i < line.len() {
            match extract_ansi_code(line, i) {
                Some(a) => {
                    items.push((true, a.code));
                    i += a.length;
                }
                None => {
                    let ch = line[i..].chars().next().unwrap();
                    items.push((false, ch.to_string()));
                    i += ch.len_utf8();
                }
            }
        }
    }

    let mut before = String::new();
    let mut sel = String::new();
    let mut after = String::new();
    let mut after_style = String::new();
    let mut tracker = SgrState::default();
    let mut col = 0usize;

    for (is_ansi, text) in &items {
        if *is_ansi {
            tracker.process(text);
            // 零宽序列归属当前位置所在段
            match seg_of(col) {
                Seg::Before => before.push_str(text),
                Seg::Sel => sel.push_str(text),
                Seg::After => after.push_str(text),
            }
            continue;
        }
        let w = if text == "\t" {
            3
        } else {
            grapheme_width(text)
        };
        match seg_of(col) {
            Seg::Before => before.push_str(text),
            Seg::Sel => sel.push_str(text),
            Seg::After => {
                if after_style.is_empty() {
                    // 进入 after 段时捕获 SGR 继承状态（pi 的 pooledStyleTracker.getActiveCodes()）
                    after_style = tracker.active_codes();
                }
                after.push_str(text);
            }
        }
        col += w;
    }

    if sel.is_empty() {
        return line.to_string();
    }
    format!("{}\x1b[7m{}\x1b[27m{}{}", before, sel, after_style, after)
}

#[cfg(test)]
mod tests {
    #![allow(non_snake_case)]

    use super::*;

    #[test]
    fn spec_20260822_agent_tui_pi_parity__visible_width_handles_cjk_and_ansi() {
        assert_eq!(visible_width("hello"), 5);
        assert_eq!(visible_width("中中"), 4);
        assert_eq!(visible_width("\x1b[31mred\x1b[0m"), 3);
        assert_eq!(visible_width("\x1b_pi:c\x07"), 0);
    }

    #[test]
    fn spec_20260822_agent_tui_pi_parity__wrap_preserves_style_across_lines() {
        let wrapped = wrap_text_with_ansi("\x1b[31mhello world foo\x1b[0m", 6);
        assert_eq!(wrapped.len(), 3);
        assert!(wrapped[0].starts_with("\x1b[31m"));
        // 第二行样式延续
        assert!(wrapped[1].starts_with("\x1b[31m"));
        assert!(wrapped.iter().all(|l| visible_width(l) <= 6));
    }

    #[test]
    fn spec_20260822_agent_tui_pi_parity__slice_by_column_keeps_ansi_state() {
        let line = "\x1b[31mabcdef\x1b[0m";
        let sliced = slice_by_column(line, 2, 3, true);
        assert!(sliced.contains("cde"));
        assert_eq!(visible_width(&sliced), 3);
    }

    #[test]
    fn spec_20260822_agent_tui_pi_parity__slice_by_column_skips_embedded_ansi_columns() {
        // 转义序列出现在切片范围内：其参数字节不得计入可见列
        let line = "\x1b[31mhello\x1b[0m world";
        let sliced = slice_by_column(line, 2, 7, true);
        assert_eq!(strip_terminal_sequences(&sliced), "llo wor");
        assert_eq!(visible_width(&sliced), 7);
    }

    #[test]
    fn spec_20260822_agent_tui_pi_parity__truncate_adds_ellipsis() {
        let t = truncate_to_width("abcdefghij", 6, "...", false);
        assert_eq!(visible_width(&t), 6);
        assert!(strip_terminal_sequences(&t).starts_with("abc..."));
    }

    #[test]
    fn spec_20260822_agent_tui_pi_parity__selection_highlights_only_selected_columns() {
        // 只选中间的 "wor"：前后段不被反显包裹
        let out = style_visible_range("hello world", 6, 9);
        let plain = strip_terminal_sequences(&out);
        assert_eq!(plain, "hello world");
        // sel 段被 \x1b[7m 包裹且只含 wor
        assert!(out.contains("\x1b[7mwor\x1b[27m"));
        // 前段 "hello " 在反显之前
        assert!(out.starts_with("hello "));
        // after 段 "ld" 不在反显内
        let after_pos = out.find("\x1b[27m").unwrap() + 5;
        assert!(out[after_pos..].contains("ld"));
        assert!(!out[after_pos..].contains("\x1b[7m"));
    }

    #[test]
    fn spec_20260822_agent_tui_pi_parity__selection_inherits_style_into_after_segment() {
        // 红色整行，选中中间：after 段应重新携带红色
        let line = "\x1b[31maabbcc\x1b[0m";
        let out = style_visible_range(line, 2, 4);
        // after 段（cc）前有恢复红色的 SGR
        let after = &out[out.rfind("\x1b[27m").unwrap() + 5..];
        assert!(after.contains("cc"));
        assert!(after.contains("\x1b[31m"));
    }

    #[test]
    fn spec_20260822_agent_tui_pi_parity__selection_full_line_and_empty_guard() {
        // 全行选择等价整行反显
        let full = style_visible_range("abc", 0, 3);
        assert_eq!(full, "\x1b[7mabc\x1b[27m");
        // 空区间原样返回
        assert_eq!(style_visible_range("abc", 2, 2), "abc");
    }

    #[test]
    fn spec_20260822_agent_tui_pi_parity__composite_flash_preserves_after_content() {
        // base 行 "hello world"：flash 占 0..6 列，after 段 "world" 应保留
        let out = composite_tui_line("hello world", "\x1b[7m COPIED \x1b[27m", 0, 8, 20);
        let plain = strip_terminal_sequences(&out);
        assert!(plain.starts_with(" COPIED"), "got: {plain:?}");
        // overlay 占 0..8，after 从列 8 开始："world" 的前 2 列被覆盖，剩 "rld"
        assert!(plain.contains("rld"), "after content preserved: {plain:?}");
        assert_eq!(visible_width(&out), 20);
    }

    #[test]
    fn spec_20260822_agent_tui_pi_parity__composite_at_offset_keeps_prefix() {
        // 从第 3 列开始合成，前缀 "abc" 保留
        let out = composite_tui_line("abcdef", "XY", 3, 2, 10);
        let plain = strip_terminal_sequences(&out);
        assert!(plain.starts_with("abcXY"));
        assert!(plain.contains("f"));
    }

    #[test]
    fn spec_20260822_agent_tui_pi_parity__selection_with_wide_chars_partial_range() {
        // CJK 宽字符：选中第一个字（列 0-2）
        let out = style_visible_range("中文abc", 0, 2);
        assert!(out.contains("\x1b[7m中\x1b[27m"));
        assert!(strip_terminal_sequences(&out).starts_with("中文abc"));
    }
}
