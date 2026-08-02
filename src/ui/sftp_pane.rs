//! SFTP 浏览面板：远端目录列表 / 进入目录 / 上传 / 下载，进度与状态。
//!
//! 通过 `Connection::open_sftp` 拿到 `(cmd_tx, event_rx)` 后由本面板持有；
//! 主线程 drain `event_rx` 更新列表/进度。下载落到 ~/Downloads（重名自动加序号）。

use std::cell::Cell;
use std::rc::Rc;

use async_channel::{Receiver, Sender};
use gpui::{
    AnyElement, App, AppContext, Bounds, ClipboardEntry, Context, Entity, FocusHandle, FontWeight,
    InteractiveElement, IntoElement, KeyDownEvent, Keystroke, MouseButton, MouseDownEvent,
    ParentElement, PathPromptOptions, Pixels, Point, Render, ScrollHandle, SharedString,
    StatefulInteractiveElement, Styled, Task, Window, canvas, div, px, rgb,
};

use crate::i18n;
use crate::ssh::{MAX_EDITOR_FILE_BYTES, RemoteEntry, SftpCmd, SftpEvent};
use crate::ui::context_menu::{
    ContextMenuState, MenuEntry, MenuItem, SftpMenuAction, render_context_menu,
};
use crate::ui::{icons, theme};

/// 传输进度快照。
#[derive(Clone, Debug, Default)]
struct Progress {
    label: String,
    transferred: u64,
    total: Option<u64>,
}

const SFTP_CHANNEL_UNAVAILABLE: &str = "sftp channel unavailable";

fn sftp_channel_unavailable() -> String {
    i18n::text("sftp.channel_unavailable")
}

const SUPPORTED_TEXT_EXTENSIONS: &[&str] = &[
    "bash", "c", "cc", "conf", "config", "cpp", "css", "csv", "fish", "go", "h", "hh", "hpp",
    "htm", "html", "ini", "java", "js", "json", "jsonl", "jsx", "kts", "kt", "log", "lua",
    "markdown", "md", "php", "py", "rb", "rs", "sh", "sql", "swift", "toml", "ts", "tsx", "txt",
    "xml", "yaml", "yml", "zsh",
];

const SUPPORTED_TEXT_FILENAMES: &[&str] = &[
    ".editorconfig",
    ".gitattributes",
    ".gitignore",
    "dockerfile",
    "gemfile",
    "hosts",
    "license",
    "makefile",
    "passwd",
    "profile",
    "rakefile",
    "readme",
    "sshd_config",
    "authorized_keys",
    "known_hosts",
];

struct RemoteEditor {
    remote: String,
    name: String,
    content: String,
    cursor: usize,
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
            content: String::new(),
            cursor: 0,
            read_only: true,
            dirty: false,
            loading: true,
            saving: false,
            error: None,
            focus,
        }
    }

    fn insert(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        self.content.insert_str(self.cursor, text);
        self.cursor += text.len();
        self.dirty = true;
    }

    fn backspace(&mut self) {
        let start = previous_char_boundary(&self.content, self.cursor);
        if start != self.cursor {
            self.content.replace_range(start..self.cursor, "");
            self.cursor = start;
            self.dirty = true;
        }
    }

    fn delete(&mut self) {
        let end = next_char_boundary(&self.content, self.cursor);
        if end != self.cursor {
            self.content.replace_range(self.cursor..end, "");
            self.dirty = true;
        }
    }

    fn move_horizontal(&mut self, direction: i8) {
        self.cursor = if direction < 0 {
            previous_char_boundary(&self.content, self.cursor)
        } else {
            next_char_boundary(&self.content, self.cursor)
        };
    }

    fn move_vertical(&mut self, direction: i8) {
        let (line_start, line_end) = line_bounds(&self.content, self.cursor);
        let column = self.content[line_start..self.cursor].chars().count();
        let target_start = if direction < 0 {
            if line_start == 0 {
                return;
            }
            self.content[..line_start - 1]
                .rfind('\n')
                .map(|idx| idx + 1)
                .unwrap_or(0)
        } else {
            if line_end == self.content.len() {
                return;
            }
            line_end + 1
        };
        let target_end = self.content[target_start..]
            .find('\n')
            .map(|idx| target_start + idx)
            .unwrap_or(self.content.len());
        self.cursor = self.content[target_start..target_end]
            .char_indices()
            .nth(column)
            .map(|(idx, _)| target_start + idx)
            .unwrap_or(target_end);
    }
}

