//! 工作区纯状态类型：LocalSession / 目录聚合 / 状态优先级。
//!
//! 与 `view.rs` 的渲染分离：本模块零 `AppShell` 依赖，可被 `registry` 与 `view`
//! 同时引用而不形成循环。`rebuild_local_dirs` / `preferred_state` 等纯函数
//! 也归于此，保持 `view.rs` 专注于 GPUI 渲染。

use std::collections::BTreeMap;
use std::path::PathBuf;

use gpui::Entity;

use crate::features::terminal::TerminalView;
use crossh_core::git_status::GitStatus;
use crossh_terminal::ConnState;

pub type LocalSessionId = u64;

/// 当前主区正在展示的工作区。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum ActiveView {
    LocalSession(LocalSessionId),
}

pub struct LocalSession {
    /// 创建会话时所属的项目目录；shell 内 `cd` 不会改变它。
    pub project_dir: PathBuf,
    /// shell 当前工作目录；可以独立于项目归属变化。
    pub cwd: PathBuf,
    pub terminal: Entity<TerminalView>,
    pub git_status: Option<GitStatus>,
    pub git_refresh: GitStatusRefresh,
    /// 固定状态：持久化 `pin_id`（`Some` 即固定，与设置中记录一一对应）。
    pub pin_id: Option<u64>,
    /// 固定标签的自定义名称；覆盖终端标题显示，重启后保留。
    pub custom_name: Option<String>,
    /// 固定标签的默认命令；`Some` 时恢复/重载自动执行。
    pub default_command: Option<String>,
}

/// 每个本地会话最多运行一个 Git 状态查询；期间的刷新请求只合并为一次后续检查。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GitStatusRefresh {
    in_flight: bool,
    pending: bool,
}

impl GitStatusRefresh {
    pub fn request(&mut self) -> bool {
        if self.in_flight {
            self.pending = true;
            false
        } else {
            self.in_flight = true;
            true
        }
    }

    pub fn finish(&mut self) -> bool {
        self.in_flight = false;
        std::mem::take(&mut self.pending)
    }
}

pub struct LocalDir {
    /// 侧栏分组对应的项目目录。
    pub project_dir: PathBuf,
    pub sessions: Vec<LocalSessionId>,
    pub active_session: Option<LocalSessionId>,
}

/// 把 `from` 处的元素移到 `to` 之前；同位/越界不动，调用方先定位下标。
/// 标签拖拽落点语义：落到哪个标签上就排到它之前。
// ponytail: drop-onto 语义；要左右半区精确插入时再加几何判断。
pub fn move_before<T>(items: &mut Vec<T>, from: usize, to: usize) {
    if from == to || from >= items.len() || to >= items.len() {
        return;
    }
    let item = items.remove(from);
    // `remove` 让后方前移：`from` 在前时 `to` 回退一位，保证落在 target 之前。
    let to = if from < to { to - 1 } else { to };
    items.insert(to, item);
}

/// 把会话按项目归属目录重建目录视图：同一项目的会话合并，保留上一次的活动会话。
/// `remembered` 是最近打开过的本地目录（无活动会话），合并进来后仍显示在侧栏。
pub fn rebuild_local_dirs(
    previous: &BTreeMap<PathBuf, LocalDir>,
    sessions: impl IntoIterator<Item = (LocalSessionId, PathBuf)>,
    remembered: impl IntoIterator<Item = PathBuf>,
    active_local_session: Option<LocalSessionId>,
) -> BTreeMap<PathBuf, LocalDir> {
    let mut next = BTreeMap::new();
    for project_dir in remembered {
        if !project_dir.is_dir() {
            continue;
        }
        next.entry(project_dir.clone()).or_insert_with(|| LocalDir {
            project_dir,
            sessions: Vec::new(),
            active_session: None,
        });
    }
    for (session_id, project_dir) in sessions {
        next.entry(project_dir.clone())
            .or_insert_with(|| LocalDir {
                project_dir,
                sessions: Vec::new(),
                active_session: None,
            })
            .sessions
            .push(session_id);
    }

    for (project_dir, dir) in &mut next {
        // 拖拽排序后 `sessions` 自身即顺序：保留上一次的相对顺序，新会话追加末尾，
        // 否则打开/关闭/`cd` 触发的重建会打乱拖拽结果。
        // ponytail: O(n²) contains，标签数极小；量大再换索引集。
        if let Some(old) = previous.get(project_dir) {
            let mut ordered: Vec<LocalSessionId> = old
                .sessions
                .iter()
                .filter(|id| dir.sessions.contains(id))
                .copied()
                .collect();
            for id in dir.sessions.iter() {
                if !ordered.contains(id) {
                    ordered.push(*id);
                }
            }
            dir.sessions = ordered;
        }
        let previous_active = previous.get(project_dir).and_then(|old| old.active_session);
        dir.active_session = active_local_session
            .filter(|id| dir.sessions.contains(id))
            .or_else(|| previous_active.filter(|id| dir.sessions.contains(id)))
            .or_else(|| dir.sessions.first().copied());
    }
    next
}

/// 多会话目录取「最活跃」的状态用于侧栏/徽标。
pub fn preferred_state(left: ConnState, right: ConnState) -> ConnState {
    if state_priority(&right) > state_priority(&left) {
        right
    } else {
        left
    }
}

