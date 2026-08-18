//! 本地会话固定/重命名生命周期契约测试 (spec 20260818-local-tab-pin-rename)。
//!
//! 与 git_sync_toast_tests 相同的引导方式：静默终端（`sleep`）占位
//! `LocalSession.terminal`，避免真实 shell 的 PTY reader 触发
//! test_scheduler 确定性守卫。此外本套测试会把设置读写重定向到临时
//! 目录，因为固定/重命名/关闭动作都会 `persist_settings` 写盘，不能
//! 污染真实 `~/.config/crossh/settings.toml`。

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

use gpui::Entity;

use crate::features::settings::persistence::set_test_settings_path;
use crate::features::terminal::view::TerminalView;
use crate::features::workspace::rename_editor::RenameEditor;
use crate::features::workspace::settings::PinnedLocalTab;
use crate::features::workspace::view::{LocalSession, LocalSessionId};

use super::AppShell;

/// 每个测试使用独立的设置目录，避免并行测试把固定记录写入同一文件互相串扰。
static NEXT_SETTINGS_DIR: AtomicUsize = AtomicUsize::new(0);

/// 引导应用；设置读写重定向到独立的临时目录（thread_local，仅本测试线程）。
fn init_app(cx: &mut gpui::TestAppContext) -> Entity<AppShell> {
    let index = NEXT_SETTINGS_DIR.fetch_add(1, Ordering::Relaxed);
    let settings_dir = std::env::temp_dir().join(format!(
        "crossh-pinned-tab-tests-{}-{index}",
        std::process::id()
    ));
    init_app_with_settings(cx, settings_dir)
}

/// 与 `init_app` 相同，但由调用方指定设置目录；契约 10 测试需要先记住
/// 正常写盘路径，再重定向到必然失败的路径做磁盘对照。
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

/// 静默终端（sleep，无输出）占位 `LocalSession.terminal`。
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

/// 手动插入一个本地会话（带静默终端），返回其 id。
/// 会话会同步进 `local_dirs`，使关闭路径可完整执行。
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
        },
    );
    shell.sync_local_dirs(cx);
}

fn pinned_records(shell: &Entity<AppShell>, cx: &mut gpui::TestAppContext) -> Vec<PinnedLocalTab> {
    cx.update(|cx| shell.read(cx).workspace_settings.pinned_local_tabs.clone())
}

