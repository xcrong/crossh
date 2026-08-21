//! SFTP 浏览面板：远端目录列表 / 进入目录 / 上传 / 下载，进度与状态。
//!
//! 通过 `Connection::open_sftp` 拿到 `(cmd_tx, event_rx)` 后由本面板持有；
//! 主线程 drain `event_rx` 更新列表/进度。下载落到 ~/Downloads（重名自动加序号）。

use std::cell::Cell;
use std::ops::Range;
use std::rc::Rc;

use async_channel::{Receiver, Sender};
use gpui::{
    AnyElement, App, AppContext, Bounds, ClipboardEntry, Context, Entity, EntityInputHandler,
    FocusHandle, InteractiveElement, IntoElement, KeyDownEvent, MouseButton, MouseDownEvent,
    ParentElement, PathPromptOptions, Pixels, Point, Render, ScrollHandle, SharedString,
    StatefulInteractiveElement, Styled, Task, UTF16Selection, Window, canvas, div, px,
};

use crate::features::workspace::pane::{PaneRisk, WorkspacePane};
use crate::shared::i18n;
use crate::shared::text_editing::TextEditingState;
use crate::shared::utf16::{
    byte_index_for_utf16, replace_utf16_range, utf16_len, utf16_offset_for_byte, utf16_slice,
};
use crossh_ssh::{MAX_EDITOR_FILE_BYTES, RemoteEntry, SftpCmd, SftpEvent};
use crossh_ui::context_menu::{
    ContextMenuState, MenuEntry, MenuItem, SftpMenuAction, render_context_menu,
};
use crossh_ui::widgets::{ime_caret_bounds, ime_input_canvas, marked_text_span, printable_char};
use crossh_ui::{icons, theme};
use crossh_ui_component::{Button, ButtonSize, ButtonVariant, ModalDialog, TextInput, scroll_y};

use super::logic::*;

#[path = "end_caret.rs"]
mod end_caret;
use self::end_caret::EndCaretInput;
#[path = "render.rs"]
mod render;
#[path = "view_input.rs"]
mod view_input;

/// 传输进度快照。
#[derive(Clone, Debug, Default)]
struct Progress {
    label: String,
    transferred: u64,
    total: Option<u64>,
}

const SFTP_ROW_HEIGHT: f32 = 34.;
const EDITOR_ROW_HEIGHT: f32 = 20.;
const VIRTUAL_LIST_OVERSCAN: usize = 6;

struct RemoteEditor {
    remote: String,
    name: String,
    state: TextEditingState,
    read_only: bool,
    dirty: bool,
    loading: bool,
    saving: bool,
    error: Option<String>,
    focus: FocusHandle,
}

impl RemoteEditor {
    fn loading(remote: String, name: String, focus: FocusHandle) -> Self {
        Self {
            remote,
            name,
            state: TextEditingState::new(String::new()),
            read_only: true,
            dirty: false,
            loading: true,
            saving: false,
            error: None,
            focus,
        }
    }

    fn insert(&mut self, text: &str) {
        if self.state.replace_selection(text) {
            self.dirty = true;
        }
    }

    fn backspace(&mut self) {
        if self.state.backspace() {
            self.dirty = true;
        }
    }

    fn delete(&mut self) {
        if self.state.delete() {
            self.dirty = true;
        }
    }

    fn move_horizontal(&mut self, direction: i8) {
        self.state.move_horizontal(direction, false);
    }

    fn move_vertical(&mut self, direction: i8) {
        move_cursor_vertical(&self.state.value, &mut self.state.cursor, direction);
    }
}

/// 路径输入模态（重命名 / 新建目录）。
struct PendingPathInput {
    /// Some(旧名) = 重命名；None = 新建目录。
    rename_from: Option<String>,
    state: EndCaretInput,
    focus: FocusHandle,
}

/// 删除确认模态。
struct ConfirmDelete {
    name: String,
    is_dir: bool,
}

pub struct SftpPane {
    cmd_tx: Sender<SftpCmd>,
    cwd: String,
    entries: Vec<RemoteEntry>,
    message: Option<String>,
    loading: bool,
    upload_input: EndCaretInput,
    progress: Option<Progress>,
    editor: Option<RemoteEditor>,
    focus: FocusHandle,
    list_scroll: ScrollHandle,
    editor_scroll: ScrollHandle,
    _drain: Option<Task<()>>,
    _picker: Option<Task<()>>,
    /// 当前打开的右键上下文菜单。
    context_menu: Option<ContextMenuState<SftpMenuAction>>,
    /// 根 div 在窗口坐标中的 bounds（右键菜单定位/外点关闭用）。
    anchor_bounds: Rc<Cell<Option<Bounds<Pixels>>>>,
    root_focus: FocusHandle,
    focus_requested: bool,
    /// 重命名 / 新建目录输入模态。
    pending_path_input: Option<PendingPathInput>,
    /// 删除确认模态。
    confirm_delete: Option<ConfirmDelete>,
}

pub(crate) fn workspace_pane(entity: Entity<SftpPane>) -> Box<dyn WorkspacePane> {
    Box::new(SftpWorkspacePane(entity))
}

struct SftpWorkspacePane(Entity<SftpPane>);

