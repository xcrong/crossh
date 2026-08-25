//! 侧栏项目一键关闭契约测试 (spec 20260821-sidebar-close-project)。
#![allow(non_snake_case)]

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

use gpui::Entity;

use crate::features::settings::persistence::set_test_settings_path;
use crate::features::terminal::view::TerminalView;
use crate::features::workspace::settings::PinnedLocalTab;
use crate::features::workspace::view::{LocalSession, LocalSessionId};

use super::AppShell;
use crate::features::workspace::view::ActiveView;
use crossh_ui::context_menu::ShellMenuAction;
use crossh_ui_component::context_menu::MenuEntry;
static NEXT_SETTINGS_DIR: AtomicUsize = AtomicUsize::new(500);

/// 测试专用的同步批量关闭：与 `stop_local_project` 相同的快照+detach 语义，
/// 但对有风险的会话视为用户取消而保留（契约 5 的同步可测分支）。
fn stop_sync(shell: &mut AppShell, project_dir: PathBuf, cx: &mut gpui::Context<AppShell>) {
    let ids: Vec<LocalSessionId> = match shell.workspace.sessions.local_dirs.get(&project_dir) {
        Some(dir) => dir.sessions.clone(),
        None => return,
    };
    if ids.is_empty() {
        return;
    }
    let views: Vec<ActiveView> = ids.iter().copied().map(ActiveView::LocalSession).collect();
    shell.detach_splits_for(&views, cx);
    for session_id in ids {
        if !shell
            .workspace
            .sessions
            .local_sessions
            .contains_key(&session_id)
        {
            continue;
        }
        let Some(risk) = shell.local_session_close_risk(session_id, cx) else {
            continue;
        };
        if risk.needs_confirmation() {
            continue;
        }
        shell.close_local_session_internal(session_id, true, cx);
    }
}

fn init_app(cx: &mut gpui::TestAppContext) -> Entity<AppShell> {
    let index = NEXT_SETTINGS_DIR.fetch_add(1, Ordering::Relaxed);
    let settings_dir = std::env::temp_dir().join(format!(
        "crossh-close-project-tests-{}-{index}",
        std::process::id()
    ));
    init_app_with_settings(cx, settings_dir)
}

