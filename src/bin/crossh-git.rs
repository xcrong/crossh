//! Standalone Git Viewer entry point.
#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

#[path = "../shared/text_editing.rs"]
pub mod text_editing;

mod shared {
    pub mod i18n {
        pub fn text(key: &str) -> String {
            rust_i18n::t!(key).to_string()
        }
    }

    pub use crate::text_editing;
}

rust_i18n::i18n!("locales", fallback = "en");

#[path = "../features/git_launcher.rs"]
mod git_launcher;

#[path = "../features/git/mod.rs"]
mod git;

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
    });
}
