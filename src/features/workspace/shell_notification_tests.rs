//! 通知点击与终端分栏交互的契约测试。
//!
//! 注意:这些测试从不推进 foreground 任务,终端的 PTY builder 因此从未
//! 完成(终端保持 display-only),不会 spawn alacritty "PTY reader" 线程。
//! 若真实 spawn 一个有输出的 PTY(例如 ssh 解析失败的错误输出),PTY
//! reader 线程在唤醒 foreground executor 时会触发 test_scheduler 的
//! 确定性守卫(跨线程调度),测试会变成不确定。remote 测试因此不用
//! `open_terminal_target`(它会 spawn `ssh test-host`),而是直接构造
//! remote 语义的静默终端(见 `push_silent_remote_terminal`)。

use super::AppShell;

/// 构造一个 remote 语义(is_remote_terminal = true)但静默的终端 tab,
/// 与 `create_terminal_target` 的结构一致,只是把 ssh 换成了 sleep:
/// ssh 到不存在的 host 会立即失败并向 PTY 写错误输出,导致 PTY reader
/// 线程在测试窗口内唤醒 foreground executor,触发 test_scheduler 守卫。
fn push_silent_remote_terminal(
    shell: &mut AppShell,
    cx: &mut gpui::Context<AppShell>,
) -> crate::features::workspace::view::ActiveView {
    use crate::features::terminal::view::TerminalView;
    use crate::features::workspace::view::{ActiveView, Tab};

    let terminal = TerminalView::from_zed_shell(
        None,
        Some("~".to_string()),
        task::Shell::WithArguments {
            program: "sleep".into(),
            args: vec!["3600".into()],
            title_override: None,
        },
        true,
        shell.terminal_settings.clone(),
        cx,
    );
    let idx = shell.workspace.sessions.remote_tabs.len();
    shell.workspace.sessions.remote_tabs.push(Tab {
        target: "test-host".to_string(),
        host_key: "test-host".to_string(),
        connection: None,
        pane: crate::features::terminal::view::workspace_pane(terminal),
    });
    ActiveView::RemoteTab(idx)
}

fn cleanup_local_sessions(shell: &gpui::Entity<AppShell>, cx: &mut gpui::TestAppContext) {
    let ids: Vec<_> = cx.update(|cx| {
        shell
            .read(cx)
            .workspace
            .sessions
            .local_sessions
            .keys()
            .copied()
            .collect()
    });
    for id in ids {
        cx.update(|cx| shell.update(cx, |shell, cx| shell.close_local_session(id, cx)));
    }
}

