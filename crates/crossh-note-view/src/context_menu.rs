//! 编辑器右键菜单：条目定义、快捷键提示与动作分发。
//!
//! 从 `window.rs` 拆出——菜单内容只依赖编辑器的 context-menu 能力探测结果，
//! 与窗口的列表/脏保护状态无关，单独成文件后 `window.rs` 仍保持在
//! `scripts/check-architecture.sh` 的 2000 行上限内。

use crossh_core::i18n::text;
use crossh_ui_component::context_menu::{ContextMenuState, MenuEntry, MenuItem};
use gpui::{Context, Pixels, Point, Window};

use super::window::NoteWindow;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum NoteMenuAction {
    Copy,
    Cut,
    Paste,
    SelectAll,
    Undo,
    Redo,
}

/// macOS 用 ⌘ 系提示，其它平台沿用 Ctrl 系（与 `lib.rs` 的键位表一致）。
///
/// 早前六个菜单项各写一遍 `if cfg!(target_os = "macos") { "⌘C" } else { "Ctrl+C" }`，
/// 同一份平台判断重复了十二次；改一处键位得改十二行，漏一行就出现两个平台提示不一致。
fn primary_shortcut(key: &str) -> String {
    if cfg!(target_os = "macos") {
        format!("⌘{key}")
    } else {
        format!("Ctrl+{key}")
    }
}

/// 重做的提示不能套 `primary_shortcut`：macOS 是 ⇧⌘Z，Linux 惯例是 Ctrl+Y，
/// 字母本身就不同（与 `lib.rs` 的 `KeyBinding` 一致）。
fn redo_shortcut() -> String {
    if cfg!(target_os = "macos") {
        "⇧⌘Z".to_string()
    } else {
        "Ctrl+Y".to_string()
    }
}

impl NoteWindow {
    pub(super) fn open_context_menu(
        &mut self,
        position: Point<Pixels>,
        cap: crossh_editor::input::InputContextMenuCapabilities,
        cx: &mut Context<Self>,
    ) {
        let has_selection = cap.has_selection();
        let editable = cap.is_editable();
        let item = |id: &'static str,
                    label: String,
                    shortcut_hint: String,
                    disabled: bool,
                    action: NoteMenuAction| {
            MenuEntry::Item(MenuItem {
                id: id.to_string(),
                label,
                shortcut_hint: Some(shortcut_hint),
                disabled,
                danger: false,
                action,
            })
        };
        let entries = vec![
            item(
                "copy",
                text("note.menu.copy"),
                primary_shortcut("C"),
                !has_selection,
                NoteMenuAction::Copy,
            ),
            item(
                "cut",
                text("note.menu.cut"),
                primary_shortcut("X"),
                !has_selection || !editable,
                NoteMenuAction::Cut,
            ),
            item(
                "paste",
                text("note.menu.paste"),
                primary_shortcut("V"),
                !editable,
                NoteMenuAction::Paste,
            ),
            MenuEntry::Separator,
            item(
                "select-all",
                text("note.menu.select_all"),
                primary_shortcut("A"),
                false,
                NoteMenuAction::SelectAll,
            ),
            MenuEntry::Separator,
            item(
                "undo",
                text("note.menu.undo"),
                primary_shortcut("Z"),
                !editable,
                NoteMenuAction::Undo,
            ),
            item(
                "redo",
                text("note.menu.redo"),
                redo_shortcut(),
                !editable,
                NoteMenuAction::Redo,
            ),
        ];
        self.context_menu = Some(ContextMenuState { position, entries });
        cx.notify();
    }

    pub(super) fn close_context_menu(&mut self, cx: &mut Context<Self>) {
        if self.context_menu.take().is_some() {
            cx.notify();
        }
    }

    /// 菜单项动作转成编辑器动作：菜单本身不实现编辑语义，
    /// 全部下发给 `crossh-editor`（与快捷键走同一条路径，行为一致）。
    pub(super) fn handle_context_menu_action(
        &mut self,
        action: NoteMenuAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.close_context_menu(cx);
        match action {
            NoteMenuAction::Copy => {
                window.dispatch_action(Box::new(crossh_editor::input::Copy), cx)
            }
            NoteMenuAction::Cut => window.dispatch_action(Box::new(crossh_editor::input::Cut), cx),
            NoteMenuAction::Paste => {
                window.dispatch_action(Box::new(crossh_editor::input::Paste), cx)
            }
            NoteMenuAction::SelectAll => {
                window.dispatch_action(Box::new(crossh_editor::input::SelectAll), cx)
            }
            NoteMenuAction::Undo => {
                window.dispatch_action(Box::new(crossh_editor::input::Undo), cx)
            }
            NoteMenuAction::Redo => {
                window.dispatch_action(Box::new(crossh_editor::input::Redo), cx)
            }
        }
    }
}
