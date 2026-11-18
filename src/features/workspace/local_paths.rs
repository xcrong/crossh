//! 本地 workspace 路径的有效性与规范化。

use std::path::PathBuf;

pub(super) fn current_local_cwd() -> PathBuf {
    normalize_local_cwd(std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/")))
        .unwrap_or_else(|| PathBuf::from("/"))
}

/// 只返回当前仍可作为本地工作目录使用的规范化路径。
pub(super) fn normalize_local_cwd(path: PathBuf) -> Option<PathBuf> {
    let path = if path.is_absolute() {
        path
    } else {
        std::env::current_dir().ok()?.join(path)
    };
    if !path.is_dir() {
        return None;
    }
    // dunce::canonicalize 在 Windows 上去掉 \\?\ 扩展路径前缀，
    // 避免传给子进程 cwd 后 PowerShell prompt 显示丑陋的完整 Provider 路径。
    let path = dunce::canonicalize(path).ok()?;
    path.is_dir().then_some(path)
}

#[cfg(test)]
mod tests {
    use super::normalize_local_cwd;

    #[test]
    fn spec_20260817_recent_local_dir_recovery_missing_path_is_rejected() {
        let missing =
            std::env::temp_dir().join(format!("crossh-missing-local-dir-{}", std::process::id()));
        assert_eq!(normalize_local_cwd(missing), None);
    }
}
