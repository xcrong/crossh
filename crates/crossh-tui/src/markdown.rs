//! markdown — pi-tui 的 components/markdown.js 移植
//!
//! 支持块级：heading(#..)/paragraph/code fence/list/blockquote/hr/space
//! 行内：**bold** *italic* `code` ~~del~~ [link](url)
//! 输出结构与 pi 一致：theme 着色 → wrap → paddingX margins → paddingY 空行

use crate::ansi::{normalize_terminal_output, visible_width, wrap_text_with_ansi};
use crate::component::Component;
use std::fmt::Write as _;

/// 强调闭合标记定位：首个「前一字符非空白」的 `marker` 出现位置（字节偏移）。
/// 空内容（紧贴开符号）不视为合法闭合。
fn emphasis_close(src_after_open: &str, marker: &str) -> Option<usize> {
    let mut from = 0usize;
    while let Some(rel) = src_after_open[from..].find(marker) {
        let abs = from + rel;
        if abs > 0 && !src_after_open[..abs].ends_with(char::is_whitespace) {
            return Some(abs);
        }
        from = abs + marker.len();
    }
    None
}

fn starts_with_visible_char(s: &str) -> bool {
    s.chars()
        .next()
        .map(|c| !c.is_whitespace())
        .unwrap_or(false)
}

/// 主题着色函数集（pi 的 MarkdownTheme）
#[derive(Clone)]
pub struct MarkdownTheme {
    pub heading: fn(&str) -> String,
    pub code: fn(&str) -> String,
    pub code_block: fn(&str) -> String,
    pub code_block_border: fn(&str) -> String,
    pub quote: fn(&str) -> String,
    pub quote_border: fn(&str) -> String,
    pub hr: fn(&str) -> String,
    pub list_bullet: fn(&str) -> String,
    pub link: fn(&str) -> String,
}

impl Default for MarkdownTheme {
    fn default() -> Self {
        Self {
            heading: |t| format!("\x1b[1m\x1b[4m{}\x1b[22m\x1b[24m", t),
            code: |t| format!("\x1b[36m{}", t),
            code_block: |t| format!("\x1b[32m{}", t),
            code_block_border: |t| format!("\x1b[90m{}\x1b[39m", t),
            quote: |t| format!("\x1b[90m\x1b[3m{}\x1b[23m\x1b[39m", t),
            quote_border: |t| format!("\x1b[90m{}\x1b[39m", t),
            hr: |t| format!("\x1b[90m{}\x1b[39m", t),
            list_bullet: |t| format!("\x1b[36m{}\x1b[39m", t),
            link: |t| format!("\x1b[34m\x1b[4m{}\x1b[24m\x1b[39m", t),
        }
    }
}

pub struct Markdown {
    pub text: String,
    pub padding_x: usize,
    pub padding_y: usize,
    pub theme: MarkdownTheme,
    /// 流式渲染中的部分代码块（无闭合 fence）也照常渲染
    cache: Option<(String, usize, Vec<String>)>,
}

impl Markdown {
    pub fn new(text: impl Into<String>, padding_x: usize, padding_y: usize) -> Self {
        Self {
            text: text.into(),
            padding_x,
            padding_y,
            theme: MarkdownTheme::default(),
            cache: None,
        }
    }
    pub fn with_theme(
        text: impl Into<String>,
        padding_x: usize,
        padding_y: usize,
        theme: MarkdownTheme,
    ) -> Self {
        Self {
            text: text.into(),
            padding_x,
            padding_y,
            theme,
            cache: None,
        }
    }
    pub fn set_text(&mut self, text: impl Into<String>) {
        self.text = text.into();
        self.cache = None;
    }

