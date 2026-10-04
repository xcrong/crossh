//! Note 窗口 — 搜索框走 TextEditingState（与侧栏/Git 同一编辑语义），内容区基于 crossh-editor TextareaState。

use std::cell::Cell;
use std::ops::Range;
use std::rc::Rc;
use std::time::Duration;

use crossh_core::input_handler::{
    editing_mark_text, editing_marked_range, editing_replace, editing_selected_range,
    editing_unmark,
};
use crossh_core::text_editing::{
    EditingKeystroke, TextEditingState, byte_index_for_utf16, handle_text_editing_key, utf16_len,
    utf16_slice,
};
use crossh_editor::Textarea;
use crossh_editor::input::{InputEvent, TextareaState};
use crossh_note::{Note, NoteStore};
use crossh_ui::widgets::ime_caret_bounds;
use crossh_ui::{icons, theme};
use crossh_ui_component::{
    BadgeTone, Button, ButtonSize, ButtonVariant, SplitResizer, StatusBar, StatusMetric,
    context_menu::{ContextMenuState, MenuEntry, MenuItem, render_context_menu},
    filter_row, filter_text_input,
};
use gpui::{
    AnyElement, App, AppContext, Bounds, ClipboardItem, Context, Edges, Entity, EntityInputHandler,
    FocusHandle, Focusable, Hsla, InteractiveElement, IntoElement, KeyDownEvent,
    ListSizingBehavior, ParentElement, Pixels, Point, SharedString, Size,
    StatefulInteractiveElement, Styled, Subscription, TitlebarOptions, UTF16Selection,
    UniformListScrollHandle, Window, WindowBounds, WindowOptions, div, point, px, uniform_list,
};

use super::format::{format_note_time, note_title, persist_error_message};
use super::markdown::render_markdown;
use super::{
    CloseFind, CloseNoteWindow, DeleteNote, EscapePreview, FindNext, FindPrev, NewNote, SaveNote,
    SelectNextNote, SelectPrevNote, ToggleFind, TogglePreview,
};

const NOTE_WINDOW_CONTEXT: &str = "NoteWindow";
// 预览正文的阅读宽度上限（px）。约 70 个西文字符，超出则居中留白，
// 避免长行在大窗口里难以扫读。
const PREVIEW_MAX_WIDTH: f32 = 720.;
// 笔记列表栏宽度：默认/最小/最大（px）。可拖拽调整，窄窗下由 min 兜底。
const LIST_PANE_DEFAULT_WIDTH: f32 = 260.;
const LIST_PANE_MIN_WIDTH: f32 = 180.;
const LIST_PANE_MAX_WIDTH: f32 = 420.;
// 编辑器内容的呼吸空间（px）。走 `set_editor_paddings` 而非外壳 padding，
// 这样行号栏紧贴左边缘，分隔线与文字之间不留多余空档。
pub(super) const EDITOR_PADDING_X: f32 = 16.;
pub(super) const EDITOR_PADDING_Y: f32 = 12.;

fn clamp_list_pane_width(width: f32) -> f32 {
    width.clamp(LIST_PANE_MIN_WIDTH, LIST_PANE_MAX_WIDTH)
}
// 搜索防抖时长：输入停止约 200ms 后才查库（与 hover/toast 的 timer 惯例一致）。
const SEARCH_DEBOUNCE: Duration = Duration::from_millis(200);
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
    // open_default 失败时的原因（store 为 None 时必有值），渲染错误行用。
    store_error: Option<String>,
    // 最近一次 list/search 失败的提示；成功后清空，失败时保留旧列表。
    list_error: Option<String>,
    notes: Vec<Note>,
    selected_id: Option<i64>,
    preview: bool,
    // 删除二次确认：第一次点击只记录目标，第二次才真删。
    pending_delete_id: Option<i64>,
    // 搜索防抖代际计数：每次输入变更加一，只有最新一代到期后才 reload。
    search_generation: u64,
    // 搜索状态；与侧栏/Git 筛选条同一编辑语义（`TextEditingState` + 共享分发）。
    search_query: TextEditingState,
    search_focus: FocusHandle,
    pub(super) content_state: Entity<TextareaState>,
    list_scroll: UniformListScrollHandle,
    // 列表栏宽度与拖拽态：拖拽期间由 SplitResizer 直接写这两处，渲染时读取。
    list_width: Rc<Cell<f32>>,
    list_dragging: Rc<Cell<bool>>,
    window_focus: FocusHandle,
    _content_sub: Subscription,
    context_menu: Option<ContextMenuState<NoteMenuAction>>,
    // 最近一次落库失败的原因。落库失败此前只写日志，用户看到的是"保存按钮点了没反应"，
    // 且 is_dirty 永远为真、状态栏永远挂"未保存"。改为记录并渲染在编辑区上方。
    persist_error: Option<String>,
    // 笔记内查找面板。匹配引擎在 `TextareaState.search_session` 里（crossh-editor
    // 提供），行为见 `find_panel.rs`。
    pub(super) find_query: TextEditingState,
    pub(super) find_focus: FocusHandle,
    pub(super) find_open: bool,
}

impl NoteWindow {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        // 打开失败不再吞掉：记下原因，渲染层给出错误行 + 状态栏标题。
        let (store, store_error) = match NoteStore::open_default() {
            Ok(store) => (Some(store), None),
            Err(e) => {
                log::warn!("note store open failed: {e}");
                (None, Some(format!("笔记库打开失败：{e}")))
            }
        };

