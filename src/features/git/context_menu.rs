//! Git 变更列表的上下文菜单动作与条目。

use crate::shared::i18n;
use crossh_ui_component::context_menu::{MenuEntry, MenuItem};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum GitMenuAction {
    StageSelected,
    UnstageSelected,
    DiscardSelected,
    SelectAll,
    ClearSelection,
}

pub(crate) fn menu_entries(
    selected_count: usize,
    stage_count: usize,
    unstage_count: usize,
    can_discard: bool,
) -> Vec<MenuEntry<GitMenuAction>> {
    vec![
        MenuEntry::SectionHeader(i18n::text("git.selection")),
        MenuEntry::Item(MenuItem {
            id: "git-stage-selected".into(),
            label: rust_i18n::t!("git.stage_selected", count = stage_count).to_string(),
            shortcut_hint: None,
            disabled: stage_count == 0,
            danger: false,
            action: GitMenuAction::StageSelected,
        }),
        MenuEntry::Item(MenuItem {
            id: "git-unstage-selected".into(),
            label: rust_i18n::t!("git.unstage_selected", count = unstage_count).to_string(),
            shortcut_hint: None,
            disabled: unstage_count == 0,
            danger: false,
            action: GitMenuAction::UnstageSelected,
        }),
        MenuEntry::Separator,
        MenuEntry::Item(MenuItem {
            id: "git-select-all".into(),
            label: i18n::text("git.select_all"),
            shortcut_hint: Some(select_all_shortcut().into()),
            disabled: false,
            danger: false,
            action: GitMenuAction::SelectAll,
        }),
        MenuEntry::Item(MenuItem {
            id: "git-clear-selection".into(),
            label: i18n::text("git.clear_selection"),
            shortcut_hint: None,
            disabled: selected_count == 0,
            danger: false,
            action: GitMenuAction::ClearSelection,
        }),
        MenuEntry::Separator,
        MenuEntry::Item(MenuItem {
            id: "git-discard-selected".into(),
            label: i18n::text("git.discard_selected"),
            shortcut_hint: None,
            disabled: !can_discard,
            danger: true,
            action: GitMenuAction::DiscardSelected,
        }),
    ]
}

#[cfg(target_os = "macos")]
const fn select_all_shortcut() -> &'static str {
    "⌘A"
}

#[cfg(not(target_os = "macos"))]
const fn select_all_shortcut() -> &'static str {
    "Ctrl+A"
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item<'a>(entries: &'a [MenuEntry<GitMenuAction>], id: &str) -> &'a MenuItem<GitMenuAction> {
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
    fn selection_actions_follow_the_selected_stage_states() {
        let entries = menu_entries(3, 2, 1, false);

        assert!(!item(&entries, "git-stage-selected").disabled);
        assert!(!item(&entries, "git-unstage-selected").disabled);
        assert!(item(&entries, "git-discard-selected").disabled);
        assert!(matches!(
            item(&entries, "git-stage-selected").action,
            GitMenuAction::StageSelected
        ));
    }
}