    // ── 行内解析 ──
    //
    // 全程使用 &str 的字节下标（与 find()/strip_prefix() 返回值一致），
    // 避免字符下标与字节长度混用导致的吞字。
    // 强调标记遵循 CommonMark flanking 规则的最小子集：
    // 开符号后必须非空白、闭符号前必须非空白——
    // 否则路径中的 `zed-*`、算式中的 `2 * 3` 会被错误配对并吞字。
    fn render_inline(&self, src: &str) -> String {
        let mut out = String::new();
        let mut i = 0usize;
        while i < src.len() {
            let rest = &src[i..];
            // `code`
            if let Some(line) = rest.strip_prefix('`')
                && let Some(end) = line.find('`')
                && end > 0
            {
                let styled = (self.theme.code)(&line[..end]);
                let _ = write!(out, "{}", styled);
                i += 1 + end + 1;
                continue;
            }
            // **bold**
            if let Some(line) = rest.strip_prefix("**")
                && starts_with_visible_char(line)
                && let Some(end) = emphasis_close(line, "**")
            {
                let inner = self.render_inline(&line[..end]);
                let _ = write!(out, "\x1b[1m{}\x1b[22m", inner);
                i += 2 + end + 2;
                continue;
            }
            // ~~del~~
            if let Some(line) = rest.strip_prefix("~~")
                && starts_with_visible_char(line)
                && let Some(end) = emphasis_close(line, "~~")
            {
                let inner = self.render_inline(&line[..end]);
                let _ = write!(out, "\x1b[9m{}\x1b[29m", inner);
                i += 2 + end + 2;
                continue;
            }
            // [text](href)
            if rest.starts_with('[')
                && let Some(close) = rest.find(']')
                && close > 0
                && rest[close + 1..].starts_with('(')
                && let Some(paren_end) = rest[close + 1..].find(')')
                && paren_end > 1
            {
                let text = &rest[1..close];
                let href = &rest[close + 2..close + 1 + paren_end];
                let styled = (self.theme.link)(text);
                // OSC8 可点击链接（pi 在支持终端用 OSC8）
                let _ = write!(
                    out,
                    "\x1b]8;;{}\x07{} \x1b[2m({})\x1b[22m\x1b]8;;\x07",
                    href, styled, href
                );
                i += close + paren_end + 2;
                continue;
            }
            // *italic*（不吞 ** 已处理）
            if let Some(line) = rest.strip_prefix('*')
                && starts_with_visible_char(line)
                && !line.starts_with('*')
                && let Some(end) = emphasis_close(line, "*")
            {
                let inner = self.render_inline(&line[..end]);
                let _ = write!(out, "\x1b[3m{}\x1b[23m", inner);
                i += 1 + end + 1;
                continue;
            }
            // 默认：原样输出一个字符，按其 UTF-8 字节宽度推进
            let ch = rest.chars().next().unwrap();
            out.push(ch);
            i += ch.len_utf8();
        }
        out
    }