        let mut this = Self::new_uncoupled(window, cx);
        if let Some(s) = store {
            this.store = Some(s);
            this.reload_notes(cx);
        } else {
            this.store_error = store_error;
        }
        this
    }

    /// 测试专用构造：不碰用户真实笔记库。
    ///
    /// [`NoteWindow::new`] 走 `NoteStore::open_default()`，会打开并写入
    /// `~/Library/Application Support/crossh/note.db`。测试并行跑时，多个窗口
    /// 同时读写同一个文件，既有断言就会随机失败（例如手动塞进 `notes` 的 id=1
    /// 被真实库返回的条目顶掉），而且测试本身会污染用户数据。
    ///
    /// 这里返回不带 store 的窗口，由各测试自行填 `notes`。
    #[cfg(test)]
    pub(super) fn new_for_test(window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self::new_uncoupled(window, cx)
    }

    /// 构造窗口但**不打开笔记库**，`new` 与 `new_for_test` 共用。
    /// 调用方负责按需挂 store 并决定是否 `reload_notes`。
    fn new_uncoupled(window: &mut Window, cx: &mut Context<Self>) -> Self {
        sync_editor_theme(cx);
        let search_focus = cx.focus_handle();
        let content_state = cx.new(|cx| {
            let mut state = TextareaState::new(window, cx)
                .placeholder("输入笔记内容... (支持 Markdown)")
                .soft_wrap(true)
                // 行号栏：笔记普遍是多行文本，缺行号时数行号/定位段落很费劲。
                .line_number(true)
                // 笔记内查找（Ctrl/Cmd+F）。匹配引擎与命中高亮由 crossh-editor 提供，
                // 这里只负责浮层面板。
                .searchable(true)
                .replaceable(true);
            // 呼吸空间走编辑器内边距而不是外壳 padding：行号栏会被推到左边距之外，
            // 与分隔线之间不再留空档。右侧留白给滚动条，底部留白让最后一行能滚上来。
            state.set_editor_paddings(Edges {
                left: px(EDITOR_PADDING_X),
                right: px(EDITOR_PADDING_X + 10.),
                top: px(EDITOR_PADDING_Y),
                bottom: px(EDITOR_PADDING_Y),
            });
            state
        });
        let note_entity = cx.entity().downgrade();
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
        content_state.update(cx, |s, _| s.on_context_menu(content_handler));

        let content_clone = content_state.clone();
        let _content_sub =
            cx.subscribe(&content_clone, |_: &mut Self, _, event: &InputEvent, cx| {
                if matches!(event, InputEvent::Change) {
                    cx.notify();
                }
            });

        let this = Self {
            store: None,
            store_error: None,
            list_error: None,
            notes: Vec::new(),
            selected_id: None,
            preview: false,
            pending_delete_id: None,
            search_generation: 0,
            search_query: TextEditingState::new(String::new()),
            search_focus,
            content_state,
            list_scroll: UniformListScrollHandle::default(),
            list_width: Rc::new(Cell::new(LIST_PANE_DEFAULT_WIDTH)),
            list_dragging: Rc::new(Cell::new(false)),
            window_focus: cx.focus_handle(),
            _content_sub,
            context_menu: None,
            persist_error: None,
            find_query: TextEditingState::new(String::new()),
            find_focus: cx.focus_handle(),
            find_open: false,
        };
        let content_focus = this.content_state.read(cx).focus_handle(cx).clone();
        cx.defer_in(window, move |_, window, cx| {
            window.focus(&content_focus, cx);
        });
        window.focus(&this.content_state.read(cx).focus_handle(cx), cx);
        this
    }

    /// 只刷新列表，不碰编辑器与选中（内部用，不做脏保护、无递归）。
    /// 查询失败记一条提示并保留旧列表，不再用 unwrap_or_default 吞掉。
    fn refresh_list(&mut self) {
        let Some(store) = &self.store else { return };
        let query = self.search_query.value.trim().to_string();
        let result = if query.is_empty() {
            store.list()
        } else {
            store.search(&query)
        };
        match result {
            Ok(notes) => {
                self.notes = notes;
                self.list_error = None;
            }
            Err(e) => {
                log::warn!("note query failed: {e}");
                self.list_error = Some("查询失败，显示旧数据".to_string());
            }
        }
    }

    /// 搜索输入变更后的统一出口：防抖代际加一，只有最新一代到期后才查库。
    /// 键盘输入与 IME 回调都走这里，输入过程中不查库。
    fn on_search_changed(&mut self, cx: &mut Context<Self>) {
        self.search_generation = self.search_generation.wrapping_add(1);
        let generation = self.search_generation;
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(SEARCH_DEBOUNCE).await;
            this.update(cx, |this, cx| {
                if this.search_generation != generation {
                    return;
                }
                this.reload_notes(cx);
            })
            .ok();
        })
        .detach();
    }

    /// Esc 在搜索框的三家一致语义：有内容则清空并截断（不关窗口）；
    /// 已空或焦点不在搜索框时返回 `false`，调用方放行冒泡（Note 即关窗口）。
    fn clear_search_on_escape(&mut self, window: &Window, cx: &mut Context<Self>) -> bool {
        if !self.search_focus.is_focused(window) || self.search_query.value.is_empty() {
            return false;
        }
        self.search_query.clear();
        self.on_search_changed(cx);
        cx.notify();
        true
    }

    /// 搜索框与侧栏/Git 同一编辑语义：Esc 先清空，其余走共享分发；
    /// 处理后经防抖查库并截断冒泡，未处理（如 Up/Down 切列表）直接放行。
    fn handle_search_key(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let ks = &event.keystroke;
        if ks.key == "escape" {
            if self.clear_search_on_escape(window, cx) {
                cx.stop_propagation();
            }
            return;
        }
        let primary = ks.modifiers.control || ks.modifiers.platform;
        let paste_text = if primary && ks.key == "v" {
            cx.read_from_clipboard()
                .and_then(|item| item.text().map(|s| s.to_string()))
        } else {
            None
        };
        let editing_ks = EditingKeystroke {
            key: ks.key.clone(),
            key_char: ks.key_char.clone(),
            control: ks.modifiers.control,
            platform: ks.modifiers.platform,
            shift: ks.modifiers.shift,
        };
        let result =
            handle_text_editing_key(&mut self.search_query, &editing_ks, paste_text.as_deref());
        if let Some(text) = result.copy_text {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
        }
        if result.handled {
            self.on_search_changed(cx);
            cx.notify();
            cx.stop_propagation();
        }
    }

    /// 脏时先落库的最 boring 实现：空内容视为无须保存；
    /// 无库时无法落库但不阻塞切换；落库失败返回 false，调用方放弃切换。
    /// 成功时不刷新列表（调用方统一 refresh，避免递归）。
    fn persist_if_dirty(&mut self, cx: &mut Context<Self>) -> bool {
        if !self.is_dirty(cx) {
            return true;
        }
        let content = self.content_state.read(cx).value().trim().to_string();
        if content.is_empty() {
            return true;
        }
        let Some(store) = &self.store else {
            return true;
        };
        self.persist_error = None;
        if let Some(id) = self.selected_id {
            if self
                .notes
                .iter()
                .find(|n| n.id == id)
                .is_some_and(|n| n.content == content)
            {
                return true;
            }
            // 直接按 id 更新，不受搜索过滤影响（被过滤掉的选中项也能存）。
            match store.update(id, &content) {
                Ok(_) => true,
                Err(e) => {
                    log::warn!("note update failed: {e}");
                    self.persist_error = Some(persist_error_message(&e));
                    false
                }
            }
        } else {
            match store.create(&content) {
                Ok(note) => {
                    self.selected_id = Some(note.id);
                    true
                }
                Err(e) => {
                    log::warn!("note create failed: {e}");
                    self.persist_error = Some(persist_error_message(&e));
                    false
                }
            }
        }
    }

    fn reload_notes(&mut self, cx: &mut Context<Self>) {
        self.refresh_list();
        // 脏保护：自动选中会覆盖编辑器，先落库；失败则放弃切换、保留草稿。
        if self.is_dirty(cx) && !self.persist_if_dirty(cx) {
            cx.notify();
            return;
        }
        // 落库可能产生新 id（create），再刷一次拿到最新列表。
        self.refresh_list();
        let still_exists = self
            .selected_id
            .is_some_and(|id| self.notes.iter().any(|n| n.id == id));
        if !still_exists {
            // 到这里已不脏（本来不脏，或刚保存成功），可安全覆盖编辑器。
            self.selected_id = None;
        }
        if self.selected_id.is_none() && !self.notes.is_empty() {
            let first = self.notes[0].clone();
            self.select_note(first.id, cx);
        } else if self.selected_id.is_none() {
            self.reset_find(cx);
            self.content_state.update(cx, |s, cx| {
                s.set_value_simple("", cx);
            });
        }
        cx.notify();
    }

    fn select_note(&mut self, id: i64, cx: &mut Context<Self>) {
        if self.selected_id == Some(id) {
            return;
        }
        // 脏保护：切换前先落库，失败则放弃切换。
        // 注意：这里不刷新列表——刷新会冲掉搜索过滤出的当前视图；
        // create 产生的新条目由调用处的 reload 统一带回（与旧行为一致）。
        if self.is_dirty(cx) && !self.persist_if_dirty(cx) {
            return;
        }
        self.pending_delete_id = None;
        self.selected_id = Some(id);
        if let Some(note) = self.notes.iter().find(|n| n.id == id).cloned() {
            self.reset_find(cx);
            self.content_state
                .update(cx, |s, cx| s.set_value_simple(note.content.clone(), cx));
            self.preview = false;
        }
        cx.notify();
    }

    fn select_note_focused(&mut self, id: i64, window: &mut Window, cx: &mut Context<Self>) {
        self.select_note(id, cx);
        window.focus(&self.content_state.read(cx).focus_handle(cx), cx);
    }

    /// 键盘导航：按 delta 移动选中（越界钳制），复用 select_note 的脏保护，
    /// 切换后聚焦编辑器，空列表直接忽略。
    fn move_selection(&mut self, delta: i32, window: &mut Window, cx: &mut Context<Self>) {
        if self.notes.is_empty() {
            return;
        }
        let len = self.notes.len() as i32;
        let current = self
            .selected_id
            .and_then(|id| self.notes.iter().position(|n| n.id == id))
            .map(|i| i as i32)
            .unwrap_or(if delta > 0 { -1 } else { len });
        let next = current.saturating_add(delta).clamp(0, len - 1) as usize;
        let id = self.notes[next].id;
        self.select_note(id, cx);
        window.focus(&self.content_state.read(cx).focus_handle(cx), cx);
    }

    fn save_current(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let content = self.content_state.read(cx).value().trim().to_string();
        if content.is_empty() {
            return;
        }
        if !self.persist_if_dirty(cx) {
            cx.notify();
            return;
        }
        self.refresh_list();
        window.focus(&self.content_state.read(cx).focus_handle(cx), cx);
        cx.notify();
    }

    fn new_note(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // 脏保护：新建前先落库当前草稿，失败则放弃新建。
        if self.is_dirty(cx) && !self.persist_if_dirty(cx) {
            return;
        }
        self.pending_delete_id = None;
        self.selected_id = None;
        self.reset_find(cx);
        self.content_state.update(cx, |s, cx| {
            s.set_value_simple("", cx);
        });
        self.preview = false;
        self.refresh_list();
        window.focus(&self.content_state.read(cx).focus_handle(cx), cx);
        cx.notify();
    }

    fn delete_current(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(id) = self.selected_id else { return };
        if self.pending_delete_id != Some(id) {
            // 第一次：只标记并提示，第二次才真删（无弹窗，用按钮状态表达）。
            self.pending_delete_id = Some(id);
            cx.notify();
            return;
        }
        self.pending_delete_id = None;
        if let Some(store) = &self.store
            && let Err(e) = store.delete(id)
        {
            log::warn!("note delete failed: {e}");
            self.list_error = Some("删除失败，请重试".to_string());
            cx.notify();
            return;
        }
        self.selected_id = None;
        // 先清空编辑器再 reload，否则旧内容会被脏保护当成草稿复活成新笔记。
        self.reset_find(cx);
        self.content_state.update(cx, |s, cx| {
            s.set_value_simple("", cx);
        });
        self.reload_notes(cx);
        if !self.notes.is_empty() {
            window.focus(&self.content_state.read(cx).focus_handle(cx), cx);
        }
    }

    /// 请求关窗（Escape / ⌘W 共用的出口）。
    ///
    /// 关窗即自动落库，因此不存在"忘了保存"的路径：
    /// - 不脏 → 直接关。
    /// - 脏且落库成功 → 关。
    /// - 脏但落不了库（无库 / 写入失败）→ **不关**，把原因渲染在编辑区上方，
    ///   草稿留在编辑器里。关窗会让进程退出（QuitMode::LastWindowClosed），
    ///   草稿将永久丢失，所以必须拦下。
    ///
    /// 无库时 `persist_if_dirty` 返回 true 是切换路径的有意语义（不阻塞浏览），
    /// 这里必须区别对待。
    fn request_close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.is_dirty(cx) && self.store.is_none() {
            self.persist_error =
                Some("笔记库不可用，当前修改无法保存；请恢复笔记库后重试".to_string());
            cx.notify();
            return;
        }
        if self.persist_if_dirty(cx) {
            window.remove_window();
        } else {
            cx.notify();
        }
    }

    fn toggle_preview(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.preview = !self.preview;
        if self.preview {
            // 预览是只读视图，查找会话留着只会白占高亮，故一并关掉。
            if self.find_open {
                self.reset_find(cx);
            }
        } else {
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
        // 笔记库打不开时 notes 为空，在标题处直接给出简化原因。
        if self.store_error.is_some() {
            return "笔记库打开失败".to_string();
        }
        if let Some(id) = self.selected_id
            && let Some(note) = self.notes.iter().find(|n| n.id == id)
        {
            return note_title(&note.content, 30);
        }
        if self.is_dirty(cx) {
            return "新建笔记".to_string();
        }
        if self.notes.is_empty() {
            return "无笔记".to_string();
        }
        "未选择".to_string()
    }

    /// 列表单行：标题（首行，超长加省略号）+ 第二行小字（更新时间 · 字数）+ 置顶按钮。
    /// `uniform_list` 只对可视区调用本方法，故行内必须自带完整 padding/border。
    fn render_note_row(&mut self, note: &Note, cx: &mut Context<Self>) -> AnyElement {
        let is_selected = Some(note.id) == self.selected_id;
        let id = note.id;
        let pin_label = if note.pinned { "📌 " } else { "" };
        let title = format!("{}{}", pin_label, note_title(&note.content, 30));
        let meta = format!(
            "{} · {} 字",
            format_note_time(note.updated_at),
            note.content.chars().count()
        );
        let transparent = gpui::Rgba {
            r: 0.,
            g: 0.,
            b: 0.,
            a: 0.,
        };

        div()
            .id(("note-item", id as usize))
            .w_full()
            .px_2()
            .py_1()
            .flex()
            .flex_col()
            .gap_1()
            .rounded(px(theme::RADIUS_SM))
            .bg(if is_selected {
                theme::accent_soft()
            } else {
                transparent
            })
            .border_1()
            .border_color(if is_selected {
                theme::accent()
            } else {
                transparent
            })
            .cursor_pointer()
            .on_click(
                cx.listener(move |this, _, window, cx| this.select_note_focused(id, window, cx)),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .gap_1()
                    .child(
                        div()
                            .min_w_0()
                            .flex_1()
                            .truncate()
                            .text_sm()
                            .text_color(theme::text())
                            .child(title),
                    )
                    .child(
                        Button::new(("pin", id as usize))
                            .size(ButtonSize::Icon(px(18.)))
                            .variant(ButtonVariant::Ghost)
                            .icon(icons::icon(icons::IconName::Pin, 10.).text_color(
                                if note.pinned {
                                    theme::accent()
                                } else {
                                    theme::muted_text()
                                },
                            ))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.toggle_pin(id, window, cx)
                            })),
                    ),
            )
            .child(
                div()
                    .w_full()
                    .truncate()
                    .text_xs()
                    .text_color(theme::muted_text())
                    .child(meta),
            )
            .into_any_element()
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
        // 删除二次确认进行中：在状态栏明示“再按一次确认删除”。
        let delete_armed = self.selected_id.is_some() && self.pending_delete_id == self.selected_id;
        if delete_armed {
            left = left.child(StatusMetric::new("再按一次确认删除").tone(BadgeTone::Danger));
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
                    // 二次确认进行中变红，松手即 UI 级确认（无弹窗）。
                    .variant(if delete_armed {
                        ButtonVariant::Danger
                    } else {
                        ButtonVariant::Ghost
                    })
                    .icon(
                        icons::icon(icons::IconName::Trash, 13.).text_color(if delete_armed {
                            theme::canvas()
                        } else {
                            theme::muted_text()
                        }),
                    )
                    .tooltip(if delete_armed {
                        "再次点击确认删除"
                    } else {
                        "删除"
                    })
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

impl EntityInputHandler for NoteWindow {
    fn text_for_range(
        &mut self,
        range: Range<usize>,
        _adjusted_range: &mut Option<Range<usize>>,
        window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<String> {
        if !self.search_focus.is_focused(window) {
            return None;
        }
        Some(utf16_slice(&self.search_query.value, range))
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        if !self.search_focus.is_focused(window) {
            return None;
        }
        let selection = editing_selected_range(&self.search_query);
        Some(UTF16Selection {
            range: selection.range,
            reversed: selection.reversed,
        })
    }

    fn marked_text_range(
        &self,
        window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Range<usize>> {
        if !self.search_focus.is_focused(window) {
            return None;
        }
        editing_marked_range(&self.search_query)
    }

    fn unmark_text(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.search_focus.is_focused(window) {
            editing_unmark(&mut self.search_query);
            self.on_search_changed(cx);
        }
        window.invalidate_character_coordinates();
        cx.notify();
    }

    fn replace_text_in_range(
        &mut self,
        replacement_range: Option<Range<usize>>,
        text: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.search_focus.is_focused(window) {
            return;
        }
        editing_replace(&mut self.search_query, replacement_range, text);
        self.on_search_changed(cx);
        window.invalidate_character_coordinates();
        cx.notify();
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        _range: Option<Range<usize>>,
        new_text: &str,
        _new_selected_range: Option<Range<usize>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.search_focus.is_focused(window) {
            return;
        }
        editing_mark_text(&mut self.search_query, new_text);
        self.on_search_changed(cx);
        window.invalidate_character_coordinates();
        cx.notify();
    }

    fn bounds_for_range(
        &mut self,
        range: Range<usize>,
        element_bounds: Bounds<Pixels>,
        window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        if !self.search_focus.is_focused(window) {
            return None;
        }
        let cursor = byte_index_for_utf16(&self.search_query.value, range.start);
        Some(ime_caret_bounds(
            window,
            element_bounds,
            &self.search_query.value[..cursor],
            px(12.),
            px(30.),
            px(0.),
        ))
    }

    fn character_index_for_point(
        &mut self,
        _point: Point<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        None
    }

    fn text_length_utf16(&mut self, window: &mut Window, _cx: &mut Context<Self>) -> Option<usize> {
        if !self.search_focus.is_focused(window) {
            return None;
        }
        Some(utf16_len(&self.search_query.value))
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

        // 列表顶部错误行：store 打不开 / 查询失败时可见（store 字段保持 Option 不变）。
        let mut list_top: Vec<AnyElement> = Vec::new();
        if let Some(e) = self.store_error.clone() {
            list_top.push(
                div()
                    .id("note-store-error")
                    .w_full()
                    .px_2()
                    .py_1()
                    .border_b_1()
                    .border_color(theme::danger())
                    .text_xs()
                    .text_color(theme::danger())
                    .child(SharedString::from(e))
                    .into_any_element(),
            );
        }
        if let Some(e) = self.list_error.clone() {
            list_top.push(
                div()
                    .id("note-list-error")
                    .w_full()
                    .px_2()
                    .py_1()
                    .border_b_1()
                    .border_color(theme::danger())
                    .text_xs()
                    .text_color(theme::danger())
                    .child(SharedString::from(e))
                    .into_any_element(),
            );
        }

        // 空列表占位：区分“无笔记”与“搜索无匹配”，保持 260px 列宽不溢出。
        let is_searching = !self.search_query.value.trim().is_empty();
        let empty_hint = if is_searching {
            "无匹配，新建试试"
        } else {
            "无笔记，新建试试"
        };
        let list_body: AnyElement = if notes.is_empty() {
            div()
                .id("note-list-empty")
                .flex_1()
                .flex()
                .items_center()
                .justify_center()
                .p_4()
                .text_sm()
                .text_color(theme::muted_text())
                .child(empty_hint)
                .into_any_element()
        } else {
            // 虚拟列表：只渲染可视区的行。旧实现 `.children(notes.iter().map(..))`
            // 全量渲染，几百条笔记就是几百个 div，每次按键都重绘。
            // `cx.processor` 要求 `'static`，故用 `this.notes`（render 开头已 clone
            // 到局部变量，避免借用跨越闭包边界）。
            let visible = notes.clone();
            uniform_list(
                "note-list",
                notes.len(),
                cx.processor(move |this, range: Range<usize>, _window, cx| {
                    range
                        .map(|ix| {
                            let note = visible[ix].clone();
                            this.render_note_row(&note, cx)
                        })
                        .collect::<Vec<_>>()
                }),
            )
            .track_scroll(&self.list_scroll)
            .flex_1()
            .min_h_0()
            .with_sizing_behavior(ListSizingBehavior::Auto)
            .into_any_element()
        };

        // 筛选条收敛到侧栏顶部：与侧栏/Git 同一样式（框内搜索图标 + 统一 m_2 边距）。
        let search = filter_row("note-search-wrap").child(
            filter_text_input(
                "note-search",
                self.search_focus.clone(),
                self.search_query.value.clone(),
                "搜索...",
                self.search_query.ime_marked_text.clone(),
                self.search_query.selection(),
                self.search_query.cursor,
            )
            .entity(cx.entity())
            .on_key_down(cx.listener(Self::handle_search_key)),
        );
        // 列表栏宽度可拖拽（SplitResizer 直接写 list_width，拖拽中每次鼠标移动重绘）。
        let list_width = clamp_list_pane_width(self.list_width.get());
        let resizer = SplitResizer::new(
            "note-list-resize",
            self.list_dragging.clone(),
            self.list_width.clone(),
        )
        .min_width(LIST_PANE_MIN_WIDTH)
        .max_width(LIST_PANE_MAX_WIDTH);
        let list = div()
            .relative()
            .w(px(list_width))
            .flex_shrink_0()
            .h_full()
            .flex()
            .flex_col()
            .overflow_hidden()
            .border_r_1()
            .border_color(theme::border())
            .bg(theme::surface())
            .child(search)
            .children(list_top)
            .child(list_body)
            .child(resizer);
        let right: AnyElement = if self.preview {
            let md = self.content_state.read(cx).value().to_string();
            div()
                .id("note-preview")
                .flex_1()
                .min_h_0()
                .h_full()
                .min_w(px(320.))
                .bg(theme::canvas())
                .text_color(theme::text())
                .overflow_y_scroll()
                // 正文按阅读宽度收窄并居中：满宽长行在 900px 窗口里难以阅读，
                // 代码块也不再撑破容器。外层滚动，TextView 自身不滚动。
                // 左右留白与编辑模式（`EDITOR_PADDING_X`）保持一致，
                // 免得切换预览/编辑时正文左右跳动。
                .child(
                    div()
                        .w_full()
                        .max_w(px(PREVIEW_MAX_WIDTH))
                        .mx_auto()
                        .px(px(EDITOR_PADDING_X))
                        .py_4()
                        .child(render_markdown(&md)),
                )
                .into_any_element()
        } else {
            // 落库失败横幅：贴在编辑区上方。用户此前只能看到"保存没反应"+ 永久亮着的
            // "未保存"，无从判断是磁盘满、库损坏还是别的。
            let persist_banner: Option<AnyElement> = self.persist_error.clone().map(|e| {
                div()
                    .id("note-persist-error")
                    .w_full()
                    .flex_shrink_0()
                    .mx(px(EDITOR_PADDING_X))
                    .mt(px(EDITOR_PADDING_Y))
                    .px_3()
                    .py_2()
                    .mb_2()
                    .rounded(px(theme::RADIUS_SM))
                    .bg(theme::danger().opacity(0.12))
                    .border_1()
                    .border_color(theme::danger())
                    .text_xs()
                    .text_color(theme::danger())
                    .child(SharedString::from(e))
                    .into_any_element()
            });
            // 编辑区铺满整个右栏：原先在外面套 `p_2` + 边框 + 圆角做成"卡片"，
            // 于是分隔线与列表之间空出 8px，还多出一圈与左侧列表冲突的圆角框。
            // 呼吸空间改由 `TextareaState::set_editor_paddings` 提供（见 `new`），
            // 它会把 padding 计入滚动条几何，滚动到底不会切掉最后一行。
            div()
                .flex_1()
                .min_h_0()
                .h_full()
                .min_w(px(320.))
                .flex()
                .flex_col()
                .overflow_hidden()
                .bg(theme::canvas())
                .text_color(theme::text())
                .children(persist_banner)
                .child(
                    div()
                        .flex_1()
                        .min_h_0()
                        .h_full()
                        .w_full()
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

        // 查找面板浮在编辑区之上（预览模式下无意义，不挂）。
        let find_panel: Option<AnyElement> =
            (self.find_open && !self.preview).then(|| self.render_find_panel(window, cx));

        let context_menu = self.context_menu.clone();
        let mut root = div()
            .size_full()
            .flex()
            .flex_col()
            .bg(theme::canvas())
            .text_color(theme::text())
            .track_focus(&self.window_focus)
            .on_action(
                cx.listener(|this, _: &CloseNoteWindow, window, cx| this.request_close(window, cx)),
            )
            .on_action(cx.listener(|this, _: &NewNote, window, cx| this.new_note(window, cx)))
            .on_action(cx.listener(|this, _: &SaveNote, window, cx| this.save_current(window, cx)))
            .on_action(
                cx.listener(|this, _: &TogglePreview, window, cx| this.toggle_preview(window, cx)),
            )
            // Escape 只退出预览；预览已关时兜底走脏检查关窗，避免"按了没反应"。
            .on_action(cx.listener(|this, _: &EscapePreview, window, cx| {
                if this.preview {
                    this.toggle_preview(window, cx);
                } else {
                    this.request_close(window, cx);
                }
            }))
            .on_action(
                cx.listener(|this, _: &DeleteNote, window, cx| this.delete_current(window, cx)),
            )
            // 列表键盘导航：Up/ctrl-p 上一条，Down/ctrl-n 下一条（复用脏保护）。
            .on_action(cx.listener(|this, _: &SelectNextNote, window, cx| {
                this.move_selection(1, window, cx)
            }))
            .on_action(cx.listener(|this, _: &SelectPrevNote, window, cx| {
                this.move_selection(-1, window, cx)
            }))
            .key_context(NOTE_WINDOW_CONTEXT)
            .on_action(cx.listener(|this, _: &ToggleFind, window, cx| this.toggle_find(window, cx)))
            .on_action(cx.listener(|this, _: &FindNext, _, cx| this.find_next(cx)))
            .on_action(cx.listener(|this, _: &FindPrev, _, cx| this.find_prev(cx)))
            .on_action(cx.listener(|this, _: &CloseFind, window, cx| this.close_find(window, cx)))
            .children(linux_titlebar)
            .child(body)
            .children(find_panel)
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

        crossh_ui::client_decorations::client_side_decorations(root, window, cx)
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

    /// 查找动作会走到编辑器的命中/高亮渲染，那条路径读 `GlobalState`。
    /// `NoteWindow::new` 不初始化它（生产路径由 `crossh_editor::init` 负责），
    /// 所以凡是触发查找面板的测试都要先补上，否则 panic。
    fn init_editor_state(cx: &mut TestAppContext) {
        cx.update(crossh_editor::init);
    }

    #[gpui::test]
    fn note_window_historic_note_loads_in_edit_state(cx: &mut TestAppContext) {
        let window = cx.add_window(NoteWindow::new_for_test);
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
        let window = cx.add_window(NoteWindow::new_for_test);
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
        let window = cx.add_window(NoteWindow::new_for_test);
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
        let window = cx.add_window(NoteWindow::new_for_test);
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

    fn sample_note(id: i64, content: &str) -> Note {
        Note {
            id,
            content: content.to_string(),
            pinned: false,
            created_at: 0,
            updated_at: 0,
        }
    }

    #[gpui::test]
    fn note_dirty_switch_without_store_keeps_moving(cx: &mut TestAppContext) {
        let window = cx.add_window(NoteWindow::new_for_test);
        cx.run_until_parked();
        window
            .update(cx, |note_window, _window, cx| {
                // 无库时无法落库但不阻塞切换（确定性路径，不碰真实笔记库）。
                let n1 = sample_note(1, "first");
                let n2 = sample_note(2, "second");
                note_window.notes = vec![n1.clone(), n2.clone()];
                note_window.selected_id = Some(n1.id);
                note_window
                    .content_state
                    .update(cx, |s, cx| s.set_value_simple(n1.content.clone(), cx));
                // 脏编辑后切换：直接切到 n2，不丢 dirty（无库则放行）。
                note_window
                    .content_state
                    .update(cx, |s, cx| s.set_value_simple("first 草稿", cx));
                assert!(note_window.is_dirty(cx));
                note_window.select_note(n2.id, cx);
                assert_eq!(note_window.selected_id, Some(n2.id));
                assert_eq!(
                    note_window.content_state.read(cx).value().to_string(),
                    "second"
                );
                // 同一条重复点击不折腾编辑器。
                note_window
                    .content_state
                    .update(cx, |s, cx| s.set_value_simple("second 草稿", cx));
                note_window.select_note(n2.id, cx);
                assert_eq!(
                    note_window.content_state.read(cx).value().to_string(),
                    "second 草稿"
                );
            })
            .unwrap();
    }

    #[gpui::test]
    fn note_delete_requires_second_confirm(cx: &mut TestAppContext) {
        let window = cx.add_window(NoteWindow::new_for_test);
        cx.run_until_parked();
        window
            .update(cx, |note_window, window, cx| {
                let n1 = sample_note(1, "first");
                let n2 = sample_note(2, "second");
                note_window.notes = vec![n1.clone(), n2.clone()];
                note_window.selected_id = Some(n1.id);
                note_window
                    .content_state
                    .update(cx, |s, cx| s.set_value_simple(n1.content.clone(), cx));
                // 第一次：只标记，不删除。
                note_window.delete_current(window, cx);
                assert_eq!(note_window.selected_id, Some(n1.id));
                assert_eq!(note_window.pending_delete_id, Some(n1.id));
                assert_eq!(
                    note_window.content_state.read(cx).value().to_string(),
                    "first"
                );
                // 第二次：真删。库中 n1 已不在（此处手动模拟库侧删除），
                // reload 后自动选中剩余第一条 n2。
                note_window.notes = vec![n2.clone()];
                note_window.delete_current(window, cx);
                assert_eq!(note_window.pending_delete_id, None);
                assert_eq!(note_window.selected_id, Some(n2.id));
                assert_eq!(
                    note_window.content_state.read(cx).value().to_string(),
                    "second"
                );
                // 空库时真删：选中清空、编辑器清空。
                note_window.delete_current(window, cx);
                assert_eq!(note_window.pending_delete_id, Some(n2.id));
                note_window.notes = Vec::new();
                note_window.delete_current(window, cx);
                assert_eq!(note_window.selected_id, None);
                assert_eq!(note_window.pending_delete_id, None);
                assert_eq!(note_window.content_state.read(cx).value().to_string(), "");
            })
            .unwrap();
    }

    #[gpui::test]
    fn note_move_selection_clamps_and_focuses(cx: &mut TestAppContext) {
        let window = cx.add_window(NoteWindow::new_for_test);
        cx.run_until_parked();
        window
            .update(cx, |note_window, window, cx| {
                let n1 = sample_note(1, "first");
                let n2 = sample_note(2, "second");
                note_window.notes = vec![n1.clone(), n2.clone()];
                note_window.selected_id = None;
                note_window.move_selection(1, window, cx);
                assert_eq!(note_window.selected_id, Some(n1.id));
                note_window.move_selection(1, window, cx);
                assert_eq!(note_window.selected_id, Some(n2.id));
                // 越界钳制：已在末尾则不动。
                note_window.move_selection(1, window, cx);
                assert_eq!(note_window.selected_id, Some(n2.id));
                note_window.move_selection(-1, window, cx);
                assert_eq!(note_window.selected_id, Some(n1.id));
                // 越界钳制：已在开头则不动。
                note_window.move_selection(-1, window, cx);
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
                // 空列表直接忽略，不 panic。
                note_window.notes = Vec::new();
                note_window.selected_id = None;
                note_window.move_selection(1, window, cx);
                assert_eq!(note_window.selected_id, None);
            })
            .unwrap();
    }

    #[gpui::test]
    fn note_store_error_shows_in_title(cx: &mut TestAppContext) {
        let window = cx.add_window(NoteWindow::new_for_test);
        cx.run_until_parked();
        window
            .update(cx, |note_window, _window, cx| {
                note_window.store_error = Some("笔记库打开失败：测试".to_string());
                note_window.notes = Vec::new();
                note_window.selected_id = None;
                assert_eq!(note_window.status_title(cx), "笔记库打开失败");
            })
            .unwrap();
    }

    #[gpui::test]
    fn note_search_reload_is_debounced(cx: &mut TestAppContext) {
        use std::time::Duration;

        // 用临时库而不是真实笔记库：断言要确定，且不污染用户数据。
        let dir = tempfile::TempDir::new().unwrap();
        let store = NoteStore::open(dir.path().join("note.db")).unwrap();
        store.create("alpha").unwrap();

        let window = cx.add_window(NoteWindow::new_for_test);
        cx.run_until_parked();
        window
            .update(cx, |note_window, window, cx| {
                note_window.store = Some(store);
                note_window.refresh_list();
                // 塞入哨兵：同步 reload 会立刻把它清掉。
                note_window.notes.push(sample_note(-999, "哨兵"));
                note_window.search_query = TextEditingState::new("zzz-无匹配");
                window.focus(&note_window.search_focus, cx);
                note_window.on_search_changed(cx);
            })
            .unwrap();
        cx.run_until_parked();
        window
            .update(cx, |note_window, _window, _cx| {
                // 代际已推进，但防抖期内未查库：哨兵仍在。
                assert_eq!(note_window.search_generation, 1);
                assert_eq!(note_window.search_query.value, "zzz-无匹配");
                assert!(note_window.notes.iter().any(|n| n.id == -999));
            })
            .unwrap();
        cx.executor().advance_clock(Duration::from_millis(500));
        cx.run_until_parked();
        window
            .update(cx, |note_window, _window, _cx| {
                // 防抖到期后 reload 生效：哨兵被查询结果取代（此处无匹配，故为空）。
                assert!(
                    !note_window.notes.iter().any(|n| n.id == -999),
                    "防抖到期后应重新查库，哨兵不再留在列表里"
                );
            })
            .unwrap();
    }

    #[gpui::test]
    fn note_search_escape_clears_before_close(cx: &mut TestAppContext) {
        let window = cx.add_window(NoteWindow::new_for_test);
        cx.run_until_parked();
        window
            .update(cx, |note_window, window, cx| {
                note_window.search_query = TextEditingState::new("abc");
                window.focus(&note_window.search_focus, cx);
                // 有内容：清空并截断（不关窗口）。
                assert!(note_window.clear_search_on_escape(window, cx));
                assert!(note_window.search_query.value.is_empty());
                // 已空：放行冒泡（关窗口逻辑不变）。
                assert!(!note_window.clear_search_on_escape(window, cx));
            })
            .unwrap();
    }

    #[gpui::test]
    fn note_find_toggles_and_matches_content(cx: &mut TestAppContext) {
        let window = cx.add_window(NoteWindow::new_for_test);
        cx.run_until_parked();
        window
            .update(cx, |note_window, window, cx| {
                note_window
                    .content_state
                    .update(cx, |s, cx| s.set_value_simple("alpha beta alpha", cx));
                assert!(!note_window.find_open);

                // 开面板：查询为空时不产生匹配。
                note_window.toggle_find(window, cx);
                assert!(note_window.find_open);
                assert!(note_window.content_state.read(cx).search_session().open);

                // 敲入查询 -> 引擎算出 2 处匹配。
                note_window.find_query = TextEditingState::new("alpha");
                note_window.apply_find_query(cx);
                assert_eq!(
                    note_window
                        .content_state
                        .read(cx)
                        .search_session()
                        .matcher
                        .matched_ranges()
                        .len(),
                    2
                );

                // 翻到下一处：选中区间应落在第二个 alpha。
                note_window.find_next(cx);
                let selected = note_window.content_state.read(cx).selected_range();
                assert_eq!(selected, 11..16);

                // 无匹配的查询不产生命中，选区也不应被改动。
                note_window.find_query = TextEditingState::new("zzz");
                note_window.apply_find_query(cx);
                assert!(
                    note_window
                        .content_state
                        .read(cx)
                        .search_session()
                        .matcher
                        .matched_ranges()
                        .is_empty()
                );
                note_window.find_next(cx);
                assert_eq!(
                    note_window.content_state.read(cx).selected_range(),
                    11..16,
                    "无匹配时 find_next 不得改写选区"
                );

                // 关面板：匹配会话关闭。
                note_window.close_find(window, cx);
                assert!(!note_window.find_open);
                assert!(!note_window.content_state.read(cx).search_session().open);
            })
            .unwrap();
    }

    #[gpui::test]
    fn note_switching_notes_resets_find(cx: &mut TestAppContext) {
        init_editor_state(cx);
        let window = cx.add_window(NoteWindow::new_for_test);
        cx.run_until_parked();
        window
            .update(cx, |note_window, window, cx| {
                let n1 = sample_note(1, "first note");
                let n2 = sample_note(2, "second note");
                note_window.notes = vec![n1.clone(), n2.clone()];
                note_window.selected_id = Some(n1.id);
                note_window
                    .content_state
                    .update(cx, |s, cx| s.set_value_simple(n1.content.clone(), cx));
                note_window.toggle_find(window, cx);
                note_window.find_query = TextEditingState::new("note");
                note_window.apply_find_query(cx);

                // 换笔记：查询与匹配必须一并重置，否则高亮落在旧文本的位置上。
                note_window.select_note(n2.id, cx);
                assert!(!note_window.find_open);
                assert!(note_window.find_query.value.is_empty());
                assert!(!note_window.content_state.read(cx).search_session().open);
            })
            .unwrap();
    }

    #[gpui::test]
    fn note_find_closes_when_entering_preview(cx: &mut TestAppContext) {
        init_editor_state(cx);
        let window = cx.add_window(NoteWindow::new_for_test);
        cx.run_until_parked();
        window
            .update(cx, |note_window, window, cx| {
                note_window.toggle_find(window, cx);
                assert!(note_window.find_open);
                note_window.toggle_preview(window, cx);
                assert!(note_window.preview);
                assert!(!note_window.find_open, "预览是只读视图，不该留着查找会话");
            })
            .unwrap();
    }

    #[gpui::test]
    fn note_editor_uses_inner_padding_not_outer_card(cx: &mut TestAppContext) {
        // 回归：编辑区原先在 Textarea 外面套 `p_2` + 边框 + 圆角做"卡片"，
        // 于是分隔线与行号之间空出 8px，且多出一圈与左侧列表冲突的圆角框。
        // 现在外壳是齐边的，呼吸空间由 `set_editor_paddings` 提供。
        let window = cx.add_window(NoteWindow::new_for_test);
        cx.run_until_parked();
        window
            .update(cx, |note_window, _window, cx| {
                let paddings = note_window.content_state.read(cx).editor_paddings();
                assert_eq!(
                    paddings.left,
                    px(EDITOR_PADDING_X),
                    "行号栏左侧应留出与其它编辑器一致的呼吸空间"
                );
                assert_eq!(paddings.top, px(EDITOR_PADDING_Y));
                // 右侧比左侧宽，给滚动条让位，滚动条不会压住文字。
                assert!(
                    paddings.right > paddings.left,
                    "右侧需额外给滚动条留位，实际 {:?}",
                    paddings
                );
            })
            .unwrap();
    }

    #[test]
    fn list_pane_width_clamps_to_bounds() {
        assert_eq!(
            clamp_list_pane_width(LIST_PANE_DEFAULT_WIDTH),
            LIST_PANE_DEFAULT_WIDTH
        );
        assert_eq!(
            clamp_list_pane_width(10.),
            LIST_PANE_MIN_WIDTH,
            "窄窗下不得低于最小宽度，否则搜索框和置顶按钮会被压没"
        );
        assert_eq!(
            clamp_list_pane_width(9999.),
            LIST_PANE_MAX_WIDTH,
            "过宽会挤掉正文区"
        );
    }
}
