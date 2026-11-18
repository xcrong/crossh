#![cfg(unix)]
//! 工作区生命周期集成测试（3.9）。
//!
//! 覆盖：打开项目 → 建 tab/分屏 → 断开重连 → daemon 重启（布局与 pane id
//! 不变、会话按 attach 物化）→ 无 attach 时 `OpenProject` 保持休眠 →
//! attach 时新项目/空项目物化、缺失根不 spawn → `cd` 子目录不改变项目数量。
//!
//! 用临时 `CROSSH_CONFIG_DIR` / `CROSSH_CACHE_DIR` 隔离，显式指定
//! `CROSSH_DAEMON_BIN`，不连接或读写真实用户目录。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crossh_session::client::{Client, WorkspaceConnection};
use crossh_session::protocol::WorkspaceMutateRequest;
use crossh_session::workspace::{
    Axis, MutateOutcome, PaneId, PaneNodeSnapshot, PaneSnapshot, PaneStatus, ProjectId,
    ProjectSnapshot, TabId, TabSnapshot, WorkspaceOp, WorkspaceSnapshot,
};

const WAIT: Duration = Duration::from_secs(20);

fn temp_base() -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "crossh-workspace-test-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn supported_shell() -> String {
    for candidate in ["/bin/zsh", "/bin/bash", "/usr/bin/zsh", "/usr/bin/bash"] {
        if Path::new(candidate).is_file() {
            return candidate.to_string();
        }
    }
    "/bin/sh".to_string()
}

fn project(snapshot: &WorkspaceSnapshot, id: ProjectId) -> &ProjectSnapshot {
    snapshot
        .projects
        .iter()
        .find(|project| project.id == id)
        .expect("project in snapshot")
}

fn tab(project: &ProjectSnapshot, id: TabId) -> &TabSnapshot {
    project
        .tabs
        .iter()
        .find(|tab| tab.id == id)
        .expect("tab in project")
}

fn find_pane(snapshot: &WorkspaceSnapshot, id: PaneId) -> Option<&PaneSnapshot> {
    snapshot
        .projects
        .iter()
        .flat_map(|project| project.tabs.iter())
        .flat_map(|tab| tab.layout.leaves())
        .find(|pane| pane.id == id)
}

fn is_live(snapshot: &WorkspaceSnapshot, id: PaneId) -> bool {
    find_pane(snapshot, id).is_some_and(|pane| {
        matches!(pane.status, PaneStatus::Live)
            && pane.session.as_deref().is_some_and(|s| !s.is_empty())
    })
}

fn layout_shape(snapshot: &WorkspaceSnapshot, project_id: ProjectId) -> Vec<String> {
    let mut out = Vec::new();
    if let Some(project) = snapshot.projects.iter().find(|p| p.id == project_id) {
        out.push(format!(
            "project:{}:active_tab={}",
            project.id.0,
            project.active_tab.map(|id| id.0).unwrap_or(0)
        ));
        for tab in &project.tabs {
            out.push(format!(
                "tab:{}:active_pane={}",
                tab.id.0,
                tab.active_pane.map(|id| id.0).unwrap_or(0)
            ));
            node_shape(&tab.layout, &mut out);
        }
    }
    out
}

fn node_shape(node: &PaneNodeSnapshot, out: &mut Vec<String>) {
    match node {
        PaneNodeSnapshot::Leaf(pane) => out.push(format!("pane:{}", pane.id.0)),
        PaneNodeSnapshot::Split {
            id,
            axis,
            ratio,
            first,
            second,
        } => {
            out.push(format!("split:{}:{axis:?}:{ratio}", id.0));
            node_shape(first, out);
            node_shape(second, out);
        }
    }
}

fn tab_sessions(project: &ProjectSnapshot, tab_id: TabId) -> BTreeMap<PaneId, Option<String>> {
    tab(project, tab_id)
        .layout
        .leaves()
        .map(|pane| (pane.id, pane.session.clone()))
        .collect()
}

