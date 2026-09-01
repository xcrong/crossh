//! Note 窗口 — 基于 crossh-editor TextareaState/InputState

use std::rc::Rc;

use crossh_editor::input::{InputEvent, InputState, TextareaState};
use crossh_editor::{Input, Textarea};
use crossh_note::{Note, NoteStore};
use crossh_ui::{icons, theme};
use crossh_ui_component::{
    BadgeTone, Button, ButtonSize, ButtonVariant, StatusBar, StatusMetric,
    context_menu::{ContextMenuState, MenuEntry, MenuItem, render_context_menu},
};
use gpui::{
    AnyElement, App, AppContext, Bounds, Context, Entity, FocusHandle, Focusable, Hsla,
    InteractiveElement, IntoElement, ParentElement, Pixels, Point, ScrollHandle, SharedString,
    Size, StatefulInteractiveElement, Styled, Subscription, TitlebarOptions, Window, WindowBounds,
    WindowOptions, div, point, px,
};

use super::markdown::render_markdown;
use super::{CloseNoteWindow, DeleteNote, NewNote, SaveNote, TogglePreview};

const NOTE_WINDOW_CONTEXT: &str = "NoteWindow";

fn sync_editor_theme(cx: &mut App) {
    let theme = crossh_editor::theme::Theme::global_mut(cx);
    theme.appearance = crossh_editor::theme::ThemeAppearance::Dark;
    let c = &mut theme.tokens.colors;
    c.background = Hsla::from(theme::canvas());
    c.foreground = Hsla::from(theme::text());
    c.surface = Hsla::from(theme::surface());
    c.surface_foreground = Hsla::from(theme::text());
    c.muted = Hsla::from(theme::surface());
    c.muted_foreground = Hsla::from(theme::muted_text());
    c.border = Hsla::from(theme::border());
    c.accent = Hsla::from(theme::accent());
    c.selection = theme::selection();
    c.primary = Hsla::from(theme::accent());
    c.primary_foreground = Hsla::from(theme::canvas());
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum NoteMenuAction {
    Copy,
    Cut,
    Paste,
    SelectAll,
    Undo,
    Redo,
}

pub struct NoteWindow {
    store: Option<NoteStore>,
    notes: Vec<Note>,
    selected_id: Option<i64>,
    preview: bool,
    search_state: Entity<InputState>,
    content_state: Entity<TextareaState>,
    list_scroll: ScrollHandle,
    window_focus: FocusHandle,
    _search_sub: Subscription,
    _content_sub: Subscription,
    context_menu: Option<ContextMenuState<NoteMenuAction>>,
}

impl NoteWindow {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        sync_editor_theme(cx);
        let store = NoteStore::open_default().ok();

        let search_state = cx.new(|cx| InputState::new(window, cx).placeholder("搜索..."));
        let content_state = cx.new(|cx| {
            TextareaState::new(window, cx)
                .placeholder("输入笔记内容... (支持 Markdown)")
                .soft_wrap(true)
        });

        let note_entity = cx.entity().downgrade();
        let search_handler = {
            let note_entity = note_entity.clone();
            Rc::new(
                move |_menu: crossh_editor::input::NativeMenu,
                      cap: crossh_editor::input::InputContextMenuCapabilities,
                      pos: Point<Pixels>,
                      _window: &mut Window,
                      cx: &mut App| {
                    if let Some(note) = note_entity.upgrade() {
                        note.update(cx, |this, cx| {
                            this.open_context_menu(pos, cap, cx);
                        });
                    }
                },
            )
        };
        let content_handler = {
            let note_entity = note_entity.clone();
            Rc::new(
                move |_menu: crossh_editor::input::NativeMenu,
                      cap: crossh_editor::input::InputContextMenuCapabilities,
                      pos: Point<Pixels>,
                      _window: &mut Window,
                      cx: &mut App| {
                    if let Some(note) = note_entity.upgrade() {
                        note.update(cx, |this, cx| {
                            this.open_context_menu(pos, cap, cx);
                        });
                    }
                },
            )
        };
        search_state.update(cx, |s, _| s.on_context_menu(search_handler));
        content_state.update(cx, |s, _| s.on_context_menu(content_handler));

        let search_clone = search_state.clone();
        let _search_sub = cx.subscribe(
            &search_clone,
            |this: &mut Self, _, event: &InputEvent, cx| {
                if matches!(event, InputEvent::Change) {
                    this.reload_notes(cx);
                }
            },
        );
        let content_clone = content_state.clone();
        let _content_sub =
            cx.subscribe(&content_clone, |_: &mut Self, _, event: &InputEvent, cx| {
                if matches!(event, InputEvent::Change) {
                    cx.notify();
                }
            });

        let mut this = Self {
            store: None,
            notes: Vec::new(),
            selected_id: None,
            preview: false,
            search_state,
            content_state,
            list_scroll: ScrollHandle::new(),
            window_focus: cx.focus_handle(),
            _search_sub,
            _content_sub,
            context_menu: None,
        };
        if let Some(s) = store {
            this.store = Some(s);
            this.reload_notes(cx);
        }
        let content_focus = this.content_state.read(cx).focus_handle(cx).clone();
        cx.defer_in(window, move |_, window, cx| {
            window.focus(&content_focus, cx);
        });
        window.focus(&this.content_state.read(cx).focus_handle(cx), cx);
        this
    }

    fn is_draft_dirty(&self, cx: &App) -> bool {
        !self.content_state.read(cx).value().trim().is_empty()
    }

    fn reload_notes(&mut self, cx: &mut Context<Self>) {
        let Some(store) = &self.store else { return };
        let query = self.search_state.read(cx).value().trim().to_string();
        let notes = if query.is_empty() {
            store.list().unwrap_or_default()
        } else {
            store.search(&query).unwrap_or_default()
        };
        let prev_selected = self.selected_id;
        self.notes = notes;
        let still_exists = self
            .selected_id
            .is_some_and(|id| self.notes.iter().any(|n| n.id == id));
        if !still_exists {
            let should_preserve_draft = self.is_draft_dirty(cx);
            self.selected_id = None;
            if should_preserve_draft && prev_selected.is_none() {
                cx.notify();
                return;
            }
            if should_preserve_draft && prev_selected.is_some() && !query.is_empty() {
                cx.notify();
                return;
            }
        }
        if self.selected_id.is_none() && !self.notes.is_empty() {
            let first = self.notes[0].clone();
            self.select_note(first.id, cx);
        } else if self.selected_id.is_none() {
            self.content_state.update(cx, |s, cx| {
                s.set_value_simple("", cx);
            });
        }
        cx.notify();
    }

    fn select_note(&mut self, id: i64, cx: &mut Context<Self>) {
        self.selected_id = Some(id);
        if let Some(note) = self.notes.iter().find(|n| n.id == id).cloned() {
            self.content_state.update(cx, |s, cx| {
                s.set_value_simple(note.content.clone(), cx);
            });
            self.preview = false;
        }
        cx.notify();
    }

    fn select_note_focused(&mut self, id: i64, window: &mut Window, cx: &mut Context<Self>) {
        self.select_note(id, cx);
        window.focus(&self.content_state.read(cx).focus_handle(cx), cx);
    }

    fn save_current(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(store) = &self.store else { return };
        let content = self.content_state.read(cx).value().trim().to_string();
        if content.is_empty() {
            return;
        }
        if let Some(id) = self.selected_id {
            if let Some(note) = self.notes.iter().find(|n| n.id == id)
                && note.content == content
            {
                return;
            }
            match store.update(id, &content) {
                Ok(_) => {
                    self.reload_notes(cx);
                    window.focus(&self.content_state.read(cx).focus_handle(cx), cx);
                }
                Err(e) => log::warn!("note update failed: {}", e),
            }
        } else {
            match store.create(&content) {
                Ok(note) => {
                    self.selected_id = Some(note.id);
                    self.reload_notes(cx);
                    window.focus(&self.content_state.read(cx).focus_handle(cx), cx);
                }
                Err(e) => log::warn!("note create failed: {}", e),
            }
        }
    }

    fn new_note(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.selected_id = None;
        self.content_state.update(cx, |s, cx| {
            s.set_value_simple("", cx);
        });
        self.preview = false;
        window.focus(&self.content_state.read(cx).focus_handle(cx), cx);
        cx.notify();
    }

    fn delete_current(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(id) = self.selected_id else { return };
        if let Some(store) = &self.store {
            let _ = store.delete(id);
        }
        self.selected_id = None;
        self.reload_notes(cx);
        if !self.notes.is_empty() {
            window.focus(&self.content_state.read(cx).focus_handle(cx), cx);
        }
    }

    fn toggle_preview(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.preview = !self.preview;
        if !self.preview {
            window.focus(&self.content_state.read(cx).focus_handle(cx), cx);
        }
        cx.notify();
    }

    fn toggle_pin(&mut self, id: i64, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(store) = &self.store
            && let Some(note) = self.notes.iter().find(|n| n.id == id)
        {
            let _ = store.set_pinned(id, !note.pinned);
            self.reload_notes(cx);
            if self.selected_id.is_some() {
                window.focus(&self.content_state.read(cx).focus_handle(cx), cx);
            }
        }
    }

    fn open_context_menu(
        &mut self,
        position: Point<Pixels>,
        cap: crossh_editor::input::InputContextMenuCapabilities,
        cx: &mut Context<Self>,
    ) {
        let has_selection = cap.has_selection();
        let editable = cap.is_editable();
        let mut entries = Vec::new();
        entries.push(MenuEntry::Item(MenuItem {
            id: "copy".into(),
            label: "复制".into(),
            shortcut_hint: Some(
                if cfg!(target_os = "macos") {
                    "⌘C"
                } else {
                    "Ctrl+C"
                }
                .into(),
            ),
            disabled: !has_selection,
            danger: false,
            action: NoteMenuAction::Copy,
        }));
        entries.push(MenuEntry::Item(MenuItem {
            id: "cut".into(),
            label: "剪切".into(),
            shortcut_hint: Some(
                if cfg!(target_os = "macos") {
                    "⌘X"
                } else {
                    "Ctrl+X"
                }
                .into(),
            ),
            disabled: !has_selection || !editable,
            danger: false,
            action: NoteMenuAction::Cut,
        }));
        entries.push(MenuEntry::Item(MenuItem {
            id: "paste".into(),
            label: "粘贴".into(),
            shortcut_hint: Some(
                if cfg!(target_os = "macos") {
                    "⌘V"
                } else {
                    "Ctrl+V"
                }
                .into(),
            ),
            disabled: !editable,
            danger: false,
            action: NoteMenuAction::Paste,
        }));
        entries.push(MenuEntry::Separator);
        entries.push(MenuEntry::Item(MenuItem {
            id: "select-all".into(),
            label: "全选".into(),
            shortcut_hint: Some(
                if cfg!(target_os = "macos") {
                    "⌘A"
                } else {
                    "Ctrl+A"
                }
                .into(),
            ),
            disabled: false,
            danger: false,
            action: NoteMenuAction::SelectAll,
        }));
        entries.push(MenuEntry::Separator);
        entries.push(MenuEntry::Item(MenuItem {
            id: "undo".into(),
            label: "撤销".into(),
            shortcut_hint: Some(
                if cfg!(target_os = "macos") {
                    "⌘Z"
                } else {
                    "Ctrl+Z"
                }
                .into(),
            ),
            disabled: !editable,
            danger: false,
            action: NoteMenuAction::Undo,
        }));
        entries.push(MenuEntry::Item(MenuItem {
            id: "redo".into(),
            label: "重做".into(),
            shortcut_hint: Some(
                if cfg!(target_os = "macos") {
                    "⇧⌘Z"
                } else {
                    "Ctrl+Y"
                }
                .into(),
            ),
            disabled: !editable,
            danger: false,
            action: NoteMenuAction::Redo,
        }));
        self.context_menu = Some(ContextMenuState { position, entries });
        cx.notify();
    }

    fn close_context_menu(&mut self, cx: &mut Context<Self>) {
        if self.context_menu.take().is_some() {
            cx.notify();
        }
    }

    fn handle_context_menu_action(
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

    fn is_dirty(&self, cx: &App) -> bool {
        let content = self.content_state.read(cx).value().trim().to_string();
        if let Some(id) = self.selected_id
            && let Some(note) = self.notes.iter().find(|n| n.id == id)
        {
            return content != note.content;
        }
        !content.is_empty()
    }

    fn is_save_disabled(&self, cx: &App) -> bool {
        let content = self.content_state.read(cx).value().trim().to_string();
        if content.is_empty() {
            return true;
        }
        !self.is_dirty(cx)
    }

    fn status_title(&self, cx: &App) -> String {
        if let Some(id) = self.selected_id
            && let Some(note) = self.notes.iter().find(|n| n.id == id)
        {
            return note
                .content
                .lines()
                .next()
                .unwrap_or("空笔记")
                .chars()
                .take(30)
                .collect();
        }
        if self.is_dirty(cx) {
            return "新建笔记".to_string();
        }
        if self.notes.is_empty() {
            return "无笔记".to_string();
        }
        "未选择".to_string()
    }

    fn render_status_bar(&self, _window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let total = self.notes.len();
        let pinned = self.notes.iter().filter(|n| n.pinned).count();
        let char_count = self.content_state.read(cx).value().chars().count();
        let dirty = self.is_dirty(cx);
        let save_disabled = self.is_save_disabled(cx);
        let title = self.status_title(cx);

        let mut left = div()
            .min_w_0()
            .flex()
            .items_center()
            .gap_2()
            .child(icons::icon(icons::IconName::FileText, 13.).text_color(theme::accent()))
            .child(
                div()
                    .min_w_0()
                    .max_w(px(240.))
                    .truncate()
                    .text_color(theme::text())
                    .child(SharedString::from(title)),
            );
        if total > 0 {
            left = left.child(StatusMetric::new(format!("{} 条", total)).tone(BadgeTone::Neutral));
        }
        if pinned > 0 {
            left = left.child(StatusMetric::new(format!("📌 {}", pinned)).tone(BadgeTone::Accent));
        }
        if dirty {
            left = left.child(StatusMetric::new("未保存").tone(BadgeTone::Warning));
        }
        if self.preview {
            left = left.child(StatusMetric::new("预览").tone(BadgeTone::Info));
        } else if char_count > 0 {
            left = left
                .child(StatusMetric::new(format!("{} 字", char_count)).tone(BadgeTone::Neutral));
        }

        let actions = div()
            .flex_shrink_0()
            .flex()
            .items_center()
            .gap_1()
            .child(
                Button::new("note-status-new")
                    .size(ButtonSize::Icon(px(22.)))
                    .variant(ButtonVariant::Ghost)
                    .icon(icons::icon(icons::IconName::Plus, 13.).text_color(theme::muted_text()))
                    .tooltip("新建笔记")
                    .on_click(cx.listener(|this, _, window, cx| this.new_note(window, cx))),
            )
            .child(
                Button::new("note-status-save")
                    .size(ButtonSize::Icon(px(22.)))
                    .variant(ButtonVariant::Ghost)
                    .icon(icons::icon(icons::IconName::Save, 13.).text_color(
                        if dirty && !save_disabled {
                            theme::accent()
                        } else {
                            theme::muted_text()
                        },
                    ))
                    .tooltip("保存")
                    .disabled(save_disabled)
                    .on_click(cx.listener(|this, _, window, cx| this.save_current(window, cx))),
            )
            .child(
                Button::new("note-status-delete")
                    .size(ButtonSize::Icon(px(22.)))
                    .variant(ButtonVariant::Ghost)
                    .icon(icons::icon(icons::IconName::Trash, 13.).text_color(theme::muted_text()))
                    .tooltip("删除")
                    .disabled(self.selected_id.is_none())
                    .on_click(cx.listener(|this, _, window, cx| this.delete_current(window, cx))),
            )
            .child(
                Button::new("note-status-preview")
                    .size(ButtonSize::Icon(px(22.)))
                    .variant(ButtonVariant::Ghost)
                    .selected(self.preview)
                    .icon(if self.preview {
                        icons::icon(icons::IconName::Pencil, 13.).text_color(theme::accent())
                    } else {
                        icons::icon(icons::IconName::FileText, 13.).text_color(theme::muted_text())
                    })
                    .tooltip(if self.preview { "编辑" } else { "预览" })
                    .on_click(cx.listener(|this, _, window, cx| this.toggle_preview(window, cx))),
            );

        StatusBar::new("note-status-bar")
            .child(left)
            .child(actions)
            .into_any_element()
    }
}

impl Focusable for NoteWindow {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.window_focus.clone()
    }
}