    // ── 块级解析 ──
    fn render_blocks(&self, src: &str, width: usize) -> Vec<String> {
        let mut lines = Vec::new();
        let content_width = width.max(1);
        let normalized = normalize_terminal_output(src);
        let src_lines: Vec<&str> = normalized.split('\n').collect();
        let mut i = 0;
        while i < src_lines.len() {
            let line = src_lines[i];
            let trimmed = line.trim_start();
            // code fence
            if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
                let marker_len = 3;
                let lang = trimmed[marker_len..].trim();
                lines.push((self.theme.code_block_border)(&format!("```{}", lang)));
                i += 1;
                while i < src_lines.len() && !src_lines[i].trim_start().starts_with("```") {
                    lines.push(format!("  {}", (self.theme.code_block)(src_lines[i])));
                    i += 1;
                }
                lines.push((self.theme.code_block_border)("```"));
                i += 1;
                continue;
            }
            // heading
            if let Some(rest) = trimmed.strip_prefix('#') {
                let level = 1 + rest.len() - rest.trim_start_matches('#').len();
                let level = level.min(6);
                let text = trimmed[level..].trim();
                let inline = self.render_inline(text);
                // pi：h1 = heading(bold(underline(text)))；h2+ = heading(bold(text))；
                // h>=3 额外前缀 "# "（heading 色包整个内容）
                let styled = if level == 1 {
                    (self.theme.heading)(&format!("\x1b[1m\x1b[4m{}\x1b[22m\x1b[24m", inline))
                } else {
                    (self.theme.heading)(&format!("\x1b[1m{}\x1b[22m", inline))
                };
                if level >= 3 {
                    let prefix = (self.theme.heading)(&format!("{} ", "#".repeat(level)));
                    lines.push(format!("{}{}", prefix, styled));
                } else {
                    lines.push(styled);
                }
                // 若下一行是空行则由空行分支统一加间距
                if src_lines
                    .get(i + 1)
                    .map(|l| !l.trim().is_empty())
                    .unwrap_or(false)
                {
                    lines.push(String::new());
                }
                i += 1;
                continue;
            }
            // hr
            if trimmed == "---" || trimmed == "***" {
                lines.push((self.theme.hr)(&"─".repeat(content_width.min(80))));
                lines.push(String::new());
                i += 1;
                continue;
            }
            // blockquote
            if trimmed.starts_with('>') {
                let content = trimmed.trim_start_matches('>').trim_start();
                let wrapped = wrap_text_with_ansi(
                    &self.render_inline(content),
                    content_width.saturating_sub(2).max(1),
                );
                for w in wrapped {
                    lines.push(format!(
                        "{}{}",
                        (self.theme.quote_border)("│ "),
                        (self.theme.quote)(&w)
                    ));
                }
                lines.push(String::new());
                i += 1;
                continue;
            }
            // unordered list
            if trimmed.starts_with("- ") || trimmed.starts_with("* ") {
                while i < src_lines.len() {
                    let item = src_lines[i].trim_start();
                    if !(item.starts_with("- ") || item.starts_with("* ")) {
                        break;
                    }
                    let text = &item[2..];
                    let bullet = (self.theme.list_bullet)("- ");
                    let cont_indent = " ".repeat(2);
                    let wrapped = wrap_text_with_ansi(
                        &self.render_inline(text),
                        content_width.saturating_sub(2).max(1),
                    );
                    for (idx, w) in wrapped.iter().enumerate() {
                        if idx == 0 {
                            lines.push(format!("{}{}", bullet, w));
                        } else {
                            lines.push(format!("{}{}", cont_indent, w));
                        }
                    }
                    i += 1;
                }
                continue;
            }
            // ordered list "1. "
            let is_ordered = {
                let mut it = trimmed.splitn(2, ' ');
                match (it.next(), it.next()) {
                    (Some(head), Some(_)) => {
                        let bytes = head.as_bytes();
                        bytes.len() >= 2
                            && bytes[..bytes.len() - 1].iter().all(|b| b.is_ascii_digit())
                            && (bytes[bytes.len() - 1] == b'.' || bytes[bytes.len() - 1] == b')')
                    }
                    _ => false,
                }
            };
            if is_ordered {
                while i < src_lines.len() {
                    let item = src_lines[i].trim_start();
                    let mut it = item.splitn(2, ' ');
                    let head = it.next().unwrap_or_default();
                    let body = it.next().unwrap_or_default();
                    if !head.ends_with('.') && !head.ends_with(')') {
                        break;
                    }
                    if !head[..head.len().saturating_sub(1)]
                        .chars()
                        .all(|c| c.is_ascii_digit())
                    {
                        break;
                    }
                    let bullet = (self.theme.list_bullet)(&format!("{} ", head));
                    let cont_indent = " ".repeat(visible_width(head) + 1);
                    let wrapped = wrap_text_with_ansi(
                        &self.render_inline(body),
                        content_width.saturating_sub(visible_width(&bullet)).max(1),
                    );
                    for (idx, w) in wrapped.iter().enumerate() {
                        if idx == 0 {
                            lines.push(format!("{}{}", bullet, w));
                        } else {
                            lines.push(format!("{}{}", cont_indent, w));
                        }
                    }
                    i += 1;
                }
                continue;
            }
            // space / empty
            if trimmed.is_empty() {
                lines.push(String::new());
                i += 1;
                continue;
            }
            // paragraph：收集到空行或下一个块级标记
            let mut para = vec![line.to_string()];
            i += 1;
            while i < src_lines.len() {
                let next = src_lines[i];
                let nt = next.trim_start();
                if nt.is_empty()
                    || nt.starts_with('#')
                    || nt.starts_with("```")
                    || nt.starts_with("~~~")
                    || nt.starts_with("> ")
                    || nt.starts_with("- ")
                    || nt.starts_with("* ")
                    || nt == "---"
                {
                    break;
                }
                para.push(next.to_string());
                i += 1;
            }
            let joined = self.render_inline(&para.join("\n"));
            for w in wrap_text_with_ansi(&joined, content_width) {
                lines.push(w);
            }
            lines.push(String::new());
        }
        // 去掉末尾多余空行（pi 的段落间空行保留一个即可）
        while lines.last().map(|l| l.is_empty()).unwrap_or(false) {
            lines.pop();
        }
        lines
    }
}

