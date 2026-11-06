//! Note 窗口 — 搜索框走 TextEditingState（与侧栏/Git 同一编辑语义），内容区基于 crossh-editor TextareaState。

use std::cell::Cell;
use std::collections::HashMap;
use std::ops::Range;
use std::rc::Rc;
use std::time::Duration;

use crossh_core::i18n::text;
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
    context_menu::{ContextMenuState, render_context_menu},
    filter_row, filter_text_input,
};
use gpui::{
    AnyElement, App, AppContext, Bounds, ClipboardItem, Context, Edges, Entity, EntityInputHandler,
    FocusHandle, Focusable, Hsla, InteractiveElement, IntoElement, KeyDownEvent,
    ListSizingBehavior, ParentElement, Pixels, Point, SharedString, Size,
    StatefulInteractiveElement, Styled, Subscription, TitlebarOptions, UTF16Selection,
    UniformListScrollHandle, Window, WindowBounds, WindowOptions, div, point, px, uniform_list,
};

use super::context_menu::NoteMenuAction;
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
/// 与 `NoteStore::list` 的排序一致：pinned DESC, updated_at DESC, id DESC。
/// 就地插入新笔记时要靠它找出与库同样的位置，否则下一次刷新列表顺序会跳一下。
fn note_precedes(a: &Note, b: &Note) -> bool {
    match a.pinned.cmp(&b.pinned) {
        std::cmp::Ordering::Equal => match a.updated_at.cmp(&b.updated_at) {
            std::cmp::Ordering::Equal => a.id > b.id,
            other => other.is_gt(),
        },
        other => other.is_gt(),
    }
}
// 搜索防抖时长：输入停止约 200ms 后才查库（与 hover/toast 的 timer 惯例一致）。
const SEARCH_DEBOUNCE: Duration = Duration::from_millis(200);
// 自动保存防抖时长：停止输入约 800ms 后才落库。与搜索同一套「代际 + timer」模式，
// 连续敲键只保留最后一次——每个字符都写一次 SQLite 在 WAL 下每次都带一次 fsync。
const AUTOSAVE_DEBOUNCE: Duration = Duration::from_millis(800);
// 软删行的保留期（秒）。撤销只保留一层且只在本次会话有效，30 天足够兜住「按错了」，
// 又不至于让按错留下的垃圾永久占库。
const SOFT_DELETE_RETENTION_SECS: i64 = 30 * 86400;
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

pub struct NoteWindow {
    store: Option<NoteStore>,
    // open_default 失败时的原因（store 为 None 时必有值），渲染错误行用。
    store_error: Option<String>,
    // 最近一次 list/search 失败的提示；成功后清空，失败时保留旧列表。
    list_error: Option<String>,
    // 列表顶部的中性提示（成功语气，如"已恢复，可在清空搜索后查看"）。与
    // `list_error` 同机制、不同语气：撤销恢复的笔记可能被搜索过滤掉而不在列表里，
    // 静默读起来就是"撤销没反应"。下次查询成功（`refresh_list`）时清掉。
    list_notice: Option<String>,
    notes: Vec<Note>,
    selected_id: Option<i64>,
    preview: bool,
    // 删除二次确认：第一次点击只记录目标，第二次才真删。
    pending_delete_id: Option<i64>,
    // 软删后的撤销目标：存储层的 `delete` 只打 `deleted_at`，没有这个入口的话
    // 软删行既不可达也永远不会被回收。只保留一层（新的删除覆盖上一个目标）。
    pending_restore_id: Option<i64>,
    // 搜索防抖代际计数：每次输入变更加一，只有最新一代到期后才 reload。
    search_generation: u64,
    // 自动保存代际计数：与 `search_generation` 同一套防抖写法。
    autosave_generation: u64,
    // 列表字数旁路缓存。渲染时每帧都要显示「N 字」，早前每行都 `chars().count()`
    // 扫一遍正文（单条上限 1 MiB）；改成刷新列表时算一次、渲染只读。
    // 不变量：只有 `refresh_list`（全量）与 `upsert_note_in_place`（单条）写它。
    char_counts: HashMap<i64, usize>,
    // 洗库（损坏库备份重建）后的提示：必须告诉用户旧数据被搬去了哪里，
    // 否则重建后的空列表读起来就是「笔记全没了」。
    recovery_notice: Option<String>,
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
    pub(super) context_menu: Option<ContextMenuState<NoteMenuAction>>,
    // 最近一次落库失败的原因。落库失败此前只写日志，用户看到的是"保存按钮点了没反应"，
    // 且 is_dirty 永远为真、状态栏永远挂"未保存"。改为记录并渲染在编辑区上方。
    persist_error: Option<String>,
    // 最近一次落库失败的**错误码**（`persist_error` 是给人看的文案）。
    // 关窗时只有 `not_found` 需要特殊处置：目标笔记已在别处被删，草稿无处可存，
    // 拦下关窗只会让用户永远关不掉窗口。
    persist_error_code: Option<String>,
    // 笔记内查找面板。匹配引擎在 `TextareaState.search_session` 里（crossh-editor
    // 提供），行为见 `find_panel.rs`。
    pub(super) find_query: TextEditingState,
    pub(super) find_focus: FocusHandle,
    pub(super) find_open: bool,
}