fn init_app_with_settings(
    cx: &mut gpui::TestAppContext,
    settings_dir: PathBuf,
) -> Entity<AppShell> {
    std::fs::create_dir_all(&settings_dir).expect("test settings dir should be created");
    set_test_settings_path(Some(settings_dir.join("settings.toml")));
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

fn insert_local_session(
    shell: &mut AppShell,
    id: LocalSessionId,
    cwd: PathBuf,
    cx: &mut gpui::Context<AppShell>,
) {
    let terminal = silent_terminal(shell, cx);
    let cwd = cwd.canonicalize().expect("test cwd should exist");
    shell.workspace.sessions.local_sessions.insert(
        id,
        LocalSession {
            project_dir: cwd.clone(),
            cwd: cwd.clone(),
            terminal,
            git_status: None,
            git_refresh: Default::default(),
            pin_id: None,
            custom_name: None,
            default_command: None,
        },
    );
    shell.sync_local_dirs(cx);
}

fn test_dir(tag: &str) -> PathBuf {
    let dir =
        std::env::temp_dir().join(format!("crossh-close-project-{tag}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("test directory should be created");
    dir.canonicalize()
        .expect("test directory should canonicalize")
}

fn pinned_records(shell: &Entity<AppShell>, cx: &mut gpui::TestAppContext) -> Vec<PinnedLocalTab> {
    cx.update(|cx| shell.read(cx).workspace_settings.pinned_local_tabs.clone())
}

fn recent_dirs(shell: &Entity<AppShell>, cx: &mut gpui::TestAppContext) -> Vec<PathBuf> {
    cx.update(|cx| shell.read(cx).workspace_settings.recent_dirs.clone())
}

// 契约 1a/1b：渲染分支纯函数
#[test]
fn spec_20260821_sidebar_close_project__render_branch_count_gt0_shows_stop_hides_forget() {
    use crate::features::workspace::sidebar::{
        local_dir_forget_button_visible, local_dir_stop_button_visible,
    };
    assert!(
        local_dir_stop_button_visible(1),
        "count>0 时应显示停止按钮 (1a)"
    );
    assert!(
        local_dir_stop_button_visible(5),
        "count>0 时应显示停止按钮 (N)"
    );
    assert!(
        !local_dir_forget_button_visible(1),
        "count>0 时不应显示忘记按钮 (1a)"
    );
    assert!(
        !local_dir_forget_button_visible(5),
        "count>0 时不应显示忘记按钮"
    );
}

#[test]
fn spec_20260821_sidebar_close_project__render_branch_count_zero_shows_forget_hides_stop() {
    use crate::features::workspace::sidebar::{
        local_dir_forget_button_visible, local_dir_stop_button_visible,
    };
    assert!(
        !local_dir_stop_button_visible(0),
        "count==0 时不应显示停止按钮 (1b)"
    );
    assert!(
        local_dir_forget_button_visible(0),
        "count==0 时应显示忘记按钮 (1b)"
    );
}

// 契约 6：右键菜单项存在性
#[test]
fn spec_20260821_sidebar_close_project__context_menu_active_has_stop() {
    use crate::features::workspace::sidebar::build_local_dir_context_menu_entries;
    let dir = PathBuf::from("/tmp/project-a");
    let entries = build_local_dir_context_menu_entries(dir.clone(), 2);
    // 应为 OpenTerminal, Reveal, Separator, Stop (共4)
    assert_eq!(entries.len(), 4, "活跃项目菜单应为 4 项");
    assert!(matches!(&entries[0], MenuEntry::Item(item) if item.id == "open-terminal"));
    assert!(matches!(&entries[1], MenuEntry::Item(item) if item.id == "reveal-finder"));
    assert!(matches!(entries[2], MenuEntry::Separator));
    assert!(
        matches!(&entries[3], MenuEntry::Item(item) if item.id == "stop-project" && matches!(item.action, ShellMenuAction::StopLocalProject(ref p) if p == &dir))
    );
    // 确保包含标签文本
    if let MenuEntry::Item(item) = &entries[3] {
        assert_eq!(
            item.label,
            crate::shared::i18n::text("context_menu.stop_project")
        );
    }
}

#[test]
fn spec_20260821_sidebar_close_project__context_menu_empty_has_forget_not_stop() {
    use crate::features::workspace::sidebar::build_local_dir_context_menu_entries;
    let dir = PathBuf::from("/tmp/project-empty");
    let entries = build_local_dir_context_menu_entries(dir.clone(), 0);
    assert_eq!(entries.len(), 4, "空记录菜单应为 4 项");
    assert!(
        matches!(&entries[3], MenuEntry::Item(item) if item.id == "forget-dir" && matches!(item.action, ShellMenuAction::ForgetLocalDir(ref p) if p == &dir))
    );
    assert!(
        !entries
            .iter()
            .any(|e| matches!(e, MenuEntry::Item(item) if item.id == "stop-project")),
        "空记录不应出现 stop-project"
    );
}

// 契约 2：批量关闭保留 recent_dirs/pinned，local_sessions 清零
#[gpui::test]
fn spec_20260821_sidebar_close_project__batch_close_retains_recent_and_pinned(
    cx: &mut gpui::TestAppContext,
) {
    let dir_a = test_dir("batch-a");
    let dir_b = test_dir("batch-b");
    let shell = init_app(cx);
    cx.update(|cx| {
        shell.update(cx, |shell, cx| {
            // 准备 recent
            shell.workspace_settings.recent_dirs = vec![dir_a.clone(), dir_b.clone()];
            // A 项目 2 会话，其中一个固定
            insert_local_session(shell, 1, dir_a.clone(), cx);
            insert_local_session(shell, 2, dir_a.clone(), cx);
            insert_local_session(shell, 3, dir_b.clone(), cx);
            shell.pin_local_session(1, cx);
            // 确保 pin 成功
            assert_eq!(shell.workspace_settings.pinned_local_tabs.len(), 1);
        });
    });
    // 执行停止项目 A（同步无窗口版本，模拟无风险全部关闭）
    cx.update(|cx| {
        shell.update(cx, |shell, cx| {
            stop_sync(shell, dir_a.clone(), cx);
        });
    });
    cx.update(|cx| {
        let shell = shell.read(cx);
        // A 的会话应清零
        assert!(!shell.workspace.sessions.local_sessions.contains_key(&1));
        assert!(!shell.workspace.sessions.local_sessions.contains_key(&2));
        // B 的会话保留
        assert!(shell.workspace.sessions.local_sessions.contains_key(&3));
        // local_dirs 中 A 应为空列表但仍保留（因为在 recent 中）
        let dir_a_entry = shell
            .workspace
            .sessions
            .local_dirs
            .get(&dir_a)
            .expect("A dir should remain");
        assert!(dir_a_entry.sessions.is_empty(), "A 的 sessions 应为空");
        let dir_b_entry = shell
            .workspace
            .sessions
            .local_dirs
            .get(&dir_b)
            .expect("B dir");
        assert_eq!(dir_b_entry.sessions, vec![3]);
        // recent 保留 A
        assert!(
            shell.workspace_settings.recent_dirs.contains(&dir_a),
            "recent 应保留 A"
        );
        // pinned 保留（契约 2：不被 retain 清除）
        assert_eq!(
            shell.workspace_settings.pinned_local_tabs.len(),
            1,
            "pinned 应保留"
        );
        assert_eq!(shell.workspace_settings.pinned_local_tabs[0].pin_id, 1);
    });
}

// 契约 3：active_view 回退
#[gpui::test]
fn spec_20260821_sidebar_close_project__active_view_fallback(cx: &mut gpui::TestAppContext) {
    let dir_a = test_dir("active-fallback-a");
    let dir_b = test_dir("active-fallback-b");
    let shell = init_app(cx);
    cx.update(|cx| {
        shell.update(cx, |shell, cx| {
            shell.workspace_settings.recent_dirs = vec![dir_a.clone(), dir_b.clone()];
            insert_local_session(shell, 10, dir_a.clone(), cx);
            insert_local_session(shell, 11, dir_a.clone(), cx);
            insert_local_session(shell, 20, dir_b.clone(), cx);
            // 设活动为 A 的第一个会话
            shell.workspace.active_view = Some(
                crate::features::workspace::view::ActiveView::LocalSession(10),
            );
            shell
                .workspace
                .sessions
                .local_dirs
                .get_mut(&dir_a)
                .unwrap()
                .active_session = Some(10);
            shell
                .workspace
                .sessions
                .local_dirs
                .get_mut(&dir_b)
                .unwrap()
                .active_session = Some(20);
        });
    });
    cx.update(|cx| {
        shell.update(cx, |shell, cx| {
            stop_sync(shell, dir_a.clone(), cx);
        });
    });
    cx.update(|cx| {
        let shell = shell.read(cx);
        // 关闭导致原活跃视图属于被关闭项目，应回退到 B 的会话
        assert_eq!(
            shell.workspace.active_view,
            Some(crate::features::workspace::view::ActiveView::LocalSession(
                20
            )),
            "active_view 应回退到 B 的会话 (dir.active_session → first_local_view)"
        );
    });
}

// 契约 3 变体：回退到远程标签当无本地剩余
#[gpui::test]
fn spec_20260821_sidebar_close_project__active_view_fallback_to_remote_when_no_local(
    cx: &mut gpui::TestAppContext,
) {
    let dir_a = test_dir("active-fallback-remote-a");
    let shell = init_app(cx);
    // 先构造一个 remote tab（静默终端）
    cx.update(|cx| {
        shell.update(cx, |shell, cx| {
            shell.workspace_settings.recent_dirs = vec![dir_a.clone()];
            insert_local_session(shell, 30, dir_a.clone(), cx);
            shell.workspace.active_view = Some(
                crate::features::workspace::view::ActiveView::LocalSession(30),
            );
            // 创建一个 remote tab
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
            shell
                .workspace
                .sessions
                .remote_tabs
                .push(crate::features::workspace::view::Tab {
                    target: "test-host".to_string(),
                    host_key: "test-host".to_string(),
                    connection: None,
                    pane: crate::features::terminal::view::workspace_pane(terminal),
                });
        });
    });
    cx.update(|cx| {
        shell.update(cx, |shell, cx| {
            stop_sync(shell, dir_a.clone(), cx);
        });
    });
    cx.update(|cx| {
        let shell = shell.read(cx);
        assert_eq!(
            shell.workspace.active_view,
            Some(crate::features::workspace::view::ActiveView::RemoteTab(0)),
            "无剩余本地时应回退到远程标签"
        );
    });
    // 清理 remote
    cx.update(|cx| {
        shell.update(cx, |shell, cx| {
            shell.close_remote_tab(0, cx);
        });
    });
}

// 契约 4：activate 后 pinned 恢复
#[gpui::test]
fn spec_20260821_sidebar_close_project__activate_restores_pinned(cx: &mut gpui::TestAppContext) {
    let dir = test_dir("restore-pinned");
    let shell = init_app(cx);
    cx.update(|cx| {
        shell.update(cx, |shell, cx| {
            shell.workspace_settings.recent_dirs = vec![dir.clone()];
            insert_local_session(shell, 40, dir.clone(), cx);
            insert_local_session(shell, 41, dir.clone(), cx);
            shell.pin_local_session(40, cx);
            shell.pin_local_session(41, cx);
            // 固定后，custom_name / default_command 可选，这里保持默认
            assert_eq!(shell.workspace_settings.pinned_local_tabs.len(), 2);
        });
    });
    // 停止项目（保留 pinned）
    cx.update(|cx| {
        shell.update(cx, |shell, cx| {
            stop_sync(shell, dir.clone(), cx);
        });
    });
    cx.update(|cx| {
        let shell = shell.read(cx);
        assert_eq!(
            shell.workspace.sessions.local_sessions.len(),
            0,
            "停止后会话清零"
        );
        assert_eq!(
            shell.workspace_settings.pinned_local_tabs.len(),
            2,
            "pinned 保留"
        );
        assert!(
            shell
                .workspace
                .sessions
                .local_dirs
                .get(&dir)
                .unwrap()
                .sessions
                .is_empty()
        );
    });
    // 再次激活项目，应重建 pinned 对应的会话
    cx.update(|cx| {
        shell.update(cx, |shell, cx| {
            shell.activate_local_dir(dir.clone(), cx);
        });
    });
    cx.update(|cx| {
        let shell = shell.read(cx);
        let count = shell
            .workspace
            .sessions
            .local_dirs
            .get(&dir)
            .unwrap()
            .sessions
            .len();
        assert_eq!(count, 2, "激活后应恢复 2 个 pinned 会话");
        // 会话应标记为 pinned
        let pinned_count = shell
            .workspace
            .sessions
            .local_sessions
            .values()
            .filter(|s| s.pin_id.is_some())
            .count();
        assert_eq!(pinned_count, 2);
    });
    // 清理
    cx.update(|cx| {
        shell.update(cx, |shell, cx| {
            let ids: Vec<_> = shell
                .workspace
                .sessions
                .local_sessions
                .keys()
                .copied()
                .collect();
            for id in ids {
                shell.close_local_session(id, cx);
            }
        });
    });
}

// 契约 4 变体：无 pinned 时恢复单个普通会话
#[gpui::test]
fn spec_20260821_sidebar_close_project__activate_restores_single_when_no_pinned(
    cx: &mut gpui::TestAppContext,
) {
    let dir = test_dir("restore-single");
    let shell = init_app(cx);
    cx.update(|cx| {
        shell.update(cx, |shell, cx| {
            shell.workspace_settings.recent_dirs = vec![dir.clone()];
            insert_local_session(shell, 50, dir.clone(), cx);
            // 不固定
            assert!(shell.workspace_settings.pinned_local_tabs.is_empty());
        });
    });
    cx.update(|cx| {
        shell.update(cx, |shell, cx| {
            stop_sync(shell, dir.clone(), cx);
        });
    });
    cx.update(|cx| {
        assert!(shell.read(cx).workspace.sessions.local_sessions.is_empty());
    });
    cx.update(|cx| {
        shell.update(cx, |shell, cx| {
            shell.activate_local_dir(dir.clone(), cx);
        });
    });
    cx.update(|cx| {
        let shell = shell.read(cx);
        let count = shell
            .workspace
            .sessions
            .local_dirs
            .get(&dir)
            .unwrap()
            .sessions
            .len();
        assert_eq!(count, 1, "无 pinned 时应创建单个普通会话");
        let session = shell
            .workspace
            .sessions
            .local_sessions
            .values()
            .next()
            .unwrap();
        assert!(session.pin_id.is_none(), "普通会话非固定");
    });
    cx.update(|cx| {
        shell.update(cx, |shell, cx| {
            let ids: Vec<_> = shell
                .workspace
                .sessions
                .local_sessions
                .keys()
                .copied()
                .collect();
            for id in ids {
                shell.close_local_session(id, cx);
            }
        });
    });
}

// 契约 5：风险确认 — 无风险全部关闭
#[gpui::test]
fn spec_20260821_sidebar_close_project__risk_no_risk_closes_all(cx: &mut gpui::TestAppContext) {
    let dir = test_dir("risk-no-risk");
    let shell = init_app(cx);
    cx.update(|cx| {
        shell.update(cx, |shell, cx| {
            shell.workspace_settings.recent_dirs = vec![dir.clone()];
            insert_local_session(shell, 60, dir.clone(), cx);
            insert_local_session(shell, 61, dir.clone(), cx);
            // 确保无 risky 标记
            shell.test_risky_sessions.clear();
        });
    });
    cx.update(|cx| {
        shell.update(cx, |shell, cx| {
            stop_sync(shell, dir.clone(), cx);
        });
    });
    cx.update(|cx| {
        assert!(
            shell.read(cx).workspace.sessions.local_sessions.is_empty(),
            "无风险时应全部关闭"
        );
    });
}

// 契约 5：有风险取消保留
#[gpui::test]
fn spec_20260821_sidebar_close_project__risk_with_risk_cancel_retains(
    cx: &mut gpui::TestAppContext,
) {
    let dir = test_dir("risk-cancel");
    let shell = init_app(cx);
    cx.update(|cx| {
        shell.update(cx, |shell, cx| {
            shell.workspace_settings.recent_dirs = vec![dir.clone()];
            insert_local_session(shell, 70, dir.clone(), cx);
            insert_local_session(shell, 71, dir.clone(), cx);
            insert_local_session(shell, 72, dir.clone(), cx);
            shell.pin_local_session(70, cx);
            shell.pin_local_session(71, cx);
            // 标记 71 为有风险（模拟 is_command_running）
            shell.test_risky_sessions.insert(71);
            // 71 风险，70、72 无风险
        });
    });
    let pinned_before = pinned_records(&shell, cx);
    let recent_before = recent_dirs(&shell, cx);
    cx.update(|cx| {
        shell.update(cx, |shell, cx| {
            stop_sync(shell, dir.clone(), cx);
        });
    });
    cx.update(|cx| {
        let shell = shell.read(cx);
        // 71 应保留，其余关闭
        assert!(
            shell.workspace.sessions.local_sessions.contains_key(&71),
            "有风险的会话应保留（用户取消）"
        );
        assert!(
            !shell.workspace.sessions.local_sessions.contains_key(&70),
            "无风险会话应关闭"
        );
        assert!(
            !shell.workspace.sessions.local_sessions.contains_key(&72),
            "无风险会话应关闭"
        );
        // local_dirs 应仅剩 71
        let dir_entry = shell.workspace.sessions.local_dirs.get(&dir).unwrap();
        assert_eq!(dir_entry.sessions, vec![71]);
        // recent/pinned 不变
        assert_eq!(shell.workspace_settings.recent_dirs, recent_before);
        assert_eq!(
            shell.workspace_settings.pinned_local_tabs.len(),
            pinned_before.len(),
            "pinned 保留"
        );
        // 验证剩余会话的 pin_id 仍对应一条 pinned 记录
        let remaining_pin = shell
            .workspace
            .sessions
            .local_sessions
            .get(&71)
            .and_then(|s| s.pin_id);
        assert!(remaining_pin.is_some(), "剩余会话应仍为 pinned");
        assert!(
            shell
                .workspace_settings
                .pinned_local_tabs
                .iter()
                .any(|t| Some(t.pin_id) == remaining_pin),
            "pinned 记录与剩余会话对应"
        );
    });
    cx.update(|cx| {
        shell.update(cx, |shell, cx| {
            shell.test_risky_sessions.clear();
            let ids: Vec<_> = shell
                .workspace
                .sessions
                .local_sessions
                .keys()
                .copied()
                .collect();
            for id in ids {
                shell.close_local_session(id, cx);
            }
        });
    });
}

// 契约 5 边界：不存在目录或已 forget 为 no-op
#[gpui::test]
fn spec_20260821_sidebar_close_project__stop_nonexistent_is_noop(cx: &mut gpui::TestAppContext) {
    let shell = init_app(cx);
    let missing = PathBuf::from("/tmp/crossh-missing-12345-nonexistent");
    cx.update(|cx| {
        shell.update(cx, |shell, cx| {
            // 不应 panic
            stop_sync(shell, missing.clone(), cx);
        });
    });
    cx.update(|cx| {
        assert!(shell.read(cx).workspace.sessions.local_sessions.is_empty());
    });
}

#[gpui::test]
fn spec_20260821_sidebar_close_project__stop_idempotent_double_click(
    cx: &mut gpui::TestAppContext,
) {
    let dir = test_dir("idempotent");
    let shell = init_app(cx);
    cx.update(|cx| {
        shell.update(cx, |shell, cx| {
            shell.workspace_settings.recent_dirs = vec![dir.clone()];
            insert_local_session(shell, 80, dir.clone(), cx);
            insert_local_session(shell, 81, dir.clone(), cx);
        });
    });
    cx.update(|cx| {
        shell.update(cx, |shell, cx| {
            stop_sync(shell, dir.clone(), cx);
            // 快速连续第二次点击应为 no-op，不 panic
            stop_sync(shell, dir.clone(), cx);
        });
    });
    cx.update(|cx| {
        assert!(shell.read(cx).workspace.sessions.local_sessions.is_empty());
        // 第二次后目录仍保留（recent）
        assert!(
            shell
                .read(cx)
                .workspace
                .sessions
                .local_dirs
                .contains_key(&dir)
        );
    });
}

// 契约 2 补充：close_local_session_internal keep_pinned 语义
#[gpui::test]
fn spec_20260821_sidebar_close_project__close_internal_keep_pinned_vs_discard(
    cx: &mut gpui::TestAppContext,
) {
    let dir = test_dir("keep-pinned-internal");
    let shell = init_app(cx);
    cx.update(|cx| {
        shell.update(cx, |shell, cx| {
            shell.workspace_settings.recent_dirs = vec![dir.clone()];
            insert_local_session(shell, 90, dir.clone(), cx);
            shell.pin_local_session(90, cx);
            assert_eq!(shell.workspace_settings.pinned_local_tabs.len(), 1);
            // keep_pinned = false 应清除 pinned
            shell.close_local_session_internal(90, false, cx);
        });
    });
    assert!(
        pinned_records(&shell, cx).is_empty(),
        "keep_pinned=false 应清除 pinned"
    );

    // 重建并测试 keep_pinned=true
    cx.update(|cx| {
        shell.update(cx, |shell, cx| {
            insert_local_session(shell, 91, dir.clone(), cx);
            shell.pin_local_session(91, cx);
            assert_eq!(shell.workspace_settings.pinned_local_tabs.len(), 1);
            shell.close_local_session_internal(91, true, cx);
        });
    });
    assert_eq!(
        pinned_records(&shell, cx).len(),
        1,
        "keep_pinned=true 应保留 pinned"
    );
    cx.update(|cx| {
        shell.update(cx, |shell, _cx| {
            // 清理残留 pinned
            shell.workspace_settings.pinned_local_tabs.clear();
        });
    });
}