impl Component for Markdown {
    fn render(&mut self, width: usize) -> Vec<String> {
        if let Some((ref text, w, ref lines)) = self.cache
            && *text == self.text
            && w == width
        {
            return lines.clone();
        }
        let content_width = width.saturating_sub(self.padding_x * 2).max(1);
        let mut rendered = Vec::new();
        if !self.text.trim().is_empty() {
            rendered = self.render_blocks(&self.text, content_width);
        }
        let left = " ".repeat(self.padding_x);
        let right = " ".repeat(self.padding_x);
        let mut result = Vec::new();
        for _ in 0..self.padding_y {
            result.push(" ".repeat(width));
        }
        for line in rendered {
            let with_margins = format!("{}{}{}", left, line, right);
            let visible = visible_width(&with_margins);
            result.push(format!(
                "{}{}",
                with_margins,
                " ".repeat(width.saturating_sub(visible))
            ));
        }
        for _ in 0..self.padding_y {
            result.push(" ".repeat(width));
        }
        self.cache = Some((self.text.clone(), width, result.clone()));
        result
    }
    fn invalidate(&mut self) {
        self.cache = None;
    }
}

#[cfg(test)]
mod tests {
    #![allow(non_snake_case)]

    use super::*;
    use crate::ansi::strip_terminal_sequences;

    #[test]
    fn spec_20260822_agent_tui_pi_parity__markdown_heading_and_paragraph() {
        let mut md = Markdown::new("# Title\n\nhello world", 0, 0);
        let lines = md.render(40);
        let plain: Vec<String> = lines
            .iter()
            .map(|l| strip_terminal_sequences(l).trim_end().to_string())
            .collect();
        assert!(plain[0].contains("Title"));
        assert_eq!(plain[1], "");
        assert_eq!(plain[2], "hello world");
    }

    #[test]
    fn spec_20260822_agent_tui_pi_parity__markdown_code_fence_styled() {
        let mut md = Markdown::new("```rust\nfn main() {}\n```", 0, 0);
        let lines = md.render(40);
        let plain: Vec<String> = lines
            .iter()
            .map(|l| strip_terminal_sequences(l).trim_end().to_string())
            .collect();
        assert!(plain[0].starts_with("```rust"));
        assert!(plain[1].contains("fn main() {}"));
        assert_eq!(plain[2], "```");
    }

    #[test]
    fn spec_20260822_agent_tui_pi_parity__markdown_list_bullets_and_wrap() {
        let mut md = Markdown::new(
            "- first\n- second item that is quite long and should wrap nicely",
            0,
            0,
        );
        let lines = md.render(30);
        let plain: Vec<String> = lines
            .iter()
            .map(|l| strip_terminal_sequences(l).trim_end().to_string())
            .collect();
        assert!(plain[0].starts_with("- first"));
        assert!(plain[1].starts_with("- second"));
        // 续行有缩进
        assert!(plain[2].starts_with("  "));
        assert!(lines.iter().all(|l| visible_width(l) <= 30));
    }

    #[test]
    fn spec_20260822_agent_tui_pi_parity__markdown_inline_styles() {
        let mut md = Markdown::new("**bold** and `code`", 0, 0);
        let lines = md.render(60);
        assert!(lines[0].contains("\x1b[1mbold\x1b[22m"));
        assert!(strip_terminal_sequences(&lines[0]).contains("and code"));
    }

    #[test]
    fn spec_20260822_agent_tui_pi_parity__markdown_padding_applied() {
        let mut md = Markdown::new("text", 2, 1);
        let lines = md.render(20);
        // 顶部 1 空行
        assert_eq!(visible_width(&lines[0]), 20);
        // 内容行：左 padding 2 空格 + 文本
        assert!(strip_terminal_sequences(&lines[1]).starts_with("  text"));
    }
}

