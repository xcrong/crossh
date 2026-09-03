//! Process-level helpers shared by the application binaries.

use std::path::{Path, PathBuf};

/// 在当前可执行文件旁查找同伴二进制，找不到时回退为按名称执行（交给 PATH）。
///
/// 发布包会把所有二进制放在同一目录（app bundle `MacOS/`、Linux tar 根目录、
/// Windows zip 根目录），因此同伴二进制优先；开发环境中直接 `cargo build`
/// 即可让主程序委托到同目录产物。
pub fn sibling_executable(name: &str) -> PathBuf {
    let filename = if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_string()
    };
    std::env::current_exe()
        .ok()
        .and_then(|current| current.parent().map(|directory| directory.join(filename)))
        .filter(|path| path.is_file())
        .unwrap_or_else(|| PathBuf::from(name))
}

/// 将命令的标准输入输出重定向到 null，避免抢占前台终端或落盘输出。
pub fn null_stdio(cmd: &mut std::process::Command) -> &mut std::process::Command {
    cmd.stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
}

/// 让命令脱离当前进程组/会话，适用于后台/同伴进程。
pub fn detach(cmd: &mut std::process::Command) -> &mut std::process::Command {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        cmd.creation_flags(CREATE_NEW_PROCESS_GROUP | DETACHED_PROCESS);
    }
    #[allow(unreachable_code)]
    {
        cmd
    }
}

/// Windows GUI 入口在 `windows` 子系统下运行（无控制台），启动时调用此函数
/// 尝试挂回父控制台，使终端里 `crossh --help` 等 CLI 输出仍然可见。
/// 从资源管理器/开始菜单启动时没有父控制台，挂接失败是预期的，直接忽略。
/// 非 Windows 上无操作。必须在任何打印输出之前调用。
pub fn attach_parent_console() {
    #[cfg(windows)]
    {
        const ATTACH_PARENT_PROCESS: u32 = 0xFFFF_FFFF;
        #[link(name = "kernel32")]
        unsafe extern "system" {
            unsafe fn AttachConsole(process_id: u32) -> i32;
        }
        // 有父控制台时恢复 CLI 输出；无父控制台时返回 0，不创建任何窗口。
        unsafe {
            AttachConsole(ATTACH_PARENT_PROCESS);
        }
    }
}

/// 构造一个分离的同伴进程命令：stdin/stdout/stderr 均重定向到 null，
/// Unix 上脱离前台进程组，Windows 上以独立进程组/DETACHED 方式创建。
pub fn sibling_command(name: &str) -> std::process::Command {
    let mut cmd = std::process::Command::new(sibling_executable(name));
    null_stdio(&mut cmd);
    detach(&mut cmd);
    cmd
}
/// 在系统文件管理器中展示给定路径：macOS `open`、Linux `xdg-open`、
/// Windows `explorer`，其他平台无等价动作。
///
/// 返回是否成功启动了展示进程。进程启动成功不代表展示必然可见（例如
/// 某些 Linux 桌面缺少 `xdg-open`），调用方按静默失败处理即可。
pub fn reveal_in_finder(path: &Path) -> bool {
    #[cfg(target_os = "macos")]
    let program = "open";
    #[cfg(target_os = "linux")]
    let program = "xdg-open";
    #[cfg(target_os = "windows")]
    let program = "explorer";
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    {
        let _ = path;
        return false;
    }
    std::process::Command::new(program)
        .arg(path)
        .spawn()
        .is_ok()
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    #[test]
    fn missing_sibling_falls_back_to_path_syntax() {
        let name = "crossh-core-test-no-such-sibling";
        let current = std::env::current_exe().expect("current exe should resolve");
        let directory = current.parent().expect("exe should have a parent");
        let sibling = if cfg!(windows) {
            directory.join(format!("{name}.exe"))
        } else {
            directory.join(name)
        };
        assert!(
            !sibling.exists(),
            "test fixture name collides with a real file: {}",
            sibling.display()
        );
        assert_eq!(sibling_executable(name), PathBuf::from(name));
    }

    #[test]
    fn attach_parent_console_never_panics() {
        // 无论有无父控制台（CI/本地通常都没有），调用必须直接返回。
        attach_parent_console();
    }
}