/// 路径输入模态（重命名 / 新建目录）。
struct PendingPathInput {
    /// Some(旧名) = 重命名；None = 新建目录。
    rename_from: Option<String>,
    value: String,
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
    upload_input: String,
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
    /// 重命名 / 新建目录输入模态。
    pending_path_input: Option<PendingPathInput>,
    /// 删除确认模态。
    confirm_delete: Option<ConfirmDelete>,
}

impl SftpPane {
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
            upload_input: String::new(),
            progress: None,
            editor: None,
            focus: cx.focus_handle(),
            list_scroll: ScrollHandle::new(),
            editor_scroll: ScrollHandle::new(),
            _drain: None,
            _picker: None,
            context_menu: None,
            anchor_bounds: Rc::new(Cell::new(None)),
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
                                        editor.content = content;
                                        editor.cursor = 0;
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

    fn parent_of(path: &str) -> String {
        let p = path.trim_end_matches('/');
        if p.is_empty() {
            return "/".to_string();
        }
        match p.rfind('/') {
            Some(0) => "/".to_string(),
            Some(idx) => p[..idx].to_string(),
            None => ".".to_string(),
        }
    }

    fn join(base: &str, name: &str) -> String {
        if base.ends_with('/') {
            format!("{base}{name}")
        } else {
            format!("{base}/{name}")
        }
    }

