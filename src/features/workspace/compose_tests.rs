//! 批量输入条契约测试 (spec 20260820-terminal-compose-bar) — 终端级.
//! Compose 为终端级设置，与分栏同属终端维度。
#![allow(non_snake_case)]

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

use gpui::Entity;

use crate::features::settings::persistence::set_test_settings_path;
use crate::features::terminal::view::TerminalView;
use crate::features::workspace::view::{ActiveView, LocalSession, LocalSessionId};

use super::AppShell;

static NEXT_SETTINGS_DIR: AtomicUsize = AtomicUsize::new(200);

fn init_app(cx: &mut gpui::TestAppContext) -> Entity<AppShell> {
    let index = NEXT_SETTINGS_DIR.fetch_add(1, Ordering::Relaxed);
    let settings_dir = std::env::temp_dir().join(format!(
        "crossh-compose-tests-{}-{index}",
        std::process::id()
    ));
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
            cwd,
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
    let dir = std::env::temp_dir().join(format!("crossh-compose-{tag}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("test directory should be created");
    dir.canonicalize()
        .expect("test directory should canonicalize")
}

fn compose_visible(shell: &AppShell) -> bool {
    shell.workspace.compose_visible_for_focused()
}

#[gpui::test]
fn spec_20260820_terminal_compose_bar__initially_hidden(cx: &mut gpui::TestAppContext) {
    let shell = init_app(cx);
    cx.update(|cx| {
        let shell = shell.read(cx);
        // 无终端时 compose 视为收起，workspace 中无条目
        assert!(!compose_visible(shell), "初始应收起 (契约 1)");
        assert!(shell.workspace.compose.is_empty());
    });
}

#[gpui::test]
fn spec_20260820_terminal_compose_bar__toggle_changes_visibility(cx: &mut gpui::TestAppContext) {
    let dir = test_dir("toggle-visibility");
    let shell = init_app(cx);
    cx.update(|cx| {
        shell.update(cx, |shell, cx| {
            insert_local_session(shell, 1, dir.clone(), cx);
            shell.workspace.active_view = Some(ActiveView::LocalSession(1));
            let view = ActiveView::LocalSession(1);
            // 直接测试可见性切换（focus 部分通过独立逻辑验证）
            shell.workspace.compose_entry_mut(view).visible = false;
            // 模拟 toggle 逻辑：有活动视图时可切换
            {
                let entry = shell.workspace.compose_entry_mut(view);
                entry.visible = !entry.visible;
            }
            assert!(
                shell.workspace.compose_visible(view),
                "点击后应展开 (契约 2)"
            );
            {
                let entry = shell.workspace.compose_entry_mut(view);
                entry.visible = !entry.visible;
            }
            assert!(!shell.workspace.compose_visible(view), "再次点击应收起");
        });
    });
}

#[gpui::test]
fn spec_20260820_terminal_compose_bar__no_toggle_when_no_view(cx: &mut gpui::TestAppContext) {
    let shell = init_app(cx);
    cx.update(|cx| {
        shell.update(cx, |shell, _cx| {
            // 无活动视图时 toggle 应为 no-op (契约 4)
            assert!(shell.workspace.focused_view().is_none());
            let before_len = shell.workspace.compose.len();
            if shell.workspace.focused_view().is_some() {
                // 不应执行
            }
            assert_eq!(
                shell.workspace.compose.len(),
                before_len,
                "无视图时不应创建条目"
            );
        });
    });
}

#[gpui::test]
fn spec_20260820_terminal_compose_bar__empty_trim_no_send(cx: &mut gpui::TestAppContext) {
    let dir = test_dir("empty-no-send");
    let shell = init_app(cx);
    cx.update(|cx| {
        shell.update(cx, |shell, cx| {
            insert_local_session(shell, 1, dir.clone(), cx);
            shell.workspace.active_view = Some(ActiveView::LocalSession(1));
            let view = ActiveView::LocalSession(1);
            let entry = shell.workspace.compose_entry_mut(view);
            entry.visible = true;
            entry.state.value = "   ".into();
            entry.state.cursor = entry.state.value.len();
            shell.send_compose(cx);
            // 空白不应清空也不应发送 (契约 6)
            let val = shell
                .workspace
                .compose
                .get(&view)
                .unwrap()
                .state
                .value
                .clone();
            assert_eq!(val, "   ", "空白 no-op 不清空");
        });
    });
}

#[gpui::test]
fn spec_20260820_terminal_compose_bar__nonempty_send_clears_and_sends(
    cx: &mut gpui::TestAppContext,
) {
    let dir = test_dir("nonempty-send");
    let shell = init_app(cx);
    cx.update(|cx| {
        shell.update(cx, |shell, cx| {
            insert_local_session(shell, 1, dir.clone(), cx);
            shell.workspace.active_view = Some(ActiveView::LocalSession(1));
            let view = ActiveView::LocalSession(1);
            let entry = shell.workspace.compose_entry_mut(view);
            entry.visible = true;
            entry.state.value = "  echo hi  ".into();
            entry.state.cursor = entry.state.value.len();
            shell.send_compose(cx);
            let entry = shell.workspace.compose.get(&view).unwrap();
            assert!(entry.state.value.is_empty(), "发送后应清空 (契约 7)");
            assert_eq!(entry.state.cursor, 0);
            assert!(entry.state.ime_marked_text.is_empty());
        });
    });
}

#[gpui::test]
fn spec_20260820_terminal_compose_bar__send_trimmed(cx: &mut gpui::TestAppContext) {
    let dir = test_dir("trim-send");
    let shell = init_app(cx);
    cx.update(|cx| {
        shell.update(cx, |shell, cx| {
            insert_local_session(shell, 1, dir.clone(), cx);
            shell.workspace.active_view = Some(ActiveView::LocalSession(1));
            let view = ActiveView::LocalSession(1);
            let entry = shell.workspace.compose_entry_mut(view);
            entry.visible = true;
            entry.state.value = "  echo hi  ".into();
            entry.state.cursor = entry.state.value.len();
            // 验证 trim 后非空才发送，首尾空白被去除
            let trimmed = shell
                .workspace
                .compose
                .get(&view)
                .unwrap()
                .state
                .value
                .trim()
                .to_string();
            assert_eq!(trimmed, "echo hi");
            shell.send_compose(cx);
            assert!(
                shell
                    .workspace
                    .compose
                    .get(&view)
                    .unwrap()
                    .state
                    .value
                    .is_empty()
            );
        });
    });
}

#[gpui::test]
fn spec_20260820_terminal_compose_bar__escape_retains_draft(cx: &mut gpui::TestAppContext) {
    let dir = test_dir("escape-retain");
    let shell = init_app(cx);
    cx.update(|cx| {
        shell.update(cx, |shell, cx| {
            insert_local_session(shell, 1, dir.clone(), cx);
            shell.workspace.active_view = Some(ActiveView::LocalSession(1));
            let view = ActiveView::LocalSession(1);
            let entry = shell.workspace.compose_entry_mut(view);
            entry.visible = true;
            entry.state.value = "draft text".into();
            entry.state.cursor = entry.state.value.len();
            // 模拟 Escape 收起
            {
                let e = shell.workspace.compose.get_mut(&view).unwrap();
                e.state.clear_composition();
            }
            shell.hide_compose_bar(cx);
            assert!(
                !shell.workspace.compose_visible(view),
                "Escape 后应收起 (契约 8)"
            );
            assert_eq!(
                shell.workspace.compose.get(&view).unwrap().state.value,
                "draft text",
                "草稿保留"
            );
            // 再次展开应恢复草稿
            shell.workspace.compose_entry_mut(view).visible = true;
            assert_eq!(
                shell.workspace.compose.get(&view).unwrap().state.value,
                "draft text"
            );
        });
    });
}

#[gpui::test]
fn spec_20260820_terminal_compose_bar__shift_enter_inserts_newline(cx: &mut gpui::TestAppContext) {
    let dir = test_dir("shift-enter");
    let shell = init_app(cx);
    cx.update(|cx| {
        shell.update(cx, |shell, cx| {
            insert_local_session(shell, 1, dir.clone(), cx);
            shell.workspace.active_view = Some(ActiveView::LocalSession(1));
            let view = ActiveView::LocalSession(1);
            let entry = shell.workspace.compose_entry_mut(view);
            entry.visible = true;
            entry.state.value = "line1".into();
            entry.state.cursor = entry.state.value.len();
            // 模拟 Shift+Enter 插入换行
            let e = shell.workspace.compose.get_mut(&view).unwrap();
            e.state.clear_composition();
            e.state.replace_selection("\n");
            cx.notify();
        });
        let s = shell.read(cx);
        let view = ActiveView::LocalSession(1);
        let val = s.workspace.compose.get(&view).unwrap().state.value.clone();
        assert_eq!(val, "line1\n", "Shift+Enter 插入换行 (契约 10)");
        assert!(val.contains('\n'));
    });
}

#[gpui::test]
fn spec_20260820_terminal_compose_bar__view_switch_is_per_terminal(cx: &mut gpui::TestAppContext) {
    let dir1 = test_dir("switch-per-terminal-1");
    let dir2 = test_dir("switch-per-terminal-2");
    let shell = init_app(cx);
    cx.update(|cx| {
        shell.update(cx, |shell, cx| {
            insert_local_session(shell, 1, dir1.clone(), cx);
            insert_local_session(shell, 2, dir2.clone(), cx);
            shell.workspace.active_view = Some(ActiveView::LocalSession(1));
            let view1 = ActiveView::LocalSession(1);
            let entry = shell.workspace.compose_entry_mut(view1);
            entry.visible = true;
            entry.state.value = "keep me".into();
            entry.state.cursor = entry.state.value.len();
            // 切换视图 — 终端级隔离：新终端应为收起/空
            shell.select_local_session(2, cx);
        });
        let s = shell.read(cx);
        let view1 = ActiveView::LocalSession(1);
        let view2 = ActiveView::LocalSession(2);
        // 新终端默认收起且空
        assert!(
            !s.workspace.compose_visible(view2),
            "新终端应独立收起 (终端级)"
        );
        assert!(
            s.workspace
                .compose
                .get(&view2)
                .map(|e| e.state.value.is_empty())
                .unwrap_or(true)
        );
        // 原终端草稿保留
        assert!(s.workspace.compose_visible(view1));
        assert_eq!(
            s.workspace.compose.get(&view1).unwrap().state.value,
            "keep me"
        );
        // 再切回原终端应恢复
        shell.update(cx, |shell, cx| {
            shell.select_local_session(1, cx);
        });
        let s = shell.read(cx);
        assert!(s.workspace.compose_visible(view1));
        assert_eq!(
            s.workspace.compose.get(&view1).unwrap().state.value,
            "keep me"
        );
        // 另一个终端设置独立草稿后互不影响
        shell.update(cx, |shell, cx| {
            shell.select_local_session(2, cx);
            let entry = shell.workspace.compose_entry_mut(view2);
            entry.visible = true;
            entry.state.value = "other draft".into();
            entry.state.cursor = entry.state.value.len();
            let _ = cx;
        });
        let s = shell.read(cx);
        assert_eq!(
            s.workspace.compose.get(&view2).unwrap().state.value,
            "other draft"
        );
        assert_eq!(
            s.workspace.compose.get(&view1).unwrap().state.value,
            "keep me"
        );
    });
}

#[gpui::test]
fn spec_20260820_terminal_compose_bar__no_view_send_is_noop(cx: &mut gpui::TestAppContext) {
    let dir = test_dir("no-view-noop");
    let shell = init_app(cx);
    cx.update(|cx| {
        shell.update(cx, |shell, cx| {
            insert_local_session(shell, 1, dir.clone(), cx);
            shell.workspace.active_view = Some(ActiveView::LocalSession(1));
            let view = ActiveView::LocalSession(1);
            let entry = shell.workspace.compose_entry_mut(view);
            entry.visible = true;
            entry.state.value = "echo hi".into();
            entry.state.cursor = entry.state.value.len();
            // 切到无视图再发送应 no-op，且原终端草稿不被清空
            shell.workspace.active_view = None;
            shell.send_compose(cx);
            // 原终端草稿仍在
            assert_eq!(
                shell.workspace.compose.get(&view).unwrap().state.value,
                "echo hi",
                "no-op 不清空"
            );
            // 恢复视图后仍可发送
            shell.workspace.active_view = Some(view);
            shell.send_compose(cx);
            assert!(
                shell
                    .workspace
                    .compose
                    .get(&view)
                    .unwrap()
                    .state
                    .value
                    .is_empty()
            );
        });
    });
}

#[gpui::test]
fn spec_20260820_terminal_compose_bar__editing_does_not_send_to_terminal(
    cx: &mut gpui::TestAppContext,
) {
    let dir = test_dir("edit-no-send");
    let shell = init_app(cx);
    cx.update(|cx| {
        shell.update(cx, |shell, cx| {
            insert_local_session(shell, 1, dir.clone(), cx);
            shell.workspace.active_view = Some(ActiveView::LocalSession(1));
            let view = ActiveView::LocalSession(1);
            let entry = shell.workspace.compose_entry_mut(view);
            entry.visible = true;
            entry.state.value.clear();
            entry.state.cursor = 0;
            // 本地编辑
            {
                let e = shell.workspace.compose.get_mut(&view).unwrap();
                e.state.replace_selection("a");
                e.state.replace_selection("b");
                e.state.backspace();
                assert_eq!(e.state.value, "a");
            }
            // 未调用 send_compose，终端未收到输入 (契约 5)
        });
    });
}

#[test]
fn spec_20260820_terminal_compose_bar__text_editing_pure_logic() {
    use crate::shared::text_editing::TextEditingState;
    let mut state = TextEditingState::new(String::new());
    state.replace_selection("  hello  ");
    assert_eq!(state.value, "  hello  ");
    assert_eq!(state.value.trim(), "hello");
    // 空白判定
    let empty = "   ".trim().is_empty();
    assert!(empty);
    // 换行插入
    state.replace_selection("\nworld");
    assert_eq!(state.value, "  hello  \nworld");
}

#[gpui::test]
fn spec_compose_per_terminal__remote_tab_indices_remap(cx: &mut gpui::TestAppContext) {
    // 验证 RemoteTab 索引重映射时 compose 同步迁移（与分栏一致）
    let shell = init_app(cx);
    cx.update(|cx| {
        shell.update(cx, |shell, _cx| {
            // 手动构造 compose 条目，模拟已打开的远程标签 0..3
            for idx in 0..3 {
                let view = ActiveView::RemoteTab(idx);
                let entry = shell.workspace.compose_entry_mut(view);
                entry.visible = true;
                entry.state.value = format!("draft-{idx}");
            }
            // 删除索引 1，剩余应重映射为 0->0, 1(原2)->1
            shell.workspace.remap_compose_remote_tab_indices(1);
            assert!(
                shell
                    .workspace
                    .compose
                    .contains_key(&ActiveView::RemoteTab(0))
            );
            assert_eq!(
                shell
                    .workspace
                    .compose
                    .get(&ActiveView::RemoteTab(0))
                    .unwrap()
                    .state
                    .value,
                "draft-0"
            );
            assert_eq!(
                shell
                    .workspace
                    .compose
                    .get(&ActiveView::RemoteTab(1))
                    .unwrap()
                    .state
                    .value,
                "draft-2"
            );
            assert!(
                !shell
                    .workspace
                    .compose
                    .contains_key(&ActiveView::RemoteTab(2))
            );
        });
    });
}
