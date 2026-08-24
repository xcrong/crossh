use gpui::{
    App, FontWeight, IntoElement, ParentElement, RenderOnce, SharedString, Styled, div, px,
};

use crate::theme;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum AvatarKind {
    #[default]
    Project,
    Host,
    Command,
}

/// A compact text identity for entities displayed in workspace rails.
#[derive(IntoElement)]
pub struct Avatar {
    label: SharedString,
    kind: AvatarKind,
}

impl Avatar {
    pub fn new(value: &str) -> Self {
        Self {
            label: SharedString::from(abbreviation(value)),
            kind: AvatarKind::default(),
        }
    }

    pub fn kind(mut self, kind: AvatarKind) -> Self {
        self.kind = kind;
        self
    }
}

impl RenderOnce for Avatar {
    fn render(self, _window: &mut gpui::Window, _cx: &mut App) -> impl IntoElement {
        let color = match self.kind {
            AvatarKind::Project => theme::accent(),
            AvatarKind::Host => theme::info(),
            AvatarKind::Command => theme::muted_text(),
        };
        div()
            .w(px(30.))
            .h(px(30.))
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(theme::RADIUS_SM))
            .border_1()
            .border_color(theme::border_strong())
            .bg(theme::raised())
            .text_xs()
            .font_weight(FontWeight::MEDIUM)
            .text_color(color)
            .child(self.label)
    }
}

fn abbreviation(value: &str) -> String {
    let token = value.trim().trim_start_matches(['.', '/']);
    if token.is_empty() {
        return "--".into();
    }
    // 同名去重标签 "crossh · Code" 使用中间点分隔，取两侧首字母保证同名不同父目录可区分
    if token.contains('·') {
        let parts: Vec<&str> = token
            .split('·')
            .map(|segment| segment.trim())
            .filter(|segment| !segment.is_empty())
            .collect();
        if parts.len() >= 2 {
            let first = parts[0]
                .chars()
                .find(|character| !character.is_whitespace());
            let last = parts
                .last()
                .and_then(|segment| segment.chars().find(|character| !character.is_whitespace()));
            if let (Some(first), Some(last)) = (first, last) {
                return format!("{first}{last}");
            }
        }
    }
    // 含空白的视为命令类（如 "du -sh ."），保持原有取前 2 非空白字符的逻辑
    if token.chars().any(|character| character.is_whitespace()) {
        let mut chars = token.chars().filter(|character| !character.is_whitespace());
        let label: String = chars.by_ref().take(2).collect();
        return if label.is_empty() { "--".into() } else { label };
    }
    // 无空白且含分隔符的项目名取首段首字母 + 末段首字母，解决 zaiwenai / zaiwenai-openapi 前缀冲突
    let delimiters = ['-', '_', '.', '+'];
    let segments: Vec<&str> = token
        .split(|character| delimiters.contains(&character))
        .filter(|segment| !segment.is_empty())
        .collect();
    if segments.len() >= 2
        && let (Some(first), Some(last)) = (
            segments[0].chars().next(),
            segments.last().and_then(|segment| segment.chars().next()),
        )
    {
        return format!("{first}{last}");
    }
    let mut chars = token.chars().filter(|character| !character.is_whitespace());
    let label: String = chars.by_ref().take(2).collect();
    if label.is_empty() { "--".into() } else { label }
}

#[cfg(test)]
mod tests {
    use super::abbreviation;

    #[test]
    fn abbreviations_are_compact_and_stable() {
        assert_eq!(abbreviation("crossh"), "cr");
        assert_eq!(abbreviation("du -sh ."), "du");
        assert_eq!(abbreviation("./script"), "sc");
    }

    #[test]
    fn abbreviation_uses_segment_initials_for_kebab_names() {
        assert_eq!(abbreviation("zaiwenai-openapi"), "zo");
        assert_eq!(abbreviation("zaiwenai"), "za");
        assert_eq!(abbreviation("my_project"), "mp");
        assert_eq!(abbreviation("crossh"), "cr");
    }

    #[test]
    fn abbreviation_distinguishes_duplicate_names_via_parent() {
        assert_eq!(abbreviation("crossh · Code"), "cC");
        assert_eq!(abbreviation("crossh · Work"), "cW");
        assert_ne!(abbreviation("crossh · Code"), abbreviation("crossh · Work"));
    }
}