impl WorkspacePane for SftpWorkspacePane {
    fn render(&self) -> AnyElement {
        self.0.clone().into_any_element()
    }

    fn title(&self, _cx: &App) -> String {
        crossh_core::terminal::remote_pane_title(&i18n::text("tab.sftp"))
    }

    fn terminal_entity_id(&self) -> Option<gpui::EntityId> {
        None
    }

    fn cwd(&self, _cx: &App) -> Option<String> {
        None
    }

    fn is_command_running(&self, _cx: &App) -> bool {
        false
    }

    fn run_command(&self, _command: &str, _cx: &mut App) {}

    fn handle_system_notification_response(
        &self,
        _response: &gpui::SystemNotificationResponse,
        _cx: &mut App,
    ) -> Option<bool> {
        None
    }

    fn request_focus(&self, cx: &mut App) {
        self.0.update(cx, |pane, cx| {
            pane.focus_requested = true;
            cx.notify();
        });
    }

    fn notify_language(&self, cx: &mut App) {
        self.0.update(cx, |_, cx| cx.notify());
    }

    fn risk(&self, cx: &App) -> PaneRisk {
        let pane = self.0.read(cx);
        PaneRisk {
            sftp_writes: usize::from(pane.has_active_write()),
            unsaved_editors: usize::from(pane.has_unsaved_changes()),
            ..PaneRisk::default()
        }
    }
}

impl SftpPane {
    pub(crate) fn has_active_write(&self) -> bool {
        self.progress.is_some() || self.editor.as_ref().is_some_and(|editor| editor.saving)
    }

    pub(crate) fn has_unsaved_changes(&self) -> bool {
        self.editor.as_ref().is_some_and(|editor| editor.dirty)
    }

    /// 用一个已有的 SFTP 桥接创建面板，并立即请求列出当前目录。
    pub fn from_bridge(
        cmd_tx: Sender<SftpCmd>,
        event_rx: Receiver<SftpEvent>,
        cx: &mut App,
    ) -> Entity<Self> {
        let initial_list_ok = try_send_command(
            &cmd_tx,
            SftpCmd::List {
                path: ".".to_string(),
            },
        )
        .is_ok();
        let entity = cx.new(|cx| Self {
            cmd_tx: cmd_tx.clone(),
            cwd: ".".to_string(),
            entries: Vec::new(),
            message: (!initial_list_ok).then(sftp_channel_unavailable),
            loading: initial_list_ok,
            upload_input: EndCaretInput::new(String::new()),
            progress: None,
            editor: None,
            focus: cx.focus_handle(),
            list_scroll: ScrollHandle::new(),
            editor_scroll: ScrollHandle::new(),
            _drain: None,
            _picker: None,
            context_menu: None,
            anchor_bounds: Rc::new(Cell::new(None)),
            root_focus: cx.focus_handle(),
            focus_requested: false,
            pending_path_input: None,
            confirm_delete: None,
        });

        let weak = entity.downgrade();
        let drain = cx.spawn(async move |cx| {
            while let Ok(ev) = event_rx.recv().await {
                let applied = weak.update(cx, |this, cx| {
                    match ev {
                        SftpEvent::Listed { path, entries } => {
                            this.cwd = path;
                            this.entries = entries;
                            this.message = None;
                            this.loading = false;
                        }
                        SftpEvent::FileRead { remote, contents } => {
                            if let Some(editor) = this
                                .editor
                                .as_mut()
                                .filter(|editor| editor.remote == remote)
                            {
                                editor.loading = false;
                                match String::from_utf8(contents) {
                                    Ok(content) => {
                                        editor.state.value = content;
                                        editor.state.cursor = 0;
                                        editor.state.clear_composition();
                                        editor.error = None;
                                    }
                                    Err(_) => {
                                        editor.error = Some(i18n::text("sftp.not_utf8"));
                                    }
                                }
                            }
                        }
                        SftpEvent::Progress {
                            label,
                            transferred,
                            total,
                        } => {
                            this.progress = Some(Progress {
                                label,
                                transferred,
                                total,
                            });
                        }
                        SftpEvent::Done { label, ok, message } => {
                            this.progress = None;
                            this.loading = false;
                            this.message = Some(if ok {
                                format!("{label}: {message}")
                            } else {
                                rust_i18n::t!(
                                    "sftp.operation_failed",
                                    label = label,
                                    message = message
                                )
                                .to_string()
                            });
                            // 传输完成后刷新当前目录。
                            this.request_list(this.cwd.clone());
                        }
                        SftpEvent::Saved {
                            remote,
                            ok,
                            message,
                        } => {
                            if let Some(editor) = this
                                .editor
                                .as_mut()
                                .filter(|editor| editor.remote == remote)
                            {
                                editor.saving = false;
                                if ok {
                                    editor.dirty = false;
                                    editor.error = None;
                                }
                            }
                            this.message = Some(if ok {
                                rust_i18n::t!("sftp.save_succeeded", message = message).to_string()
                            } else {
                                rust_i18n::t!("sftp.save_failed", message = message).to_string()
                            });
                        }
                        SftpEvent::Error(e) => {
                            this.progress = None;
                            this.loading = false;
                            if let Some(editor) = &mut this.editor {
                                if editor.loading {
                                    editor.error = Some(e.clone());
                                }
                                editor.loading = false;
                                editor.saving = false;
                            }
                            this.message = Some(e);
                        }
                        SftpEvent::Closed => {
                            this.progress = None;
                            this.loading = false;
                            if let Some(editor) = &mut this.editor {
                                editor.loading = false;
                                editor.saving = false;
                                editor.error = Some(i18n::text("sftp.closed"));
                            }
                            this.message = Some(i18n::text("sftp.closed"));
                        }
                    }
                    cx.notify();
                });
                if applied.is_err() {
                    break;
                }
            }
        });
        entity.update(cx, |this, _cx| this._drain = Some(drain));
        entity
    }

