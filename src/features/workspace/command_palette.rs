//! Command K 基础面板：静态命令列表 + 模糊过滤 + 键盘导航。

use gpui::prelude::FluentBuilder;
use gpui::{
    AnyElement, ClipboardEntry, ClipboardItem, Context, FocusHandle, InteractiveElement,
    IntoElement, KeyDownEvent, ParentElement, ScrollHandle, SharedString,
    StatefulInteractiveElement, Styled, Window, div, px,
};

use crate::shared::i18n;
use crate::shared::text_editing::{EditingKeystroke, TextEditingState, handle_text_editing_key};
use crossh_ui::{icons, theme};
use crossh_ui_component::{ModalField, SharedTextState};

use super::AppShell;

/// 面板内单条命令的静态定义。
#[derive(Clone, Debug)]
pub(crate) struct PaletteCommand {
    pub id: &'static str,
    pub label: String,
    pub icon: icons::IconName,
    pub kind: PaletteCommandKind,
    pub shortcut: Option<String>,
}

/// 面板可执行的最小命令集（MVP）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PaletteCommandKind {
    NewTerminal,
    OpenProject,
    CloseActiveTab,
    ToggleHostSidebar,
    ToggleTimestamps,
    ToggleScratchTerminal,
    OpenSettings,
}

/// 面板状态：输入 + 选中 + 焦点。
pub(crate) struct CommandPaletteState {
    pub query: TextEditingState,
    pub focus: FocusHandle,
    pub selected: usize,
    pub scroll: ScrollHandle,
}

impl CommandPaletteState {
    pub(crate) fn new(focus: FocusHandle) -> Self {
        Self {
            query: TextEditingState::new(String::new()),
            focus,
            selected: 0,
            scroll: ScrollHandle::new(),
        }
    }

    /// 当前过滤后的命令列表（大小写不敏感的子串匹配）。
    pub(crate) fn filtered(&self) -> Vec<PaletteCommand> {
        let query = self.query.value.trim().to_lowercase();
        let all = palette_commands();
        if query.is_empty() {
            return all;
        }
        all.into_iter()
            .filter(|cmd| {
                cmd.label.to_lowercase().contains(&query) || cmd.id.to_lowercase().contains(&query)
            })
            .collect()
    }

    pub(crate) fn move_selection(&mut self, delta: i32) {
        let len = self.filtered().len();
        if len == 0 {
            self.selected = 0;
            return;
        }
        let next = (self.selected as i32 + delta).rem_euclid(len as i32);
        self.selected = next as usize;
    }

    pub(crate) fn clamp_selection(&mut self) {
        let len = self.filtered().len();
        if len == 0 {
            self.selected = 0;
        } else if self.selected >= len {
            self.selected = len - 1;
        }
    }
}

/// 静态命令列表（MVP）。
pub(crate) fn palette_commands() -> Vec<PaletteCommand> {
    vec![
        PaletteCommand {
            id: "new_terminal",
            label: i18n::text("app_menu.new_terminal"),
            icon: icons::IconName::Terminal,
            kind: PaletteCommandKind::NewTerminal,
            shortcut: Some("⌘T".into()),
        },
        PaletteCommand {
            id: "open_project",
            label: i18n::text("app_menu.open_project"),
            icon: icons::IconName::FolderOpen,
            kind: PaletteCommandKind::OpenProject,
            shortcut: Some("⌘O".into()),
        },
        PaletteCommand {
            id: "close_tab",
            label: i18n::text("app_menu.close_tab"),
            icon: icons::IconName::X,
            kind: PaletteCommandKind::CloseActiveTab,
            shortcut: Some("⌘W".into()),
        },
        PaletteCommand {
            id: "toggle_host_sidebar",
            label: i18n::text("app_menu.toggle_host_sidebar"),
            icon: icons::IconName::PanelLeft,
            kind: PaletteCommandKind::ToggleHostSidebar,
            shortcut: None,
        },
        PaletteCommand {
            id: "toggle_timestamps",
            label: i18n::text("app_menu.toggle_timestamps"),
            icon: icons::IconName::Clock,
            kind: PaletteCommandKind::ToggleTimestamps,
            shortcut: None,
        },
        PaletteCommand {
            id: "toggle_scratch_terminal",
            label: i18n::text("tooltip.scratch_terminal"),
            icon: icons::IconName::Terminal,
            kind: PaletteCommandKind::ToggleScratchTerminal,
            shortcut: None,
        },
        PaletteCommand {
            id: "open_settings",
            label: i18n::text("app_menu.settings"),
            icon: icons::IconName::Settings,
            kind: PaletteCommandKind::OpenSettings,
            shortcut: Some("⌘,".into()),
        },
    ]
}