#[test]
fn probe_inline_only() {
    use crate::ansi::strip_terminal_sequences;
    let md = Markdown::new("", 0, 0);
    let cases = [
        "熟悉 gpui",
        "（在 ~/.cargo",
        "熟悉 gpui / gpui_platform（在 ~/.cargo/git/checkouts/zed-*/）",
        "执行 cargo check",
    ];
    for c in cases {
        let out = md.render_inline(c);
        assert_eq!(
            strip_terminal_sequences(&out),
            c,
            "inline 丢字: {c:?} -> {out:?}"
        );
    }
}

#[test]
fn probe_blocks_only() {
    use crate::ansi::strip_terminal_sequences;
    let md = Markdown::new("", 0, 0);
    let c = "熟悉 gpui / gpui_platform（在 ~/.cargo/git/checkouts/zed-*/）";
    let out = md.render_blocks(c, 80);
    let joined: String = out.iter().map(|l| strip_terminal_sequences(l)).collect();
    assert!(joined.contains("熟悉"), "blocks 丢字: {joined:?}");
    assert!(joined.contains("在"), "blocks 丢字: {joined:?}");
}
#[cfg(test)]
mod markdown_char_loss_tests {
    #![allow(non_snake_case)]
    use super::*;
    use crate::ansi::strip_terminal_sequences;

    /// 回归：render_inline 曾用 len_utf8() 步进 Vec<char> 下标（字符/字节单位混用），
    /// CJK 一次跳 3 个下标吞掉后续字符；且游离 `*` 会与行内任意 `*` 错误配对。
    #[test]
    fn spec_20260822_agent_tui_pi_parity__inline_keeps_cjk_around_ascii_boundaries() {
        let md = Markdown::new("", 0, 0);
        let cases = [
            // (源文本, 渲染后的可见文本——合法强调消耗标记符)
            ("熟悉 gpui", "熟悉 gpui"),
            ("（在 ~/.cargo", "（在 ~/.cargo"),
            ("**加粗** 后续文本", "加粗 后续文本"),
            ("**加粗**中文", "加粗中文"),
            ("2 * 3 与 4 * 5", "2 * 3 与 4 * 5"),
            (
                "路径 zed-* 与 *配对* 星号，以及 `未闭合反引号 的行",
                "路径 zed-* 与 配对 星号，以及 `未闭合反引号 的行",
            ),
            (
                "执行 cargo check / test / fmt，并跑 scripts/check-architecture.sh",
                "执行 cargo check / test / fmt，并跑 scripts/check-architecture.sh",
            ),
        ];
        for (src, expected) in cases {
            let out = md.render_inline(src);
            assert_eq!(
                strip_terminal_sequences(&out),
                expected,
                "inline 丢字: {src:?} -> {out:?}"
            );
        }
    }

    /// 端到端：整段 Markdown 渲染后可见文本不丢字（忽略换行与补白空格）
    #[test]
    fn spec_20260822_agent_tui_pi_parity__markdown_rendering_preserves_characters() {
        let cases = [
            "熟悉 gpui / gpui_platform（在 ~/.cargo/git/checkouts/zed-*/）",
            "- 会日语 Skill：\n- terminal-pty-capture 端到端调试",
            "### 3. 工程纪律",
            "Git 操作检查 git diff，需要权限",
            "帮我看看这个 bug 为什么复现；给 XX 功能加个设置项",
        ];
        for case in cases {
            let mut md = Markdown::new(case, 0, 1);
            let out = md.render(80);
            let src_plain: String = case.chars().filter(|c| !c.is_whitespace()).collect();
            let out_plain: String = out
                .iter()
                .map(|l| strip_terminal_sequences(l))
                .collect::<String>()
                .chars()
                .filter(|c| !c.is_whitespace())
                .collect();
            assert_eq!(
                src_plain, out_plain,
                "渲染丢字! 源: {case:?}\n输出: {out:?}"
            );
        }
    }
}