    fn request_list(&mut self, path: String) {
        self.loading = true;
        self.message = None;
        if try_send_command(&self.cmd_tx, SftpCmd::List { path }).is_err() {
            self.loading = false;
            self.message = Some(sftp_channel_unavailable());
        }
    }

    fn download(&mut self, name: &str) {
        let remote = join(&self.cwd, name);
        let target = downloads_dir().join(name);
        let Some(local) = unique_local_path(&target) else {
            self.message = Some(rust_i18n::t!("sftp.no_local_name", name = name).to_string());
            return;
        };
        if try_send_command(&self.cmd_tx, SftpCmd::Download { remote, local }).is_err() {
            self.message = Some(sftp_channel_unavailable());
        } else {
            self.message = Some(rust_i18n::t!("sftp.prepare_download", name = name).to_string());
        }
    }

    fn open_file_or_download(&mut self, name: &str, cx: &mut Context<Self>) {
        // 进入编辑器视图前清掉浮层，避免残留。
        self.context_menu = None;
        self.pending_path_input = None;
        self.confirm_delete = None;
        if !is_supported_text_file(name) {
            self.download(name);
            return;
        }

        let remote = join(&self.cwd, name);
        let mut editor = RemoteEditor::loading(remote.clone(), name.to_string(), cx.focus_handle());
        if try_send_command(&self.cmd_tx, SftpCmd::ReadFile { remote }).is_err() {
            editor.loading = false;
            editor.error = Some(sftp_channel_unavailable());
            self.message = Some(sftp_channel_unavailable());
        } else {
            self.message = None;
        }
        self.editor = Some(editor);
    }

