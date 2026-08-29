//! Git Push / Pull 完成结果接入 Toaster 的契约测试
//! (spec 20260817-git-sync-toast)。
//!
//! 注意:这些测试会推进 foreground 任务并等待真实 `git` 子进程完成,
//! 因此不能通过 `open_local_session` 创建带真实 shell 的终端——真实
//! shell 的 PTY reader 线程在测试窗口内输出 prompt 时会唤醒 foreground
//! executor,触发 test_scheduler 的确定性守卫。这里用静默终端
//! (`sleep`,见 `silent_terminal`)占位 `LocalSession.terminal`,
//! `run_git_sync` 只读取 `cwd`,不触碰终端。

use super::AppShell;
use gpui::Entity;

use crate::features::terminal::view::TerminalView;
use crate::features::workspace::view::LocalSession;
use crate::shared::i18n;

use std::path::{Path, PathBuf};
use std::process::Command;

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

/// 静默终端(sleep,无输出)占位 `LocalSession.terminal`;真实 shell 的
/// PTY reader 线程会在测试窗口内输出 prompt,触发 test_scheduler 守卫。
fn silent_terminal(shell: &AppShell, cx: &mut gpui::App) -> Entity<TerminalView> {
    TerminalView::from_zed_shell(
        None,
        Some("~".to_string()),
        task::Shell::WithArguments {
            program: "sleep".into(),
            args: vec!["3600".into()],
            title_override: None,
        },
        false,
        shell.terminal_settings.clone(),
        cx,
    )
}

/// 在 temp 目录初始化一个带 commit 的 git 仓库;`with_origin` 为真时
/// 再挂一个本地 bare 远程供 push/pull 成功。
fn git_repo(tag: &str, with_origin: bool) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "crossh-git-sync-toast-{tag}-{}-{}",
        std::process::id(),
        with_origin
    ));
    std::fs::remove_dir_all(&dir).ok();
    std::fs::create_dir_all(&dir).unwrap();
    run_git(&dir, &["init", "-q"]);
    run_git(
        &dir,
        &[
            "-c",
            "user.name=test",
            "-c",
            "user.email=test@example.com",
            "commit",
            "-q",
            "--allow-empty",
            "-m",
            "init",
        ],
    );
    if with_origin {
        let remote = dir.join("remote.git");
        run_git(&dir, &["init", "-q", "--bare", "remote.git"]);
        run_git(&dir, &["remote", "add", "origin", remote.to_str().unwrap()]);
        // 预置 upstream,使后续 pull 处于 up-to-date(exit 0)。
        run_git(&dir, &["push", "-q", "-u", "origin", "HEAD"]);
    }
    dir
}

/// 有 upstream 但 push 必失败的仓库:本地领先 1 个 commit,origin URL 已改坏。
/// 失败后 `reconcile_git_sync_error` 只在 ahead==0 时清除错误,此仓库
/// ahead=1,错误必须保留在按钮上。
fn broken_origin_repo(tag: &str) -> PathBuf {
    let dir = git_repo(tag, true);
    run_git(
        &dir,
        &[
            "-c",
            "user.name=test",
            "-c",
            "user.email=test@example.com",
            "commit",
            "-q",
            "--allow-empty",
            "-m",
            "second",
        ],
    );
    run_git(
        &dir,
        &[
            "remote",
            "set-url",
            "origin",
            "file:///nonexistent-crossh-remote.git",
        ],
    );
    dir
}

