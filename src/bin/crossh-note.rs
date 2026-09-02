//! Standalone Note Viewer entry point.
//!
//! 共享逻辑通过 `#[path]` 直接复用 `src/features/note/*` 与
//! `note_launcher.rs` / `infrastructure/theme.rs`，与主 `crossh` 二进制同源。
//! `crates/crossh-note` 仅承载存储层；编辑器已迁移至 `crates/crossh-editor` 的
//! `TextareaState/InputState`，不再依赖 `src/shared/text_editing.rs`。

#[path = "../features/note_launcher.rs"]
mod note_launcher;

#[path = "../features/note/mod.rs"]
mod note;

#[path = "../infrastructure/theme.rs"]
mod infrastructure_theme;

use gpui::{App, QuitMode};
use release_channel as zed_release_channel;
use theme::LoadThemes;

fn main() {
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
    });
}