    fn download(&mut self, name: &str) {
        let remote = Self::join(&self.cwd, name);
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

        let remote = Self::join(&self.cwd, name);
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
                this.upload_input = path.to_string_lossy().to_string();
                this.message = None;
                cx.notify();
            });
        });
        self._picker = Some(task);
    }

    fn do_upload(&mut self, cx: &mut Context<Self>) {
        let input = self.upload_input.trim();
        if input.is_empty() {
            self.message = Some(i18n::text("sftp.enter_local_path"));
            cx.notify();
            return;
        }
        let local = std::path::PathBuf::from(crate::config::expand_tilde(input));
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
        let remote = Self::join(&self.cwd, &basename);
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
            (!editor.read_only && editor.dirty && !editor.saving)
                .then(|| (editor.remote.clone(), editor.content.as_bytes().to_vec()))
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
            "home" => editor.cursor = line_bounds(&editor.content, editor.cursor).0,
            "end" => editor.cursor = line_bounds(&editor.content, editor.cursor).1,
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
                self.upload_input.pop();
                cx.notify();
            }
            _ => {
                if let Some(ch) = printable_char(ks) {
                    self.upload_input.push(ch);
                    cx.notify();
                }
            }
        }
    }

    /// 右键打开上下文菜单；同时在窗口注册一次「点击面板外即关闭」的监听。
    fn open_context_menu(
        &mut self,
        position: Point<Pixels>,
        entries: Vec<MenuEntry<SftpMenuAction>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.context_menu = Some(ContextMenuState { position, entries });
        let weak = cx.entity().downgrade();
        let anchor = self.anchor_bounds.clone();
        window.on_mouse_event({
            let weak = weak.clone();
            move |ev: &MouseDownEvent, _phase, window, cx| {
                let closed = weak
                    .update(cx, |this, _| {
                        let outside = anchor
                            .get()
                            .is_some_and(|bounds| !bounds.contains(&ev.position));
                        if this.context_menu.is_some() && outside {
                            this.context_menu = None;
                            true
                        } else {
                            false
                        }
                    })
                    .unwrap_or(false);
                if closed {
                    window.refresh();
                }
            }
        });
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
                let path = Self::join(&self.cwd, &name);
                self.request_list(path);
            }
            SftpMenuAction::Download(name) => self.download(&name),
            SftpMenuAction::UploadHere(name) => {
                let path = Self::join(&self.cwd, &name);
                self.request_list(path);
                window.focus(&self.focus, cx);
            }
            SftpMenuAction::Rename(name) => {
                self.confirm_delete = None;
                let focus = cx.focus_handle();
                self.pending_path_input = Some(PendingPathInput {
                    rename_from: Some(name.clone()),
                    value: name,
                    focus: focus.clone(),
                });
                window.focus(&focus, cx);
            }
            SftpMenuAction::NewDir => {
                self.confirm_delete = None;
                let focus = cx.focus_handle();
                self.pending_path_input = Some(PendingPathInput {
                    rename_from: None,
                    value: String::new(),
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
        let value = input.value.trim().to_string();
        if value.is_empty() {
            self.pending_path_input = None;
            cx.notify();
            return;
        }
        let remote = Self::join(&self.cwd, &value);
        let command = match &input.rename_from {
            Some(from) => SftpCmd::Rename {
                from: Self::join(&self.cwd, from),
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
                    input.value.pop();
                }
                cx.notify();
            }
            _ => {
                if let Some(ch) = printable_char(ks)
                    && let Some(input) = &mut self.pending_path_input
                {
                    input.value.push(ch);
                    cx.notify();
                }
            }
        }
    }

    fn confirm_delete_submit(&mut self, cx: &mut Context<Self>) {
        let Some(confirm) = self.confirm_delete.take() else {
            return;
        };
        let remote = Self::join(&self.cwd, &confirm.name);
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
    fn render_path_input_modal(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let Some(input) = &self.pending_path_input else {
            return div().into_any_element();
        };
        let focus = input.focus.clone();
        let value = input.value.clone();
        let is_rename = input.rename_from.is_some();
        let title = if is_rename {
            i18n::text("context_menu.rename")
        } else {
            i18n::text("context_menu.new_folder")
        };

        let input_el = div()
            .id("sftp-path-input")
            .w_full()
            .h(px(34.))
            .px_3()
            .flex()
            .items_center()
            .mt_2()
            .bg(theme::canvas())
            .border_1()
            .border_color(theme::border_strong())
            .rounded(px(theme::RADIUS_SM))
            .text_sm()
            .text_color(theme::text())
            .track_focus(&focus)
            .tab_stop(true)
            .focus(|style| style.border_color(theme::focus_ring()))
            .on_click({
                let focus = focus.clone();
                move |_ev, window, cx| window.focus(&focus, cx)
            })
            .on_key_down(cx.listener(SftpPane::handle_path_input_key))
            .child(SharedString::from(if value.is_empty() {
                i18n::text("sftp.name_placeholder")
            } else {
                value
            }));

        let mut buttons = div().flex().flex_row().gap_2().mt_4();
        buttons = buttons
            .child(
                div()
                    .id("sftp-path-confirm")
                    .h(px(30.))
                    .px_3()
                    .flex()
                    .items_center()
                    .rounded(px(theme::RADIUS_SM))
                    .cursor_pointer()
                    .bg(theme::accent())
                    .hover(|s| s.bg(rgb(0x82e3bf)))
                    .text_xs()
                    .text_color(theme::canvas())
                    .child(SharedString::from(if is_rename {
                        i18n::text("context_menu.rename")
                    } else {
                        i18n::text("context_menu.create")
                    }))
                    .on_click(cx.listener(|this, _ev, _window, cx| {
                        this.submit_path_input(cx);
                    })),
            )
            .child(
                div()
                    .id("sftp-path-cancel")
                    .h(px(30.))
                    .px_3()
                    .flex()
                    .items_center()
                    .rounded(px(theme::RADIUS_SM))
                    .cursor_pointer()
                    .bg(theme::raised())
                    .hover(|s| s.bg(theme::border_strong()))
                    .text_xs()
                    .text_color(theme::text())
                    .child(SharedString::from(i18n::text("prompt.cancel")))
                    .on_click(cx.listener(|this, _ev, _window, cx| {
                        this.cancel_path_input(cx);
                    })),
            );

        let card = div()
            .w(px(360.))
            .p_5()
            .bg(theme::surface())
            .border_1()
            .border_color(theme::border_strong())
            .rounded(px(theme::RADIUS_MD))
            .shadow_md()
            .flex()
            .flex_col()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .text_sm()
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(theme::text())
                    .child(icons::icon(icons::IconName::Pencil, 17.).text_color(theme::info()))
                    .child(SharedString::from(title)),
            )
            .child(input_el)
            .child(buttons);

        div()
            .absolute()
            .size_full()
            .top_0()
            .left_0()
            .flex()
            .items_center()
            .justify_center()
            .bg(theme::scrim())
            .id("sftp-path-scrim")
            .on_click(cx.listener(|this, _ev, _window, cx| {
                this.cancel_path_input(cx);
            }))
            .child(card)
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
        let mut buttons = div().flex().flex_row().gap_2().mt_4();
        buttons = buttons
            .child(
                div()
                    .id("sftp-delete-confirm")
                    .h(px(30.))
                    .px_3()
                    .flex()
                    .items_center()
                    .rounded(px(theme::RADIUS_SM))
                    .cursor_pointer()
                    .bg(theme::danger())
                    .hover(|s| s.bg(rgb(0xf49b9b)))
                    .text_xs()
                    .text_color(theme::canvas())
                    .child(SharedString::from(i18n::text("context_menu.delete")))
                    .on_click(cx.listener(|this, _ev, _window, cx| {
                        this.confirm_delete_submit(cx);
                    })),
            )
            .child(
                div()
                    .id("sftp-delete-cancel")
                    .h(px(30.))
                    .px_3()
                    .flex()
                    .items_center()
                    .rounded(px(theme::RADIUS_SM))
                    .cursor_pointer()
                    .bg(theme::raised())
                    .hover(|s| s.bg(theme::border_strong()))
                    .text_xs()
                    .text_color(theme::text())
                    .child(SharedString::from(i18n::text("prompt.cancel")))
                    .on_click(cx.listener(|this, _ev, _window, cx| {
                        this.cancel_delete(cx);
                    })),
            );

        let card = div()
            .w(px(380.))
            .p_5()
            .bg(theme::surface())
            .border_1()
            .border_color(theme::border_strong())
            .rounded(px(theme::RADIUS_MD))
            .shadow_md()
            .flex()
            .flex_col()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .text_sm()
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(theme::text())
                    .child(icons::icon(icons::IconName::Trash, 17.).text_color(theme::danger()))
                    .child(SharedString::from(rust_i18n::t!(
                        "context_menu.delete_title",
                        kind = kind
                    ))),
            )
            .child(
                div()
                    .mt_2()
                    .text_xs()
                    .text_color(theme::muted_text())
                    .child(SharedString::from(rust_i18n::t!(
                        "context_menu.delete_body",
                        name = name
                    ))),
            )
            .child(buttons);

        div()
            .absolute()
            .size_full()
            .top_0()
            .left_0()
            .flex()
            .items_center()
            .justify_center()
            .bg(theme::scrim())
            .id("sftp-delete-scrim")
            .on_click(cx.listener(|this, _ev, _window, cx| {
                this.cancel_delete(cx);
            }))
            .child(card)
            .into_any_element()
    }

    fn render_editor(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let Some(editor) = self.editor.as_ref() else {
            return div().into_any_element();
        };

        let name = editor.name.clone();
        let read_only = editor.read_only;
        let dirty = editor.dirty;
        let saving = editor.saving;
        let loading = editor.loading;
        let error = editor.error.clone();
        let content = editor.content.clone();
        let cursor = editor.cursor;
        let focus = editor.focus.clone();
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
                div()
                    .id("sftp-editor-back")
                    .h(px(28.))
                    .px_2()
                    .flex()
                    .items_center()
                    .gap_2()
                    .rounded(px(theme::RADIUS_SM))
                    .cursor_pointer()
                    .hover(|s| s.bg(theme::raised()))
                    .text_xs()
                    .text_color(theme::text())
                    .child(icons::icon(icons::IconName::ArrowLeft, 14.).text_color(theme::text()))
                    .child(SharedString::from(i18n::text("sftp.file_list")))
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
                div()
                    .id("sftp-editor-edit")
                    .h(px(28.))
                    .px_2()
                    .flex()
                    .items_center()
                    .gap_2()
                    .rounded(px(theme::RADIUS_SM))
                    .cursor_pointer()
                    .bg(theme::raised())
                    .hover(|s| s.bg(theme::border_strong()))
                    .text_xs()
                    .text_color(theme::text())
                    .child(icons::icon(icons::IconName::Pencil, 14.).text_color(theme::text()))
                    .child(SharedString::from(i18n::text("sftp.enter_editing")))
                    .on_click(cx.listener(|this, _ev, window, cx| {
                        this.enter_editor_edit(window, cx);
                    })),
            );
        } else {
            actions = actions.child(
                div()
                    .id("sftp-editor-read-only")
                    .h(px(28.))
                    .px_2()
                    .flex()
                    .items_center()
                    .gap_2()
                    .rounded(px(theme::RADIUS_SM))
                    .cursor_pointer()
                    .bg(theme::raised())
                    .hover(|s| s.bg(theme::border_strong()))
                    .text_xs()
                    .text_color(theme::text())
                    .child(icons::icon(icons::IconName::ShieldAlert, 14.).text_color(theme::text()))
                    .child(SharedString::from(i18n::text("sftp.read_only")))
                    .on_click(cx.listener(|this, _ev, _window, cx| {
                        this.leave_editor_edit(cx);
                    })),
            );
            if dirty {
                actions = actions.child(
                    div()
                        .id("sftp-editor-save")
                        .h(px(28.))
                        .px_2()
                        .flex()
                        .items_center()
                        .gap_2()
                        .rounded(px(theme::RADIUS_SM))
                        .cursor_pointer()
                        .bg(theme::accent())
                        .hover(|s| s.bg(rgb(0x82e3bf)))
                        .text_xs()
                        .text_color(theme::canvas())
                        .child(icons::icon(icons::IconName::Save, 14.).text_color(theme::canvas()))
                        .child(SharedString::from(if saving {
                            i18n::text("sftp.saving_short")
                        } else {
                            i18n::text("sftp.save")
                        }))
                        .on_click(cx.listener(|this, _ev, _window, cx| {
                            this.save_editor(cx);
                        })),
                );
                actions = actions.child(
                    div()
                        .id("sftp-editor-discard")
                        .h(px(28.))
                        .px_2()
                        .flex()
                        .items_center()
                        .gap_2()
                        .rounded(px(theme::RADIUS_SM))
                        .cursor_pointer()
                        .bg(theme::raised())
                        .hover(|s| s.bg(theme::border_strong()))
                        .text_xs()
                        .text_color(theme::text())
                        .child(icons::icon(icons::IconName::X, 14.).text_color(theme::text()))
                        .child(SharedString::from(i18n::text("sftp.discard")))
                        .on_click(cx.listener(|this, _ev, _window, cx| {
                            this.discard_editor(cx);
                        })),
                );
            }
        }
        header = header.child(actions);

        let mut body = div()
            .id("sftp-editor-body")
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .px_3()
            .py_2()
            .track_scroll(&self.editor_scroll)
            .overflow_y_scroll()
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
            for (line_idx, line) in content.split('\n').enumerate() {
                let mut row = div()
                    .flex()
                    .flex_row()
                    .flex_shrink_0()
                    .min_h(px(20.))
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
                    row = row
                        .child(SharedString::from(line[..cursor_byte].to_string()))
                        .child(
                            div()
                                .w(px(1.))
                                .h(px(18.))
                                .flex_shrink_0()
                                .bg(theme::accent()),
                        )
                        .child(SharedString::from(line[cursor_byte..].to_string()));
                } else {
                    row = row.child(SharedString::from(line.to_string()));
                }
                body = body.child(row);
            }
        }

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

