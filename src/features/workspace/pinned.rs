//! 本地会话固定记录的纯逻辑：身份分配与失效清理。
//!
//! 本模块无 gpui 依赖（Logic must not depend on UI）。`PinnedLocalTab`
//! 的数据结构定义在 `super::settings`（feature-owned settings），这里只
//! 承载生命周期语义，供启动恢复与运行时动作复用。

use std::path::Path;

use super::local_paths::normalize_local_cwd;
use super::settings::PinnedLocalTab;

/// 下一个可用 `pin_id`：现有记录 `max + 1`；空列表从 1 开始。
/// 分配后不回收，删除/取消固定不影响其他记录的身份。
pub(super) fn next_pin_id(tabs: &[PinnedLocalTab]) -> u64 {
    tabs.iter()
        .map(|tab| tab.pin_id)
        .max()
        .map_or(1, |max| max + 1)
}

/// 过滤出属于指定项目的固定记录，保持持久化列表顺序。
/// 项目归属显示（标签条）与项目归属恢复（启动/激活）共用此过滤。
pub(super) fn pinned_tabs_for_project<'a>(
    tabs: &'a [PinnedLocalTab],
    project_dir: &Path,
) -> Vec<&'a PinnedLocalTab> {
    tabs.iter()
        .filter(|tab| tab.project_dir == project_dir)
        .collect()
}

/// 过滤固定记录中已失效的目录（删除、改名、普通文件、不可访问），
/// 并把存活记录的路径规范化为绝对目录。返回结果与输入相等时表示
/// 没有需要清理的失效记录（调用方据此决定是否回写持久化）。
pub(super) fn prune_missing_pinned_tabs(tabs: Vec<PinnedLocalTab>) -> Vec<PinnedLocalTab> {
    tabs.into_iter()
        .filter_map(|mut tab| {
            tab.project_dir = normalize_local_cwd(tab.project_dir)?;
            tab.cwd = normalize_local_cwd(tab.cwd)?;
            Some(tab)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::{next_pin_id, pinned_tabs_for_project, prune_missing_pinned_tabs};
    use crate::features::workspace::settings::PinnedLocalTab;

    fn tab(pin_id: u64, project_dir: &str, cwd: &str) -> PinnedLocalTab {
        PinnedLocalTab {
            pin_id,
            project_dir: PathBuf::from(project_dir),
            cwd: PathBuf::from(cwd),
            custom_name: None,
            default_command: None,
        }
    }

    #[test]
    fn spec_20260818_local_tab_pin_next_id_starts_at_one_and_never_recycles() {
        assert_eq!(next_pin_id(&[]), 1);
        assert_eq!(next_pin_id(&[tab(3, "/a", "/a"), tab(7, "/b", "/b")]), 8);
        // 删除后不回收：max 仍按现存记录推导，不产生冲突。
        assert_eq!(next_pin_id(&[tab(5, "/c", "/c")]), 6);
        assert_eq!(next_pin_id(&[]), 1);
    }

    #[test]
    fn spec_20260818_local_tab_pin_project_filter_keeps_matching_records_in_order() {
        use std::path::Path;
        let tabs = vec![tab(1, "/a", "/a"), tab(2, "/b", "/b"), tab(3, "/a", "/a2")];
        let filtered = pinned_tabs_for_project(&tabs, Path::new("/a"));
        assert_eq!(
            filtered.iter().map(|t| t.pin_id).collect::<Vec<_>>(),
            vec![1, 3],
            "只保留匹配项目的记录且保持持久化顺序"
        );
        assert!(
            pinned_tabs_for_project(&tabs, Path::new("/c")).is_empty(),
            "无匹配记录返回空"
        );
    }

    #[test]
    fn spec_20260818_local_tab_pin_prune_keeps_existing_dirs_and_drops_missing() {
        let root = std::env::temp_dir().join(format!("crossh-pinned-prune-{}", std::process::id()));
        let existing = root.join("existing");
        let file = root.join("file");
        let missing = root.join("missing");
        std::fs::create_dir_all(&existing).expect("test directory should be created");
        std::fs::write(&file, b"not a directory").expect("test file should be created");

        let tabs = vec![
            tab(
                1,
                existing.to_string_lossy().as_ref(),
                existing.to_string_lossy().as_ref(),
            ),
            tab(
                2,
                file.to_string_lossy().as_ref(),
                file.to_string_lossy().as_ref(),
            ),
            tab(
                3,
                missing.to_string_lossy().as_ref(),
                missing.to_string_lossy().as_ref(),
            ),
        ];
        let pruned = prune_missing_pinned_tabs(tabs);
        assert_eq!(pruned.len(), 1);
        assert_eq!(pruned[0].pin_id, 1);
        assert_eq!(pruned[0].project_dir, existing.canonicalize().unwrap());
        assert_eq!(pruned[0].cwd, existing.canonicalize().unwrap());
    }

    #[test]
    fn spec_20260818_local_tab_pin_prune_preserves_order_and_blank_list() {
        let root =
            std::env::temp_dir().join(format!("crossh-pinned-prune-order-{}", std::process::id()));
        std::fs::create_dir_all(&root).expect("test directory should be created");
        let a = root.join("a");
        let c = root.join("c");
        std::fs::create_dir_all(&a).expect("test directory should be created");
        std::fs::create_dir_all(&c).expect("test directory should be created");

        let tabs = vec![
            tab(
                1,
                a.to_string_lossy().as_ref(),
                a.to_string_lossy().as_ref(),
            ),
            tab(
                2,
                root.join("missing").to_string_lossy().as_ref(),
                root.join("missing").to_string_lossy().as_ref(),
            ),
            tab(
                3,
                c.to_string_lossy().as_ref(),
                c.to_string_lossy().as_ref(),
            ),
        ];
        let pruned = prune_missing_pinned_tabs(tabs);
        assert_eq!(
            pruned.iter().map(|t| t.pin_id).collect::<Vec<_>>(),
            vec![1, 3],
            "清理后保持原顺序"
        );
        assert!(prune_missing_pinned_tabs(Vec::new()).is_empty());
    }
}
