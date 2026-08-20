//! 固定标签默认命令契约测试 (spec 20260821-pinned-tab-default-command)。

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

use gpui::Entity;

use crate::features::settings::persistence::set_test_settings_path;
use crate::features::terminal::view::TerminalView;
use crate::features::workspace::default_command_editor::DefaultCommandEditor;
use crate::features::workspace::settings::PinnedLocalTab;
use crate::features::workspace::view::{LocalSession, LocalSessionId};

use super::AppShell;

static NEXT_SETTINGS_DIR: AtomicUsize = AtomicUsize::new(100);

fn init_app(cx: &mut gpui::TestAppContext) -> Entity<AppShell> {
    let index = NEXT_SETTINGS_DIR.fetch_add(1, Ordering::Relaxed);
    let settings_dir = std::env::temp_dir().join(format!(
        "crossh-default-cmd-tests-{}-{index}",
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

fn pinned_records(shell: &Entity<AppShell>, cx: &mut gpui::TestAppContext) -> Vec<PinnedLocalTab> {
    cx.update(|cx| shell.read(cx).workspace_settings.pinned_local_tabs.clone())
}

fn session_default_command(
    shell: &Entity<AppShell>,
    cx: &mut gpui::TestAppContext,
    id: LocalSessionId,
) -> Option<String> {
    cx.update(|cx| {
        shell
            .read(cx)
            .workspace
            .sessions
            .local_sessions
            .get(&id)
            .and_then(|s| s.default_command.clone())
    })
}

fn test_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("crossh-default-cmd-{tag}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("test directory should be created");
    dir.canonicalize()
        .expect("test directory should canonicalize")
}

#[gpui::test]
fn spec_20260821_pinned_tab_default_command_edit_persists_and_trims(cx: &mut gpui::TestAppContext) {
    let dir = test_dir("edit-persist");
    let shell = init_app(cx);
    cx.update(|cx| {
        shell.update(cx, |shell, cx| {
            insert_local_session(shell, 1, dir.clone(), cx);
            shell.pin_local_session(1, cx);
            shell.default_command_editor = Some(DefaultCommandEditor::new(
                1,
                String::new(),
                cx.focus_handle(),
            ));
            shell.default_command_editor.as_mut().unwrap().state.value = "  opencode  ".into();
            shell.submit_default_command(cx);
        });
    });
    assert_eq!(
        session_default_command(&shell, cx, 1),
        Some("opencode".to_string()),
        "trim 后持久化"
    );
    let records = pinned_records(&shell, cx);
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].default_command.as_deref(), Some("opencode"));
    // 幂等
    cx.update(|cx| {
        shell.update(cx, |shell, cx| {
            shell.default_command_editor = Some(DefaultCommandEditor::new(
                1,
                "opencode".into(),
                cx.focus_handle(),
            ));
            shell.submit_default_command(cx);
        });
    });
    assert_eq!(
        session_default_command(&shell, cx, 1),
        Some("opencode".to_string())
    );
}

#[gpui::test]
fn spec_20260821_pinned_tab_default_command_blank_clears(cx: &mut gpui::TestAppContext) {
    let dir = test_dir("blank-clear");
    let shell = init_app(cx);
    cx.update(|cx| {
        shell.update(cx, |shell, cx| {
            insert_local_session(shell, 1, dir.clone(), cx);
            shell.pin_local_session(1, cx);
            shell.default_command_editor = Some(DefaultCommandEditor::new(
                1,
                String::new(),
                cx.focus_handle(),
            ));
            shell.default_command_editor.as_mut().unwrap().state.value = "ssh host".into();
            shell.submit_default_command(cx);
            shell.default_command_editor = Some(DefaultCommandEditor::new(
                1,
                String::new(),
                cx.focus_handle(),
            ));
            shell.default_command_editor.as_mut().unwrap().state.value = "   ".into();
            shell.submit_default_command(cx);
        });
    });
    assert_eq!(session_default_command(&shell, cx, 1), None, "空白清除");
    let records = pinned_records(&shell, cx);
    assert_eq!(records[0].default_command, None);
}

#[gpui::test]
fn spec_20260821_pinned_tab_default_command_cancel_keeps_state(cx: &mut gpui::TestAppContext) {
    let dir = test_dir("cancel-keep");
    let shell = init_app(cx);
    cx.update(|cx| {
        shell.update(cx, |shell, cx| {
            insert_local_session(shell, 1, dir.clone(), cx);
            shell.pin_local_session(1, cx);
            shell.default_command_editor = Some(DefaultCommandEditor::new(
                1,
                String::new(),
                cx.focus_handle(),
            ));
            shell.default_command_editor.as_mut().unwrap().state.value = "opencode".into();
            shell.submit_default_command(cx);
            shell.default_command_editor = Some(DefaultCommandEditor::new(
                1,
                String::new(),
                cx.focus_handle(),
            ));
            shell.default_command_editor.as_mut().unwrap().state.value = "changed".into();
            shell.cancel_default_command(cx);
        });
    });
    assert_eq!(
        session_default_command(&shell, cx, 1),
        Some("opencode".to_string()),
        "取消不改"
    );
    assert_eq!(
        pinned_records(&shell, cx)[0].default_command.as_deref(),
        Some("opencode")
    );
    assert!(
        cx.update(|cx| shell.read(cx).default_command_editor.is_none()),
        "取消后弹窗关闭"
    );
}

#[gpui::test]
fn spec_20260821_pinned_tab_default_command_submit_after_close_is_ignored(
    cx: &mut gpui::TestAppContext,
) {
    let dir = test_dir("submit-after-close");
    let shell = init_app(cx);
    cx.update(|cx| {
        shell.update(cx, |shell, cx| {
            insert_local_session(shell, 1, dir.clone(), cx);
            shell.pin_local_session(1, cx);
            shell.default_command_editor = Some(DefaultCommandEditor::new(
                1,
                String::new(),
                cx.focus_handle(),
            ));
            shell.default_command_editor.as_mut().unwrap().state.value = "opencode".into();
            shell.close_local_session(1, cx);
            shell.submit_default_command(cx);
        });
    });
    assert!(
        pinned_records(&shell, cx).is_empty(),
        "关闭后提交不写持久化"
    );
}

#[gpui::test]
fn spec_20260821_pinned_tab_default_command_clear_removes(cx: &mut gpui::TestAppContext) {
    let dir = test_dir("clear");
    let shell = init_app(cx);
    cx.update(|cx| {
        shell.update(cx, |shell, cx| {
            insert_local_session(shell, 1, dir.clone(), cx);
            shell.pin_local_session(1, cx);
            shell.default_command_editor = Some(DefaultCommandEditor::new(
                1,
                String::new(),
                cx.focus_handle(),
            ));
            shell.default_command_editor.as_mut().unwrap().state.value = "opencode".into();
            shell.submit_default_command(cx);
            shell.clear_default_command(1, cx);
        });
    });
    assert_eq!(session_default_command(&shell, cx, 1), None);
    assert_eq!(pinned_records(&shell, cx)[0].default_command, None);
    // 再次清除幂等
    cx.update(|cx| {
        shell.update(cx, |shell, cx| {
            shell.clear_default_command(1, cx);
        });
    });
    assert_eq!(session_default_command(&shell, cx, 1), None);
}

#[gpui::test]
fn spec_20260821_pinned_tab_default_command_reload_disabled_when_none_or_running(
    cx: &mut gpui::TestAppContext,
) {
    let dir = test_dir("reload-disabled");
    let shell = init_app(cx);
    cx.update(|cx| {
        shell.update(cx, |shell, cx| {
            insert_local_session(shell, 1, dir.clone(), cx);
            shell.pin_local_session(1, cx);
        });
    });
    // 无命令时 reload 应为 disabled：验证菜单条目 disabled 标志
    let entries = crate::features::workspace::tab_strip::local_session_menu_entries(
        1,
        false,
        true,
        true,
        dir.clone(),
        None,
        false,
    );
    let reload = entries
        .iter()
        .find(|e| matches!(e, crossh_ui::context_menu::MenuEntry::Item(item) if item.id == "reload-default-command"))
        .expect("reload entry");
    if let crossh_ui::context_menu::MenuEntry::Item(item) = reload {
        assert!(item.disabled, "无命令时重载应 disabled (契约 5)");
    }
    let clear = entries
        .iter()
        .find(|e| matches!(e, crossh_ui::context_menu::MenuEntry::Item(item) if item.id == "clear-default-command"))
        .expect("clear entry");
    if let crossh_ui::context_menu::MenuEntry::Item(item) = clear {
        assert!(item.disabled, "无命令时清除应 disabled (契约 7)");
    }
    // 有命令但 is_command_running=true 时 reload disabled
    let entries_running = crate::features::workspace::tab_strip::local_session_menu_entries(
        1,
        false,
        true,
        true,
        dir.clone(),
        Some("opencode".into()),
        true,
    );
    let reload_running = entries_running
        .iter()
        .find(|e| matches!(e, crossh_ui::context_menu::MenuEntry::Item(item) if item.id == "reload-default-command"))
        .expect("reload entry");
    if let crossh_ui::context_menu::MenuEntry::Item(item) = reload_running {
        assert!(item.disabled, "运行中时重载应 disabled (契约 6)");
    }
}

#[gpui::test]
fn spec_20260821_pinned_tab_default_command_unpinned_has_no_default_command_entries(
    _cx: &mut gpui::TestAppContext,
) {
    let dir = test_dir("unpinned-menu");
    let entries = crate::features::workspace::tab_strip::local_session_menu_entries(
        99, false, false, true, dir, None, false,
    );
    assert!(
        !entries.iter().any(|e| matches!(e, crossh_ui::context_menu::MenuEntry::Item(item) if item.id.contains("default-command"))),
        "未固定标签不应出现默认命令三项 (契约 9)"
    );
}

#[gpui::test]
fn spec_20260821_pinned_tab_default_command_restore_applies_to_created_session(
    cx: &mut gpui::TestAppContext,
) {
    let dir = test_dir("restore-cmd");
    let shell = init_app(cx);
    cx.update(|cx| {
        shell.update(cx, |shell, cx| {
            // 手动插入会话并通过 apply_pin 模拟恢复路径
            insert_local_session(shell, 7, dir.clone(), cx);
            shell.apply_pin_to_session(7, 5, Some("agent".into()), Some("opencode".into()), cx);
        });
    });
    assert_eq!(
        session_default_command(&shell, cx, 7),
        Some("opencode".to_string())
    );
}

#[gpui::test]
fn spec_20260821_pinned_tab_default_command_activate_restores_with_default_command(
    cx: &mut gpui::TestAppContext,
) {
    let project = test_dir("restore-project-direct2");
    let shell = init_app(cx);
    cx.update(|cx| {
        shell.update(cx, |shell, cx| {
            // 模拟持久化中有一条带 default_command 的记录
            shell
                .workspace_settings
                .pinned_local_tabs
                .push(PinnedLocalTab {
                    pin_id: 9,
                    project_dir: project.clone(),
                    cwd: project.clone(),
                    custom_name: Some("agent".into()),
                    default_command: Some("ssh vps".into()),
                });
            shell.activate_local_dir(project.clone(), cx);
        });
    });
    cx.update(|cx| {
        let sessions = &shell.read(cx).workspace.sessions.local_sessions;
        // 激活有固定记录的项目只恢复固定标签（契约 11），应为 1 个且带 default_command
        assert_eq!(sessions.len(), 1, "恢复后应只有 1 个会话");
        let session = sessions.values().next().unwrap();
        assert_eq!(session.pin_id, Some(9));
        assert_eq!(
            session.default_command.as_deref(),
            Some("ssh vps"),
            "恢复时应用 default_command (契约 4/11)"
        );
    });
}

#[gpui::test]
fn spec_20260821_pinned_tab_default_command_normalization_trims_and_is_idempotent(
    _cx: &mut gpui::TestAppContext,
) {
    let settings = crate::features::workspace::settings::WorkspaceSettings {
        pinned_local_tabs: vec![PinnedLocalTab {
            pin_id: 1,
            project_dir: PathBuf::from("/a"),
            cwd: PathBuf::from("/a"),
            custom_name: None,
            default_command: Some("  ssh host  ".into()),
        }],
        ..Default::default()
    }
    .normalized();
    assert_eq!(
        settings.pinned_local_tabs[0].default_command.as_deref(),
        Some("ssh host")
    );
    assert_eq!(settings.clone().normalized(), settings, "幂等");
    // 空白归一
    let blank = crate::features::workspace::settings::WorkspaceSettings {
        pinned_local_tabs: vec![PinnedLocalTab {
            pin_id: 1,
            project_dir: PathBuf::from("/a"),
            cwd: PathBuf::from("/a"),
            custom_name: None,
            default_command: Some("   ".into()),
        }],
        ..Default::default()
    }
    .normalized();
    assert_eq!(blank.pinned_local_tabs[0].default_command, None);
    // 验证 skip_serializing_if：None 时不写入 toml
    let ws = crate::features::workspace::settings::WorkspaceSettings::default();
    let encoded = toml::to_string(&ws).unwrap();
    assert!(!encoded.contains("default_command"));
    let ws_with = crate::features::workspace::settings::WorkspaceSettings {
        pinned_local_tabs: vec![PinnedLocalTab {
            pin_id: 1,
            project_dir: PathBuf::from("/a"),
            cwd: PathBuf::from("/a"),
            custom_name: None,
            default_command: Some("opencode".into()),
        }],
        ..Default::default()
    };
    let encoded_with = toml::to_string(&ws_with).unwrap();
    assert!(encoded_with.contains("default_command"));
}