#[gpui::test]
fn spec_notification_response_from_split_right_pane_keeps_the_split(cx: &mut gpui::TestAppContext) {
    use gpui::{Entity, SharedString, SystemNotificationResponse};

    use crate::features::workspace::registry::SplitSide;
    use crate::features::workspace::view::ActiveView;

    let dir = std::env::temp_dir().join(format!(
        "crossh-notification-split-test-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let dir = dir.canonicalize().unwrap();

    let shell: Entity<AppShell> = cx.update(|cx| {
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
    });

    let (left_view, right_view) = cx.update(|cx| {
        shell.update(cx, |shell, cx| {
            let _ = shell.open_local_session(dir.clone(), dir.clone(), cx);
            let left = shell.workspace.active_view.expect("left pane is active");
            let right = shell
                .open_local_session_for_split(dir.clone(), dir.clone(), cx)
                .expect("split pane should be created");
            assert!(shell.workspace.begin_terminal_split(right));
            shell.workspace.focus_terminal_split(SplitSide::Right);
            (left, right)
        })
    });

    let tag: SharedString = cx.update(|cx| {
        let y = shell
            .read(cx)
            .workspace
            .sessions
            .local_sessions
            .get(&match right_view {
                ActiveView::LocalSession(id) => id,
                _ => unreachable!(),
            })
            .expect("right pane session exists")
            .terminal
            .entity_id();
        format!("crossh-terminal-{y}-bell-0").into()
    });

    cx.simulate_system_notification_response(SystemNotificationResponse {
        tag,
        action_id: None,
    });

    cx.update(|cx| {
        let shell = shell.read(cx);
        assert_eq!(shell.workspace.active_view, Some(left_view));
        let split = shell
            .workspace
            .terminal_splits
            .get(&left_view)
            .copied()
            .expect("split must be preserved");
        assert_eq!(split.left, left_view);
        assert_eq!(split.right, right_view);
        assert_eq!(split.focused, SplitSide::Right);
    });

    cleanup_local_sessions(&shell, cx);

    std::fs::remove_dir_all(dir).ok();
}

#[gpui::test]
fn spec_notification_response_from_remote_split_right_pane_keeps_the_split(
    cx: &mut gpui::TestAppContext,
) {
    use gpui::{Entity, SharedString, SystemNotificationResponse};

    use crate::features::workspace::registry::SplitSide;
    use crate::features::workspace::view::ActiveView;

    let shell: Entity<AppShell> = cx.update(|cx| {
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
    });

    // 不通过 open_terminal_target 创建 remote 终端:它 spawn `ssh -tt
    // test-host ...`,解析不存在的 host 立即失败并向 PTY 写错误输出,
    // PTY reader 线程读到输出会唤醒 foreground executor,与
    // test_scheduler 的确定性守卫形成窗口竞争(时稳时不稳)。
    // 这里直接构造 remote 语义(is_remote_terminal = true)的静默终端。
    let (left_view, right_view) = cx.update(|cx| {
        shell.update(cx, |shell, cx| {
            let left = push_silent_remote_terminal(shell, cx);
            shell.workspace.active_view = Some(left);
            let right = push_silent_remote_terminal(shell, cx);
            assert!(shell.workspace.begin_terminal_split(right));
            shell.workspace.focus_terminal_split(SplitSide::Right);
            (left, right)
        })
    });

    let tag: SharedString = cx.update(|cx| {
        let y = shell
            .read(cx)
            .workspace
            .sessions
            .remote_tabs
            .get(match right_view {
                ActiveView::RemoteTab(index) => index,
                _ => unreachable!(),
            })
            .expect("right remote pane exists")
            .pane
            .terminal_entity_id()
            .expect("remote pane is a terminal");
        format!("crossh-terminal-{y}-bell-0").into()
    });

    cx.simulate_system_notification_response(SystemNotificationResponse {
        tag,
        action_id: None,
    });

    cx.update(|cx| {
        let shell = shell.read(cx);
        assert_eq!(shell.workspace.active_view, Some(left_view));
        let split = shell
            .workspace
            .terminal_splits
            .get(&left_view)
            .copied()
            .expect("split must be preserved");
        assert_eq!(split.left, left_view);
        assert_eq!(split.right, right_view);
        assert_eq!(split.focused, SplitSide::Right);
    });

    // 关闭 remote tabs：与 local 测试的 cleanup_local_sessions 对称。
    // 不清理会留下存活的 PTY；Windows(conpty) 下 PTY reader 线程在
    // 测试窗口内活动会触发 test_scheduler 的非确定性守卫（时稳时不稳）。
    cx.update(|cx| {
        shell.update(cx, |shell, cx| {
            while !shell.workspace.sessions.remote_tabs.is_empty() {
                shell.close_remote_tab(0, cx);
            }
        })
    });
}

#[gpui::test]
fn spec_notification_click_in_rendered_window_keeps_the_split(cx: &mut gpui::TestAppContext) {
    use gpui::{SharedString, SystemNotificationResponse};

    use crate::features::workspace::registry::SplitSide;
    use crate::features::workspace::view::ActiveView;

    let dir = std::env::temp_dir().join(format!(
        "crossh-notification-window-test-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let dir = dir.canonicalize().unwrap();

    let (shell, left_view, right_view) = cx.update(|cx| {
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
        let shell = window
            .update(cx, |_, window, _| window.root::<AppShell>())
            .expect("main window update")
            .expect("main window root")
            .expect("main window root is AppShell");
        let (left_view, right_view) = shell.update(cx, |shell, cx| {
            let _ = shell.open_local_session(dir.clone(), dir.clone(), cx);
            let left = shell.workspace.active_view.expect("left pane is active");
            let right = shell
                .open_local_session_for_split(dir.clone(), dir.clone(), cx)
                .expect("split pane should be created");
            assert!(shell.workspace.begin_terminal_split(right));
            shell.workspace.focus_terminal_split(SplitSide::Right);
            (left, right)
        });
        (shell, left_view, right_view)
    });

    let tag: SharedString = cx.update(|cx| {
        let y = shell
            .read(cx)
            .workspace
            .sessions
            .local_sessions
            .get(&match right_view {
                ActiveView::LocalSession(id) => id,
                _ => unreachable!(),
            })
            .expect("right pane session exists")
            .terminal
            .entity_id();
        format!("crossh-terminal-{y}-bell-0").into()
    });

    cx.simulate_system_notification_response(SystemNotificationResponse {
        tag,
        action_id: None,
    });

    cx.update(|cx| {
        let shell = shell.read(cx);
        assert_eq!(shell.workspace.active_view, Some(left_view));
        let split = shell
            .workspace
            .terminal_splits
            .get(&left_view)
            .copied()
            .expect("split must be preserved");
        assert_eq!(split.left, left_view);
        assert_eq!(split.right, right_view);
        assert_eq!(split.focused, SplitSide::Right);
    });

    cleanup_local_sessions(&shell, cx);

    std::fs::remove_dir_all(dir).ok();
}

#[gpui::test]
fn spec_notification_from_split_right_pane_while_other_tab_active_returns_to_split_owner(
    cx: &mut gpui::TestAppContext,
) {
    use gpui::{Entity, SharedString, SystemNotificationResponse};

    use crate::features::workspace::registry::SplitSide;
    use crate::features::workspace::view::ActiveView;

    let dir_a =
        std::env::temp_dir().join(format!("crossh-notif-split-owner-a-{}", std::process::id()));
    let dir_b =
        std::env::temp_dir().join(format!("crossh-notif-split-owner-b-{}", std::process::id()));
    std::fs::create_dir_all(&dir_a).unwrap();
    std::fs::create_dir_all(&dir_b).unwrap();
    let dir_a = dir_a.canonicalize().unwrap();
    let dir_b = dir_b.canonicalize().unwrap();

    let shell: Entity<AppShell> = cx.update(|cx| {
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
    });

    let (left_view, right_view) = cx.update(|cx| {
        shell.update(cx, |shell, cx| {
            let _ = shell.open_local_session(dir_a.clone(), dir_a.clone(), cx);
            let left = shell.workspace.active_view.expect("left pane is active");
            let right = shell
                .open_local_session_for_split(dir_a.clone(), dir_a.clone(), cx)
                .expect("split pane should be created");
            assert!(shell.workspace.begin_terminal_split(right));
            shell.workspace.focus_terminal_split(SplitSide::Right);
            (left, right)
        })
    });

    let other_tab = cx.update(|cx| {
        shell.update(cx, |shell, cx| {
            let _ = shell.open_local_session(dir_b.clone(), dir_b.clone(), cx);
            let other = shell.workspace.active_view.expect("other tab is active");
            assert_ne!(other, left_view);
            other
        })
    });

    let tag: SharedString = cx.update(|cx| {
        let y = shell
            .read(cx)
            .workspace
            .sessions
            .local_sessions
            .get(&match right_view {
                ActiveView::LocalSession(id) => id,
                _ => unreachable!(),
            })
            .expect("right pane session exists")
            .terminal
            .entity_id();
        format!("crossh-terminal-{y}-bell-0").into()
    });

    cx.simulate_system_notification_response(SystemNotificationResponse {
        tag,
        action_id: None,
    });

    cx.update(|cx| {
        let shell = shell.read(cx);
        // 期望:通知来自分栏右窗格时,即便当前停在别的 Tab,
        // 也切回分栏属主 Tab 恢复分栏,并把焦点放进发出通知的窗格。
        assert_eq!(shell.workspace.active_view, Some(left_view));
        let split = shell
            .workspace
            .terminal_splits
            .get(&left_view)
            .copied()
            .expect("split must be preserved");
        assert_eq!(split.left, left_view);
        assert_eq!(split.right, right_view);
        assert_eq!(split.focused, SplitSide::Right);
        let _ = other_tab;
    });

    cleanup_local_sessions(&shell, cx);

    std::fs::remove_dir_all(dir_a).ok();
    std::fs::remove_dir_all(dir_b).ok();
}