fn printable_char(ks: &Keystroke) -> Option<char> {
    if ks.modifiers.control || ks.modifiers.platform {
        return None;
    }
    ks.key_char.as_ref().and_then(|s| s.chars().next())
}

fn is_supported_text_file(name: &str) -> bool {
    let filename = std::path::Path::new(name)
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or(name)
        .to_ascii_lowercase();
    if SUPPORTED_TEXT_FILENAMES.contains(&filename.as_str()) || filename.starts_with(".env") {
        return true;
    }
    std::path::Path::new(&filename)
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| SUPPORTED_TEXT_EXTENSIONS.contains(&extension))
}

fn previous_char_boundary(text: &str, cursor: usize) -> usize {
    text[..cursor]
        .char_indices()
        .next_back()
        .map(|(idx, _)| idx)
        .unwrap_or(0)
}

fn next_char_boundary(text: &str, cursor: usize) -> usize {
    text[cursor..]
        .chars()
        .next()
        .map(|ch| cursor + ch.len_utf8())
        .unwrap_or(cursor)
}

fn line_bounds(text: &str, cursor: usize) -> (usize, usize) {
    let start = text[..cursor].rfind('\n').map(|idx| idx + 1).unwrap_or(0);
    let end = text[cursor..]
        .find('\n')
        .map(|idx| cursor + idx)
        .unwrap_or(text.len());
    (start, end)
}

