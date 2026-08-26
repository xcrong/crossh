//! 外部编辑器的检测与独立进程启动（纯逻辑，零 GPUI 依赖）。
//!
//! 与 `git_launcher.rs` 同级：保持轻量，只承载「解析出哪个编辑器命令」、
//! 「枚举本机已安装编辑器」和「构造分离进程的 Command」三部分可测逻辑，
//! GPUI 只出现在调用点。检测候选列表是写死的代码常量，不可被设置覆盖。

use std::collections::HashSet;
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::OnceLock;

/// macOS GUI 进程（dock/访达启动）继承的 PATH 仅为 `/usr/bin:/bin:/usr/sbin:/sbin`，
/// 导致 Homebrew 安装的 `zed`/`code`（位于 `/opt/homebrew/bin`、`/usr/local/bin`）
/// 无法被 `detect_editors` 发现。额外探查登录 shell 与固定回退目录以补全。
#[cfg(unix)]
const FALLBACK_PATH_DIRS: &[&str] = &[
    "/opt/homebrew/bin",
    "/usr/local/bin",
    "/opt/homebrew/sbin",
    "/usr/local/sbin",
];
#[cfg(windows)]
const FALLBACK_PATH_DIRS: &[&str] = &[];

static CACHED_LOGIN_SHELL_PATH: OnceLock<Option<OsString>> = OnceLock::new();

/// 合并 `env_path`、登录 shell 的 PATH 与固定回退目录，去重且保持顺序：
/// env 优先，其次 shell，最后回退。纯函数便于测试注入。
fn merge_paths(env_path: &OsStr, shell_path: Option<&OsStr>) -> OsString {
    let mut seen = HashSet::new();
    let mut dirs: Vec<PathBuf> = Vec::new();
    for dir in std::env::split_paths(env_path) {
        if seen.insert(dir.clone()) {
            dirs.push(dir);
        }
    }
    if let Some(shell) = shell_path {
        for dir in std::env::split_paths(shell) {
            if seen.insert(dir.clone()) {
                dirs.push(dir);
            }
        }
    }
    for fallback in FALLBACK_PATH_DIRS {
        let pb = PathBuf::from(fallback);
        if seen.insert(pb.clone()) {
            dirs.push(pb);
        }
    }
    std::env::join_paths(dirs).unwrap_or_default()
}

#[cfg(unix)]
#[allow(clippy::collapsible_if)]
fn login_shell_path() -> Option<OsString> {
    let mut candidates: Vec<OsString> = Vec::new();
    if let Some(shell) = std::env::var_os("SHELL")
        && !shell.is_empty()
    {
        candidates.push(shell);
    }
    candidates.push(OsString::from("/bin/zsh"));
    candidates.push(OsString::from("/bin/bash"));
    for shell in candidates {
        let output = Command::new(&shell)
            .arg("-l")
            .arg("-c")
            .arg("printf '%s' \"$PATH\"")
            .output();
        if let Ok(out) = output
            && out.status.success()
            && let Ok(s) = String::from_utf8(out.stdout)
        {
            let trimmed = s.trim().to_string();
            if !trimmed.is_empty() {
                return Some(OsString::from(trimmed));
            }
        }
    }
    None
}

#[cfg(windows)]
fn login_shell_path() -> Option<OsString> {
    None
}

fn cached_login_shell_path() -> Option<OsString> {
    CACHED_LOGIN_SHELL_PATH
        .get_or_init(login_shell_path)
        .clone()
}

/// 对外暴露的合并后 PATH，自动补全 shell 与回退目录。
/// 调用方（设置下拉、tooltip、启动）应使用此函数而非直接 `var_os("PATH")`。
pub(crate) fn effective_path() -> OsString {
    let env_path = std::env::var_os("PATH").unwrap_or_default();
    let shell = cached_login_shell_path();
    merge_paths(&env_path, shell.as_deref())
}

