//! Standalone Note Viewer entry point.
//!
//! 共享逻辑通过 `#[path]` 直接复用 `src/features/note/*` 与
//! `note_launcher.rs` / `infrastructure/theme.rs`，与主 `crossh` 二进制同源。
//! `crates/crossh-note` 仅承载存储层；搜索框与侧栏/Git 共用 `src/shared` 的
//! `TextEditingState` 编辑语义，此处按 `crossh-git` 的同构方式挂载。

#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

use crossh_core::single_instance;

#[path = "../features/note_launcher.rs"]
mod note_launcher;

#[path = "../features/note/mod.rs"]
mod note;

#[path = "../shared/text_editing.rs"]
pub mod text_editing;

#[path = "../shared/input_handler.rs"]
pub mod input_handler;

mod shared {
    pub use crate::input_handler;
    pub use crate::text_editing;
}

#[path = "../infrastructure/theme.rs"]
mod infrastructure_theme;

use gpui::{App, QuitMode};
use release_channel as zed_release_channel;
use theme::LoadThemes;

fn main() {
    // Windows GUI 子系统下无控制台：挂回父控制台，保证 --help/错误输出可见。
    crossh_core::process::attach_parent_console();

    let args = std::env::args().skip(1);
    let command = match note_launcher::parse_cli(args) {
        Ok(command) => command,
        Err(error) => {
            eprintln!("crossh-note: {error}\n");
            note_launcher::print_help("crossh-note");
            std::process::exit(2);
        }
    };

    let note_launcher::NoteCliCommand::Open = command else {
        note_launcher::print_help("crossh-note");
        return;
    };

    // 已有实例直接转发后退出，避免每点一次多一个进程/窗口。
    // 被拒时直接报错退出，不再另起实例——与主实例一致。
    match single_instance::try_forward_note() {
        single_instance::ForwardOutcome::Forwarded => {
            println!("crossh-note is already running");
            return;
        }
        single_instance::ForwardOutcome::Rejected(error) => {
            eprintln!("crossh-note: {error}");
            std::process::exit(2);
        }
        single_instance::ForwardOutcome::NoInstance => {}
    }

    let app = gpui_platform::application()
        .with_assets(crossh_ui::assets::UiAssetSource::default())
        .with_quit_mode(QuitMode::LastWindowClosed);
    app.run(move |cx: &mut App| {
        cx.set_app_identity("me.xcrong.crossh.note", "Crossh Note");
        cx.init_colors();
        let app_version =
            zed_release_channel::AppVersion::load(env!("CARGO_PKG_VERSION"), None, None);
        zed_release_channel::init(app_version, cx);
        theme::init(LoadThemes::JustBase, cx);
        infrastructure_theme::install_crossh_theme(cx);
        crossh_ui::assets::load_fonts(cx).expect("Crossh fonts should load");
        note::init(cx);
        note::open_note_window(cx);
        // 单实例投递桥：监听线程只做阻塞收包，前台任务经 open_note_window 落地
        // （同进程复用窗口，见 window.rs）。与主实例同构（main.rs）。
        let (open_tx, open_rx) = tokio::sync::mpsc::unbounded_channel::<()>();
        if let Err(error) = single_instance::serve_note(move || {
            let _ = open_tx.send(());
        }) {
            // 监听失败不阻断启动：退化为无复用，新窗口照常打开。
            log::warn!("note single-instance listener unavailable: {error}");
        }
        cx.spawn(async move |cx| {
            let mut open_rx = open_rx;
            while open_rx.recv().await.is_some() {
                cx.update(note::open_note_window);
            }
        })
        .detach();
    });
}