/// ~/Downloads。
fn downloads_dir() -> std::path::PathBuf {
    std::env::var_os("HOME")
        .map(|h| std::path::PathBuf::from(&h).join("Downloads"))
        .unwrap_or_else(|| std::path::PathBuf::from("."))
}

/// 重名时追加 ` (1)`、` (2)`… 避免覆盖。
fn unique_local_path(path: &std::path::Path) -> Option<std::path::PathBuf> {
    if !path.exists() {
        return Some(path.to_path_buf());
    }
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    let ext = path.extension().map(|s| s.to_string_lossy().to_string());
    let parent = path.parent().unwrap_or_else(|| std::path::Path::new("."));
    for i in 1..1000 {
        let name = match &ext {
            Some(e) => format!("{stem} ({i}).{e}"),
            None => format!("{stem} ({i})"),
        };
        let candidate = parent.join(&name);
        if !candidate.exists() {
            return Some(candidate);
        }
    }
    None
}

fn try_send_command(tx: &Sender<SftpCmd>, command: SftpCmd) -> Result<(), &'static str> {
    tx.try_send(command).map_err(|_| SFTP_CHANNEL_UNAVAILABLE)
}

impl Render for SftpPane {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.editor.is_some() {
            return self.render_editor(window, cx);
        }

        let cwd = self.cwd.clone();