/// 自动检测候选的默认顺序：第一项 `zed`，随后是常用编辑器命令名。
/// 该列表是程序默认值，写死在代码中，不暴露为设置项（见
/// docs/specs/20260820-open-project-in-editor.md）。
pub(crate) const DEFAULT_EDITOR_PRIORITY: &[&str] = &[
    "zed",
    "code",
    "code-insiders",
    "cursor",
    "windsurf",
    "idea",
    "pycharm",
    "webstorm",
    "rider",
    "goland",
    "clion",
    "rubymine",
    "datagrip",
    "subl",
    "mate",
    "xed",
];

/// 枚举 PATH 中按默认候选顺序实际检测到的编辑器：每个候选在其第一个
/// 命中的 PATH 目录处解析为完整路径，整体保持候选顺序、天然无重复。
///
/// `exists` 是平台相关的可执行判定，注入以便纯逻辑测试。
pub(crate) fn detect_editors(path_env: &OsStr, exists: impl Fn(&Path) -> bool) -> Vec<String> {
    DEFAULT_EDITOR_PRIORITY
        .iter()
        .filter_map(|candidate| {
            for directory in std::env::split_paths(path_env) {
                let path = directory.join(candidate);
                if exists(&path) {
                    return Some(path.to_string_lossy().into_owned());
                }
            }
            None
        })
        .collect()
}

/// 解析实际使用的编辑器命令。
///
/// - `configured` 非空白时直接采用（不参与检测，也不做存在性校验，
///   由 spawn 失败错误提示引导用户修正配置）；
/// - 否则按 [`DEFAULT_EDITOR_PRIORITY`] 候选顺序，在 `path_env` 的每个
///   PATH 目录中查找第一个存在且可执行的文件，返回其完整路径。
///
/// `exists` 是平台相关的可执行判定，注入以便纯逻辑测试。
pub(crate) fn resolve_editor(
    configured: Option<&str>,
    path_env: &OsStr,
    exists: impl Fn(&Path) -> bool,
) -> Option<String> {
    if let Some(command) = configured.map(str::trim).filter(|c| !c.is_empty()) {
        return Some(command.to_string());
    }
    detect_editors(path_env, exists).into_iter().next()
}

/// 命令的展示名：优先取 basename，退化时显示完整值（tooltip / 下拉框共用）。
pub(crate) fn command_display_name(command: &str) -> String {
    Path::new(command)
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| command.to_string())
}

/// 平台相关的可执行判定（Unix 检查可执行位；Windows 检查 PATHEXT 扩展名）。
pub(crate) fn executable_exists(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(path)
            .is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
    }
    #[cfg(windows)]
    {
        if !path.is_file() {
            return false;
        }
        let Some(extension) = path.extension().and_then(|ext| ext.to_str()) else {
            return false;
        };
        let pathext = std::env::var("PATHEXT").unwrap_or_else(|_| ".EXE;.CMD;.BAT;.COM".into());
        pathext
            .split(';')
            .any(|ext| ext.trim_start_matches('.').eq_ignore_ascii_case(extension))
    }
}

#[cfg(windows)]
fn is_batch_binary(binary: &str) -> bool {
    let lower = binary.to_ascii_lowercase();
    lower.ends_with(".cmd") || lower.ends_with(".bat")
}

/// 构造「以 `directory` 为参数与工作目录」的分离进程命令。
/// 命令本身不执行；Windows 上 `.cmd`/`.bat` 批处理经 `cmd /C` 包装。
pub(crate) fn editor_process_command(binary: &str, directory: &Path) -> Command {
    #[cfg(windows)]
    let mut command = if is_batch_binary(binary) {
        let mut command = Command::new("cmd");
        command.arg("/C").arg(binary).arg(directory);
        command
    } else {
        let mut command = Command::new(binary);
        command.arg(directory);
        command
    };
    #[cfg(not(windows))]
    let mut command = {
        let mut command = Command::new(binary);
        command.arg(directory);
        command
    };

    command
        .current_dir(directory)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());

    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }

    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;

        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        command.creation_flags(CREATE_NEW_PROCESS_GROUP | DETACHED_PROCESS);
    }

    command
}

#[cfg(test)]
mod tests {
    use std::ffi::OsStr;
    use std::path::{Path, PathBuf};

    use super::{
        DEFAULT_EDITOR_PRIORITY, command_display_name, detect_editors, editor_process_command,
        resolve_editor,
    };

