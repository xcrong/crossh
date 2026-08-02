//! SFTP 浏览面板：远端目录列表 / 进入目录 / 上传 / 下载，进度与状态。
//!
//! 通过 `Connection::open_sftp` 拿到 `(cmd_tx, event_rx)` 后由本面板持有；
//! 主线程 drain `event_rx` 更新列表/进度。下载落到 ~/Downloads（重名自动加序号）。

use async_channel::{Receiver, Sender};
use gpui::{
    AnyElement, App, AppContext, ClipboardEntry, Context, Entity, FocusHandle, InteractiveElement,
    IntoElement, KeyDownEvent, Keystroke, ParentElement, PathPromptOptions, Render, SharedString,
    StatefulInteractiveElement, Styled, Task, Window, div, px, rgb,
};

use crate::ssh::{MAX_EDITOR_FILE_BYTES, RemoteEntry, SftpCmd, SftpEvent};

/// 传输进度快照。
#[derive(Clone, Debug, Default)]
struct Progress {
    label: String,
    transferred: u64,
    total: Option<u64>,
}

const SFTP_CHANNEL_UNAVAILABLE: &str = "SFTP 通道不可用";

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
    _drain: Option<Task<()>>,
    _picker: Option<Task<()>>,
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
            message: (!initial_list_ok).then(|| SFTP_CHANNEL_UNAVAILABLE.to_string()),
            loading: initial_list_ok,
            upload_input: String::new(),
            progress: None,
            editor: None,
            focus: cx.focus_handle(),
            _drain: None,
            _picker: None,
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
                                        editor.error = Some("该文件不是 UTF-8 文本文件".into());
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
                                format!("{label} 失败: {message}")
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
                                format!("保存成功: {message}")
                            } else {
                                format!("保存失败: {message}")
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
                                editor.error = Some("SFTP 已关闭".into());
                            }
                            this.message = Some("SFTP 已关闭".into());
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
            self.message = Some(SFTP_CHANNEL_UNAVAILABLE.to_string());
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
            self.message = Some(format!("无法为 {} 找到可用的本地文件名", name));
            return;
        };
        if try_send_command(&self.cmd_tx, SftpCmd::Download { remote, local }).is_err() {
            self.message = Some(SFTP_CHANNEL_UNAVAILABLE.to_string());
        } else {
            self.message = Some(format!("准备下载: {name}"));
        }
    }

    fn open_file_or_download(&mut self, name: &str, cx: &mut Context<Self>) {
        if !is_supported_text_file(name) {
            self.download(name);
            return;
        }

        let remote = Self::join(&self.cwd, name);
        let mut editor = RemoteEditor::loading(remote.clone(), name.to_string(), cx.focus_handle());
        if try_send_command(&self.cmd_tx, SftpCmd::ReadFile { remote }).is_err() {
            editor.loading = false;
            editor.error = Some(SFTP_CHANNEL_UNAVAILABLE.to_string());
            self.message = Some(SFTP_CHANNEL_UNAVAILABLE.to_string());
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
            prompt: Some("选择要上传的文件".into()),
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
            self.message = Some("请输入本地文件路径".into());
            cx.notify();
            return;
        }
        let local = std::path::PathBuf::from(crate::config::expand_tilde(input));
        if !local.is_file() {
            self.message = Some(format!("本地文件不存在: {}", local.display()));
            cx.notify();
            return;
        }
        let basename = local
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "upload.bin".into());
        let remote = Self::join(&self.cwd, &basename);
        if try_send_command(&self.cmd_tx, SftpCmd::Upload { local, remote }).is_err() {
            self.message = Some(SFTP_CHANNEL_UNAVAILABLE.to_string());
        } else {
            self.message = Some(format!("准备上传: {basename}"));
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
            self.message = Some("有未保存修改，请先保存或放弃".into());
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
                editor.error = Some("文件内容超过编辑器大小上限".into());
            }
            cx.notify();
            return;
        }
        if try_send_command(&self.cmd_tx, SftpCmd::WriteFile { remote, contents }).is_err() {
            self.message = Some(SFTP_CHANNEL_UNAVAILABLE.to_string());
        } else {
            if let Some(editor) = &mut self.editor {
                editor.saving = true;
                editor.error = None;
            }
            self.message = Some("正在保存…".into());
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
            .bg(rgb(0x18181b))
            .border_b_1()
            .border_color(rgb(0x2a2a2e))
            .child(
                div()
                    .id("sftp-editor-back")
                    .px_2()
                    .py_1()
                    .cursor_pointer()
                    .bg(rgb(0x2a2a2e))
                    .hover(|s| s.bg(rgb(0x3a3a40)))
                    .text_xs()
                    .text_color(rgb(0xe6e6e6))
                    .child(SharedString::from("← 文件列表"))
                    .on_click(cx.listener(|this, _ev, _window, cx| {
                        this.close_editor(cx);
                    })),
            )
            .child(
                div()
                    .flex_1()
                    .text_xs()
                    .text_color(rgb(0xe6e6e6))
                    .child(SharedString::from(name)),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(if read_only {
                        rgb(0x9aa5ff)
                    } else {
                        rgb(0x8ed6a7)
                    })
                    .child(SharedString::from(if read_only {
                        "只读"
                    } else {
                        "编辑中"
                    })),
            );

        let mut actions = div().flex().flex_row().items_center().gap_1();
        if read_only {
            actions = actions.child(
                div()
                    .id("sftp-editor-edit")
                    .px_2()
                    .py_1()
                    .cursor_pointer()
                    .bg(rgb(0x2a2a2e))
                    .hover(|s| s.bg(rgb(0x3a3a40)))
                    .text_xs()
                    .text_color(rgb(0xe6e6e6))
                    .child(SharedString::from("进入编辑"))
                    .on_click(cx.listener(|this, _ev, window, cx| {
                        this.enter_editor_edit(window, cx);
                    })),
            );
        } else {
            actions = actions.child(
                div()
                    .id("sftp-editor-read-only")
                    .px_2()
                    .py_1()
                    .cursor_pointer()
                    .bg(rgb(0x2a2a2e))
                    .hover(|s| s.bg(rgb(0x3a3a40)))
                    .text_xs()
                    .text_color(rgb(0xe6e6e6))
                    .child(SharedString::from("只读"))
                    .on_click(cx.listener(|this, _ev, _window, cx| {
                        this.leave_editor_edit(cx);
                    })),
            );
            if dirty {
                actions = actions.child(
                    div()
                        .id("sftp-editor-save")
                        .px_2()
                        .py_1()
                        .cursor_pointer()
                        .bg(rgb(0x4352a3))
                        .hover(|s| s.bg(rgb(0x5364c5)))
                        .text_xs()
                        .text_color(rgb(0xffffff))
                        .child(SharedString::from(if saving {
                            "保存中…"
                        } else {
                            "保存"
                        }))
                        .on_click(cx.listener(|this, _ev, _window, cx| {
                            this.save_editor(cx);
                        })),
                );
                actions = actions.child(
                    div()
                        .id("sftp-editor-discard")
                        .px_2()
                        .py_1()
                        .cursor_pointer()
                        .bg(rgb(0x2a2a2e))
                        .hover(|s| s.bg(rgb(0x3a3a40)))
                        .text_xs()
                        .text_color(rgb(0xe6e6e6))
                        .child(SharedString::from("放弃"))
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
            .overflow_y_scroll()
            .bg(rgb(0x101012))
            .track_focus(&focus)
            .tab_stop(true)
            .focus(|style| style.border_color(rgb(0x6e7cff)))
            .on_click({
                let focus = focus.clone();
                move |_ev, window, cx| window.focus(&focus, cx)
            })
            .on_key_down(cx.listener(SftpPane::handle_editor_key));

        if loading {
            body = body.child(
                div()
                    .text_xs()
                    .text_color(rgb(0xb0b0b8))
                    .child(SharedString::from("读取中…")),
            );
        } else if let Some(error) = error.clone() {
            body = body.child(
                div()
                    .text_xs()
                    .text_color(rgb(0xe58f8f))
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
                    .text_color(rgb(0xe6e6e6))
                    .child(
                        div()
                            .w(px(42.))
                            .flex_shrink_0()
                            .text_color(rgb(0x5f606b))
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
                        .child(div().w(px(1.)).h(px(18.)).flex_shrink_0().bg(rgb(0x9aa5ff)))
                        .child(SharedString::from(line[cursor_byte..].to_string()));
                } else {
                    row = row.child(SharedString::from(line.to_string()));
                }
                body = body.child(row);
            }
        }

        let footer_text = if loading {
            "正在读取远端文件…".to_string()
        } else if saving {
            "正在保存远端文件…".to_string()
        } else if dirty {
            "有未保存修改".to_string()
        } else if error.is_some() {
            "无法编辑此文件".to_string()
        } else {
            "已保存".to_string()
        };
        div()
            .size_full()
            .min_h_0()
            .flex()
            .flex_col()
            .bg(rgb(0x121214))
            .child(header)
            .child(body)
            .child(
                div()
                    .flex_shrink_0()
                    .px_3()
                    .py_1()
                    .bg(rgb(0x18181b))
                    .border_t_1()
                    .border_color(rgb(0x2a2a2e))
                    .text_xs()
                    .text_color(rgb(0x8e8e98))
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
            .bg(rgb(0x18181b))
            .border_b_1()
            .border_color(rgb(0x2a2a2e))
            .child(
                div()
                    .id("sftp-up")
                    .px_2()
                    .py_1()
                    .cursor_pointer()
                    .bg(rgb(0x2a2a2e))
                    .hover(|s| s.bg(rgb(0x3a3a40)))
                    .text_xs()
                    .text_color(rgb(0xe6e6e6))
                    .child(SharedString::from("↑ 上级"))
                    .on_click(cx.listener(|this, _ev, _w, cx| {
                        let p = Self::parent_of(&this.cwd);
                        this.request_list(p);
                        cx.notify();
                    })),
            )
            .child(
                div()
                    .flex_1()
                    .text_xs()
                    .text_color(rgb(0xb0b0b8))
                    .child(SharedString::from(cwd)),
            )
            .child(
                div()
                    .id("sftp-refresh")
                    .px_2()
                    .py_1()
                    .cursor_pointer()
                    .bg(rgb(0x2a2a2e))
                    .hover(|s| s.bg(rgb(0x3a3a40)))
                    .text_xs()
                    .text_color(rgb(0xe6e6e6))
                    .child(SharedString::from("刷新"))
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
                .px_2()
                .py_1()
                .cursor_pointer()
                .hover(|s| s.bg(rgb(0x232327)))
                .child(div().text_xs().text_color(rgb(0x888892)).child(if is_dir {
                    "📁"
                } else {
                    "📄"
                }))
                .child(
                    div()
                        .flex_1()
                        .text_xs()
                        .text_color(rgb(0xe6e6e6))
                        .child(SharedString::from(name.clone())),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(rgb(0x6a6a72))
                        .child(SharedString::from(size)),
                )
                .on_click(cx.listener(move |this, _ev, _w, cx| {
                    if is_dir {
                        let p = Self::join(&this.cwd, &name);
                        this.request_list(p);
                    } else {
                        this.open_file_or_download(&name, cx);
                    }
                    cx.notify();
                }));
            list = list.child(row);
        }

        // 底部：上传输入 + 进度/消息。
        let focus = self.focus.clone();
        let upload_val = self.upload_input.clone();
        let input = div()
            .id("sftp-upload-input")
            .flex_1()
            .px_2()
            .py_1()
            .bg(rgb(0x121214))
            .border_1()
            .border_color(rgb(0x3a3a40))
            .text_xs()
            .text_color(rgb(0xe6e6e6))
            .track_focus(&focus)
            .tab_stop(true)
            .focus(|style| style.border_color(rgb(0x6e7cff)))
            .focus_visible(|style| style.border_color(rgb(0x9aa5ff)))
            .on_click({
                let focus = focus.clone();
                move |_ev, window, cx| window.focus(&focus, cx)
            })
            .on_key_down(cx.listener(SftpPane::handle_input_key))
            .child(SharedString::from(if upload_val.is_empty() {
                "本地文件路径（回车上传）".to_string()
            } else {
                upload_val
            }));

        let mut bottom = div()
            .flex()
            .flex_col()
            .gap_1()
            .px_3()
            .py_2()
            .bg(rgb(0x18181b))
            .border_t_1()
            .border_color(rgb(0x2a2a2e))
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
                            .px_2()
                            .py_1()
                            .cursor_pointer()
                            .bg(rgb(0x2a2a2e))
                            .hover(|s| s.bg(rgb(0x3a3a40)))
                            .text_xs()
                            .text_color(rgb(0xe6e6e6))
                            .child(SharedString::from("上传"))
                            .on_click(cx.listener(|this, _ev, _w, cx| {
                                this.do_upload(cx);
                            })),
                    )
                    .child(
                        div()
                            .id("sftp-choose-file")
                            .px_2()
                            .py_1()
                            .cursor_pointer()
                            .bg(rgb(0x2a2a2e))
                            .hover(|s| s.bg(rgb(0x3a3a40)))
                            .text_xs()
                            .text_color(rgb(0xe6e6e6))
                            .child(SharedString::from("选择文件"))
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
            bottom = bottom.child(div().text_xs().text_color(rgb(0xb0b0b8)).child(
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
                    .text_color(rgb(0xb0b0b8))
                    .child(SharedString::from("加载中…")),
            );
        } else if let Some(msg) = &self.message {
            bottom = bottom.child(
                div()
                    .text_xs()
                    .text_color(rgb(0xb0b0b8))
                    .child(SharedString::from(msg.clone())),
            );
        }

        div()
            .size_full()
            .min_h_0()
            .flex()
            .flex_col()
            .bg(rgb(0x121214))
            .child(top)
            .child(list)
            .child(bottom)
            .into_any_element()
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