        // 顶部：上级 / 当前路径 / 刷新。
        let top = div()
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
                div()
                    .id("sftp-up")
                    .h(px(28.))
                    .px_2()
                    .flex()
                    .items_center()
                    .gap_2()
                    .rounded(px(theme::RADIUS_SM))
                    .cursor_pointer()
                    .hover(|s| s.bg(theme::raised()))
                    .text_xs()
                    .text_color(theme::text())
                    .child(icons::icon(icons::IconName::ArrowUp, 14.).text_color(theme::text()))
                    .child(SharedString::from(i18n::text("sftp.parent")))
                    .on_click(cx.listener(|this, _ev, _w, cx| {
                        let p = Self::parent_of(&this.cwd);
                        this.request_list(p);
                        cx.notify();
                    })),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_xs()
                    .text_color(theme::muted_text())
                    .child(SharedString::from(cwd)),
            )
            .child(
                div()
                    .id("sftp-refresh")
                    .w(px(28.))
                    .h(px(28.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(theme::RADIUS_SM))
                    .px_2()
                    .cursor_pointer()
                    .text_color(theme::muted_text())
                    .hover(|s| s.bg(theme::raised()).text_color(theme::text()))
                    .child(
                        icons::icon(icons::IconName::RefreshCw, 14.)
                            .text_color(theme::muted_text())
                            .hover(|s| s.text_color(theme::text())),
                    )
                    .on_click(cx.listener(|this, _ev, _w, cx| {
                        this.request_list(this.cwd.clone());
                        cx.notify();
                    })),
            );

        // 列表区。
        let mut list = div()
            .id("sftp-entry-list")
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .px_2()
            .py_2()
            .track_scroll(&self.list_scroll)
            .overflow_y_scroll();
        for (idx, e) in self.entries.iter().enumerate() {
            let name = e.name.clone();
            let is_dir = e.is_dir;
            let size = if is_dir {
                String::new()
            } else {
                format_size(e.size)
            };
            let row = div()
                .id(("entry", idx))
                .flex()
                .flex_row()
                .flex_shrink_0()
                .items_center()
                .gap_2()
                .h(px(34.))
                .px_2()
                .rounded(px(theme::RADIUS_SM))
                .cursor_pointer()
                .hover(|s| s.bg(theme::surface()))
                .child(
                    icons::icon(
                        if is_dir {
                            icons::IconName::Folder
                        } else {
                            icons::IconName::FileText
                        },
                        15.,
                    )
                    .text_color(if is_dir {
                        theme::warning()
                    } else {
                        theme::muted_text()
                    }),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_xs()
                        .text_color(theme::text())
                        .child(SharedString::from(name.clone())),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(theme::faint_text())
                        .child(SharedString::from(size)),
                )
                .on_click({
                    let name_click = name.clone();
                    cx.listener(move |this, _ev, _w, cx| {
                        if is_dir {
                            let p = Self::join(&this.cwd, &name_click);
                            this.request_list(p);
                        } else {
                            this.open_file_or_download(&name_click, cx);
                        }
                        cx.notify();
                    })
                })
                .on_mouse_down(MouseButton::Right, {
                    let name_menu = name.clone();
                    cx.listener(move |this, ev: &MouseDownEvent, window, cx| {
                        let mut entries = vec![];
                        if is_dir {
                            entries.push(MenuEntry::Item(MenuItem {
                                id: "navigate".into(),
                                label: i18n::text("context_menu.open_folder"),
                                shortcut_hint: None,
                                disabled: false,
                                danger: false,
                                action: SftpMenuAction::Navigate(name_menu.clone()),
                            }));
                            entries.push(MenuEntry::Item(MenuItem {
                                id: "upload-here".into(),
                                label: i18n::text("context_menu.upload_here"),
                                shortcut_hint: None,
                                disabled: false,
                                danger: false,
                                action: SftpMenuAction::UploadHere(name_menu.clone()),
                            }));
                        } else {
                            entries.push(MenuEntry::Item(MenuItem {
                                id: "download".into(),
                                label: i18n::text("context_menu.download"),
                                shortcut_hint: None,
                                disabled: false,
                                danger: false,
                                action: SftpMenuAction::Download(name_menu.clone()),
                            }));
                        }
                        entries.push(MenuEntry::Separator);
                        entries.push(MenuEntry::Item(MenuItem {
                            id: "rename".into(),
                            label: i18n::text("context_menu.rename"),
                            shortcut_hint: None,
                            disabled: false,
                            danger: false,
                            action: SftpMenuAction::Rename(name_menu.clone()),
                        }));
                        entries.push(MenuEntry::Item(MenuItem {
                            id: "delete".into(),
                            label: i18n::text("context_menu.delete"),
                            shortcut_hint: None,
                            disabled: false,
                            danger: true,
                            action: SftpMenuAction::Delete {
                                name: name_menu.clone(),
                                is_dir,
                            },
                        }));
                        entries.push(MenuEntry::Separator);
                        entries.push(MenuEntry::Item(MenuItem {
                            id: "new-folder".into(),
                            label: i18n::text("context_menu.new_folder"),
                            shortcut_hint: None,
                            disabled: false,
                            danger: false,
                            action: SftpMenuAction::NewDir,
                        }));
                        entries.push(MenuEntry::Item(MenuItem {
                            id: "refresh".into(),
                            label: i18n::text("context_menu.refresh"),
                            shortcut_hint: None,
                            disabled: false,
                            danger: false,
                            action: SftpMenuAction::Refresh,
                        }));
                        this.open_context_menu(ev.position, entries, window, cx);
                    })
                });
            list = list.child(row);
        }

        // 底部：上传输入 + 进度/消息。
        let focus = self.focus.clone();
        let upload_val = self.upload_input.clone();
        let input = div()
            .id("sftp-upload-input")
            .flex_1()
            .h(px(32.))
            .px_2()
            .flex()
            .items_center()
            .rounded(px(theme::RADIUS_SM))
            .bg(theme::canvas())
            .border_1()
            .border_color(theme::border_strong())
            .text_xs()
            .text_color(theme::text())
            .track_focus(&focus)
            .tab_stop(true)
            .focus(|style| style.border_color(theme::focus_ring()))
            .focus_visible(|style| style.border_color(theme::accent()))
            .on_click({
                let focus = focus.clone();
                move |_ev, window, cx| window.focus(&focus, cx)
            })
            .on_key_down(cx.listener(SftpPane::handle_input_key))
            .child(SharedString::from(if upload_val.is_empty() {
                i18n::text("sftp.local_path_placeholder")
            } else {
                upload_val
            }));

        let mut bottom = div()
            .flex()
            .flex_col()
            .gap_1()
            .px_3()
            .py_2()
            .bg(theme::surface())
            .border_t_1()
            .border_color(theme::border())
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .child(input)
                    .child(
                        div()
                            .id("sftp-upload-btn")
                            .h(px(28.))
                            .px_2()
                            .flex()
                            .items_center()
                            .gap_2()
                            .rounded(px(theme::RADIUS_SM))
                            .cursor_pointer()
                            .bg(theme::accent())
                            .hover(|s| s.bg(rgb(0x82e3bf)))
                            .text_xs()
                            .text_color(theme::canvas())
                            .child(
                                icons::icon(icons::IconName::Upload, 14.)
                                    .text_color(theme::canvas()),
                            )
                            .child(SharedString::from(i18n::text("sftp.upload")))
                            .on_click(cx.listener(|this, _ev, _w, cx| {
                                this.do_upload(cx);
                            })),
                    )
                    .child(
                        div()
                            .id("sftp-choose-file")
                            .h(px(28.))
                            .px_2()
                            .flex()
                            .items_center()
                            .gap_2()
                            .rounded(px(theme::RADIUS_SM))
                            .cursor_pointer()
                            .bg(theme::raised())
                            .hover(|s| s.bg(theme::border_strong()))
                            .text_xs()
                            .text_color(theme::text())
                            .child(
                                icons::icon(icons::IconName::FolderOpen, 14.)
                                    .text_color(theme::text()),
                            )
                            .child(SharedString::from(i18n::text("sftp.choose_file")))
                            .on_click(cx.listener(|this, _ev, _w, cx| {
                                this.choose_upload_file(cx);
                            })),
                    ),
            );

        if let Some(p) = &self.progress {
            let pct = p
                .total
                .filter(|&t| t > 0)
                .map(|t| ((p.transferred as f64 / t as f64) * 100.0) as u32)
                .unwrap_or(0);
            bottom = bottom.child(div().text_xs().text_color(theme::info()).child(
                SharedString::from(format!(
                    "{}: {} / {} ({}%)",
                    p.label,
                    format_size(p.transferred),
                    p.total.map(format_size).unwrap_or_else(|| "?".into()),
                    pct
                )),
            ));
        } else if self.loading {
            bottom = bottom.child(
                div()
                    .text_xs()
                    .text_color(theme::muted_text())
                    .child(SharedString::from(i18n::text("sftp.loading"))),
            );
        } else if let Some(msg) = &self.message {
            bottom = bottom.child(
                div()
                    .text_xs()
                    .text_color(theme::muted_text())
                    .child(SharedString::from(msg.clone())),
            );
        }

        // 无交互的全尺寸 canvas：仅用于在 prepaint 阶段捕获根 div 的窗口坐标
        // bounds（右键菜单定位 / 点击面板外关闭）。
        let anchor = self.anchor_bounds.clone();
        let bounds_canvas = canvas(
            {
                let anchor = anchor.clone();
                move |bounds, _window, _cx| {
                    anchor.set(Some(bounds));
                    bounds
                }
            },
            move |_bounds, _state, _window, _cx| {},
        )
        .absolute()
        .left_0()
        .top_0()
        .size_full();

        let mut root = div()
            .relative()
            .size_full()
            .min_h_0()
            .flex()
            .flex_col()
            .bg(theme::canvas())
            .on_key_down(cx.listener(SftpPane::handle_root_key))
            .child(bounds_canvas)
            .child(top)
            .child(list)
            .child(bottom);

        if let Some(menu) = self.context_menu.clone() {
            let anchor = self
                .anchor_bounds
                .get()
                .map(|bounds| bounds.origin)
                .unwrap_or_else(|| Point::new(px(0.), px(0.)));
            root = root.child(render_context_menu(
                &menu,
                anchor,
                window,
                cx,
                |this, action, window, cx| this.dispatch_menu_action(action, window, cx),
                |this, cx| this.close_context_menu(cx),
            ));
        }
        root = root.child(self.render_path_input_modal(cx));
        root = root.child(self.render_delete_confirm(cx));
        root.into_any_element()
    }
}

