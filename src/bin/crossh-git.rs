//! Standalone Git Viewer entry point.

#[path = "../shared/text_editing.rs"]
pub mod text_editing;

#[path = "../shared/utf16.rs"]
pub mod utf16;

mod shared {
    pub mod i18n {
        pub fn text(key: &str) -> String {
            rust_i18n::t!(key).to_string()
        }
    }

    pub use crate::text_editing;
    pub use crate::utf16;
}

rust_i18n::i18n!("locales", fallback = "en");

#[path = "../features/git_launcher.rs"]
mod git_launcher;

#[path = "../features/git/mod.rs"]
mod git;

use gpui::{App, QuitMode};

fn main() {
    let args = std::env::args().skip(1);
    let command = match git_launcher::parse_cli(
        args,
        std::env::current_dir().map_err(|error| error.to_string()),
    ) {
        Ok(command) => command,
        Err(error) => {
            eprintln!("crossh-git: {error}\n");
            git_launcher::print_standalone_cli_help();
            std::process::exit(2);
        }
    };

    let git_launcher::GitCliCommand::Open(cwd) = command else {
        git_launcher::print_standalone_cli_help();
        return;
    };

    let app = gpui_platform::application()
        .with_assets(crossh_ui::assets::UiAssetSource::default())
        .with_quit_mode(QuitMode::LastWindowClosed);
    app.run(move |cx: &mut App| {
        cx.set_app_identity("io.github.xcrong.crossh.git", "Crossh Git");
        cx.init_colors();
        crossh_ui::assets::load_fonts(cx).expect("Crossh fonts should load");
        git::init(cx);
        git::open_git_window(cwd.clone(), cx);
    });
}
