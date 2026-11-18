//! 本地系统相关配置与工具。
//!
//! crossh 的配置根只有一个解析器：
//!
//! - `CROSSH_CONFIG_DIR` 的值**直接就是**配置根（debug/release 都不加后缀）；
//! - 未设置时取 `home_dir()/.config/crossh[-dev]`（debug 加 `-dev`）；
//! - settings 新写入落点是 `settings.toml`，读取允许回退到旧版
//!   `~/.config/crossh/settings.toml`；
//! - 工作区文件是 `daemon/workspace.json`。
//!
//! 不使用 `dirs::config_dir()`：macOS 会返回 `~/Library/Application Support`，
//! 与现有文档、用户预期和旧 settings 路径不一致。

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

/// 配置根环境变量。值直接作为根，不再拼接 `crossh`。
pub const CONFIG_DIR_ENV: &str = "CROSSH_CONFIG_DIR";

/// 由显式输入解析配置根（便于单元测试）。
///
/// `home` 缺失且未设置 `CROSSH_CONFIG_DIR` 时返回 `None`；`config_dir`
/// 为空字符串视为未设置。
pub fn config_root_with(
    home: Option<&Path>,
    config_dir: Option<&OsStr>,
    debug: bool,
) -> Option<PathBuf> {
    if let Some(dir) = config_dir.filter(|value| !value.is_empty()) {
        return Some(PathBuf::from(dir));
    }
    let home = home?;
    let suffix = if debug { "-dev" } else { "" };
    Some(home.join(".config").join(format!("crossh{suffix}")))
}

/// 当前进程的配置根。
pub fn config_root() -> Option<PathBuf> {
    let home = dirs::home_dir();
    let env = std::env::var_os(CONFIG_DIR_ENV);
    config_root_with(home.as_deref(), env.as_deref(), cfg!(debug_assertions))
}

/// 旧版 settings 的固定路径（只读回退与迁移源）。
pub fn legacy_settings_path_with(home: &Path) -> PathBuf {
    home.join(".config").join("crossh").join("settings.toml")
}

/// 旧版 settings 路径；无 home 时为 `None`。
pub fn legacy_settings_path() -> Option<PathBuf> {
    dirs::home_dir().map(|home| legacy_settings_path_with(&home))
}

/// 新版 settings 的唯一写入落点。
pub fn settings_path() -> Option<PathBuf> {
    config_root().map(|root| root.join("settings.toml"))
}

/// settings 读取路径：新落点存在时用它；否则在未设置
/// `CROSSH_CONFIG_DIR` 且旧文件存在时只读回退到旧路径。
pub fn settings_read_path() -> Option<PathBuf> {
    let write = settings_path()?;
    if write.is_file() {
        return Some(write);
    }
    if std::env::var_os(CONFIG_DIR_ENV).is_some_and(|value| !value.is_empty()) {
        return Some(write);
    }
    legacy_settings_path()
        .filter(|path| path.is_file())
        .or(Some(write))
}

/// 工作区文件路径。
pub fn workspace_path() -> Option<PathBuf> {
    config_root().map(|root| root.join("daemon").join("workspace.json"))
}

/// `daemon.error` 路径（与 `workspace.json` 同目录）。
pub fn daemon_error_path() -> Option<PathBuf> {
    config_root().map(|root| root.join("daemon").join("daemon.error"))
}

/// Unix 下把目录权限收敛为 `0700` / 文件为 `0600`。
#[cfg(unix)]
pub fn restrict_dir(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
}

#[cfg(not(unix))]
pub fn restrict_dir(_path: &Path) -> std::io::Result<()> {
    Ok(())
}

/// Unix 下把文件权限收敛为 `0600`。
#[cfg(unix)]
pub fn restrict_file(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
}

#[cfg(not(unix))]
pub fn restrict_file(_path: &Path) -> std::io::Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_config_dir_is_the_root_without_extra_crossh() {
        let root = config_root_with(
            Some(Path::new("/home/me")),
            Some(OsStr::new("/tmp/cfg")),
            false,
        )
        .unwrap();
        assert_eq!(root, PathBuf::from("/tmp/cfg"));
        assert_eq!(
            root.join("settings.toml"),
            PathBuf::from("/tmp/cfg/settings.toml")
        );
        assert_eq!(
            root.join("daemon").join("workspace.json"),
            PathBuf::from("/tmp/cfg/daemon/workspace.json")
        );
    }

    #[test]
    fn default_root_uses_home_and_debug_suffix() {
        let debug = config_root_with(Some(Path::new("/home/me")), None, true).unwrap();
        let release = config_root_with(Some(Path::new("/home/me")), None, false).unwrap();
        assert_eq!(debug, PathBuf::from("/home/me/.config/crossh-dev"));
        assert_eq!(release, PathBuf::from("/home/me/.config/crossh"));
        assert_eq!(
            debug.join("settings.toml"),
            PathBuf::from("/home/me/.config/crossh-dev/settings.toml")
        );
    }

    #[test]
    fn empty_env_is_treated_as_unset() {
        let root =
            config_root_with(Some(Path::new("/home/me")), Some(OsStr::new("")), false).unwrap();
        assert_eq!(root, PathBuf::from("/home/me/.config/crossh"));
    }

    #[test]
    fn missing_home_and_env_yields_none() {
        assert_eq!(config_root_with(None, None, false), None);
    }
}