fn format_size(b: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    const GB: u64 = MB * 1024;
    if b >= GB {
        format!("{:.1} GB", b as f64 / GB as f64)
    } else if b >= MB {
        format!("{:.1} MB", b as f64 / MB as f64)
    } else if b >= KB {
        format!("{:.1} KB", b as f64 / KB as f64)
    } else {
        format!("{b} B")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn parent_of_handles_root_and_nested_paths() {
        assert_eq!(SftpPane::parent_of("/"), "/");
        assert_eq!(SftpPane::parent_of("////"), "/");
        assert_eq!(SftpPane::parent_of("/home/user"), "/home");
        assert_eq!(SftpPane::parent_of("/home/user/"), "/home");
        assert_eq!(SftpPane::parent_of("."), ".");
    }

    #[test]
    fn join_handles_relative_and_root_bases() {
        assert_eq!(SftpPane::join(".", "notes.txt"), "./notes.txt");
        assert_eq!(SftpPane::join("/", "notes.txt"), "/notes.txt");
        assert_eq!(
            SftpPane::join("/home/user", "notes.txt"),
            "/home/user/notes.txt"
        );
    }

    #[test]
    fn command_queue_reports_full_and_closed_channels() {
        let (tx, rx) = async_channel::bounded(1);
        let list = || SftpCmd::List {
            path: ".".to_string(),
        };

        assert_eq!(try_send_command(&tx, list()), Ok(()));
        assert_eq!(try_send_command(&tx, list()), Err(SFTP_CHANNEL_UNAVAILABLE));

        drop(rx);
        assert_eq!(try_send_command(&tx, list()), Err(SFTP_CHANNEL_UNAVAILABLE));
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
}
