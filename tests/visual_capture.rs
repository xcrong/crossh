//! Main-thread visual capture executable for the real Crossh workspace.

#![allow(dead_code, unused_imports)]

#[cfg(target_os = "macos")]
#[path = "../src/app/mod.rs"]
mod app;
#[cfg(target_os = "macos")]
#[path = "../src/features/mod.rs"]
mod features;
#[cfg(target_os = "macos")]
#[path = "../src/features/git/mod.rs"]
mod git;
#[cfg(target_os = "macos")]
#[path = "../src/infrastructure/mod.rs"]
mod infrastructure;
#[cfg(target_os = "macos")]
#[path = "../src/shared/mod.rs"]
mod shared;

#[cfg(target_os = "macos")]
use std::sync::Arc;
#[cfg(target_os = "macos")]
use std::{fs, path::Path, path::PathBuf, process::Command};

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
        Arc::new(crossh_ui::assets::UiAssetSource::default()),
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
        git::init(cx);
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

    let git_fixture = create_git_fixture();
    capture_git_window(
        &mut cx,
        size(px(1000.), px(640.)),
        git_fixture.clone(),
        false,
        false,
        "/tmp/crossh-git-standard.png",
    );
    capture_git_window(
        &mut cx,
        size(px(720.), px(480.)),
        git_fixture.clone(),
        false,
        false,
        "/tmp/crossh-git-compact-list.png",
    );
    capture_git_window(
        &mut cx,
        size(px(720.), px(480.)),
        git_fixture.clone(),
        true,
        false,
        "/tmp/crossh-git-compact-diff.png",
    );
    capture_git_window(
        &mut cx,
        size(px(720.), px(480.)),
        git_fixture.clone(),
        false,
        true,
        "/tmp/crossh-git-compact-error.png",
    );
    let empty_git_fixture = create_empty_git_fixture();
    capture_git_window(
        &mut cx,
        size(px(720.), px(480.)),
        empty_git_fixture.clone(),
        false,
        false,
        "/tmp/crossh-git-compact-empty.png",
    );
    let _ = fs::remove_dir_all(git_fixture);
    let _ = fs::remove_dir_all(empty_git_fixture);
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

#[cfg(target_os = "macos")]
fn create_git_fixture() -> PathBuf {
    let fixture = std::env::temp_dir().join("crossh-git-visual-fixture");
    let _ = fs::remove_dir_all(&fixture);
    fs::create_dir_all(fixture.join("src/features/git")).expect("create git fixture");
    run_git(&fixture, &["init", "-q"]);
    run_git(&fixture, &["config", "user.email", "visual@crossh.local"]);
    run_git(&fixture, &["config", "user.name", "Crossh Visual"]);
    fs::write(
        fixture.join("src/features/git/window.rs"),
        "fn render() {\n    println!(\"before\");\n}\n",
    )
    .expect("write tracked source");
    fs::write(fixture.join("README.md"), "# Visual fixture\n").expect("write readme");
    run_git(&fixture, &["add", "-A"]);
    run_git(&fixture, &["commit", "-qm", "initial"]);

    fs::write(
        fixture.join("src/features/git/window.rs"),
        concat!(
            "fn render() {\n",
            "    println!(\"after\");\n",
            "    let deliberately_long_line = \"this line verifies that horizontal scrolling preserves every character in a wide diff viewport\";\n",
            "}\n"
        ),
    )
    .expect("modify tracked source");
    fs::write(
        fixture.join("src/features/git/model.rs"),
        "pub struct ChangeKey { pub path: String }\n",
    )
    .expect("write staged source");
    run_git(&fixture, &["add", "src/features/git/model.rs"]);
    fs::write(fixture.join("notes-中文.md"), "检查中文路径与未跟踪状态\n")
        .expect("write untracked note");
    fixture
}

#[cfg(target_os = "macos")]
fn create_empty_git_fixture() -> PathBuf {
    let fixture = std::env::temp_dir().join("crossh-git-empty-visual-fixture");
    let _ = fs::remove_dir_all(&fixture);
    fs::create_dir_all(&fixture).expect("create empty Git fixture");
    run_git(&fixture, &["init", "-q"]);
    fixture
}

#[cfg(target_os = "macos")]
fn run_git(cwd: &Path, args: &[&str]) {
    let output = Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(args)
        .output()
        .expect("git fixture command should run");
    assert!(output.status.success(), "{args:?}: {:?}", output.stderr);
}

#[cfg(target_os = "macos")]
fn capture_git_window(
    cx: &mut VisualTestAppContext,
    window_size: Size<gpui::Pixels>,
    cwd: PathBuf,
    show_compact_diff: bool,
    show_error: bool,
    output: &str,
) {
    let window = cx
        .open_offscreen_window(window_size, |_window, cx| {
            git::visual_fixture(cwd, show_compact_diff, show_error, cx)
        })
        .expect("Git visual test window should open");
    cx.run_until_parked();
    cx.update_window(window.into(), |_, window, _cx| window.refresh())
        .expect("Git visual test window should refresh");
    cx.run_until_parked();
    cx.capture_screenshot(window.into())
        .expect("Git screenshot should render")
        .save(output)
        .expect("Git screenshot should be saved");
    println!("saved {output}");
}