    fn exists_named<'a>(names: &'a [&'a str]) -> impl Fn(&Path) -> bool + 'a {
        move |path| names.iter().any(|name| path.ends_with(name))
    }

    #[test]
    fn spec_20260820_open_project_in_editor_configured_command_wins_over_detection() {
        let result = resolve_editor(Some("my-editor"), OsStr::new("/nonexistent"), |_| false);
        assert_eq!(result.as_deref(), Some("my-editor"));
    }

    #[test]
    fn spec_20260820_open_project_in_editor_blank_configured_command_falls_back_to_detection() {
        let result = resolve_editor(
            Some("   "),
            OsStr::new("/usr/bin"),
            exists_named(&["zed", "code"]),
        );
        assert_eq!(result.as_deref(), Some("/usr/bin/zed"));
    }

    #[test]
    fn spec_20260820_open_project_in_editor_default_order_decides_first_hit() {
        let result = resolve_editor(None, OsStr::new("/usr/bin"), exists_named(&["code", "zed"]));
        assert_eq!(
            result.as_deref(),
            Some("/usr/bin/zed"),
            "默认列表第一项 zed 应优先命中"
        );
        assert_eq!(DEFAULT_EDITOR_PRIORITY[0], "zed");
        assert!(
            DEFAULT_EDITOR_PRIORITY.contains(&"code")
                && DEFAULT_EDITOR_PRIORITY.contains(&"cursor")
        );
    }

    #[test]
    fn spec_20260820_open_project_in_editor_path_directory_order_resolves_same_candidate() {
        let result = resolve_editor(
            None,
            OsStr::new("/opt/bin:/usr/local/bin"),
            exists_named(&["zed"]),
        );
        assert_eq!(result.as_deref(), Some("/opt/bin/zed"));
    }

    #[test]
    fn spec_20260820_open_project_in_editor_no_executable_anywhere_returns_none() {
        let result = resolve_editor(None, OsStr::new("/usr/bin"), |_| false);
        assert_eq!(result, None);
    }

    #[test]
    fn spec_20260820_open_project_in_editor_detect_lists_all_found_editors_in_default_order() {
        let detected = detect_editors(
            OsStr::new("/opt/bin:/usr/local/bin"),
            exists_named(&["code", "cursor", "subl"]),
        );
        assert_eq!(
            detected,
            ["/opt/bin/code", "/opt/bin/cursor", "/opt/bin/subl",],
            "缺 zed 时按候选顺序列出全部命中项"
        );
    }

    #[test]
    fn spec_20260820_open_project_in_editor_detect_deduplicates_across_path_dirs() {
        let detected = detect_editors(OsStr::new("/a:/b:/c"), exists_named(&["code"]));
        assert_eq!(detected, ["/a/code"], "同一候选只取首个命中的 PATH 目录");
    }

    #[test]
    fn spec_20260820_open_project_in_editor_detect_empty_when_nothing_is_installed() {
        let detected = detect_editors(OsStr::new("/usr/bin"), |_| false);
        assert!(detected.is_empty());
    }

    #[test]
    fn spec_20260820_open_project_in_editor_display_name_prefers_basename() {
        assert_eq!(command_display_name("/usr/local/bin/zed"), "zed");
        assert_eq!(command_display_name("code"), "code");
        assert_eq!(
            command_display_name("/用户/编辑器/我的 IDE.app/bin/xed"),
            "xed"
        );
    }

    #[test]
    fn spec_20260820_open_project_in_editor_launch_command_targets_directory() {
        let command = editor_process_command("/usr/bin/zed", Path::new("/repo/sub"));
        assert_eq!(command.get_program(), Path::new("/usr/bin/zed"));
        assert_eq!(
            command.get_args().collect::<Vec<_>>(),
            [Path::new("/repo/sub").as_os_str()]
        );
        assert_eq!(command.get_current_dir(), Some(Path::new("/repo/sub")));
    }

    #[test]
    fn spec_20260820_open_project_in_editor_configured_binary_name_is_preserved() {
        let command = editor_process_command("code", Path::new("/repo"));
        assert_eq!(command.get_program(), Path::new("code"));
        assert_eq!(
            command.get_args().collect::<Vec<_>>(),
            [Path::new("/repo").as_os_str()]
        );
    }

    #[test]
    fn spec_20260820_open_project_in_editor_paths_with_spaces_and_utf8_survive() {
        let directory = PathBuf::from("/用户/我的 项目");
        let command = editor_process_command("zed", &directory);
        assert_eq!(
            command.get_args().collect::<Vec<_>>(),
            [directory.as_os_str()]
        );
        assert_eq!(command.get_current_dir(), Some(directory.as_path()));
    }

    #[test]
    fn spec_gui_minimal_path_augmented_with_homebrew_fallback() {
        // 复现截图问题：GUI 进程 PATH 仅为 /usr/bin:/bin:/usr/sbin:/sbin
        let gui_path = OsStr::new("/usr/bin:/bin:/usr/sbin:/sbin");
        let merged = super::merge_paths(gui_path, None);
        let dirs: Vec<PathBuf> = std::env::split_paths(&merged).collect();
        assert!(
            dirs.contains(&PathBuf::from("/opt/homebrew/bin")),
            "回退应补全 brew 路径，实际: {dirs:?}"
        );
        assert!(
            dirs.contains(&PathBuf::from("/usr/local/bin")),
            "回退应补全 /usr/local/bin"
        );
        let pos_bin = dirs
            .iter()
            .position(|p| p == &PathBuf::from("/usr/bin"))
            .unwrap();
        let pos_brew = dirs
            .iter()
            .position(|p| p == &PathBuf::from("/opt/homebrew/bin"))
            .unwrap();
        assert!(
            pos_brew > pos_bin,
            "回退目录应追加在 env 之后，保持原有 PATH 优先"
        );
    }

    #[test]
    fn spec_merge_dedup_shell_path() {
        let env = OsStr::new("/opt/homebrew/bin:/usr/bin");
        let shell = Some(OsStr::new("/opt/homebrew/bin:/usr/local/bin:/usr/bin"));
        let merged = super::merge_paths(env, shell);
        let dirs: Vec<PathBuf> = std::env::split_paths(&merged).collect();
        // 去重后应为: env 的 /opt/homebrew/bin, /usr/bin, 再 shell 的 /usr/local/bin, 最后回退 sbin
        assert_eq!(
            dirs[0],
            PathBuf::from("/opt/homebrew/bin"),
            "首位保持 env 第一项"
        );
        assert_eq!(dirs[1], PathBuf::from("/usr/bin"));
        assert!(dirs.contains(&PathBuf::from("/usr/local/bin")));
        // sbin 回退应追加在最后且不重复
        assert!(dirs.contains(&PathBuf::from("/opt/homebrew/sbin")));
        // 确认去重：/opt/homebrew/bin 只出现一次
        assert_eq!(
            dirs.iter()
                .filter(|p| p.as_path() == Path::new("/opt/homebrew/bin"))
                .count(),
            1
        );
    }

    #[test]
    fn spec_detect_with_merged_path_finds_homebrew_editors_from_gui_minimal_path() {
        let gui_path = OsStr::new("/usr/bin:/bin:/usr/sbin:/sbin");
        let merged = super::merge_paths(gui_path, None);
        // 真实环境：GUI PATH 只有 /usr/bin/xed，zed/code 仅在 brew 目录
        let exists = |path: &Path| {
            let s = path.to_string_lossy();
            s == "/usr/bin/xed" || s == "/opt/homebrew/bin/zed" || s == "/opt/homebrew/bin/code"
        };
        let detected = detect_editors(&merged, exists);
        assert!(
            detected.iter().any(|p| p.ends_with("/usr/bin/xed")),
            "应通过原始 PATH 找到 xed，实际: {detected:?}"
        );
        assert!(
            detected.iter().any(|p| p == "/opt/homebrew/bin/zed"),
            "应通过回退找到 zed，实际: {detected:?}"
        );
        assert!(
            detected.iter().any(|p| p == "/opt/homebrew/bin/code"),
            "应通过回退找到 code，实际: {detected:?}"
        );
        // 候选顺序：zed 优先于 code 优先于 xed
        assert_eq!(detected[0], "/opt/homebrew/bin/zed");
    }
}