impl AppShell {
    pub(crate) fn toggle_command_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.command_palette.is_some() {
            self.close_command_palette(cx);
        } else {
            self.open_command_palette(window, cx);
        }
    }

    pub(crate) fn open_command_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // 与其他模态互斥：先关闭 rename/default
        if self.rename_editor.take().is_some() || self.default_command_editor.take().is_some() {
            // 已关闭
        }
        let focus = cx.focus_handle();
        let state = CommandPaletteState::new(focus.clone());
        self.command_palette = Some(state);
        window.focus(&focus, cx);
        cx.notify();
    }

    pub(crate) fn close_command_palette(&mut self, cx: &mut Context<Self>) {
        if self.command_palette.take().is_some() {
            cx.notify();
        }
    }

    pub(crate) fn execute_palette_selected(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(palette) = self.command_palette.as_ref() else {
            return;
        };
        let filtered = palette.filtered();
        let Some(cmd) = filtered.get(palette.selected).cloned() else {
            return;
        };
        let kind = cmd.kind;
        self.close_command_palette(cx);
        self.execute_palette_kind(kind, window, cx);
    }

    fn execute_palette_kind(
        &mut self,
        kind: PaletteCommandKind,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match kind {
            PaletteCommandKind::NewTerminal => {
                self.handle_new_terminal(&crate::NewTerminal, window, cx);
            }
            PaletteCommandKind::OpenProject => {
                self.choose_project_directory(cx);
            }
            PaletteCommandKind::CloseActiveTab => {
                self.handle_close_active_tab(&crate::CloseActiveTab, window, cx);
            }
            PaletteCommandKind::ToggleHostSidebar => {
                self.toggle_host_sidebar(cx);
            }
            PaletteCommandKind::ToggleTimestamps => {
                self.toggle_timestamps(cx);
            }
            PaletteCommandKind::ToggleScratchTerminal => {
                self.toggle_scratch_terminal(window, cx);
            }
            PaletteCommandKind::OpenSettings => {
                self.toggle_settings(cx);
            }
        }
    }

    pub(crate) fn handle_command_palette_key(
        &mut self,
        ev: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.command_palette.is_none() {
            return;
        }
        let key = ev.keystroke.key.as_str();
        match key {
            "escape" => {
                self.close_command_palette(cx);
                return;
            }
            "enter" | "return" => {
                // 需先释放可变借用再执行
                let has_selection = {
                    if let Some(p) = self.command_palette.as_ref() {
                        !p.filtered().is_empty()
                    } else {
                        false
                    }
                };
                if has_selection {
                    self.execute_palette_selected(window, cx);
                }
                return;
            }
            "arrowup" | "up" => {
                if let Some(p) = self.command_palette.as_mut() {
                    p.move_selection(-1);
                }
                cx.notify();
                return;
            }
            "arrowdown" | "down" => {
                if let Some(p) = self.command_palette.as_mut() {
                    p.move_selection(1);
                }
                cx.notify();
                return;
            }
            _ => {}
        }

        // 文本编辑（包括 IME）
        let ks = &ev.keystroke;
        let primary = ks.modifiers.control || ks.modifiers.platform;
        let paste_text = if primary && ks.key == "v" {
            cx.read_from_clipboard().and_then(|item| {
                item.into_entries().find_map(|entry| match entry {
                    ClipboardEntry::String(value) => Some(value.text),
                    _ => None,
                })
            })
        } else {
            None
        };
        // 记录编辑前的 query 以检测是否变化后重置选中
        let before = self
            .command_palette
            .as_ref()
            .map(|p| p.query.value.clone())
            .unwrap_or_default();
        if let Some(palette) = self.command_palette.as_mut() {
            let editing_ks = EditingKeystroke {
                key: ks.key.clone(),
                key_char: ks.key_char.clone(),
                control: ks.modifiers.control,
                platform: ks.modifiers.platform,
                shift: ks.modifiers.shift,
            };
            let result =
                handle_text_editing_key(&mut palette.query, &editing_ks, paste_text.as_deref());
            if let Some(text) = result.copy_text {
                cx.write_to_clipboard(ClipboardItem::new_string(text));
            }
            if result.handled {
                // 查询变化后重置选中到首项
                if palette.query.value != before {
                    palette.selected = 0;
                }
                palette.clamp_selection();
                cx.notify();
            }
        }
    }
}

