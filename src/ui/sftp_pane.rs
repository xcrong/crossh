//! SFTP 浏览面板：远端目录列表 / 进入目录 / 上传 / 下载，进度与状态。
//!
//! 通过 `Connection::open_sftp` 拿到 `(cmd_tx, event_rx)` 后由本面板持有；
//! 主线程 drain `event_rx` 更新列表/进度。下载落到 ~/Downloads（重名自动加序号）。

use async_channel::{Receiver, Sender};
use gpui::{
    AnyElement, App, AppContext, Context, Entity, FocusHandle, InteractiveElement, IntoElement,
    Keystroke, KeyDownEvent, ParentElement, Render, SharedString, StatefulInteractiveElement,
    Styled, Task, Window, div, rgb,
};

use crate::ssh::{RemoteEntry, SftpCmd, SftpEvent};

/// 传输进度快照。
#[derive(Clone, Debug, Default)]
struct Progress {
    label: String,
    transferred: u64,
    total: Option<u64>,
}

pub struct SftpPane {
    cmd_tx: Sender<SftpCmd>,
    cwd: String,
    entries: Vec<RemoteEntry>,
    message: Option<String>,
    upload_input: String,
    progress: Option<Progress>,
    focus: FocusHandle,
    _drain: Option<Task<()>>,
}

impl SftpPane {
    /// 用一个已有的 SFTP 桥接创建面板，并立即请求列出当前目录。
    pub fn from_bridge(
        cmd_tx: Sender<SftpCmd>,
        event_rx: Receiver<SftpEvent>,
        cx: &mut App,
    ) -> Entity<Self> {
        let entity = cx.new(|cx| Self {
            cmd_tx: cmd_tx.clone(),
            cwd: ".".to_string(),
            entries: Vec::new(),
            message: None,
            upload_input: String::new(),
            progress: None,
            focus: cx.focus_handle(),
            _drain: None,
        });

        // 请求初始列表。
        let _ = cmd_tx.try_send(SftpCmd::List {
            path: ".".to_string(),
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
                        SftpEvent::Done {
                            label, ok, message,
                        } => {
                            this.progress = None;
                            this.message = Some(if ok {
                                format!("{label}: {message}")
                            } else {
                                format!("{label} 失败: {message}")
                            });
                            // 传输完成后刷新当前目录。
                            let _ = this.cmd_tx.try_send(SftpCmd::List {
                                path: this.cwd.clone(),
                            });
                        }
                        SftpEvent::Error(e) => this.message = Some(e),
                        SftpEvent::Closed => this.message = Some("SFTP 已关闭".into()),
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

    fn cd(&self, path: String) {
        let _ = self.cmd_tx.try_send(SftpCmd::List { path });
    }

    fn parent_of(path: &str) -> String {
        let p = path.trim_end_matches('/');
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

    fn download(&self, name: &str) {
        let remote = Self::join(&self.cwd, name);
        let local = downloads_dir().join(name);
        let local = unique_local_path(&local);
        let _ = self.cmd_tx.try_send(SftpCmd::Download { remote, local });
    }

    fn do_upload(&mut self, cx: &mut Context<Self>) {
        let local = std::path::PathBuf::from(crate::config::expand_tilde(&self.upload_input));
        let basename = local
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "upload.bin".into());
        let remote = Self::join(&self.cwd, &basename);
        let _ = self.cmd_tx.try_send(SftpCmd::Upload { local, remote });
        self.upload_input.clear();
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
}

fn printable_char(ks: &Keystroke) -> Option<char> {
    if ks.modifiers.control || ks.modifiers.platform {
        return None;
    }
    ks.key_char.as_ref().and_then(|s| s.chars().next())
}

/// ~/Downloads。
fn downloads_dir() -> std::path::PathBuf {
    std::env::var_os("HOME")
        .map(|h| std::path::PathBuf::from(&h).join("Downloads"))
        .unwrap_or_else(|| std::path::PathBuf::from("."))
}

/// 重名时追加 ` (1)`、` (2)`… 避免覆盖。
fn unique_local_path(path: &std::path::Path) -> std::path::PathBuf {
    if !path.exists() {
        return path.to_path_buf();
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
            return candidate;
        }
    }
    path.to_path_buf()
}

impl Render for SftpPane {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
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
                        this.cd(p);
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
                        this.cd(this.cwd.clone());
                        cx.notify();
                    })),
            );

        // 列表区。
        let mut list = div().flex_1().flex().flex_col().px_2().py_2();
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
                        this.cd(p);
                    } else {
                        this.download(&name);
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
                    ),
            );

        if let Some(p) = &self.progress {
            let pct = p
                .total
                .filter(|&t| t > 0)
                .map(|t| ((p.transferred as f64 / t as f64) * 100.0) as u32)
                .unwrap_or(0);
            bottom = bottom.child(
                div()
                    .text_xs()
                    .text_color(rgb(0xb0b0b8))
                    .child(SharedString::from(format!(
                        "{}: {} / {} ({}%)",
                        p.label,
                        format_size(p.transferred),
                        p.total.map(format_size).unwrap_or_else(|| "?".into()),
                        pct
                    ))),
            );
        } else if let Some(msg) = &self.message {
            bottom = bottom.child(
                div()
                    .text_xs()
                    .text_color(rgb(0xb0b0b8))
                    .child(SharedString::from(msg.clone())),
            );
        }

        let _ = top;
        let _: AnyElement = div().into_any_element();

        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(rgb(0x121214))
            .child(top)
            .child(list)
            .child(bottom)
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
