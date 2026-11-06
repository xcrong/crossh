//! Standalone Note Viewer entry point.
//!
//! 纯装配层：窗口内容来自 `crossh-note-view`，存储层来自 `crossh-note`，
//! 文本编辑语义与启动器来自 `crossh-core`，主题来自 `crossh-ui`。

#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

use crossh_core::{note_launcher, single_instance};
use crossh_note_view as note;

use gpui::{App, QuitMode};

fn main() {
    // Windows GUI 子系统下无控制台：挂回父控制台，保证 --help/错误输出可见。
    // 必须是 `main` 的第一句：先挂控制台再初始化日志，否则 Windows GUI 子系统的
    // stderr 副本会被丢掉（`logging::init` 立刻 tee 到 stderr，见
    // `crossh-core/src/logging.rs`）。
    crossh_core::process::attach_parent_console();

    // 独立子进程不经过根 bin 的 `app::bootstrap::init()`：不在这里初始化，
    // 本进程里所有 `log::warn!` / `log::error!` 都没有 sink（stderr 与文件都收不到）。
    // 必须早于 `parse_cli`（任何 warn 都要有 sink），但**启动记录不记在这里**：
    // `--help` / 参数错误这类立刻退出的路径也是同一入口，在这里记 INFO 会让它比
    // `crossh --help` / `crossh-git --help` 多打一行。启动记录只记在真正开窗的路径上。
    // `init` 一并装 panic hook：父进程把本进程 stderr 重定向到 null，
    // 没有 hook 的话 panic 在 run.log 里一痕不见。
    crossh_core::logging::init();

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

    // 跟随主应用的语言设置：独立二进制不挂载 settings feature，
    // 只读 settings.toml 的 language 键（缺失/非法时跟随系统）。
    rust_i18n::set_locale(crossh_core::locale::persisted_locale_code());

    // 真正要开窗了才记启动记录：`--help` / 参数错误 / 单实例转发都已在上面的分支
    // 退出，它们的输出保持与其它 bin 一样安静。
    log::info!("---- crossh-note startup (pid {}) ----", std::process::id());

    let app = gpui_platform::application()
        .with_assets(crossh_ui::assets::UiAssetSource::default())
        .with_quit_mode(QuitMode::LastWindowClosed);
    app.run(move |cx: &mut App| {
        cx.set_app_identity("me.xcrong.crossh.note", "Crossh Note");
        cx.init_colors();
        crossh_ui::theme::install_crossh_theme(cx);
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