async fn wait_for(
    conn: &WorkspaceConnection,
    what: &str,
    pred: impl Fn(&WorkspaceSnapshot) -> bool,
) -> WorkspaceSnapshot {
    let deadline = Instant::now() + WAIT;
    loop {
        let snapshot = conn.current();
        if pred(&snapshot) {
            return (*snapshot).clone();
        }
        if Instant::now() >= deadline {
            panic!("timeout waiting for {what}; last snapshot: {snapshot:?}");
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

async fn mutate(
    conn: &WorkspaceConnection,
    op: WorkspaceOp,
) -> crossh_session::protocol::WorkspaceMutateReply {
    let reply = conn.mutate(op, None).await.expect("workspace mutate");
    assert_eq!(reply.outcome, MutateOutcome::Applied, "reply: {reply:?}");
    reply
}

#[tokio::test]
async fn workspace_lifecycle() {
    let base = temp_base();
    let config = base.join("config");
    let cache = base.join("cache");
    std::fs::create_dir_all(&config).unwrap();
    std::fs::create_dir_all(&cache).unwrap();

    let project_root = base.join("project");
    let sub = project_root.join("sub");
    std::fs::create_dir_all(&sub).unwrap();
    let empty_project = base.join("empty-project");
    std::fs::create_dir_all(&empty_project).unwrap();
    let attached_project = base.join("attached-project");
    std::fs::create_dir_all(&attached_project).unwrap();
    let missing_project = base.join("missing-project");

    // SAFETY: 单测试二进制内最早设置，之后只读；子进程继承隔离目录。
    unsafe {
        std::env::set_var("CROSSH_CONFIG_DIR", &config);
        std::env::set_var("CROSSH_CACHE_DIR", &cache);
        std::env::set_var("CROSSH_DAEMON_BIN", env!("CARGO_BIN_EXE_crossh-daemon"));
        std::env::set_var("SHELL", supported_shell());
    }

    // 清理可能残留的旧 daemon（正常时返回 NotFound）。
    let _ = crossh_session::daemon_stop().await;

    let conn = WorkspaceConnection::connect().await.unwrap();
    assert!(conn.current().projects.is_empty());

    // 1. 打开项目、建 tab、分屏。
    let opened = mutate(
        &conn,
        WorkspaceOp::OpenProject {
            root: project_root.clone(),
        },
    )
    .await;
    let project_id = opened.created.project.expect("project id");
    let first_tab = opened.created.tab.expect("tab id");
    let first_pane = opened.created.pane.expect("pane id");
    let snapshot = wait_for(&conn, "first pane live", |snapshot| {
        is_live(snapshot, first_pane)
    })
    .await;
    assert_eq!(snapshot.projects.len(), 1);

    let created = mutate(
        &conn,
        WorkspaceOp::CreateTab {
            project_id: Some(project_id),
            title: Some("second".to_string()),
            cwd: None,
            default_command: None,
        },
    )
    .await;
    let second_tab = created.created.tab.expect("second tab id");
    let second_pane = created.created.pane.expect("second pane id");

    let split = mutate(
        &conn,
        WorkspaceOp::SplitPane {
            pane_id: second_pane,
            axis: Axis::Column,
            cwd: None,
        },
    )
    .await;
    let split_pane = split.created.pane.expect("split pane id");
    assert!(split.created.split.is_some());

    let before_restart = wait_for(&conn, "all panes live", |snapshot| {
        is_live(snapshot, first_pane)
            && is_live(snapshot, second_pane)
            && is_live(snapshot, split_pane)
    })
    .await;
    let before_shape = layout_shape(&before_restart, project_id);
    let before_sessions = tab_sessions(project(&before_restart, project_id), second_tab);
    assert!(before_sessions.values().all(Option::is_some));

    // 2. 客户端断开重连：布局与绑定不变。
    drop(conn);
    tokio::time::sleep(Duration::from_millis(200)).await;
    let conn = WorkspaceConnection::connect().await.unwrap();
    let reconnected = wait_for(&conn, "live panes after reconnect", |snapshot| {
        is_live(snapshot, first_pane)
            && is_live(snapshot, second_pane)
            && is_live(snapshot, split_pane)
    })
    .await;
    assert_eq!(layout_shape(&reconnected, project_id), before_shape);
    assert_eq!(
        tab_sessions(project(&reconnected, project_id), second_tab),
        before_sessions
    );

    // 3. 停止 daemon 并重启：布局与 pane id 不变，会话身份更新。
    drop(conn);
    crossh_session::daemon_stop().await.unwrap();

    let addr = crossh_session::ensure_daemon().await.unwrap();
    let mut bare = Client::connect_to(&addr).await.unwrap();
    assert!(
        bare.list().await.unwrap().is_empty(),
        "restart must not materialize sessions before attach"
    );
    let restarted = bare.workspace_get().await.unwrap();
    assert_eq!(layout_shape(&restarted, project_id), before_shape);
    assert_eq!(project(&restarted, project_id).tabs.len(), 2);

    // 无 attach 客户端：新项目只改布局，pane 保持休眠。
    let reply = bare
        .workspace_mutate(WorkspaceMutateRequest {
            request_id: 1,
            expected_revision: None,
            op: WorkspaceOp::OpenProject {
                root: empty_project.clone(),
            },
        })
        .await
        .unwrap();
    assert_eq!(reply.outcome, MutateOutcome::Applied);
    let empty_id = reply.created.project.expect("empty project id");
    let empty_pane = reply.created.pane.expect("empty project pane");
    let dormant = bare.workspace_get().await.unwrap();
    assert!(matches!(
        find_pane(&dormant, empty_pane).unwrap().status,
        PaneStatus::Dormant { .. }
    ));
    assert_eq!(dormant.projects.len(), 2);
    drop(bare);

    // attach 只物化活跃 tab（此时是新开的空项目）。
    let conn = WorkspaceConnection::connect().await.unwrap();
    wait_for(&conn, "empty project pane live after attach", |snapshot| {
        is_live(snapshot, empty_pane)
    })
    .await;
    assert_eq!(project(&conn.current(), empty_id).tabs.len(), 1);

    // 4. 已有 attach 客户端：新项目立即物化；缺失根保持不可用且不 spawn。
    let attached = mutate(
        &conn,
        WorkspaceOp::OpenProject {
            root: attached_project.clone(),
        },
    )
    .await;
    let attached_id = attached.created.project.expect("attached project id");
    let attached_pane = attached.created.pane.expect("attached project pane");
    wait_for(&conn, "attached project pane live", |snapshot| {
        is_live(snapshot, attached_pane)
    })
    .await;
    assert_eq!(project(&conn.current(), attached_id).tabs.len(), 1);

    let missing = mutate(
        &conn,
        WorkspaceOp::OpenProject {
            root: missing_project.clone(),
        },
    )
    .await;
    let missing_id = missing.created.project.expect("missing project id");
    let missing_pane = missing.created.pane.expect("missing project pane");
    let unavailable = wait_for(&conn, "missing project unavailable", |snapshot| {
        snapshot
            .projects
            .iter()
            .any(|project| project.id == missing_id && !project.available)
    })
    .await;
    let missing_pane = find_pane(&unavailable, missing_pane).unwrap();
    assert!(matches!(
        missing_pane.status,
        PaneStatus::Unavailable { .. }
    ));
    assert_eq!(missing_pane.session, None);

    // 5. 重新激活原项目：活跃 tab 物化，后台 tab 保持休眠。
    mutate(&conn, WorkspaceOp::ActivateProject { project_id }).await;
    let active = wait_for(&conn, "project tabs materialize", |snapshot| {
        is_live(snapshot, second_pane) && is_live(snapshot, split_pane)
    })
    .await;
    let first_tab_pane = find_pane(&active, first_pane).unwrap();
    assert!(matches!(first_tab_pane.status, PaneStatus::Dormant { .. }));
    assert_eq!(first_tab_pane.session, None);

    let after_restart_sessions = tab_sessions(project(&active, project_id), second_tab);
    assert!(
        after_restart_sessions
            .iter()
            .all(|(pane, session)| session.is_some() && before_sessions[pane] != *session),
        "sessions must be recreated with new ids: {before_sessions:?} vs {after_restart_sessions:?}"
    );

    // 非活跃 tab 在 ActivateTab 后补齐物化。
    mutate(&conn, WorkspaceOp::ActivateTab { tab_id: first_tab }).await;
    let first_tab_live = wait_for(&conn, "first tab materialize", |snapshot| {
        is_live(snapshot, first_pane)
    })
    .await;
    let live_session = find_pane(&first_tab_live, first_pane)
        .unwrap()
        .session
        .clone()
        .expect("live session");

    // 6. shell `cd` 到子目录：pane cwd 更新，项目数量不变。
    let addr = crossh_session::instance::read_daemon_addr().unwrap();
    let mut session_client = Client::connect_to(&addr).await.unwrap();
    session_client.attach(&live_session).await.unwrap();
    session_client.input(b"cd sub\n").await.unwrap();

    let expected_cwd = dunce::canonicalize(&sub).unwrap();
    let moved = wait_for(&conn, "pane cwd updates after cd", |snapshot| {
        find_pane(snapshot, first_pane).is_some_and(|pane| {
            dunce::canonicalize(&pane.cwd).unwrap_or_else(|_| pane.cwd.clone()) == expected_cwd
        })
    })
    .await;
    assert_eq!(
        moved.projects.len(),
        4,
        "cd must not turn subdirectories into projects"
    );
    let canonical_root = dunce::canonicalize(&project_root).unwrap();
    assert_eq!(
        moved
            .projects
            .iter()
            .filter(|project| project.root == canonical_root)
            .count(),
        1
    );
    drop(session_client);

    // 7. 清理：停止 daemon 与临时目录。
    drop(conn);
    crossh_session::daemon_stop().await.unwrap();
    let _ = std::fs::remove_dir_all(&base);
}