impl gpui::Render for NoteWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        sync_editor_theme(cx);
        let notes = self.notes.clone();

        let header = div()
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .p_2()
            .border_b_1()
            .border_color(theme::border())
            .bg(theme::surface())
            .text_color(theme::text())
            .child(div().flex_1().child(Input::new(&self.search_state)));

        let list = div()
            .w(px(260.))
            .min_w(px(180.))
            .max_w(px(320.))
            .h_full()
            .flex()
            .flex_col()
            .overflow_hidden()
            .border_r_1()
            .border_color(theme::border())
            .bg(theme::surface())
            .child(
                div()
                    .id("note-list")
                    .flex_1()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .p_2()
                    .overflow_y_scroll()
                    .track_scroll(&self.list_scroll)
                    .children(notes.iter().map(|note| {
                        let is_selected = Some(note.id) == self.selected_id;
                        let id = note.id;
                        let pin_label = if note.pinned { "📌 " } else { "" };
                        let preview_text = note
                            .content
                            .lines()
                            .next()
                            .unwrap_or("空笔记")
                            .chars()
                            .take(30)
                            .collect::<String>();
                        div()
                            .id(("note-item", note.id as usize))
                            .w_full()
                            .p_2()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .rounded(px(theme::RADIUS_SM))
                            .bg(if is_selected {
                                theme::accent_soft()
                            } else {
                                gpui::Rgba {
                                    r: 0.,
                                    g: 0.,
                                    b: 0.,
                                    a: 0.,
                                }
                            })
                            .border_1()
                            .border_color(if is_selected {
                                theme::accent()
                            } else {
                                gpui::Rgba {
                                    r: 0.,
                                    g: 0.,
                                    b: 0.,
                                    a: 0.,
                                }
                            })
                            .cursor_pointer()
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.select_note_focused(id, window, cx)
                            }))
                            .child(
                                div()
                                    .flex()
                                    .flex_row()
                                    .items_center()
                                    .justify_between()
                                    .child(
                                        div()
                                            .text_sm()
                                            .text_color(theme::text())
                                            .child(format!("{}{}", pin_label, preview_text)),
                                    )
                                    .child(
                                        Button::new(("pin", note.id as usize))
                                            .size(ButtonSize::Icon(px(18.)))
                                            .variant(ButtonVariant::Ghost)
                                            .icon(
                                                icons::icon(icons::IconName::Pin, 10.).text_color(
                                                    if note.pinned {
                                                        theme::accent()
                                                    } else {
                                                        theme::muted_text()
                                                    },
                                                ),
                                            )
                                            .on_click(cx.listener(move |this, _, window, cx| {
                                                this.toggle_pin(id, window, cx)
                                            })),
                                    ),
                            )
                    })),
            );

        let right: AnyElement = if self.preview {
            let md = self.content_state.read(cx).value().to_string();
            div()
                .id("note-preview")
                .flex_1()
                .min_h_0()
                .h_full()
                .min_w(px(320.))
                .p_3()
                .bg(theme::canvas())
                .text_color(theme::text())
                .overflow_y_scroll()
                .child(render_markdown(&md))
                .into_any_element()
        } else {
            div()
                .flex_1()
                .min_h_0()
                .h_full()
                .min_w(px(320.))
                .flex()
                .flex_col()
                .overflow_hidden()
                .p_2()
                .bg(theme::canvas())
                .text_color(theme::text())
                .child(
                    div()
                        .flex_1()
                        .min_h_0()
                        .h_full()
                        .w_full()
                        .border_1()
                        .border_color(theme::border())
                        .rounded(px(theme::RADIUS_SM))
                        .overflow_hidden()
                        .bg(theme::canvas())
                        .text_color(theme::text())
                        .child(Textarea::new(&self.content_state)),
                )
                .into_any_element()
        };

        let body = div()
            .flex_1()
            .min_h_0()
            .w_full()
            .flex()
            .flex_row()
            .overflow_hidden()
            .child(list)
            .child(right);

        let linux_titlebar =
            crossh_ui::linux_titlebar::render_linux_titlebar(window, cx, "Note".into());

        let context_menu = self.context_menu.clone();
        let mut root = div()
            .size_full()
            .flex()
            .flex_col()
            .bg(theme::canvas())
            .text_color(theme::text())
            .track_focus(&self.window_focus)
            .on_action(cx.listener(|_, _: &CloseNoteWindow, window, _| {
                window.remove_window();
            }))
            .on_action(cx.listener(|this, _: &NewNote, window, cx| this.new_note(window, cx)))
            .on_action(cx.listener(|this, _: &SaveNote, window, cx| this.save_current(window, cx)))
            .on_action(
                cx.listener(|this, _: &TogglePreview, window, cx| this.toggle_preview(window, cx)),
            )
            .on_action(
                cx.listener(|this, _: &DeleteNote, window, cx| this.delete_current(window, cx)),
            )
            .key_context(NOTE_WINDOW_CONTEXT)
            .children(linux_titlebar)
            .child(header)
            .child(body)
            .child(self.render_status_bar(window, cx));

        if let Some(menu) = context_menu {
            root = root.child(render_context_menu(
                &menu,
                point(px(0.), px(0.)),
                window,
                cx,
                |this: &mut Self, action: NoteMenuAction, window, cx| {
                    this.handle_context_menu_action(action, window, cx)
                },
                |this: &mut Self, cx| this.close_context_menu(cx),
            ));
        }

        root
    }
}

