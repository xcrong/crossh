//! 本地系统相关配置与工具。
//!
//! crossh 的配置根只有一个解析器：
//!
//! - `CROSSH_CONFIG_DIR` 的值**直接就是**配置根（debug/release 都不加后缀），
//!   须为绝对路径；相对路径按进程当前工作目录解析后做词法规范化
//!   （会随 cwd 漂移，仅为兼容兜底，请改用绝对路径）；
//! - 未设置时取 `home_dir()/.config/crossh[-dev]`（debug 加 `-dev`）；
//! - settings 新写入落点是 `settings.toml`，读取允许回退到旧版
//!   `~/.config/crossh/settings.toml`；
//! - 工作区文件是 `daemon/workspace.json`。
//!
//! 权限说明：Unix 下目录收敛为 `0700` / 文件为 `0600`；
//! Windows 下 `restrict_dir` / `restrict_file` 目前是空操作，
//! 不显式设置 DACL（与旧行为一致，仅文档化，不在此做 ACL）。
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
/// 为空字符串视为未设置。`config_dir` 须为绝对路径并做词法规范化
/// （`.` / `..` / 重复分隔符，不触碰文件系统）；相对路径按
/// `std::env::current_dir()` 解析后规范化并打印警告（会随 cwd 漂移，
/// 仅为兼容兜底）。
pub fn config_root_with(
    home: Option<&Path>,
    config_dir: Option<&OsStr>,
    debug: bool,
) -> Option<PathBuf> {
    if let Some(dir) = config_dir.filter(|value| !value.is_empty()) {
        let raw = PathBuf::from(dir);
        if raw.is_absolute() {
            return Some(normalize_lexically(&raw));
        }
        let absolute = match std::env::current_dir() {
            Ok(cwd) => cwd.join(&raw),
            Err(_) => raw.clone(),
        };
        let normalized = normalize_lexically(&absolute);
        eprintln!(
            "[crossh] {} 使用相对路径 {:?}，已按当前工作目录解析为 {:?}，请改用绝对路径",
            CONFIG_DIR_ENV, raw, normalized
        );
        return Some(normalized);
    }
    let home = home?;
    let suffix = if debug { "-dev" } else { "" };
    Some(home.join(".config").join(format!("crossh{suffix}")))
}

/// 词法规范化路径：消解 `.` / `..` / 重复分隔符，不访问文件系统。
/// `..` 超出根时停在根上；空结果回退为 `.`。
fn normalize_lexically(path: &Path) -> PathBuf {
    use std::path::Component;
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            _ => out.push(component.as_os_str()),
        }
    }
    if out.as_os_str().is_empty() {
        PathBuf::from(".")
    } else {
        out
    }
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

/// 非 Unix 平台：空操作，不显式设置 Windows DACL（见模块文档）。
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

/// 非 Unix 平台：空操作，不显式设置 Windows DACL（见模块文档）。
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

    #[test]
    fn absolute_env_is_normalized_lexically() {
        let root = config_root_with(
            Some(Path::new("/home/me")),
            Some(OsStr::new("/tmp/a/../cfg")),
            false,
        )
        .unwrap();
        assert_eq!(root, PathBuf::from("/tmp/cfg"));
        let dotted = config_root_with(
            Some(Path::new("/home/me")),
            Some(OsStr::new("/tmp/./cfg")),
            false,
        )
        .unwrap();
        assert_eq!(dotted, PathBuf::from("/tmp/cfg"));
    }

    #[test]
    fn relative_env_resolves_against_cwd_to_absolute() {
        let root = config_root_with(
            Some(Path::new("/home/me")),
            Some(OsStr::new("relative-root/../cfg")),
            false,
        )
        .unwrap();
        assert!(root.is_absolute());
        assert_eq!(
            root,
            normalize_lexically(&std::env::current_dir().unwrap().join("cfg"))
        );
    }
}
