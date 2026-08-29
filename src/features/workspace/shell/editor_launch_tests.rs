//! 「在外部编辑器中打开」状态栏按钮的错误 Toast 契约测试
//! (spec 20260820-open-project-in-editor)。
//!
//! 与 git_sync_toast_tests 相同的引导方式；`open_project_in_editor` 只读取
//! `workspace_settings` 与 PATH，不触碰终端，因此无需插入本地会话。
//!
//! 检测候选列表是代码常量（不可注入），本文件测试通过可注入 PATH 变体
//! `open_project_in_editor_with_path_env` 传入必然空目录来控制检测结果，
//! 不修改进程环境，避免与并发的 git 测试互相干扰。

use super::AppShell;
use gpui::Entity;

use crate::shared::i18n;

use std::path::Path;

/// 必然不包含任何编辑器命令的 PATH 值。
const EMPTY_PATH: &str = "/nonexistent-crossh-editor-path";

/// 取消编辑器覆盖项，使解析完全依赖写死的检测列表。
fn clear_editor_override(shell: &mut AppShell) {
    shell.workspace_settings.editor_command = None;
}

fn init_app(cx: &mut gpui::TestAppContext) -> Entity<AppShell> {
    cx.update(|cx| {
        use release_channel;
        use settings as zed_settings;
        use theme::LoadThemes;
        use theme_settings as zed_theme_settings;
        let app_version = release_channel::AppVersion::load(env!("CARGO_PKG_VERSION"), None, None);
        release_channel::init(app_version, cx);
        zed_settings::init(cx);
        zed_theme_settings::init(LoadThemes::JustBase, cx);
        crate::infrastructure::theme::install_crossh_theme(cx);
        super::open_main_window(cx);
        let window = cx.windows().into_iter().next().expect("main window");
        window
            .update(cx, |_, window, _| window.root::<AppShell>())
            .expect("main window update")
            .expect("main window root")
            .expect("main window root is AppShell")
    })
}

#[gpui::test]
fn spec_20260820_open_project_in_editor_no_detected_editor_shows_error_toast(
    cx: &mut gpui::TestAppContext,
) {
    let shell = init_app(cx);
    cx.update(|cx| {
        shell.update(cx, |shell, cx| {
            clear_editor_override(shell);
            shell.open_project_in_editor_with_path_env(Path::new("/repo"), EMPTY_PATH.into(), cx);
        });
    });

    let toast = cx
        .update(|cx| shell.read(cx).workspace.toaster.active().cloned())
        .expect("未检测到编辑器时必须弹出错误 Toast");
    assert_eq!(
        toast.notice.tone,
        crate::features::workspace::toaster::ToastTone::Error
    );
    assert_eq!(toast.notice.message, i18n::text("toast.editor_not_found"));
}

#[gpui::test]
fn spec_20260820_open_project_in_editor_spawn_failure_shows_error_toast(
    cx: &mut gpui::TestAppContext,
) {
    let shell = init_app(cx);
    cx.update(|cx| {
        shell.update(cx, |shell, cx| {
            shell.workspace_settings.editor_command =
                Some("/nonexistent-crossh-editor-bin".to_string());
            shell.open_project_in_editor(Path::new("/repo"), cx);
        });
    });

    let toast = cx
        .update(|cx| shell.read(cx).workspace.toaster.active().cloned())
        .expect("启动失败时必须弹出错误 Toast");
    assert_eq!(
        toast.notice.tone,
        crate::features::workspace::toaster::ToastTone::Error
    );
    assert_eq!(
        toast.notice.message,
        i18n::text("toast.editor_spawn_failed")
    );
}

#[gpui::test]
fn spec_20260820_open_project_in_editor_blank_command_treated_as_unconfigured(
    cx: &mut gpui::TestAppContext,
) {
    let shell = init_app(cx);
    cx.update(|cx| {
        shell.update(cx, |shell, cx| {
            shell.workspace_settings.editor_command = Some("   ".to_string());
            shell.open_project_in_editor_with_path_env(Path::new("/repo"), EMPTY_PATH.into(), cx);
        });
    });

    let toast = cx
        .update(|cx| shell.read(cx).workspace.toaster.active().cloned())
        .expect("空白命令视为未配置，回退检测后仍无命中应弹错误 Toast");
    assert_eq!(
        toast.notice.message,
        i18n::text("toast.editor_not_found"),
        "空白 editor_command 不得以空命令启动，也不得被当作有效配置"
    );
}