pub fn open_note_window(cx: &mut App) {
    if let Some(window) = cx.windows().iter().find_map(|h| h.downcast::<NoteWindow>()) {
        let _ = window.update(cx, |note, window, cx| {
            window.activate_window();
            window.focus(&note.content_state.read(cx).focus_handle(cx), cx);
        });
        return;
    }
    if cx.windows().is_empty() {
        create_note_window(cx);
    } else {
        cx.defer(create_note_window);
    }
}

fn create_note_window(cx: &mut App) {
    let bounds = Bounds::centered(
        None,
        Size {
            width: px(900.),
            height: px(600.),
        },
        cx,
    );
    let handle = cx
        .open_window(
            WindowOptions {
                titlebar: Some(TitlebarOptions {
                    title: Some("Note".into()),
                    ..Default::default()
                }),
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                window_min_size: Some(Size {
                    width: px(640.),
                    height: px(400.),
                }),
                ..Default::default()
            },
            |window, cx| cx.new(|cx| NoteWindow::new(window, cx)),
        )
        .expect("Note window should open");
    let _ = handle.update(cx, |note, window, cx| {
        window.focus(&note.content_state.read(cx).focus_handle(cx), cx);
    });
    cx.activate(true);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossh_note::Note;
    use gpui::TestAppContext;

    #[gpui::test]
    fn note_window_historic_note_loads_in_edit_state(cx: &mut TestAppContext) {
        let window = cx.add_window(NoteWindow::new);
        cx.run_until_parked();
        window
            .update(cx, |note_window, window, cx| {
                let historic = Note {
                    id: 1,
                    content: "hello 历史笔记".to_string(),
                    pinned: false,
                    created_at: 0,
                    updated_at: 0,
                };
                note_window.notes = vec![historic.clone()];
                note_window.select_note_focused(historic.id, window, cx);
                assert_eq!(note_window.notes.len(), 1);
                assert_eq!(note_window.selected_id, Some(historic.id));
                assert_eq!(
                    note_window.content_state.read(cx).value().to_string(),
                    "hello 历史笔记"
                );
                assert!(!note_window.preview);
                assert!(
                    note_window
                        .content_state
                        .read(cx)
                        .focus_handle(cx)
                        .is_focused(window)
                );
                note_window.content_state.update(cx, |state, cx| {
                    state.set_value_simple("hello 历史笔记 modified", cx);
                });
                assert_eq!(
                    note_window.content_state.read(cx).value().to_string(),
                    "hello 历史笔记 modified"
                );
            })
            .unwrap();
    }

    #[gpui::test]
    fn note_window_select_via_list_focuses_content(cx: &mut TestAppContext) {
        let window = cx.add_window(NoteWindow::new);
        cx.run_until_parked();
        window
            .update(cx, |note_window, window, cx| {
                let n1 = Note {
                    id: 1,
                    content: "first".to_string(),
                    pinned: false,
                    created_at: 0,
                    updated_at: 0,
                };
                let n2 = Note {
                    id: 2,
                    content: "second".to_string(),
                    pinned: false,
                    created_at: 0,
                    updated_at: 1,
                };
                note_window.notes = vec![n2.clone(), n1.clone()];
                note_window.selected_id = Some(n2.id);
                note_window
                    .content_state
                    .update(cx, |s, cx| s.set_value_simple(n2.content.clone(), cx));
                note_window.select_note_focused(n1.id, window, cx);
                assert_eq!(note_window.selected_id, Some(n1.id));
                assert_eq!(
                    note_window.content_state.read(cx).value().to_string(),
                    "first"
                );
                assert!(
                    note_window
                        .content_state
                        .read(cx)
                        .focus_handle(cx)
                        .is_focused(window)
                );
                assert!(!note_window.preview);
            })
            .unwrap();
    }

    #[gpui::test]
    fn note_status_dirty_and_save_disabled(cx: &mut TestAppContext) {
        let window = cx.add_window(NoteWindow::new);
        cx.run_until_parked();
        window
            .update(cx, |note_window, _window, cx| {
                note_window.notes = Vec::new();
                note_window.selected_id = None;
                note_window
                    .content_state
                    .update(cx, |s, cx| s.set_value_simple("", cx));
                assert!(!note_window.is_dirty(cx));
                assert!(note_window.is_save_disabled(cx));
                assert_eq!(note_window.status_title(cx), "无笔记");
                note_window
                    .content_state
                    .update(cx, |s, cx| s.set_value_simple("hello", cx));
                assert!(note_window.is_dirty(cx));
                assert!(!note_window.is_save_disabled(cx));
                assert_eq!(note_window.status_title(cx), "新建笔记");
                let note = Note {
                    id: 1,
                    content: "hello".to_string(),
                    pinned: false,
                    created_at: 0,
                    updated_at: 0,
                };
                note_window.notes = vec![note.clone()];
                note_window.selected_id = Some(note.id);
                note_window
                    .content_state
                    .update(cx, |s, cx| s.set_value_simple(note.content.clone(), cx));
                assert!(!note_window.is_dirty(cx));
                assert!(note_window.is_save_disabled(cx));
                assert_eq!(note_window.status_title(cx), "hello");
                note_window
                    .content_state
                    .update(cx, |s, cx| s.set_value_simple("hello world", cx));
                assert!(note_window.is_dirty(cx));
                assert!(!note_window.is_save_disabled(cx));
            })
            .unwrap();
    }

    #[gpui::test]
    fn note_status_title_fallbacks(cx: &mut TestAppContext) {
        let window = cx.add_window(NoteWindow::new);
        cx.run_until_parked();
        window
            .update(cx, |note_window, _window, cx| {
                note_window.notes = vec![Note {
                    id: 1,
                    content: "".to_string(),
                    pinned: false,
                    created_at: 0,
                    updated_at: 0,
                }];
                note_window.selected_id = None;
                note_window
                    .content_state
                    .update(cx, |s, cx| s.set_value_simple("", cx));
                assert_eq!(note_window.status_title(cx), "未选择");
            })
            .unwrap();
    }
}