fn run_git(cwd: &Path, args: &[&str]) {
    let output = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .expect("git should run");
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// 手动插入一个本地会话(带静默终端),返回其 id。
fn insert_local_session(shell: &mut AppShell, cwd: PathBuf, cx: &mut gpui::Context<AppShell>) {
    let terminal = silent_terminal(shell, cx);
    shell.workspace.sessions.local_sessions.insert(
        1,
        LocalSession {
            project_dir: cwd.clone(),
            cwd,
            terminal,
            git_status: None,
            git_refresh: Default::default(),
            pin_id: None,
            custom_name: None,
            default_command: None,
        },
    );
}

/// 推进 foreground 直到 Toast 出现(或超时)。git 在 background executor
/// 真实执行,完成时唤醒 foreground 提交 toast。
fn wait_for_toast(shell: &Entity<AppShell>, cx: &mut gpui::TestAppContext) {
    let mut ticks = 0;
    while cx.update(|cx| shell.read(cx).workspace.toaster.active().is_none()) {
        assert!(
            ticks < 2_000,
            "timed out waiting for git sync toast (git subprocess hung?)"
        );
        cx.run_until_parked();
        ticks += 1;
    }
}

#[gpui::test]
fn spec_20260817_git_sync_toast_push_success_shows_success_toast(cx: &mut gpui::TestAppContext) {
    let repo = git_repo("push-success", true);
    let shell = init_app(cx);
    cx.update(|cx| {
        shell.update(cx, |shell, cx| {
            insert_local_session(shell, repo, cx);
            shell.run_git_sync(1, super::GitSyncOperation::Push, cx);
        });
    });
    wait_for_toast(&shell, cx);

    let active = cx.update(|cx| shell.read(cx).workspace.toaster.active().cloned());
    let toast = active.expect("a toast must be shown after successful push");
    assert_eq!(
        toast.notice.tone,
        crate::features::workspace::toaster::ToastTone::Success
    );
    assert_eq!(toast.notice.message, i18n::text("git.push_success"));
}

#[gpui::test]
fn spec_20260817_git_sync_toast_push_failure_shows_error_toast(cx: &mut gpui::TestAppContext) {
    let repo = broken_origin_repo("push-failure");
    let shell = init_app(cx);
    cx.update(|cx| {
        shell.update(cx, |shell, cx| {
            insert_local_session(shell, repo, cx);
            shell.run_git_sync(1, super::GitSyncOperation::Push, cx);
        });
    });
    wait_for_toast(&shell, cx);

    let (active, error_kept) = cx.update(|cx| {
        let shell = shell.read(cx);
        (
            shell.workspace.toaster.active().cloned(),
            shell.git_sync.get(&1).and_then(|state| state.error.clone()),
        )
    });
    let toast = active.expect("a toast must be shown after failed push");
    assert_eq!(
        toast.notice.tone,
        crate::features::workspace::toaster::ToastTone::Error
    );
    assert_eq!(toast.notice.message, i18n::text("git.push_failed"));
    assert!(
        error_kept.is_some(),
        "failure detail must stay on the button (error state preserved)"
    );
}

#[gpui::test]
fn spec_20260817_git_sync_toast_pull_success_shows_success_toast(cx: &mut gpui::TestAppContext) {
    let repo = git_repo("pull-success", true);
    let shell = init_app(cx);
    cx.update(|cx| {
        shell.update(cx, |shell, cx| {
            insert_local_session(shell, repo, cx);
            shell.run_git_sync(1, super::GitSyncOperation::Pull, cx);
        });
    });
    wait_for_toast(&shell, cx);

    let active = cx.update(|cx| shell.read(cx).workspace.toaster.active().cloned());
    let toast = active.expect("a toast must be shown after successful pull");
    assert_eq!(
        toast.notice.tone,
        crate::features::workspace::toaster::ToastTone::Success
    );
    assert_eq!(toast.notice.message, i18n::text("git.pull_success"));
}

#[gpui::test]
fn spec_20260817_git_sync_toast_running_guard_keeps_first_operation_toast(
    cx: &mut gpui::TestAppContext,
) {
    let repo = git_repo("running-guard", true);
    let shell = init_app(cx);
    cx.update(|cx| {
        shell.update(cx, |shell, cx| {
            insert_local_session(shell, repo, cx);
            // Push 仍在 running 时立刻点击 Pull:守卫必须忽略第二次触发,
            // 最终 toast 只反映第一次操作(Push)的结果。
            shell.run_git_sync(1, super::GitSyncOperation::Push, cx);
            shell.run_git_sync(1, super::GitSyncOperation::Pull, cx);
        });
    });
    wait_for_toast(&shell, cx);

    let active = cx.update(|cx| shell.read(cx).workspace.toaster.active().cloned());
    let toast = active.expect("a toast must be shown");
    assert_eq!(
        toast.notice.tone,
        crate::features::workspace::toaster::ToastTone::Success
    );
    assert_eq!(toast.notice.message, i18n::text("git.push_success"));
}