impl NoteWindow {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        // 打开失败不再吞掉：记下原因，渲染层给出错误行 + 状态栏标题。
        let mut this = Self::new_uncoupled(window, cx);
        match NoteStore::open_default() {
            Ok(store) => this.attach_store(store, cx),
            Err(e) => {
                log::warn!("note store open failed: {e}");
                this.store_error =
                    Some(rust_i18n::t!("note.error.store_open", error = e).to_string());
            }
        }
        this
    }

    /// 挂上已打开的库：先播报洗库备份，再清一把过期软删行，最后加载列表。
    fn attach_store(&mut self, store: NoteStore, cx: &mut Context<Self>) {
        // 洗过库（损坏库被备份重建）时列表是空的，用户看到的只有“笔记全没了”。
        // 备份路径是旧数据唯一的线索，必须显式给出。
        self.recovery_notice = store.recovered_from.as_ref().map(|path| {
            rust_i18n::t!(
                "note.error.recovered_backup",
                path = path.display().to_string()
            )
            .to_string()
        });
        // 软删行不能永远长下去：每次打开清一把超过保留期的。失败只 warn：
        // 行留着只是占空间，而打不开库是用户不能工作。
        if let Err(e) = store.purge_deleted(SOFT_DELETE_RETENTION_SECS) {
            log::warn!("note purge_deleted failed: {e}");
        }
        self.store = Some(store);
        self.reload_notes(cx);
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

    /// 测试专用：直接挂一个已打开的临时库（不碰用户真实笔记库）。
    ///
    /// 走与 [`NoteWindow::new`] 相同的 `attach_store`，所以洗库提示、过期软删清理、
    /// 列表加载这些真实路径都能被断言到。
    #[cfg(test)]
    pub(super) fn with_store_for_test(
        window: &mut Window,
        cx: &mut Context<Self>,
        store: NoteStore,
    ) -> Self {
        let mut this = Self::new_uncoupled(window, cx);
        this.attach_store(store, cx);
        this
    }

    /// 构造窗口但**不打开笔记库**，`new` 与 `new_for_test` 共用。
    /// 调用方负责按需挂 store 并决定是否 `reload_notes`。
    fn new_uncoupled(window: &mut Window, cx: &mut Context<Self>) -> Self {
        sync_editor_theme(cx);
        let search_focus = cx.focus_handle();
        let content_state = cx.new(|cx| {
            let mut state = TextareaState::new(window, cx)
                .placeholder(text("note.editor.placeholder"))
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
        let _content_sub = cx.subscribe(&content_clone, |this: &mut Self, _, event, cx| {
            if matches!(event, InputEvent::Change) {
                // 内容一变就排一次防抖落库：崩溃/断电最多丢 800ms 输入，
                // 而不是整篇草稿等下一个切笔记/⌘S/关窗才落库。
                this.schedule_autosave(cx);
                cx.notify();
            }
        });

        let this = Self {
            store: None,
            store_error: None,
            list_error: None,
            list_notice: None,
            notes: Vec::new(),
            selected_id: None,
            preview: false,
            pending_delete_id: None,
            pending_restore_id: None,
            search_generation: 0,
            autosave_generation: 0,
            char_counts: HashMap::new(),
            recovery_notice: None,
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
            persist_error_code: None,
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
                // 字数旁路缓存的唯一全量写点：渲染每帧要显示「N 字」，不能每次都全量扫正文。
                self.char_counts = notes
                    .iter()
                    .map(|n| (n.id, n.content.chars().count()))
                    .collect();
                self.notes = notes;
                self.list_error = None;
                // 查询成功即"操作已生效"：顶部中性提示到此为止，不必常驻。
                self.list_notice = None;
            }
            Err(e) => {
                log::warn!("note query failed: {e}");
                self.list_error = Some(text("note.error.query_failed"));
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
        // 落库是唯一需要 owned `String` 的地方（存储层签名是 `&str`）；判空与会话内
        // 比较都走借用的 `text()`，见 `is_dirty`。
        // 落库用**原文**：存储层已按原文保存，这里再 trim 会把 Markdown
        // 缩进代码块的首行缩进和结尾空行吃掉——用户每按一次保存就丢一部分内容。
        // trim 只能用来判「这算不算空」。
        let raw = self.content_state.read(cx).text().to_string();
        if raw.chars().all(char::is_whitespace) {
            return true;
        }
        // `store` 是 `self` 字段的借用，先在块内用完，后面才能改 `self` 的其它字段。
        let result = {
            let Some(store) = self.store.as_ref() else {
                return true;
            };
            match self.selected_id {
                // 直接按 id 更新，不受搜索过滤影响（被过滤掉的选中项也能存）。
                Some(id) => match store.update(id, &raw) {
                    // 目标行被外部删除/软删后 update 恒返回 `not_found`。就此失败的话，
                    // `select_note` / `new_note` / `reload_notes` 的脏保护会全部早退：
                    // 切不了笔记、建不了新笔记、"未保存"常驻，草稿的出路只剩关窗丢掉。
                    // 回退为 create 把草稿存成新笔记，收尾逻辑与 `Ok` 分支相同。
                    Err(e) if e == "not_found" => store.create(&raw),
                    other => other,
                },
                None => store.create(&raw),
            }
        };
        self.apply_persist_result(result)
    }

    /// 落库结果归集：成功则清错误、把落库后的 `Note` 并回内存列表；失败则挂错误横幅。
    fn apply_persist_result(&mut self, result: Result<Note, String>) -> bool {
        match result {
            Ok(note) => {
                self.persist_error = None;
                self.persist_error_code = None;
                self.selected_id = Some(note.id);
                self.upsert_note_in_place(note);
                true
            }
            Err(e) => {
                log::warn!("note persist failed: {e}");
                self.persist_error = Some(persist_error_message(&e));
                self.persist_error_code = Some(e);
                false
            }
        }
    }

    /// 把落库后的 `Note` 并回内存列表，**故意不动排序**：
    ///
    /// - 已有的行就地换 content/updated_at、保持列表位置。`list()` 按 `updated_at`
    ///   排序，自动保存每 800ms 成功一次就 `refresh_list` 的话，正在编辑的那行会不断
    ///   跳到列表顶部、光标下的行跑掉。代价：排序与「刚刚/N 分钟前」的显示滞后到下次
    ///   刷新（切笔记 / ⌘S / 搜索）才更新——打字时不跳行比相对时间实时更值钱。
    /// - 新建的行（首次自动保存一篇草稿）按 `list()` 的排序
    ///   （pinned DESC, updated_at DESC, id DESC）插到对应位置，否则它压根不出现在列表里。
    fn upsert_note_in_place(&mut self, note: Note) {
        // 字数缓存的单条写点（见 `char_counts` 的不变量）。
        self.char_counts
            .insert(note.id, note.content.chars().count());
        if let Some(existing) = self.notes.iter_mut().find(|n| n.id == note.id) {
            existing.content = note.content;
            existing.updated_at = note.updated_at;
            existing.pinned = note.pinned;
            return;
        }
        // 首次自动保存（create）会多出一条新笔记。搜索词生效时不能插进本地列表：
        // 它会作为一条**不匹配当前过滤条件**的行出现在结果里（`save_current` 之后
        // 会 `refresh_list` 自愈，autosave 不会）。此时不动本地列表，
        // 下次 `refresh_list` 自然把它带回（匹配该词时）。
        if !self.search_query.value.trim().is_empty() {
            return;
        }
        let at = self
            .notes
            .iter()
            .position(|n| note_precedes(&note, n))
            .unwrap_or(self.notes.len());
        self.notes.insert(at, note);
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
            // 这里**不能**复用 `select_note`：它的脏保护会看到 `selected_id` 已被
            // `refresh_list` 清成 `None`、编辑器里还留着上一篇的正文，于是判脏、
            // 走 `persist_if_dirty` 的 `None => store.create(raw)` 分支，把上一篇
            // （刚被搜索过滤掉、列表里已看不见）`create` 成一条重复笔记。
            // 草稿在上面的 persist 里已按 id 落库，此处直接就地载入即可。
            // 与 `select_note` 一致：切换选中即清掉「待确认删除」标记，否则标记会跨自动
            // 选中保留（用户刚标记删除 A、随后列表又自动选回 A），下一次点删除跳过二次确认。
            self.pending_delete_id = None;
            self.selected_id = Some(first.id);
            self.reset_find(cx);
            self.content_state.update(cx, |s, cx| {
                s.set_value_simple(first.content.clone(), cx);
            });
            self.preview = false;
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
        // 失同步护栏：id 不在本地列表时**清空选中并返回**，绝不先把 `selected_id`
        // 设成它。否则选中项指向一条不在 `notes` 里的笔记、编辑器还留着上一篇正文，
        // `is_dirty` 退化成"非空即脏"，autosave 就会把上一篇的正文写进这个 id
        // ——"把 A 的内容写进 B"。
        let Some(note) = self.notes.iter().find(|n| n.id == id).cloned() else {
            self.selected_id = None;
            cx.notify();
            return;
        };
        self.selected_id = Some(id);
        self.reset_find(cx);
        self.content_state
            .update(cx, |s, cx| s.set_value_simple(note.content.clone(), cx));
        self.preview = false;
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
        // 判空借用 `&Rope`；落库用原文（见 `persist_if_dirty`）。
        if self
            .content_state
            .read(cx)
            .text()
            .chars()
            .all(char::is_whitespace)
        {
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

    /// 内容变更后的防抖落库调度。与 `on_search_changed` 同一套写法：
    /// 代际加一、到期时只有仍是最新一代才落库，连续敲键只保留最后一次。
    fn schedule_autosave(&mut self, cx: &mut Context<Self>) {
        self.autosave_generation = self.autosave_generation.wrapping_add(1);
        let generation = self.autosave_generation;
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(AUTOSAVE_DEBOUNCE).await;
            this.update(cx, |this, cx| {
                if this.autosave_generation != generation {
                    return;
                }
                this.autosave(cx);
            })
            .ok();
        })
        .detach();
    }

    /// 防抖到期后落库。直接复用 `persist_if_dirty`：
    /// 不脏 / 空内容 / 无库都直接返回，失败沿用编辑区上方的错误横幅，不另写一套。
    fn autosave(&mut self, cx: &mut Context<Self>) {
        if !self.persist_if_dirty(cx) {
            log::warn!("note autosave failed");
        }
        // 不 `refresh_list`：落库结果已由 `upsert_note_in_place` 并回列表，
        // 列表位置保持不变（原因见 `upsert_note_in_place` 的注释）。
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
        if let Some(store) = &self.store {
            match store.delete(id) {
                Ok(()) => {
                    // 存储层是软删：留一个撤销入口，否则接口行既不可达也永远不会被回收。
                    // 只保留一层（新的删除覆盖上一个目标），够兜住“手滑按了两次”。
                    self.pending_restore_id = Some(id);
                }
                // `not_found` 表示这行已被别处删掉（或已软删）——用户要的结果已经达成。
                // 若就此挂错早退，列表里那行还会留在内存里，用户再点两次得到同一个错，
                // 窗口看起来永远删不掉。所以当成功处理，走与 `Ok` 相同的 reload 收尾。
                // 撤销目标照旧记下：目标若是别处软删的，restore 能正常拉回来；
                // 真被物理清理了，`undo_delete` 已有一条 graceful 的失败提示。
                Err(e) if e == "not_found" => {
                    self.pending_restore_id = Some(id);
                }
                Err(e) => {
                    log::warn!("note delete failed: {e}");
                    self.list_error = Some(text("note.error.delete_failed"));
                    cx.notify();
                    return;
                }
            }
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

    /// 撤销上一次软删：恢复后重新加载；只有被恢复的笔记**确实在列表里可见**时
    /// 才选中并聚焦它。
    fn undo_delete(&mut self, id: i64, window: &mut Window, cx: &mut Context<Self>) {
        let result = self.store.as_ref().map(|store| store.restore(id));
        match result {
            Some(Ok(())) => {
                self.pending_restore_id = None;
                self.reload_notes(cx);
                if self.notes.iter().any(|n| n.id == id) {
                    self.select_note_focused(id, window, cx);
                } else {
                    // 搜索过滤把恢复的笔记挡在列表外：不能借机把用户从正在编辑的
                    // 笔记上拉走（选中与焦点都保持原样），也不能静默——撤销按钮与
                    // "已删除 · 可撤销"会同时消失，用户读到的是"撤销没反应"。
                    // 故只给一条中性提示，指向重新看见它的办法。
                    self.list_notice = Some(text("note.error.restored_hidden"));
                }
                cx.notify();
            }
            Some(Err(e)) => {
                log::warn!("note restore failed: {e}");
                // 目标已被物理清理（超出保留期）或在别处已恢复：撤销目标失效。
                self.pending_restore_id = None;
                self.reload_notes(cx);
                // reload 成功会清空 list_error，所以提示要在 reload 之后写。
                self.list_error = Some(text("note.error.undo_failed"));
                cx.notify();
            }
            None => {}
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
    /// - 例外：失败原因是 `not_found`（该笔记已被另一个窗口/实例删掉）→ 放行。
    ///   此时草稿在那条记录上**再也存不进去**，拦下只会让用户永远关不掉窗口，
    ///   只能强杀进程——关不掉本身就成了丢稿的原因。
    ///   这一支现在只是纯兜底：`persist_if_dirty` 已把 `not_found` 回退成 create，
    ///   正常路径下不会再拿到这个错误码。
    ///
    /// 无库时 `persist_if_dirty` 返回 true 是切换路径的有意语义（不阻塞浏览），
    /// 这里必须区别对待。
    fn request_close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.is_dirty(cx) && self.store.is_none() {
            self.persist_error = Some(text("note.error.store_unavailable"));
            cx.notify();
            return;
        }
        if self.persist_if_dirty(cx) {
            window.remove_window();
            return;
        }
        if self.persist_error_code.as_deref() == Some("not_found") {
            log::warn!("closing note window: draft has nowhere to go (target note was deleted)");
            window.remove_window();
            return;
        }
        cx.notify();
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

    fn is_dirty(&self, cx: &App) -> bool {
        // 借用 `&Rope` 比较：`value()` 每次调用都把整篇正文（单条上限 1 MiB）
        // 物化成新的 `SharedString`，而状态栏每帧要问好几次。`chars()` 逐字符比较
        // 在第一个不同处短路，不需要双方先算长度。
        // 比较用**原文**，trim 只用于判「是否算空」：旧实现 trim 后比较，库里原文
        // 带首尾空白时每次都判脏（恒脏），还会把 Markdown 缩进代码块的首行缩进
        // 当成"没改动"。
        let state = self.content_state.read(cx);
        let text = state.text();
        if let Some(id) = self.selected_id
            && let Some(note) = self.notes.iter().find(|n| n.id == id)
        {
            return !text.chars().eq(note.content.chars());
        }
        !text.chars().all(char::is_whitespace)
    }

    fn is_save_disabled(&self, cx: &App) -> bool {
        // 判空也借用 `&Rope`：与 `is_dirty` 同一理由，别为判空物化整篇正文。
        let blank = {
            let state = self.content_state.read(cx);
            state.text().chars().all(char::is_whitespace)
        };
        blank || !self.is_dirty(cx)
    }

    fn status_title(&self, cx: &App) -> String {
        // 笔记库打不开时 notes 为空，在标题处直接给出简化原因。
        if self.store_error.is_some() {
            return text("note.status.no_store");
        }
        if let Some(id) = self.selected_id
            && let Some(note) = self.notes.iter().find(|n| n.id == id)
        {
            return note_title(&note.content, 30);
        }
        if self.is_dirty(cx) {
            return text("note.status.new_note");
        }
        if self.notes.is_empty() {
            return text("note.status.no_notes");
        }
        text("note.status.no_selection")
    }

    /// 列表单行：标题（首行，超长加省略号）+ 第二行小字（更新时间 · 字数）+ 置顶按钮。
    /// `uniform_list` 只对可视区调用本方法，故行内必须自带完整 padding/border。
    fn render_note_row(&mut self, note: &Note, cx: &mut Context<Self>) -> AnyElement {
        let is_selected = Some(note.id) == self.selected_id;
        let id = note.id;
        let pin_label = if note.pinned { "📌 " } else { "" };
        let title = format!("{}{}", pin_label, note_title(&note.content, 30));
        let meta = rust_i18n::t!(
            "note.list.meta",
            time = format_note_time(note.updated_at),
            count = self
                .char_counts
                .get(&note.id)
                .copied()
                .unwrap_or_else(|| note.content.chars().count())
        )
        .to_string();
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
        // `len_chars()` 是 O(1) 的树长度，不扫全文（早前是 `value().chars().count()`，
        // 每帧把整篇正文物化再数一遍）。
        let char_count = self.content_state.read(cx).text().len_chars();
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
            left = left.child(
                StatusMetric::new(
                    rust_i18n::t!("note.status.note_count", count = total).to_string(),
                )
                .tone(BadgeTone::Neutral),
            );
        }
        if pinned > 0 {
            left = left.child(StatusMetric::new(format!("📌 {}", pinned)).tone(BadgeTone::Accent));
        }
        if dirty {
            left =
                left.child(StatusMetric::new(text("note.status.unsaved")).tone(BadgeTone::Warning));
        }
        // 删除二次确认进行中：在状态栏明示“再按一次确认删除”。
        let delete_armed = self.selected_id.is_some() && self.pending_delete_id == self.selected_id;
        if delete_armed {
            left = left.child(
                StatusMetric::new(text("note.status.confirm_delete")).tone(BadgeTone::Danger),
            );
        }
        // 软删后可撤销：与“再按一次确认删除”同一处表达状态，不引入 Toast/弹窗。
        let undo_armed = self.pending_restore_id.is_some();
        if undo_armed {
            left = left.child(
                StatusMetric::new(text("note.status.deleted_undoable")).tone(BadgeTone::Warning),
            );
        }
        if self.preview {
            left = left.child(StatusMetric::new(text("note.action.preview")).tone(BadgeTone::Info));
        } else if char_count > 0 {
            left = left.child(
                StatusMetric::new(
                    rust_i18n::t!("note.status.char_count", count = char_count).to_string(),
                )
                .tone(BadgeTone::Neutral),
            );
        }

        let mut actions = div()
            .flex_shrink_0()
            .flex()
            .items_center()
            .gap_1()
            .child(
                Button::new("note-status-new")
                    .size(ButtonSize::Icon(px(22.)))
                    .variant(ButtonVariant::Ghost)
                    .icon(icons::icon(icons::IconName::Plus, 13.).text_color(theme::muted_text()))
                    .tooltip(text("note.action.new_note"))
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
                    .tooltip(text("note.action.save"))
                    .disabled(save_disabled)
                    .on_click(cx.listener(|this, _, window, cx| this.save_current(window, cx))),
            );
        // 软删后**额外**多一个撤销位，而不是用它替换删除按钮：`pending_restore_id`
        // 会一直留到下次删除或成功查询，替换的话鼠标用户在整个会话里都点不到删除
        // （⌘D 只绑在编辑器的 Input context，列表侧没有入口）。
        // 代价只是多一个 22px 图标位。
        if let Some(id) = self.pending_restore_id {
            actions = actions.child(
                Button::new("note-status-undo")
                    .size(ButtonSize::Icon(px(22.)))
                    .variant(ButtonVariant::Ghost)
                    .icon(icons::icon(icons::IconName::RefreshCw, 13.).text_color(theme::accent()))
                    .tooltip(text("note.action.undo_delete"))
                    .on_click(
                        cx.listener(move |this, _, window, cx| this.undo_delete(id, window, cx)),
                    ),
            );
        }
        let actions = actions
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
                        text("note.action.confirm_delete")
                    } else {
                        text("note.action.delete")
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
                    .tooltip(if self.preview {
                        text("note.action.edit")
                    } else {
                        text("note.action.preview")
                    })
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
        // 列表顶部错误行：store 打不开 / 查询失败 / 洗过库时可见。
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
        // 列表顶部中性提示行（如撤销恢复被搜索挡住）。与错误行同构，但用 accent 色：
        // 这不是"现在坏了"，而是"操作生效了，只是当前视图看不到"。
        if let Some(msg) = self.list_notice.clone() {
            list_top.push(
                div()
                    .id("note-list-notice")
                    .debug_selector(|| "note-list-notice".into())
                    .w_full()
                    .px_2()
                    .py_1()
                    .border_b_1()
                    .border_color(theme::accent())
                    .text_xs()
                    .text_color(theme::accent())
                    .child(SharedString::from(msg))
                    .into_any_element(),
            );
        }
        // 洗库提示：与错误行同构（列表顶部一条醒目行），但用 warning 色——
        // 这不是“现在坏了”，而是“旧数据已经搬到别处了”，指向还在的备份文件。
        if let Some(msg) = self.recovery_notice.clone() {
            list_top.push(
                div()
                    .id("note-recovery-notice")
                    .debug_selector(|| "note-recovery-notice".into())
                    .w_full()
                    .px_2()
                    .py_1()
                    .border_b_1()
                    .border_color(theme::warning())
                    .text_xs()
                    .text_color(theme::warning())
                    .child(SharedString::from(msg))
                    .into_any_element(),
            );
        }

        // 空列表占位：区分“无笔记”与“搜索无匹配”，保持 260px 列宽不溢出。
        let is_searching = !self.search_query.value.trim().is_empty();
        let empty_hint = if is_searching {
            text("note.list.no_matches")
        } else {
            text("note.list.empty")
        };
        let list_body: AnyElement = if self.notes.is_empty() {
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
            // processor 内按索引取单行：早前在 render 与闭包外各 clone 一份 `notes`，
            // 也就是**每帧把全部笔记正文拷两遍**（单条上限 1 MiB），现在只拷可视行。
            let count = self.notes.len();
            uniform_list(
                "note-list",
                count,
                cx.processor(move |this, range: Range<usize>, _window, cx| {
                    range
                        .filter_map(|ix| {
                            // 用 `get`：列表长度与本帧传入的 count 理应一致，
                            // 但两者之间若有一帧错位，宁可少画一行也不能 panic。
                            let note = this.notes.get(ix)?.clone();
                            Some(this.render_note_row(&note, cx))
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
                text("note.list.search_placeholder"),
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
            // 预览渲染需要 owned String（渲染器拿 &str）：从 Rope 直接物化一次，
            // 不经 `value()` 中间再拷一遍。
            let md = self.content_state.read(cx).text().to_string();
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
            crossh_ui::linux_titlebar::render_linux_titlebar(window, cx, text("note.title").into());

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
                    title: Some(text("note.title").into()),
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
mod tests;