    fn choose_upload_file(&mut self, cx: &mut Context<Self>) {
        let paths_receiver = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some(i18n::text("sftp.choose_upload_file").into()),
        });
        let task = cx.spawn(async move |weak, cx| {
            let Ok(Ok(Some(paths))) = paths_receiver.await else {
                return;
            };
            let Some(path) = paths.into_iter().next() else {
                return;
            };
            let _ = weak.update(cx, |this, cx| {
                this.upload_input = EndCaretInput::new(path.to_string_lossy().to_string());
                this.message = None;
                cx.notify();
            });
        });
        self._picker = Some(task);
    }

    fn do_upload(&mut self, cx: &mut Context<Self>) {
        let input = self.upload_input.value.trim();
        if input.is_empty() {
            self.message = Some(i18n::text("sftp.enter_local_path"));
            cx.notify();
            return;
        }
        let local = std::path::PathBuf::from(crossh_core::config::expand_tilde(input));
        if !local.is_file() {
            self.message =
                Some(rust_i18n::t!("sftp.local_file_missing", path = local.display()).to_string());
            cx.notify();
            return;
        }
        let basename = local
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "upload.bin".into());
        let remote = join(&self.cwd, &basename);
        if try_send_command(&self.cmd_tx, SftpCmd::Upload { local, remote }).is_err() {
            self.message = Some(sftp_channel_unavailable());
        } else {
            self.message = Some(rust_i18n::t!("sftp.prepare_upload", name = basename).to_string());
            self.upload_input.clear();
        }
        cx.notify();
    }

    fn enter_editor_edit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let focus = if let Some(editor) = &mut self.editor {
            if !editor.loading && editor.error.is_none() {
                editor.read_only = false;
                Some(editor.focus.clone())
            } else {
                None
            }
        } else {
            None
        };
        if let Some(focus) = focus {
            window.focus(&focus, cx);
            cx.notify();
        }
    }

    fn leave_editor_edit(&mut self, cx: &mut Context<Self>) {
        if let Some(editor) = &mut self.editor {
            editor.read_only = true;
            cx.notify();
        }
    }

    fn close_editor(&mut self, cx: &mut Context<Self>) {
        if self.editor.as_ref().is_some_and(|editor| editor.dirty) {
            self.message = Some(i18n::text("sftp.unsaved_changes"));
        } else {
            self.editor = None;
            self.message = None;
        }
        cx.notify();
    }

    fn discard_editor(&mut self, cx: &mut Context<Self>) {
        self.editor = None;
        self.message = None;
        cx.notify();
    }

    fn save_editor(&mut self, cx: &mut Context<Self>) {
        let Some((remote, contents)) = self.editor.as_ref().and_then(|editor| {
            (!editor.read_only && editor.dirty && !editor.saving).then(|| {
                (
                    editor.remote.clone(),
                    editor.state.value.as_bytes().to_vec(),
                )
            })
        }) else {
            return;
        };
        if contents.len() as u64 > MAX_EDITOR_FILE_BYTES {
            if let Some(editor) = &mut self.editor {
                editor.error = Some(i18n::text("sftp.editor_file_too_large"));
            }
            cx.notify();
            return;
        }
        if try_send_command(&self.cmd_tx, SftpCmd::WriteFile { remote, contents }).is_err() {
            self.message = Some(sftp_channel_unavailable());
        } else {
            if let Some(editor) = &mut self.editor {
                editor.saving = true;
                editor.error = None;
            }
            self.message = Some(i18n::text("sftp.saving").to_string());
        }
        cx.notify();
    }

    fn handle_editor_key(&mut self, ev: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        let ks = &ev.keystroke;
        let primary = ks.modifiers.control || ks.modifiers.platform;
        if primary && ks.key == "s" {
            self.save_editor(cx);
            return;
        }

        let pasted = if primary && ks.key == "v" {
            cx.read_from_clipboard().and_then(|item| {
                item.into_entries().find_map(|entry| match entry {
                    ClipboardEntry::String(value) => Some(value.text),
                    _ => None,
                })
            })
        } else {
            None
        };

        let Some(editor) = &mut self.editor else {
            return;
        };
        if editor.read_only || editor.loading || editor.error.is_some() || editor.saving {
            return;
        }
        editor.state.clear_composition();
        if let Some(text) = pasted {
            editor.insert(&text);
            cx.notify();
            return;
        }

        match ks.key.as_str() {
            "backspace" => editor.backspace(),
            "delete" => editor.delete(),
            "left" => editor.move_horizontal(-1),
            "right" => editor.move_horizontal(1),
            "up" => editor.move_vertical(-1),
            "down" => editor.move_vertical(1),
            "home" => editor.state.cursor = line_bounds(&editor.state.value, editor.state.cursor).0,
            "end" => editor.state.cursor = line_bounds(&editor.state.value, editor.state.cursor).1,
            "enter" | "return" => editor.insert("\n"),
            "tab" => editor.insert("\t"),
            "escape" => editor.read_only = true,
            _ => {
                if let Some(ch) = printable_char(ks) {
                    editor.insert(&ch.to_string());
                } else {
                    return;
                }
            }
        }
        cx.notify();
    }

    fn handle_input_key(&mut self, ev: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        let ks = &ev.keystroke;
        match ks.key.as_str() {
            "enter" | "return" => self.do_upload(cx),
            "escape" => {
                self.upload_input.clear();
                cx.notify();
            }
            "backspace" => {
                self.upload_input.value.pop();
                self.upload_input.unmark();
                cx.notify();
            }
            _ => {
                if let Some(ch) = printable_char(ks) {
                    self.upload_input.value.push(ch);
                    self.upload_input.unmark();
                    cx.notify();
                }
            }
        }
    }

    /// 右键打开上下文菜单；外部点击监听在 canvas 的 paint 阶段注册。
    fn open_context_menu(
        &mut self,
        position: Point<Pixels>,
        entries: Vec<MenuEntry<SftpMenuAction>>,
        cx: &mut Context<Self>,
    ) {
        self.context_menu = Some(ContextMenuState { position, entries });
        cx.notify();
    }

    fn close_context_menu(&mut self, cx: &mut Context<Self>) {
        if self.context_menu.take().is_some() {
            cx.notify();
        }
    }

    fn dispatch_menu_action(
        &mut self,
        action: SftpMenuAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match action {
            SftpMenuAction::Navigate(name) => {
                let path = join(&self.cwd, &name);
                self.request_list(path);
            }
            SftpMenuAction::Download(name) => self.download(&name),
            SftpMenuAction::UploadHere(name) => {
                let path = join(&self.cwd, &name);
                self.request_list(path);
                window.focus(&self.focus, cx);
            }
            SftpMenuAction::Rename(name) => {
                self.confirm_delete = None;
                let focus = cx.focus_handle();
                self.pending_path_input = Some(PendingPathInput {
                    rename_from: Some(name.clone()),
                    state: EndCaretInput::new(name),
                    focus: focus.clone(),
                });
                window.focus(&focus, cx);
            }
            SftpMenuAction::NewDir => {
                self.confirm_delete = None;
                let focus = cx.focus_handle();
                self.pending_path_input = Some(PendingPathInput {
                    rename_from: None,
                    state: EndCaretInput::new(String::new()),
                    focus: focus.clone(),
                });
                window.focus(&focus, cx);
            }
            SftpMenuAction::Delete { name, is_dir } => {
                self.pending_path_input = None;
                self.confirm_delete = Some(ConfirmDelete { name, is_dir });
            }
            SftpMenuAction::Refresh => self.request_list(self.cwd.clone()),
        }
        self.close_context_menu(cx);
    }

    /// 提交路径输入（Enter）：重命名或新建目录。
    fn submit_path_input(&mut self, cx: &mut Context<Self>) {
        let Some(input) = &self.pending_path_input else {
            return;
        };
        let value = input.state.value.trim().to_string();
        if value.is_empty() {
            self.pending_path_input = None;
            cx.notify();
            return;
        }
        let remote = join(&self.cwd, &value);
        let command = match &input.rename_from {
            Some(from) => SftpCmd::Rename {
                from: join(&self.cwd, from),
                to: remote,
            },
            None => SftpCmd::Mkdir { path: remote },
        };
        self.pending_path_input = None;
        if try_send_command(&self.cmd_tx, command).is_err() {
            self.message = Some(sftp_channel_unavailable());
        }
        cx.notify();
    }

    fn cancel_path_input(&mut self, cx: &mut Context<Self>) {
        if self.pending_path_input.take().is_some() {
            cx.notify();
        }
    }

    fn handle_path_input_key(&mut self, ev: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        let ks = &ev.keystroke;
        match ks.key.as_str() {
            "enter" | "return" => self.submit_path_input(cx),
            "escape" => self.cancel_path_input(cx),
            "backspace" => {
                if let Some(input) = &mut self.pending_path_input {
                    input.state.value.pop();
                    input.state.unmark();
                }
                cx.notify();
            }
            _ => {
                if let Some(ch) = printable_char(ks)
                    && let Some(input) = &mut self.pending_path_input
                {
                    input.state.value.push(ch);
                    input.state.unmark();
                    cx.notify();
                }
            }
        }
    }

    fn confirm_delete_submit(&mut self, cx: &mut Context<Self>) {
        let Some(confirm) = self.confirm_delete.take() else {
            return;
        };
        let remote = join(&self.cwd, &confirm.name);
        if try_send_command(&self.cmd_tx, SftpCmd::Remove { path: remote }).is_err() {
            self.message = Some(sftp_channel_unavailable());
        }
        cx.notify();
    }

    fn cancel_delete(&mut self, cx: &mut Context<Self>) {
        if self.confirm_delete.take().is_some() {
            cx.notify();
        }
    }

    /// 根级 Escape：关闭菜单 / 模态。
    fn handle_root_key(&mut self, ev: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        if ev.keystroke.key != "escape" {
            return;
        }
        if self.context_menu.is_some() {
            self.close_context_menu(cx);
        } else if self.pending_path_input.is_some() {
            self.cancel_path_input(cx);
        } else if self.confirm_delete.is_some() {
            self.cancel_delete(cx);
        }
    }

    /// 路径输入模态（重命名 / 新建目录）；未打开时返回空元素。
    fn render_path_input_modal(&mut self, _window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let Some(input) = &self.pending_path_input else {
            return div().into_any_element();
        };
        let focus = input.focus.clone();
        let value = input.state.value.clone();
        let is_rename = input.rename_from.is_some();
        let title = if is_rename {
            i18n::text("context_menu.rename")
        } else {
            i18n::text("context_menu.new_folder")
        };

        // 已收敛为 TextInput：选中/placeholder/caret/IME 由组件统一渲染，
        // 替代原手写 text_caret + marked_text_span + ime_input_canvas。
        let input_el = div().mt_2().child(
            TextInput::new("sftp-path-input", focus.clone())
                .value(value.clone())
                .placeholder(i18n::text("sftp.name_placeholder"))
                .ime_marked_text(input.state.ime_marked_text.clone())
                .caret_height(px(16.))
                .height(px(34.))
                .padding_x(px(12.))
                .text_size(px(14.))
                .full_width()
                .entity(cx.entity())
                .on_key_down(cx.listener(SftpPane::handle_path_input_key)),
        );

        let mut buttons = div().flex().flex_row().gap_2();
        buttons = buttons
            .child(
                Button::new("sftp-path-confirm")
                    .size(ButtonSize::Medium)
                    .variant(ButtonVariant::Primary)
                    .label(if is_rename {
                        i18n::text("context_menu.rename")
                    } else {
                        i18n::text("context_menu.create")
                    })
                    .on_click(cx.listener(|this, _ev, _window, cx| {
                        this.submit_path_input(cx);
                    })),
            )
            .child(
                Button::new("sftp-path-cancel")
                    .size(ButtonSize::Medium)
                    .variant(ButtonVariant::Secondary)
                    .label(i18n::text("prompt.cancel"))
                    .on_click(cx.listener(|this, _ev, _window, cx| {
                        this.cancel_path_input(cx);
                    })),
            );

        ModalDialog::new(
            title,
            icons::icon(icons::IconName::Pencil, 17.).text_color(theme::info()),
        )
        .width(px(360.))
        .scrim_id("sftp-path-scrim")
        .on_backdrop_click(cx.listener(|this, _ev, _window, cx| {
            this.cancel_path_input(cx);
        }))
        .child(input_el)
        .actions(buttons)
        .into_any_element()
    }

    /// 删除确认模态；未打开时返回空元素。
    fn render_delete_confirm(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let Some(confirm) = &self.confirm_delete else {
            return div().into_any_element();
        };
        let name = confirm.name.clone();
        let kind = if confirm.is_dir {
            i18n::text("context_menu.folder")
        } else {
            i18n::text("context_menu.file")
        };
        let mut buttons = div().flex().flex_row().gap_2();
        buttons = buttons
            .child(
                Button::new("sftp-delete-confirm")
                    .size(ButtonSize::Medium)
                    .variant(ButtonVariant::Danger)
                    .label(i18n::text("context_menu.delete"))
                    .on_click(cx.listener(|this, _ev, _window, cx| {
                        this.confirm_delete_submit(cx);
                    })),
            )
            .child(
                Button::new("sftp-delete-cancel")
                    .size(ButtonSize::Medium)
                    .variant(ButtonVariant::Secondary)
                    .label(i18n::text("prompt.cancel"))
                    .on_click(cx.listener(|this, _ev, _window, cx| {
                        this.cancel_delete(cx);
                    })),
            );

        ModalDialog::new(
            rust_i18n::t!("context_menu.delete_title", kind = kind),
            icons::icon(icons::IconName::Trash, 17.).text_color(theme::danger()),
        )
        .width(px(380.))
        .scrim_id("sftp-delete-scrim")
        .on_backdrop_click(cx.listener(|this, _ev, _window, cx| {
            this.cancel_delete(cx);
        }))
        .body(rust_i18n::t!("context_menu.delete_body", name = name))
        .actions(buttons)
        .into_any_element()
    }

    fn render_editor(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let Some(editor) = self.editor.as_ref() else {
            return div().into_any_element();
        };

        let name = editor.name.clone();
        let read_only = editor.read_only;
        let dirty = editor.dirty;
        let saving = editor.saving;
        let loading = editor.loading;
        let error = editor.error.clone();
        let content = editor.state.value.clone();
        let cursor = editor.state.cursor;
        let focus = editor.focus.clone();
        let ime_marked_text = editor.state.ime_marked_text.clone();
        let ime_replacement = editor.state.ime_replacement;
        let cursor_line = content[..cursor]
            .bytes()
            .filter(|byte| *byte == b'\n')
            .count();
        let cursor_line_start = line_bounds(&content, cursor).0;
        let cursor_column = content[cursor_line_start..cursor].chars().count();

        let mut header = div()
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .px_3()
            .py_2()
            .bg(theme::surface())
            .border_b_1()
            .border_color(theme::border())
            .child(
                Button::new("sftp-editor-back")
                    .size(ButtonSize::Small)
                    .variant(ButtonVariant::Ghost)
                    .icon(icons::icon(icons::IconName::ArrowLeft, 14.))
                    .label(i18n::text("sftp.file_list"))
                    .on_click(cx.listener(|this, _ev, _window, cx| {
                        this.close_editor(cx);
                    })),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_xs()
                    .text_color(theme::text())
                    .child(SharedString::from(name)),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(if read_only {
                        theme::info()
                    } else {
                        theme::accent()
                    })
                    .child(SharedString::from(if read_only {
                        i18n::text("sftp.read_only")
                    } else {
                        i18n::text("sftp.editing")
                    })),
            );

        let mut actions = div().flex().flex_row().items_center().gap_1();
        if read_only {
            actions = actions.child(
                Button::new("sftp-editor-edit")
                    .size(ButtonSize::Small)
                    .variant(ButtonVariant::Secondary)
                    .icon(icons::icon(icons::IconName::Pencil, 14.).text_color(theme::text()))
                    .label(i18n::text("sftp.enter_editing"))
                    .on_click(cx.listener(|this, _ev, window, cx| {
                        this.enter_editor_edit(window, cx);
                    })),
            );
        } else {
            actions = actions.child(
                Button::new("sftp-editor-read-only")
                    .size(ButtonSize::Small)
                    .variant(ButtonVariant::Secondary)
                    .icon(icons::icon(icons::IconName::ShieldAlert, 14.).text_color(theme::text()))
                    .label(i18n::text("sftp.read_only"))
                    .on_click(cx.listener(|this, _ev, _window, cx| {
                        this.leave_editor_edit(cx);
                    })),
            );
            if dirty {
                actions = actions.child(
                    Button::new("sftp-editor-save")
                        .size(ButtonSize::Small)
                        .variant(ButtonVariant::Primary)
                        .icon(icons::icon(icons::IconName::Save, 14.).text_color(theme::canvas()))
                        .label(if saving {
                            i18n::text("sftp.saving_short")
                        } else {
                            i18n::text("sftp.save")
                        })
                        .on_click(cx.listener(|this, _ev, _window, cx| {
                            this.save_editor(cx);
                        })),
                );
                actions = actions.child(
                    Button::new("sftp-editor-discard")
                        .size(ButtonSize::Small)
                        .variant(ButtonVariant::Secondary)
                        .icon(icons::icon(icons::IconName::X, 14.).text_color(theme::text()))
                        .label(i18n::text("sftp.discard"))
                        .on_click(cx.listener(|this, _ev, _window, cx| {
                            this.discard_editor(cx);
                        })),
                );
            }
        }
        header = header.child(actions);

        let mut body = scroll_y(&self.editor_scroll)
            .id("sftp-editor-body")
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .px_3()
            .py_2()
            .relative()
            .bg(theme::canvas())
            .track_focus(&focus)
            .tab_stop(true)
            .focus(|style| style.border_color(theme::focus_ring()))
            .on_click({
                let focus = focus.clone();
                move |_ev, window, cx| window.focus(&focus, cx)
            })
            .on_key_down(cx.listener(SftpPane::handle_editor_key));

        if loading {
            body = body.child(
                div()
                    .text_xs()
                    .text_color(theme::muted_text())
                    .child(SharedString::from(i18n::text("sftp.reading_short"))),
            );
        } else if let Some(error) = error.clone() {
            body = body.child(
                div()
                    .text_xs()
                    .text_color(theme::danger())
                    .child(SharedString::from(error)),
            );
        } else {
            let line_count = content.matches('\n').count() + 1;
            let scroll_offset = self.editor_scroll.offset().y.as_f32().max(0.);
            let first_line = ((scroll_offset / EDITOR_ROW_HEIGHT).floor() as usize)
                .min(line_count.saturating_sub(1));
            let visible_lines = (window.viewport_size().height.as_f32() / EDITOR_ROW_HEIGHT).ceil()
                as usize
                + VIRTUAL_LIST_OVERSCAN;
            let last_line = (first_line + visible_lines).min(line_count);

            body = body.child(
                div()
                    .h(px(first_line as f32 * EDITOR_ROW_HEIGHT))
                    .flex_shrink_0(),
            );
            for (line_idx, line) in content
                .split('\n')
                .enumerate()
                .skip(first_line)
                .take(last_line.saturating_sub(first_line))
            {
                let mut row = div()
                    .flex()
                    .flex_row()
                    .flex_shrink_0()
                    .min_h(px(EDITOR_ROW_HEIGHT))
                    .text_xs()
                    .text_color(theme::text())
                    .child(
                        div()
                            .w(px(42.))
                            .flex_shrink_0()
                            .text_color(theme::faint_text())
                            .child(SharedString::from(format!("{:>4} ", line_idx + 1))),
                    );
                if !read_only && line_idx == cursor_line {
                    let cursor_byte = line
                        .char_indices()
                        .nth(cursor_column)
                        .map(|(idx, _)| idx)
                        .unwrap_or(line.len());
                    row = row.child(SharedString::from(line[..cursor_byte].to_string()));
                    if ime_marked_text.is_empty() {
                        row = row.child(
                            div()
                                .w(px(1.))
                                .h(px(18.))
                                .flex_shrink_0()
                                .bg(theme::accent()),
                        );
                    }
                    if !ime_marked_text.is_empty() {
                        row = row.child(marked_text_span(ime_marked_text.clone()));
                    }
                    let suffix_start = ime_replacement
                        .filter(|_| !ime_marked_text.is_empty())
                        .map(|(_, end)| end.saturating_sub(cursor_line_start))
                        .unwrap_or(cursor_byte)
                        .min(line.len());
                    row = row.child(SharedString::from(line[suffix_start..].to_string()));
                } else {
                    row = row.child(SharedString::from(line.to_string()));
                }
                body = body.child(row);
            }
            body = body.child(
                div()
                    .h(px((line_count - last_line) as f32 * EDITOR_ROW_HEIGHT))
                    .flex_shrink_0(),
            );
        }
        body = body.child(ime_input_canvas(focus.clone(), cx.entity()));

        let footer_text = if loading {
            i18n::text("sftp.reading_remote")
        } else if saving {
            i18n::text("sftp.saving_remote")
        } else if dirty {
            i18n::text("sftp.unsaved_changes_short")
        } else if error.is_some() {
            i18n::text("sftp.cannot_edit")
        } else {
            i18n::text("sftp.saved")
        };
        div()
            .size_full()
            .min_h_0()
            .flex()
            .flex_col()
            .bg(theme::canvas())
            .child(header)
            .child(body)
            .child(
                div()
                    .flex_shrink_0()
                    .px_3()
                    .py_1()
                    .bg(theme::surface())
                    .border_t_1()
                    .border_color(theme::border())
                    .text_xs()
                    .text_color(theme::muted_text())
                    .child(SharedString::from(footer_text)),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shared::text_editing::{next_char_boundary, previous_char_boundary};
    use gpui::TestAppContext;
    use std::path::Path;

    #[test]
    fn command_queue_reports_full_and_closed_channels() {
        let (tx, rx) = async_channel::bounded(1);
        let list = || SftpCmd::List {
            path: ".".to_string(),
        };

        assert_eq!(try_send_command(&tx, list()), Ok(()));
        assert_eq!(
            try_send_command(&tx, list()),
            Err("sftp channel unavailable")
        );

        drop(rx);
        assert_eq!(
            try_send_command(&tx, list()),
            Err("sftp channel unavailable")
        );
    }

    #[test]
    fn unique_local_path_returns_unused_path_without_overwriting() {
        let path = Path::new("/definitely-missing-crossh-downloads/notes.txt");
        assert_eq!(unique_local_path(path), Some(path.to_path_buf()));
    }

    #[test]
    fn format_size_uses_human_readable_units() {
        assert_eq!(format_size(0), "0 B");
        assert_eq!(format_size(1024), "1.0 KB");
        assert_eq!(format_size(1024 * 1024), "1.0 MB");
        assert_eq!(format_size(1024 * 1024 * 1024), "1.0 GB");
    }

    #[test]
    fn supported_text_files_are_opened_in_editor() {
        assert!(is_supported_text_file("notes.md"));
        assert!(is_supported_text_file("/etc/ssh/sshd_config"));
        assert!(is_supported_text_file("Dockerfile"));
        assert!(is_supported_text_file(".env.production"));
        assert!(!is_supported_text_file("photo.png"));
        assert!(!is_supported_text_file("archive.tar.gz"));
    }

    #[test]
    fn editor_cursor_helpers_respect_utf8_and_line_boundaries() {
        let text = "ab你好\nxyz";
        let end_of_first_line = "ab你好".len();
        assert_eq!(
            previous_char_boundary(text, end_of_first_line),
            "ab你".len()
        );
        assert_eq!(next_char_boundary(text, 2), "ab你".len());
        assert_eq!(line_bounds(text, 2), (0, end_of_first_line));
        assert_eq!(
            line_bounds(text, text.len()),
            (end_of_first_line + 1, text.len())
        );
    }

    #[gpui::test]
    fn spec_20260818_merge_sftp_text_editing_remote_editor_edits_keep_utf8_semantics(
        cx: &mut TestAppContext,
    ) {
        let focus = cx.update(|cx| cx.focus_handle());
        let make = || {
            let mut editor = RemoteEditor::loading("srv".into(), "f.txt".into(), focus.clone());
            editor.read_only = false;
            editor.loading = false;
            editor
        };

        let mut editor = make();
        editor.insert("a✓b");
        assert!(editor.dirty);
        assert_eq!(editor.state.value, "a✓b");
        assert_eq!(editor.state.cursor, "a✓b".len());

        editor.dirty = false;
        editor.insert("");
        assert!(!editor.dirty);
        assert_eq!(editor.state.value, "a✓b");
        assert_eq!(editor.state.cursor, "a✓b".len());

        editor.backspace();
        assert!(editor.dirty);
        assert_eq!(editor.state.value, "a✓");
        assert_eq!(editor.state.cursor, "a✓".len());

        editor.dirty = false;
        editor.state.cursor = 0;
        editor.backspace();
        assert!(!editor.dirty);
        assert_eq!(editor.state.value, "a✓");

        editor.state.cursor = 1;
        editor.delete();
        assert!(editor.dirty);
        assert_eq!(editor.state.value, "a");
        assert_eq!(editor.state.cursor, 1);

        editor.dirty = false;
        editor.state.cursor = 1;
        editor.delete();
        assert!(!editor.dirty);
        assert_eq!(editor.state.value, "a");

        editor.state.cursor = 0;
        editor.delete();
        assert!(editor.dirty);
        assert_eq!(editor.state.value, "");
        assert_eq!(editor.state.cursor, 0);

        editor.dirty = false;
        editor.state.cursor = 0;
        editor.delete();
        assert!(!editor.dirty);
        assert_eq!(editor.state.value, "");

        let mut editor = make();
        editor.insert("ab你好\nxyz");
        assert_eq!(editor.state.cursor, "ab你好\nxyz".len());
        editor.move_horizontal(-1);
        assert_eq!(editor.state.cursor, "ab你好\nxy".len());
        editor.move_horizontal(1);
        assert_eq!(editor.state.cursor, "ab你好\nxyz".len());
        editor.move_vertical(-1);
        assert_eq!(editor.state.cursor, "ab你".len());
        editor.move_vertical(1);
        assert_eq!(editor.state.cursor, "ab你好\nxyz".len());
    }

    #[gpui::test]
    fn spec_20260818_merge_sftp_text_editing_remote_editor_ime_state_round_trip(
        cx: &mut TestAppContext,
    ) {
        let focus = cx.update(|cx| cx.focus_handle());
        let mut editor = RemoteEditor::loading("srv".into(), "f.txt".into(), focus);
        editor.read_only = false;
        editor.loading = false;
        editor.insert("ab");

        let replacement = (1, 2);
        editor.state.ime_replacement = Some(replacement);
        editor.state.cursor = replacement.0;
        editor.state.ime_marked_text.clear();
        editor.state.ime_marked_text.push('中');

        if let Some((_, end)) = editor.state.ime_replacement.take() {
            editor.state.cursor = end;
        }
        editor.state.ime_marked_text.clear();
        assert_eq!(editor.state.cursor, 2);
        assert!(editor.state.ime_marked_text.is_empty());
        assert_eq!(editor.state.ime_replacement, None);
    }

    #[gpui::test]
    fn workspace_pane_focus_request_is_recorded_on_the_entity(cx: &mut TestAppContext) {
        let pane = cx.update(|cx| {
            let (cmd_tx, _cmd_rx) = async_channel::bounded(1);
            cx.new(|cx| SftpPane {
                cmd_tx,
                cwd: ".".into(),
                entries: Vec::new(),
                message: None,
                loading: false,
                upload_input: EndCaretInput::new(String::new()),
                progress: None,
                editor: None,
                focus: cx.focus_handle(),
                list_scroll: ScrollHandle::new(),
                editor_scroll: ScrollHandle::new(),
                _drain: None,
                _picker: None,
                context_menu: None,
                anchor_bounds: Rc::new(Cell::new(None)),
                root_focus: cx.focus_handle(),
                focus_requested: false,
                pending_path_input: None,
                confirm_delete: None,
            })
        });

        cx.update(|cx| SftpWorkspacePane(pane.clone()).request_focus(cx));

        assert!(pane.read_with(cx, |pane, _| pane.focus_requested));
    }
}
