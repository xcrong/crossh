//! Standalone Note Viewer entry point.

#[path = "../shared/text_editing.rs"]
pub mod text_editing;

#[path = "../shared/utf16.rs"]
pub mod utf16;

#[path = "../shared/input_handler.rs"]
pub mod input_handler;

mod shared {
    pub mod i18n {
        #[allow(dead_code)]
        pub fn text(key: &str) -> String {
            rust_i18n::t!(key).to_string()
        }
    }

    pub use crate::input_handler;
    pub use crate::text_editing;
    pub use crate::utf16;
}

rust_i18n::i18n!("locales", fallback = "en");

#[path = "../features/note_launcher.rs"]
mod note_launcher;

#[path = "../features/note/mod.rs"]
mod note;

#[path = "../infrastructure/theme.rs"]
mod infrastructure_theme;

use gpui::{App, QuitMode};
use release_channel as zed_release_channel;
use settings as zed_settings;
use theme::LoadThemes;
use theme_settings as zed_theme_settings;
fn main() {
    let args = std::env::args().skip(1);
    let command = match note_launcher::parse_cli(args) {
        Ok(command) => command,
        Err(error) => {
            eprintln!("crossh-note: {error}\n");
            note_launcher::print_standalone_cli_help();
            std::process::exit(2);
        }
    };

    let note_launcher::NoteCliCommand::Open = command else {
        note_launcher::print_standalone_cli_help();
        return;
    };

    let app = gpui_platform::application()
        .with_assets(crossh_ui::assets::UiAssetSource::default())
        .with_quit_mode(QuitMode::LastWindowClosed);
    app.run(move |cx: &mut App| {
        cx.set_app_identity("io.github.xcrong.crossh.note", "Crossh Note");
        cx.init_colors();
        let app_version =
            zed_release_channel::AppVersion::load(env!("CARGO_PKG_VERSION"), None, None);
        zed_release_channel::init(app_version, cx);
        zed_settings::init(cx);
        zed_theme_settings::init(LoadThemes::JustBase, cx);
        infrastructure_theme::install_crossh_theme(cx);
        crossh_ui::assets::load_fonts(cx).expect("Crossh fonts should load");
        editor::init(cx);
        note::init(cx);
        note::open_note_window(cx);
    });
}
