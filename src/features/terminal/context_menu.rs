//! Terminal-owned context-menu actions and entry construction.

use crate::shared::i18n;
use crossh_ui::context_menu::{MenuEntry, MenuItem};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum TerminalMenuAction {
    Copy,
    Paste,
    SelectAll,
    OpenUrl(String),
}

pub(crate) fn menu_entries(
    connected: bool,
    has_selection: bool,
    hovered_word: Option<String>,
) -> Vec<MenuEntry<TerminalMenuAction>> {
    let mut entries = vec![
        MenuEntry::Item(MenuItem {
            id: "copy".into(),
            label: i18n::text("context_menu.copy"),
            shortcut_hint: Some(copy_shortcut().into()),
            disabled: !has_selection,
            danger: false,
            action: TerminalMenuAction::Copy,
        }),
        MenuEntry::Item(MenuItem {
            id: "paste".into(),
            label: i18n::text("context_menu.paste"),
            shortcut_hint: Some(paste_shortcut().into()),
            disabled: !connected,
            danger: false,
            action: TerminalMenuAction::Paste,
        }),
        MenuEntry::Item(MenuItem {
            id: "select-all".into(),
            label: i18n::text("context_menu.select_all"),
            shortcut_hint: Some(select_all_shortcut().into()),
            disabled: false,
            danger: false,
            action: TerminalMenuAction::SelectAll,
        }),
    ];

    if let Some(url) = hovered_word.filter(|word| is_openable_url(word)) {
        entries.push(MenuEntry::Separator);
        entries.push(MenuEntry::Item(MenuItem {
            id: "open-url".into(),
            label: i18n::text("context_menu.open_link"),
            shortcut_hint: None,
            disabled: false,
            danger: false,
            action: TerminalMenuAction::OpenUrl(url),
        }));
    }

    entries
}

fn is_openable_url(value: &str) -> bool {
    let value = value.trim();
    ["http://", "https://", "ftp://", "file://", "mailto:"]
        .iter()
        .any(|scheme| value.starts_with(scheme))
}

#[cfg(target_os = "macos")]
const fn copy_shortcut() -> &'static str {
    "⌘C"
}

#[cfg(not(target_os = "macos"))]
const fn copy_shortcut() -> &'static str {
    "Ctrl+Shift+C"
}

#[cfg(target_os = "macos")]
const fn paste_shortcut() -> &'static str {
    "⌘V"
}

#[cfg(not(target_os = "macos"))]
const fn paste_shortcut() -> &'static str {
    "Ctrl+Shift+V"
}

#[cfg(target_os = "macos")]
const fn select_all_shortcut() -> &'static str {
    "⌘A"
}

#[cfg(not(target_os = "macos"))]
const fn select_all_shortcut() -> &'static str {
    "Ctrl+Shift+A"
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item<'a>(
        entries: &'a [MenuEntry<TerminalMenuAction>],
        id: &str,
    ) -> &'a MenuItem<TerminalMenuAction> {
        entries
            .iter()
            .find_map(|entry| match entry {
                MenuEntry::Item(item) if item.id == id => Some(item),
                MenuEntry::CheckedItem { item, .. } if item.id == id => Some(item),
                _ => None,
            })
            .expect("menu item should exist")
    }

    #[test]
    fn clipboard_actions_follow_terminal_state() {
        let entries = menu_entries(false, false, None);

        assert!(item(&entries, "copy").disabled);
        assert!(item(&entries, "paste").disabled);
        assert!(!item(&entries, "select-all").disabled);

        let entries = menu_entries(true, true, None);
        assert!(!item(&entries, "copy").disabled);
        assert!(!item(&entries, "paste").disabled);
    }

    #[test]
    fn only_openable_urls_get_a_link_action() {
        let entries = menu_entries(true, false, Some("https://example.com".into()));
        assert!(matches!(
            entries.last(),
            Some(MenuEntry::Item(MenuItem {
                action: TerminalMenuAction::OpenUrl(url),
                ..
            })) if url == "https://example.com"
        ));

        let entries = menu_entries(true, false, Some("not a url".into()));
        assert!(!entries.iter().any(|entry| {
            matches!(
                entry,
                MenuEntry::Item(MenuItem {
                    action: TerminalMenuAction::OpenUrl(_),
                    ..
                })
            )
        }));
    }
}