pub(crate) fn render_command_palette(
    shell: &mut AppShell,
    _window: &mut Window,
    cx: &mut Context<AppShell>,
) -> AnyElement {
    let Some(palette) = shell.command_palette.as_ref() else {
        return div().into_any_element();
    };
    let filtered = palette.filtered();
    let selected = palette.selected;
    let focus = palette.focus.clone();
    let scroll = palette.scroll.clone();
    let query_state = SharedTextState::new(palette.query.value.clone())
        .with_cursor(palette.query.cursor)
        .with_anchor(palette.query.anchor)
        .with_ime_marked_text(palette.query.ime_marked_text.clone())
        .with_ime_replacement(palette.query.ime_replacement);

    // 搜索输入
    let input = ModalField::new("command-palette-input", focus.clone(), &query_state)
        .placeholder("Type a command...".to_string())
        .entity(cx.entity())
        .scrollable(scroll)
        .on_key_down(
            cx.listener(|this, ev, window, cx| this.handle_command_palette_key(ev, window, cx)),
        );

    let list = if filtered.is_empty() {
        div()
            .flex_1()
            .flex()
            .items_center()
            .justify_center()
            .py_6()
            .text_sm()
            .text_color(theme::faint_text())
            .child(SharedString::from("No matching commands"))
            .into_any_element()
    } else {
        let mut container = div().flex_1().min_h_0().flex().flex_col().gap_1().p_2();
        container.style().overflow.y = Some(gpui::Overflow::Scroll);
        for (idx, cmd) in filtered.iter().enumerate() {
            let is_selected = idx == selected;
            let label = SharedString::from(cmd.label.clone());
            let shortcut = cmd
                .shortcut
                .clone()
                .map(SharedString::from)
                .unwrap_or_default();
            let icon = cmd.icon;
            let row = div()
                .id(SharedString::from(format!("palette-row-{idx}")))
                .flex_shrink_0()
                .flex()
                .items_center()
                .gap_2()
                .px_3()
                .h(px(36.))
                .rounded(px(theme::RADIUS_SM))
                .cursor_pointer()
                .when(is_selected, |el| el.bg(theme::accent_soft()))
                .hover(|s| s.bg(theme::raised()))
                .child(icons::icon(icon, 14.).text_color(if is_selected {
                    theme::accent()
                } else {
                    theme::muted_text()
                }))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_sm()
                        .text_color(theme::text())
                        .child(label),
                )
                .when(!shortcut.is_empty(), |el| {
                    el.child(
                        div()
                            .flex_shrink_0()
                            .text_xs()
                            .text_color(theme::faint_text())
                            .child(shortcut),
                    )
                })
                .on_click(cx.listener(move |this, _ev, window, cx| {
                    // 点击直接执行该项
                    if let Some(palette) = this.command_palette.as_mut() {
                        palette.selected = idx;
                    }
                    this.execute_palette_selected(window, cx);
                }));
            container = container.child(row);
        }
        container.into_any_element()
    };

    let header = div().flex_shrink_0().px_3().pt_3().pb_2().child(input);

    let card = div()
        .w(px(560.))
        .max_h(px(420.))
        .flex()
        .flex_col()
        .bg(theme::surface())
        .border_1()
        .border_color(theme::border_strong())
        .rounded(px(theme::RADIUS_MD))
        .shadow_lg()
        .child(header)
        .child(div().h(px(1.)).w_full().flex_shrink_0().bg(theme::border()))
        .child(list);

    div()
        .id("command-palette-scrim")
        .absolute()
        .inset_0()
        .flex()
        .items_start()
        .justify_center()
        .pt(px(120.))
        .bg(theme::scrim())
        .on_mouse_down(
            gpui::MouseButton::Left,
            cx.listener(|this, _ev, _window, cx| {
                this.close_command_palette(cx);
            }),
        )
        .child(
            div()
                .id("command-palette-card")
                .on_mouse_down(
                    gpui::MouseButton::Left,
                    cx.listener(|_, _ev, _window, cx| {
                        cx.stop_propagation();
                    }),
                )
                .child(card),
        )
        .into_any_element()
}
