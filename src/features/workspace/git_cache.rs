//! 底部 Git 状态的进程级共享缓存：以 canonical cwd 为 key 去重与广播。
//!
//! 每个窗口的 `AppShell` 各自持有按 `PaneId` 的 `git_status` 映射做渲染，
//! 跨窗口/跨分屏的收敛靠这里：同一 cwd 同时只 fork 一个 `git status` 子进程，
//! 完成后写回缓存并通知所有 `observe_global` 的窗口各自扇出更新。

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use gpui::{App, Global};

use crossh_core::git_status::GitStatus;

/// 同一 cwd 的共享 Git 状态；`None` 表示已探明非仓库（同样要广播，避免反复 fork）。
#[derive(Default)]
pub(crate) struct GitStatusCache {
    entries: BTreeMap<PathBuf, Option<GitStatus>>,
    in_flight: BTreeSet<PathBuf>,
}

impl Global for GitStatusCache {}

impl GitStatusCache {
    /// 不同窗口可能拿到 `/tmp` 与 `/private/tmp` 两种写法，canonicalize 后再当 key。
    /// 路径已删除等极端情况回退原文，宁可多 fork 一次也不丢更新。
    pub(crate) fn canonical_key(cwd: &Path) -> PathBuf {
        dunce::canonicalize(cwd).unwrap_or_else(|_| cwd.to_path_buf())
    }

    pub(crate) fn ensure(cx: &mut App) {
        if !cx.has_global::<Self>() {
            cx.set_global(Self::default());
        }
    }

    /// 已落盘的共享结果；外层 `None` 表示还没人查过。
    pub(crate) fn cached(cx: &App, key: &Path) -> Option<Option<GitStatus>> {
        if !cx.has_global::<Self>() {
            return None;
        }
        cx.global::<Self>().entries.get(key).cloned()
    }

    pub(crate) fn is_in_flight(cx: &App, key: &Path) -> bool {
        cx.has_global::<Self>() && cx.global::<Self>().in_flight.contains(key)
    }

    /// 抢到返回 true（调用方负责 fork）；抢不到说明已有在途请求，等广播即可。
    pub(crate) fn try_start(cx: &mut App, key: PathBuf) -> bool {
        Self::ensure(cx);
        cx.global_mut::<Self>().in_flight.insert(key)
    }

    /// 落盘并释放占位；`global_mut` 自带全局通知，各窗口的 `observe_global` 会被唤醒。
    pub(crate) fn complete(cx: &mut App, key: &Path, status: Option<GitStatus>) {
        Self::ensure(cx);
        let cache = cx.global_mut::<Self>();
        cache.in_flight.remove(key);
        cache.entries.insert(key.to_path_buf(), status);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[gpui::test]
    fn same_cwd_shares_one_flight_slot(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            let same = PathBuf::from("/repo");
            let other = PathBuf::from("/other");
            assert!(GitStatusCache::try_start(cx, same.clone()));
            // 同一 cwd 第二次抢不到：在途合并，不再 fork。
            assert!(!GitStatusCache::try_start(cx, same.clone()));
            // 不同 cwd 互不干扰。
            assert!(GitStatusCache::try_start(cx, other.clone()));
            assert!(GitStatusCache::is_in_flight(cx, &same));

            GitStatusCache::complete(cx, &same, None);
            assert!(!GitStatusCache::is_in_flight(cx, &same));
            // 非仓库的 `None` 同样落盘广播，下次 observer 直接复用。
            assert_eq!(GitStatusCache::cached(cx, &same), Some(None));
            // 没查过的路径返回外层 `None`，调用方照常 fork。
            assert_eq!(GitStatusCache::cached(cx, &PathBuf::from("/unknown")), None);
        });
    }

    #[test]
    fn canonical_key_falls_back_when_path_is_gone() {
        let missing = PathBuf::from("/crossh-definitely-missing-dir-12345");
        assert_eq!(GitStatusCache::canonical_key(&missing), missing);
    }
}
