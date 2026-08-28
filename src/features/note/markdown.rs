//! Markdown 预览渲染：pulldown-cmark -> GPUI 元素

use gpui::{AnyElement, IntoElement, ParentElement, Styled, div, px};
use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};

use crossh_ui::theme;

pub fn render_markdown(md: &str) -> AnyElement {
    if md.trim().is_empty() {
        return div()
            .text_sm()
            .text_color(theme::muted_text())
            .child("预览为空")
            .into_any_element();
    }

    let mut opts = Options::empty();
    opts.insert(Options::all());
    let parser = Parser::new_ext(md, opts);

    let mut blocks: Vec<AnyElement> = Vec::new();
    let mut inline: Vec<String> = Vec::new();
    let mut in_code_block = false;
    let mut code_buf = String::new();

    // 用于列表/引用缩进（简单计数）
    let mut list_depth: usize = 0;

    for event in parser {
        match event {
            Event::Start(Tag::Paragraph) => {
                if in_code_block {
                    continue;
                }
                if !inline.is_empty() {
                    blocks.push(render_line(std::mem::take(&mut inline), false, list_depth));
                }
            }
            Event::End(TagEnd::Paragraph) => {
                if in_code_block {
                    continue;
                }
                if !inline.is_empty() {
                    blocks.push(render_line(std::mem::take(&mut inline), false, list_depth));
                }
            }
            Event::Start(Tag::Heading { level, .. }) => {
                if in_code_block {
                    continue;
                }
                if !inline.is_empty() {
                    blocks.push(render_line(std::mem::take(&mut inline), false, list_depth));
                }
                let lvl = match level {
                    pulldown_cmark::HeadingLevel::H1 => 1,
                    pulldown_cmark::HeadingLevel::H2 => 2,
                    pulldown_cmark::HeadingLevel::H3 => 3,
                    _ => 4,
                };
                inline.push(format!("#{} ", lvl));
            }
            Event::End(TagEnd::Heading(_)) => {
                if in_code_block {
                    continue;
                }
                blocks.push(render_line(std::mem::take(&mut inline), true, list_depth));
            }
            Event::Start(Tag::Strong) => {
                if !in_code_block {
                    inline.push("**".to_string());
                }
            }
            Event::End(TagEnd::Strong) => {
                if !in_code_block {
                    inline.push("**".to_string());
                }
            }
            Event::Start(Tag::Emphasis) => {
                if !in_code_block {
                    inline.push("*".to_string());
                }
            }
            Event::End(TagEnd::Emphasis) => {
                if !in_code_block {
                    inline.push("*".to_string());
                }
            }
            Event::Start(Tag::CodeBlock(_)) => {
                if !inline.is_empty() {
                    blocks.push(render_line(std::mem::take(&mut inline), false, list_depth));
                }
                in_code_block = true;
                code_buf.clear();
            }
            Event::End(TagEnd::CodeBlock) => {
                in_code_block = false;
                let code = std::mem::take(&mut code_buf);
                if !code.trim().is_empty() || !code.is_empty() {
                    blocks.push(render_code_block(code, list_depth));
                }
            }
            Event::Start(Tag::BlockQuote) => {
                if !in_code_block && !inline.is_empty() {
                    blocks.push(render_line(std::mem::take(&mut inline), false, list_depth));
                }
                // 引用用缩进表示
                list_depth = list_depth.saturating_add(1);
            }
            Event::End(TagEnd::BlockQuote) => {
                if !in_code_block && !inline.is_empty() {
                    blocks.push(render_line(std::mem::take(&mut inline), false, list_depth));
                }
                list_depth = list_depth.saturating_sub(1);
            }
            Event::Start(Tag::List(_)) => {
                if !in_code_block && !inline.is_empty() {
                    blocks.push(render_line(std::mem::take(&mut inline), false, list_depth));
                }
                list_depth = list_depth.saturating_add(1);
            }
            Event::End(TagEnd::List(_)) => {
                if !in_code_block && !inline.is_empty() {
                    blocks.push(render_line(std::mem::take(&mut inline), false, list_depth));
                }
                list_depth = list_depth.saturating_sub(1);
            }
            Event::Start(Tag::Item) => {
                if in_code_block {
                    continue;
                }
                if !inline.is_empty() {
                    blocks.push(render_line(std::mem::take(&mut inline), false, list_depth));
                }
                inline.push("• ".to_string());
            }
            Event::End(TagEnd::Item) => {
                if in_code_block {
                    continue;
                }
                // list_depth 在 Item 结束时仍为当前列表层级，用它作为缩进
                let indent = list_depth.max(1);
                blocks.push(render_line(std::mem::take(&mut inline), false, indent));
            }
            Event::Text(text) => {
                if in_code_block {
                    code_buf.push_str(&text);
                } else {
                    inline.push(text.to_string());
                }
            }
            Event::Code(code) => {
                if in_code_block {
                    code_buf.push_str(&code);
                } else {
                    inline.push(format!("`{}`", code));
                }
            }
            Event::Html(html) => {
                if in_code_block {
                    code_buf.push_str(&html);
                } else {
                    inline.push(html.to_string());
                }
            }
            Event::InlineHtml(html) => {
                if in_code_block {
                    code_buf.push_str(&html);
                } else {
                    inline.push(html.to_string());
                }
            }
            Event::SoftBreak | Event::HardBreak => {
                if in_code_block {
                    code_buf.push('\n');
                } else {
                    inline.push(" ".to_string());
                }
            }
            Event::Start(Tag::HtmlBlock) => {
                if !in_code_block && !inline.is_empty() {
                    blocks.push(render_line(std::mem::take(&mut inline), false, list_depth));
                }
            }
            Event::End(TagEnd::HtmlBlock) => {
                if !in_code_block && !inline.is_empty() {
                    blocks.push(render_line(std::mem::take(&mut inline), false, list_depth));
                }
            }
            Event::FootnoteReference(_) => {}
            Event::TaskListMarker(_) => {}
            Event::Rule => {
                if in_code_block {
                    continue;
                }
                if !inline.is_empty() {
                    blocks.push(render_line(std::mem::take(&mut inline), false, list_depth));
                }
                blocks.push(
                    div()
                        .h(px(1.))
                        .w_full()
                        .bg(theme::border())
                        .my_2()
                        .into_any_element(),
                );
            }
            _ => {}
        }
    }
    if in_code_block && !code_buf.is_empty() {
        blocks.push(render_code_block(code_buf, list_depth));
    } else if !inline.is_empty() {
        blocks.push(render_line(inline, false, list_depth));
    }

    div()
        .flex()
        .flex_col()
        .gap_2()
        .children(blocks)
        .into_any_element()
}

fn render_line(parts: Vec<String>, is_heading: bool, indent: usize) -> AnyElement {
    let text = parts.join("");
    if text.trim().is_empty() {
        return div().h(px(4.)).into_any_element();
    }
    let mut el = div()
        .w_full()
        .text_sm()
        .text_color(theme::text())
        .child(text);
    if is_heading {
        el = el
            .text_color(theme::accent())
            .font_weight(gpui::FontWeight::BOLD);
    }
    if indent > 0 {
        el = el.ml(px((indent * 12) as f32));
    }
    el.into_any_element()
}

fn render_code_block(code: String, indent: usize) -> AnyElement {
    let trimmed = code.trim_end().to_string();
    if trimmed.is_empty() {
        return div().h(px(4.)).into_any_element();
    }
    let mut el = div()
        .w_full()
        .p_2()
        .my_1()
        .bg(theme::surface())
        .border_1()
        .border_color(theme::border())
        .rounded(px(theme::RADIUS_SM))
        .child(
            div()
                .w_full()
                .text_xs()
                .text_color(theme::text())
                .child(trimmed),
        );
    if indent > 0 {
        el = el.ml(px((indent * 12) as f32));
    }
    el.into_any_element()
}