fn state_priority(state: &ConnState) -> u8 {
    match state {
        ConnState::Connected => 4,
        ConnState::Connecting => 3,
        ConnState::Error(_) => 2,
        ConnState::Closed => 1,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    #[test]
    fn git_status_refresh_coalesces_overlapping_requests() {
        let mut refresh = GitStatusRefresh::default();
        assert!(refresh.request());
        assert!(!refresh.request());
        assert!(!refresh.request());
        assert!(refresh.finish());
        assert!(refresh.request());
        assert!(!refresh.finish());
    }

    #[test]
    fn project_directories_keep_sessions_isolated() {
        let previous = BTreeMap::from([
            (
                PathBuf::from("/Users/me/one"),
                LocalDir {
                    project_dir: PathBuf::from("/Users/me/one"),
                    sessions: vec![1, 2],
                    active_session: Some(2),
                },
            ),
            (
                PathBuf::from("/Users/me/two"),
                LocalDir {
                    project_dir: PathBuf::from("/Users/me/two"),
                    sessions: vec![3],
                    active_session: Some(3),
                },
            ),
        ]);
        let current = vec![
            (1, PathBuf::from("/Users/me/one")),
            (2, PathBuf::from("/Users/me/two")),
            (3, PathBuf::from("/Users/me/two")),
        ];
        let dirs = rebuild_local_dirs(&previous, current, Vec::new(), Some(2));
        assert_eq!(dirs[&PathBuf::from("/Users/me/one")].sessions, vec![1]);
        assert_eq!(
            dirs[&PathBuf::from("/Users/me/one")].active_session,
            Some(1)
        );
        // 新来者（2）追加到存量（3）之后：重建保留上一次的相对顺序。
        assert_eq!(dirs[&PathBuf::from("/Users/me/two")].sessions, vec![3, 2]);
        assert_eq!(
            dirs[&PathBuf::from("/Users/me/two")].active_session,
            Some(2)
        );
    }
    #[test]
    fn move_before_drops_onto_target_and_ignores_bad_positions() {
        let mut ids = vec![1, 2, 3, 4];
        move_before(&mut ids, 0, 2);
        assert_eq!(ids, vec![2, 1, 3, 4]);
        move_before(&mut ids, 3, 1);
        assert_eq!(ids, vec![2, 4, 1, 3]);
        move_before(&mut ids, 1, 1);
        move_before(&mut ids, 0, 9);
        assert_eq!(ids, vec![2, 4, 1, 3]);
    }

    #[test]
    fn rebuild_keeps_drag_order_for_surviving_sessions() {
        let previous = BTreeMap::from([(
            PathBuf::from("/p"),
            LocalDir {
                project_dir: PathBuf::from("/p"),
                sessions: vec![3, 1, 2],
                active_session: Some(1),
            },
        )]);
        let dirs = rebuild_local_dirs(
            &previous,
            vec![
                (1, PathBuf::from("/p")),
                (2, PathBuf::from("/p")),
                (3, PathBuf::from("/p")),
                (4, PathBuf::from("/p")),
            ],
            Vec::new(),
            Some(1),
        );
        assert_eq!(dirs[&PathBuf::from("/p")].sessions, vec![3, 1, 2, 4]);
    }

    #[test]
    fn project_group_does_not_follow_a_session_cwd() {
        let project_dir = PathBuf::from("/Users/me/one");
        let dirs = rebuild_local_dirs(
            &BTreeMap::new(),
            vec![(1, project_dir.clone()), (2, project_dir.clone())],
            Vec::new(),
            Some(2),
        );
        assert_eq!(dirs[&project_dir].sessions, vec![1, 2]);
        assert_eq!(dirs[&project_dir].active_session, Some(2));
        assert!(!dirs.contains_key(&PathBuf::from("/Users/me/two")));
    }

    #[test]
    fn remembered_dirs_stay_without_live_sessions() {
        let previous = BTreeMap::new();
        let root =
            std::env::temp_dir().join(format!("crossh-remembered-dir-test-{}", std::process::id()));
        let one = root.join("one");
        let two = root.join("two");
        std::fs::create_dir_all(&one).expect("first test directory should be created");
        std::fs::create_dir_all(&two).expect("second test directory should be created");
        let remembered = vec![one.clone(), two.clone()];
        let current = vec![(1, one.clone())];
        let dirs = rebuild_local_dirs(&previous, current, remembered, Some(1));
        assert_eq!(dirs[&one].sessions, vec![1]);
        assert_eq!(dirs[&two].sessions, Vec::<LocalSessionId>::new());
        assert_eq!(dirs[&two].active_session, None);
        std::fs::remove_dir_all(root).expect("test directory should be removed");
    }

    #[test]
    fn spec_20260817_recent_local_dir_recovery_missing_remembered_dir_is_not_restored() {
        let root =
            std::env::temp_dir().join(format!("crossh-recent-dir-recovery-{}", std::process::id()));
        let existing = root.join("existing");
        let missing = root.join("missing");
        std::fs::create_dir_all(&existing).expect("test directory should be created");
        let dirs = rebuild_local_dirs(
            &BTreeMap::new(),
            Vec::new(),
            vec![existing.clone(), missing.clone()],
            None,
        );
        assert!(dirs.contains_key(&existing));
        assert!(!dirs.contains_key(&missing));
        std::fs::remove_dir_all(root).expect("test directory should be removed");
    }

    #[test]
    fn project_group_prefers_a_live_session_state() {
        assert_eq!(
            preferred_state(ConnState::Closed, ConnState::Connecting),
            ConnState::Connecting
        );
        assert_eq!(
            preferred_state(ConnState::Connecting, ConnState::Connected),
            ConnState::Connected
        );
        assert_eq!(
            preferred_state(ConnState::Connected, ConnState::Error("failed".into())),
            ConnState::Connected
        );
    }
}