/// 关闭全部本地会话（恢复测试开的 display-only 终端，保持与
/// shell_notification_tests 相同的清理方式）。
fn close_all_local_sessions(shell: &Entity<AppShell>, cx: &mut gpui::TestAppContext) {
    let ids: Vec<LocalSessionId> = cx.update(|cx| {
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

fn session_pin(
    shell: &Entity<AppShell>,
    cx: &mut gpui::TestAppContext,
    id: LocalSessionId,
) -> Option<u64> {
    cx.update(|cx| {
        shell
            .read(cx)
            .workspace
            .sessions
            .local_sessions
            .get(&id)
            .and_then(|session| session.pin_id)
    })
}

fn session_name(
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
            .and_then(|session| session.custom_name.clone())
    })
}

fn test_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("crossh-pinned-tab-{tag}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("test directory should be created");
    dir.canonicalize()
        .expect("test directory should canonicalize")
}

#[gpui::test]
fn spec_20260818_local_tab_pin_pin_appends_record_and_marks_session(cx: &mut gpui::TestAppContext) {
    let dir = test_dir("pin-once");
    let shell = init_app(cx);
    cx.update(|cx| {
        shell.update(cx, |shell, cx| {
            insert_local_session(shell, 1, dir.clone(), cx);
            shell.pin_local_session(1, cx);
        });
    });

    let records = pinned_records(&shell, cx);
    assert_eq!(records.len(), 1, "固定后必须有一条持久化记录");
    assert_eq!(records[0].pin_id, 1, "首个 pin_id 从 1 开始");
    assert_eq!(records[0].project_dir, dir);
    assert_eq!(records[0].cwd, dir);
    assert_eq!(records[0].custom_name, None, "初始无自定义名称");
    assert_eq!(session_pin(&shell, cx, 1), Some(1));
}

#[gpui::test]
fn spec_20260818_local_tab_pin_repeated_pin_is_idempotent(cx: &mut gpui::TestAppContext) {
    let dir = test_dir("pin-twice");
    let shell = init_app(cx);
    cx.update(|cx| {
        shell.update(cx, |shell, cx| {
            insert_local_session(shell, 1, dir.clone(), cx);
            shell.pin_local_session(1, cx);
            shell.pin_local_session(1, cx);
        });
    });

    let records = pinned_records(&shell, cx);
    assert_eq!(records.len(), 1, "重复固定不新增记录（契约 9）");
    assert_eq!(records[0].pin_id, 1);
    assert_eq!(session_pin(&shell, cx, 1), Some(1));
}

#[gpui::test]
fn spec_20260818_local_tab_pin_same_directory_sessions_pin_independently(
    cx: &mut gpui::TestAppContext,
) {
    let dir = test_dir("pin-same-dir");
    let shell = init_app(cx);
    cx.update(|cx| {
        shell.update(cx, |shell, cx| {
            insert_local_session(shell, 1, dir.clone(), cx);
            insert_local_session(shell, 2, dir.clone(), cx);
            shell.pin_local_session(1, cx);
            shell.pin_local_session(2, cx);
        });
    });

    let records = pinned_records(&shell, cx);
    assert_eq!(records.len(), 2, "同目录多个会话各自独立固定");
    assert_eq!(records[0].pin_id, 1);
    assert_eq!(records[1].pin_id, 2, "pin_id 递增分配");
    assert_eq!(session_pin(&shell, cx, 1), Some(1));
    assert_eq!(session_pin(&shell, cx, 2), Some(2));
}

#[gpui::test]
fn spec_20260818_local_tab_pin_rename_applies_and_persists(cx: &mut gpui::TestAppContext) {
    let dir = test_dir("rename");
    let shell = init_app(cx);
    cx.update(|cx| {
        shell.update(cx, |shell, cx| {
            insert_local_session(shell, 1, dir.clone(), cx);
            shell.pin_local_session(1, cx);
            shell.rename_editor = Some(RenameEditor::new(1, String::new(), cx.focus_handle()));
            shell.rename_editor.as_mut().unwrap().state.value = "  work  ".into();
            shell.submit_rename_local_session(cx);
        });
    });

    assert_eq!(
        session_name(&shell, cx, 1),
        Some("work".to_string()),
        "名称 trim 后生效"
    );
    let records = pinned_records(&shell, cx);
    assert_eq!(
        records[0].custom_name,
        Some("work".to_string()),
        "名称持久化到固定记录"
    );
    assert!(
        cx.update(|cx| shell.read(cx).rename_editor.is_none()),
        "提交后弹窗关闭"
    );
}

#[gpui::test]
fn spec_20260818_local_tab_pin_blank_rename_clears_custom_name(cx: &mut gpui::TestAppContext) {
    let dir = test_dir("rename-blank");
    let shell = init_app(cx);
    cx.update(|cx| {
        shell.update(cx, |shell, cx| {
            insert_local_session(shell, 1, dir.clone(), cx);
            shell.pin_local_session(1, cx);
            shell.rename_editor = Some(RenameEditor::new(1, String::new(), cx.focus_handle()));
            shell.rename_editor.as_mut().unwrap().state.value = "work".into();
            shell.submit_rename_local_session(cx);

            shell.rename_editor = Some(RenameEditor::new(1, String::new(), cx.focus_handle()));
            shell.rename_editor.as_mut().unwrap().state.value = "   ".into();
            shell.submit_rename_local_session(cx);
        });
    });

    assert_eq!(
        session_name(&shell, cx, 1),
        None,
        "空白名称清除自定义名称（契约 4）"
    );
    let records = pinned_records(&shell, cx);
    assert_eq!(records[0].custom_name, None);
}

#[gpui::test]
fn spec_20260818_local_tab_pin_cancel_rename_keeps_state(cx: &mut gpui::TestAppContext) {
    let dir = test_dir("rename-cancel");
    let shell = init_app(cx);
    cx.update(|cx| {
        shell.update(cx, |shell, cx| {
            insert_local_session(shell, 1, dir.clone(), cx);
            shell.pin_local_session(1, cx);
            shell.rename_editor = Some(RenameEditor::new(1, String::new(), cx.focus_handle()));
            shell.rename_editor.as_mut().unwrap().state.value = "work".into();
            shell.cancel_rename_local_session(cx);
        });
    });

    assert_eq!(session_name(&shell, cx, 1), None, "取消不应用名称");
    let records = pinned_records(&shell, cx);
    assert_eq!(records[0].custom_name, None, "取消不改持久化");
    assert!(
        cx.update(|cx| shell.read(cx).rename_editor.is_none()),
        "取消后弹窗关闭"
    );
}

#[gpui::test]
fn spec_20260818_local_tab_pin_unpin_removes_record_keeps_session(cx: &mut gpui::TestAppContext) {
    let dir = test_dir("unpin");
    let shell = init_app(cx);
    cx.update(|cx| {
        shell.update(cx, |shell, cx| {
            insert_local_session(shell, 1, dir.clone(), cx);
            shell.pin_local_session(1, cx);
            shell.unpin_local_session(1, cx);
        });
    });

    assert_eq!(
        session_pin(&shell, cx, 1),
        None,
        "取消固定后会话回到普通状态"
    );
    assert!(
        cx.update(|cx| shell
            .read(cx)
            .workspace
            .sessions
            .local_sessions
            .contains_key(&1)),
        "会话保持打开（契约 7）"
    );
    assert!(pinned_records(&shell, cx).is_empty(), "持久化记录移除");
}

#[gpui::test]
fn spec_20260818_local_tab_pin_close_removes_pinned_record(cx: &mut gpui::TestAppContext) {
    let dir = test_dir("close");
    let shell = init_app(cx);
    cx.update(|cx| {
        shell.update(cx, |shell, cx| {
            insert_local_session(shell, 1, dir.clone(), cx);
            shell.pin_local_session(1, cx);
            shell.close_local_session(1, cx);
        });
    });

    assert!(
        pinned_records(&shell, cx).is_empty(),
        "关闭即移除固定记录（契约 8）"
    );
    assert!(
        cx.update(|cx| shell.read(cx).workspace.sessions.local_sessions.is_empty()),
        "会话已关闭"
    );
}

#[gpui::test]
fn spec_20260818_local_tab_pin_rename_submit_after_close_is_ignored(cx: &mut gpui::TestAppContext) {
    let dir = test_dir("rename-after-close");
    let shell = init_app(cx);
    cx.update(|cx| {
        shell.update(cx, |shell, cx| {
            insert_local_session(shell, 1, dir.clone(), cx);
            shell.pin_local_session(1, cx);
            shell.rename_editor = Some(RenameEditor::new(1, String::new(), cx.focus_handle()));
            shell.rename_editor.as_mut().unwrap().state.value = "work".into();
            shell.close_local_session(1, cx);
            shell.submit_rename_local_session(cx);
        });
    });

    assert!(
        pinned_records(&shell, cx).is_empty(),
        "关闭后提交不写持久化（边界）"
    );
}

#[gpui::test]
fn spec_20260818_local_tab_pin_restore_applies_pin_state_to_created_session(
    cx: &mut gpui::TestAppContext,
) {
    let dir = test_dir("restore");
    let shell = init_app(cx);
    cx.update(|cx| {
        shell.update(cx, |shell, cx| {
            // 恢复路径第二阶段：把固定记录状态应用到「创建出来的指定会话」。
            insert_local_session(shell, 7, dir.clone(), cx);
            shell.apply_pin_to_session(7, 5, Some("restored".into()), cx);
        });
    });

    assert_eq!(session_pin(&shell, cx, 7), Some(5), "pin_id 来自持久化记录");
    assert_eq!(
        session_name(&shell, cx, 7),
        Some("restored".to_string()),
        "自定义名称应用到恢复的会话（契约 5）"
    );
    // 恢复不产生新的持久化记录（记录本身已在启动时加载）。
    assert!(pinned_records(&shell, cx).is_empty());
}

#[gpui::test]
fn spec_20260818_local_tab_pin_second_pin_after_unpin_gets_fresh_id(cx: &mut gpui::TestAppContext) {
    let dir = test_dir("re-pin");
    let shell = init_app(cx);
    cx.update(|cx| {
        shell.update(cx, |shell, cx| {
            insert_local_session(shell, 1, dir.clone(), cx);
            shell.pin_local_session(1, cx);
            shell.unpin_local_session(1, cx);
            shell.pin_local_session(1, cx);
        });
    });

    let records = pinned_records(&shell, cx);
    assert_eq!(records.len(), 1, "重新固定产生一条新记录");
    // unpin 后记录列表为空，重新固定从 1 开始分配（与 next_pin_id 空列表语义一致）。
    assert_eq!(records[0].pin_id, 1);
    assert_eq!(session_pin(&shell, cx, 1), Some(1));
}

/// 契约 10：设置保存失败时内存状态保持原行为、应用不崩溃。
/// 把写盘目标重定向到一个已存在的目录本身，`fs::write` 对目录路径必然失败
/// （macOS/Linux EISDIR，Windows 访问拒绝），以此可靠触发保存失败路径；
/// 再对照正常路径下已落盘的文件未被更新，证明失败确实发生而非静默成功。
#[gpui::test]
fn spec_20260818_local_tab_pin_save_failure_keeps_memory_state(cx: &mut gpui::TestAppContext) {
    let dir = test_dir("save-fail");
    let settings_dir = std::env::temp_dir().join(format!(
        "crossh-pinned-tab-tests-{}-save-fail-good",
        std::process::id()
    ));
    let shell = init_app_with_settings(cx, settings_dir.clone());
    cx.update(|cx| {
        shell.update(cx, |shell, cx| {
            insert_local_session(shell, 1, dir.clone(), cx);
            shell.pin_local_session(1, cx);
        });
    });

    // 把写盘目标重定向到一个目录：此后每次 persist_settings 必然失败。
    let failing_dir = std::env::temp_dir().join(format!(
        "crossh-pinned-tab-tests-{}-save-fail-dir",
        std::process::id()
    ));
    std::fs::create_dir_all(&failing_dir).expect("failing dir should be created");
    set_test_settings_path(Some(failing_dir));

    cx.update(|cx| {
        shell.update(cx, |shell, cx| {
            shell.rename_editor = Some(RenameEditor::new(1, String::new(), cx.focus_handle()));
            shell.rename_editor.as_mut().unwrap().state.value = "work".into();
            shell.submit_rename_local_session(cx);
        });
    });

    // 保存失败后内存状态保持原行为：名称生效、会话仍固定、记录仍在。
    assert_eq!(session_name(&shell, cx, 1), Some("work".to_string()));
    assert_eq!(session_pin(&shell, cx, 1), Some(1));
    let records = pinned_records(&shell, cx);
    assert_eq!(records.len(), 1, "保存失败不丢内存记录");
    assert_eq!(records[0].custom_name.as_deref(), Some("work"));
    // 磁盘未被更新：固定阶段写入的文件仍无 custom_name，证明保存确实失败
    // 且未产生半写内容（契约 10：内存保持、应用不退出）。
    let saved = std::fs::read_to_string(settings_dir.join("settings.toml")).expect("settings file");
    assert!(!saved.contains("custom_name"), "保存失败时磁盘不得被更新");
}

/// 预置 settings.toml：写入最近目录与固定记录（TOML 单引号字面量，
/// Windows 反斜杠路径无需转义）。
fn seed_settings(settings_dir: &std::path::Path, body: &str) -> std::path::PathBuf {
    std::fs::create_dir_all(settings_dir).expect("settings dir should be created");
    let path = settings_dir.join("settings.toml");
    std::fs::write(&path, body).expect("settings should be written");
    path
}

/// 契约 5 Rev-2：启动时不得自动打开任何会话，也不得恢复任何固定记录；
/// 记录保留在持久化列表中，待打开对应项目时按契约 11 恢复。
#[gpui::test]
fn spec_20260818_local_tab_pin_startup_auto_opens_no_session(cx: &mut gpui::TestAppContext) {
    let project_a = test_dir("startup-silent-a");
    let project_b = test_dir("startup-silent-b");
    let settings_dir = std::env::temp_dir().join(format!(
        "crossh-pinned-tab-tests-{}-startup-silent",
        std::process::id()
    ));
    let (a, b) = (project_a.to_string_lossy(), project_b.to_string_lossy());
    // 最近打开的是 B，A 与 B 各有一条固定记录——启动仍不得打开任何会话。
    seed_settings(
        &settings_dir,
        &format!(
            "recent_local_dirs = ['{b}']\n\
             \n\
             [[pinned_local_tabs]]\n\
             pin_id = 1\n\
             project_dir = '{a}'\n\
             cwd = '{a}'\n\
             \n\
             [[pinned_local_tabs]]\n\
             pin_id = 2\n\
             project_dir = '{b}'\n\
             cwd = '{b}'\n\
             custom_name = 'beta'\n"
        ),
    );

    let shell = init_app_with_settings(cx, settings_dir);
    cx.update(|cx| {
        assert!(
            shell.read(cx).workspace.sessions.local_sessions.is_empty(),
            "启动不自动打开任何会话（契约 5 Rev-2）"
        );
    });
    assert_eq!(
        pinned_records(&shell, cx).len(),
        2,
        "固定记录保留在持久化列表"
    );
}

/// 契约 11 Rev-3：激活有固定记录的项目时只恢复固定标签，不额外打开
/// 普通会话；激活无固定记录的新项目时打开一个普通会话；均幂等。
#[gpui::test]
fn spec_20260818_local_tab_pin_activating_project_restores_its_records(
    cx: &mut gpui::TestAppContext,
) {
    let project_b = test_dir("activate-restore");
    let project_c = test_dir("activate-fresh");
    let settings_dir = std::env::temp_dir().join(format!(
        "crossh-pinned-tab-tests-{}-activate-restore",
        std::process::id()
    ));
    let b = project_b.to_string_lossy();
    // 无 recent 目录（启动无「当前项目」）但有 B 的固定记录：启动不恢复。
    seed_settings(
        &settings_dir,
        &format!(
            "[[pinned_local_tabs]]\n\
             pin_id = 5\n\
             project_dir = '{b}'\n\
             cwd = '{b}'\n\
             custom_name = 'beta'\n"
        ),
    );
    let shell = init_app_with_settings(cx, settings_dir);
    cx.update(|cx| {
        assert!(
            shell.read(cx).workspace.sessions.local_sessions.is_empty(),
            "recent 为空时启动不恢复任何固定记录（契约 5 Rev-2）"
        );
    });

    // 激活有固定记录的项目 B → 只恢复固定标签，不额外打开普通会话。
    cx.update(|cx| {
        shell.update(cx, |shell, cx| {
            shell.activate_local_dir(project_b.clone(), cx);
        });
    });
    cx.update(|cx| {
        let sessions = &shell.read(cx).workspace.sessions.local_sessions;
        assert_eq!(
            sessions.len(),
            1,
            "激活有固定记录的项目只恢复固定标签，不额外打开普通会话（契约 11 Rev-3）"
        );
        let (_, session) = sessions.iter().next().expect("one restored session");
        assert_eq!(session.pin_id, Some(5), "会话即恢复的固定标签");
        assert_eq!(session.custom_name.as_deref(), Some("beta"));
    });

    // 幂等：再次激活不重复打开会话，也不改写记录。
    cx.update(|cx| {
        shell.update(cx, |shell, cx| {
            shell.activate_local_dir(project_b.clone(), cx);
        });
    });
    cx.update(|cx| {
        assert_eq!(
            shell.read(cx).workspace.sessions.local_sessions.len(),
            1,
            "已有会话的固定记录不得重复打开（契约 11 幂等）"
        );
    });

    // 激活无固定记录的新项目 C → 打开一个普通会话（现状合适）。
    cx.update(|cx| {
        shell.update(cx, |shell, cx| {
            shell.activate_local_dir(project_c.clone(), cx);
        });
    });
    cx.update(|cx| {
        let sessions = &shell.read(cx).workspace.sessions.local_sessions;
        assert_eq!(
            sessions.len(),
            2,
            "激活无固定记录的新项目打开一个普通会话（B 的固定标签保持打开）"
        );
        assert!(
            sessions.values().any(|session| session.pin_id.is_none()),
            "新项目会话是普通会话"
        );
    });
    close_all_local_sessions(&shell, cx);
}

/// 契约 11 Rev-4：恢复时记录目录失效 → 跳过并即时清理、不得污染当前
/// 活动会话的固定身份；恢复后项目无会话时兜底打开普通会话。
#[gpui::test]
fn spec_20260818_local_tab_pin_stale_record_restore_skips_cleans_and_falls_back(
    cx: &mut gpui::TestAppContext,
) {
    let project_a = test_dir("stale-fallback-a");
    let project_b = test_dir("stale-fallback-b");
    let stale_cwd = project_b.join("inner");
    std::fs::create_dir_all(&stale_cwd).expect("stale cwd should be created");
    let settings_dir = std::env::temp_dir().join(format!(
        "crossh-pinned-tab-tests-{}-stale-fallback",
        std::process::id()
    ));
    let b = project_b.to_string_lossy();
    let inner = stale_cwd.to_string_lossy();
    // B 有一条固定记录，cwd 指向其子目录（稍后删除制造失效）。
    seed_settings(
        &settings_dir,
        &format!(
            "[[pinned_local_tabs]]\n\
             pin_id = 5\n\
             project_dir = '{b}'\n\
             cwd = '{inner}'\n\
             custom_name = 'stale'\n"
        ),
    );
    let shell = init_app_with_settings(cx, settings_dir);
    // 先激活无记录项目 A，得到活动会话（潜在的污染目标）。
    cx.update(|cx| {
        shell.update(cx, |shell, cx| {
            shell.activate_local_dir(project_a.clone(), cx);
        });
    });
    let session_a = cx.update(|cx| {
        let sessions = &shell.read(cx).workspace.sessions.local_sessions;
        assert_eq!(sessions.len(), 1, "A 激活打开一个普通会话");
        sessions.keys().copied().next().expect("session A")
    });
    // 让 B 记录的 cwd 失效（project_dir 仍存在）。
    std::fs::remove_dir_all(&stale_cwd).expect("stale cwd should be removed");
    // 激活 B：记录失效 → 跳过恢复且不污染 A 会话；兜底打开普通会话。
    cx.update(|cx| {
        shell.update(cx, |shell, cx| {
            shell.activate_local_dir(project_b.clone(), cx);
        });
    });
    assert_eq!(
        session_pin(&shell, cx, session_a),
        None,
        "失效记录的固定身份不得写到既有会话（契约 11 Rev-4）"
    );
    assert!(
        pinned_records(&shell, cx).is_empty(),
        "失效记录在恢复时即时清理（契约 11 Rev-4）"
    );
    cx.update(|cx| {
        let sessions = &shell.read(cx).workspace.sessions.local_sessions;
        assert_eq!(sessions.len(), 2, "A 会话保持 + B 兜底普通会话");
        let (b_session_id, b_session) = sessions
            .iter()
            .find(|(id, _)| **id != session_a)
            .expect("B session");
        assert_ne!(b_session_id, &session_a);
        assert_eq!(b_session.pin_id, None, "兜底会话是普通会话");
        assert_eq!(b_session.project_dir, project_b);
    });
    // 幂等：再次激活 B 不重复打开。
    cx.update(|cx| {
        shell.update(cx, |shell, cx| {
            shell.activate_local_dir(project_b.clone(), cx);
        });
    });
    cx.update(|cx| {
        assert_eq!(
            shell.read(cx).workspace.sessions.local_sessions.len(),
            2,
            "兜底会话再次激活不重复打开（契约 11 幂等）"
        );
    });
    close_all_local_sessions(&shell, cx);
}

/// 契约 11 Rev-4：恢复列表部分失效时，失效记录被清理、有效记录的身份
/// 不被覆盖，也不污染当前活动会话。
#[gpui::test]
fn spec_20260818_local_tab_pin_partial_stale_restore_keeps_valid_identity(
    cx: &mut gpui::TestAppContext,
) {
    let project_a = test_dir("stale-partial-a");
    let project_c = test_dir("stale-partial-c");
    let stale_cwd = project_c.join("inner");
    std::fs::create_dir_all(&stale_cwd).expect("stale cwd should be created");
    let settings_dir = std::env::temp_dir().join(format!(
        "crossh-pinned-tab-tests-{}-stale-partial",
        std::process::id()
    ));
    let c = project_c.to_string_lossy();
    let inner = stale_cwd.to_string_lossy();
    seed_settings(
        &settings_dir,
        &format!(
            "[[pinned_local_tabs]]\n\
             pin_id = 6\n\
             project_dir = '{c}'\n\
             cwd = '{inner}'\n\
             \n\
             [[pinned_local_tabs]]\n\
             pin_id = 7\n\
             project_dir = '{c}'\n\
             cwd = '{c}'\n\
             custom_name = 'gamma'\n"
        ),
    );
    let shell = init_app_with_settings(cx, settings_dir);
    // 先激活 A 得到活动会话（潜在的污染目标）。
    cx.update(|cx| {
        shell.update(cx, |shell, cx| {
            shell.activate_local_dir(project_a.clone(), cx);
        });
    });
    let session_a = cx.update(|cx| {
        shell
            .read(cx)
            .workspace
            .sessions
            .local_sessions
            .keys()
            .copied()
            .next()
            .expect("session A")
    });
    // 失效 pin6 的 cwd（pin7 仍有效）。
    std::fs::remove_dir_all(&stale_cwd).expect("stale cwd should be removed");
    // 激活 C：pin6 跳过并清理，pin7 恢复且身份保持。
    cx.update(|cx| {
        shell.update(cx, |shell, cx| {
            shell.activate_local_dir(project_c.clone(), cx);
        });
    });
    assert_eq!(
        session_pin(&shell, cx, session_a),
        None,
        "失效记录固定身份不得污染活动会话（契约 11 Rev-4）"
    );
    cx.update(|cx| {
        let sessions = &shell.read(cx).workspace.sessions.local_sessions;
        assert_eq!(sessions.len(), 2, "A 会话保持 + C 恢复 pin7");
        let c_session = sessions
            .iter()
            .find(|(id, _)| **id != session_a)
            .map(|(_, session)| session)
            .expect("C session");
        assert_eq!(c_session.pin_id, Some(7), "有效记录身份不被失效记录覆盖");
        assert_eq!(c_session.custom_name.as_deref(), Some("gamma"));
    });
    let records = pinned_records(&shell, cx);
    assert_eq!(records.len(), 1, "失效记录即时清理，只剩 pin7");
    assert_eq!(records[0].pin_id, 7, "保留有效记录");
    close_all_local_sessions(&shell, cx);
}
