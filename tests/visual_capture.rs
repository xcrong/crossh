//! Main-thread visual capture executable for the real Crossh workspace.

#![allow(dead_code, unused_imports)]

#[cfg(target_os = "macos")]
#[path = "../src/app/mod.rs"]
mod app;
#[cfg(target_os = "macos")]
#[path = "../src/features/mod.rs"]
mod features;
#[cfg(target_os = "macos")]
#[path = "../src/infrastructure/mod.rs"]
mod infrastructure;
#[cfg(target_os = "macos")]
#[path = "../src/shared/mod.rs"]
mod shared;

#[cfg(target_os = "macos")]
use std::sync::Arc;

#[cfg(target_os = "macos")]
use assets::Assets as ZedAssets;
#[cfg(target_os = "macos")]
use gpui::{Size, VisualTestAppContext, actions, px, size};
#[cfg(target_os = "macos")]
use theme::LoadThemes;

#[cfg(target_os = "macos")]
rust_i18n::i18n!("locales", fallback = "en");

#[cfg(target_os = "macos")]
actions!(
    crossh,
    [
        About,
        CheckForUpdates,
        CloseActiveTab,
        CloseWindow,
        Hide,
        HideOthers,
        MinimizeWindow,
        NewTerminal,
        OpenProject,
        OpenSettings,
        Quit,
        ShowAll,
        ToggleFullScreen,
        ToggleHostSidebar,
        ToggleQuickCommands,
        ToggleTimestamps,
        ZoomWindow
    ]
);

#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("GPUI's pinned headless renderer is currently available only on macOS");
}

#[cfg(target_os = "macos")]
fn main() {
    let platform = gpui_platform::current_platform(true);
    let mut cx = VisualTestAppContext::with_asset_source(
        platform,
        Arc::new(crossh_ui::assets::UiAssetSource),
    );

    cx.update(|cx| {
        cx.set_app_identity("io.crossh.app.visual-test", "Crossh Visual Test");
        cx.init_colors();
        let app_version = release_channel::AppVersion::load(env!("CARGO_PKG_VERSION"), None, None);
        release_channel::init(app_version, cx);
        settings::init(cx);
        theme_settings::init(LoadThemes::JustBase, cx);
        infrastructure::theme::install_crossh_theme(cx);
        ZedAssets
            .load_fonts(cx)
            .expect("Zed embedded fonts should load");
        features::settings::init(cx);
        features::terminal::init(cx);
    });

    capture_workspace(
        &mut cx,
        size(px(1100.), px(720.)),
        "/tmp/crossh-workspace.png",
    );
    capture_workspace(
        &mut cx,
        size(px(700.), px(420.)),
        "/tmp/crossh-workspace-compact.png",
    );
}

#[cfg(target_os = "macos")]
fn capture_workspace(cx: &mut VisualTestAppContext, window_size: Size<gpui::Pixels>, output: &str) {
    let window = cx
        .open_offscreen_window(window_size, |_window, cx| {
            features::workspace::AppShell::new(cx)
        })
        .expect("visual test window should open");

    cx.run_until_parked();
    cx.update_window(window.into(), |_, window, _cx| window.refresh())
        .expect("visual test window should refresh");
    cx.run_until_parked();

    cx.capture_screenshot(window.into())
        .expect("workspace screenshot should render")
        .save(output)
        .expect("workspace screenshot should be saved");
    println!("saved {output}");
}
