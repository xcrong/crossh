//! Standalone Git Viewer entry point.
#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

use crossh_core::{git_launcher, single_instance};
use crossh_git_view as git;

use gpui::{App, QuitMode};

fn main() {
    // Windows GUI 子系统下无控制台：挂回父控制台，保证 --help/错误输出可见。
    crossh_core::process::attach_parent_console();

    let args = std::env::args().skip(1);
    let command = match git_launcher::parse_cli(
        args,
        std::env::current_dir().map_err(|error| error.to_string()),
    ) {
        Ok(command) => command,
        Err(error) => {
            eprintln!("crossh-git: {error}\n");
            git_launcher::print_help("crossh-git");
            std::process::exit(2);
        }
    };

    let git_launcher::GitCliCommand::Open(cwd) = command else {
        git_launcher::print_help("crossh-git");
        return;
    };

    // 已有实例直接转发后退出，避免每点一次多一个进程/窗口。
    // 被拒（目录在实例侧已失效等）时直接报错退出，不再另起实例——与主实例一致。
    let forward_cwd = dunce::canonicalize(&cwd).unwrap_or_else(|_| cwd.clone());
    if forward_cwd.is_dir() {
        match single_instance::try_forward_git(&forward_cwd) {
            single_instance::ForwardOutcome::Forwarded => {
                println!(
                    "crossh-git: opened {} in the running instance",
                    forward_cwd.display()
                );
                return;
            }
            single_instance::ForwardOutcome::Rejected(error) => {
                eprintln!("crossh-git: {error}");
                std::process::exit(2);
            }
            single_instance::ForwardOutcome::NoInstance => {}
        }
    }

    // 跟随主应用的语言设置：独立二进制不挂载 settings feature，
    // 只读 settings.toml 的 language 键（缺失/非法时跟随系统）。
    rust_i18n::set_locale(crossh_core::locale::persisted_locale_code());

    let app = gpui_platform::application()
        .with_assets(crossh_ui::assets::UiAssetSource::default())
        .with_quit_mode(QuitMode::LastWindowClosed);
    app.run(move |cx: &mut App| {
        cx.set_app_identity("me.xcrong.crossh.git", "Crossh Git");
        cx.init_colors();
        crossh_ui::assets::load_fonts(cx).expect("Crossh fonts should load");
        git::init(cx);
        git::open_git_window(cwd.clone(), cx);
        // 单实例投递桥：监听线程只做阻塞收包，前台任务经 open_git_window 落地
        // （目录切换复用窗口，见 window.rs）。与主实例同构（main.rs）。
        let (open_tx, open_rx) = tokio::sync::mpsc::unbounded_channel::<std::path::PathBuf>();
        if let Err(error) = single_instance::serve_git(move |request| {
            let _ = open_tx.send(request);
        }) {
            // 监听失败不阻断启动：退化为无复用，新窗口照常打开。
            log::warn!("git single-instance listener unavailable: {error}");
        }
        cx.spawn(async move |cx| {
            let mut open_rx = open_rx;
            while let Some(directory) = open_rx.recv().await {
                cx.update(|cx| git::open_git_window(directory, cx));
            }
        })
        .detach();
    });
}
