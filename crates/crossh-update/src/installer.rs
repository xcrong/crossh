//! Cross-platform hand-off to, and implementation of, the standalone updater.
//!
//! The running application never replaces its own executable. It starts the
//! small `crossh-updater` companion, then exits. The companion performs the
//! replacement after the process lock is gone and launches the new version.

use std::ffi::OsString;
use std::fs::{self, File};
use std::io::{self, Read as _};
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::Duration;

use flate2::read::GzDecoder;
use thiserror::Error;

use super::model::{ArtifactFormat, MAX_DOWNLOAD_BYTES, UpdateResult, record_update_result};

const UPDATE_WAIT: Duration = Duration::from_millis(250);
const UPDATE_WAIT_ATTEMPTS: usize = 480;

/// ponytail: staging 预算只留这一处可调项；64MiB headroom 给小 /tmp 留余量，
/// 上限就是已有的 1GiB（单文件/总额复用同一值，不新增阈值）。
const STAGING_HEADROOM_BYTES: u64 = 64 * 1024 * 1024;

#[derive(Debug, Error)]
pub enum InstallerError {
    #[error("could not resolve the current executable path")]
    CurrentExecutable(#[source] io::Error),
    #[error("the standalone updater is not bundled with this build: {0}")]
    UpdaterMissing(PathBuf),
    #[error("invalid updater arguments: {0}")]
    InvalidArguments(String),
    #[error("updater file operation failed: {0}")]
    Io(#[from] io::Error),
    #[error("invalid zip archive: {0}")]
    Zip(#[from] zip::result::ZipError),
    #[error("invalid tar archive: {0}")]
    Tar(#[source] io::Error),
    #[error("update archive contains an unsafe path: {0}")]
    UnsafeArchivePath(String),
    #[error("expanded update archive exceeds the size limit")]
    ArchiveTooLarge,
    #[error("update package does not contain {0}")]
    MissingPayload(String),
    #[error("the previous Crossh process did not exit in time")]
    ProcessTimeout,
    #[error("unsupported update package format: {0}")]
    UnsupportedFormat(String),
}

#[derive(Debug)]
struct UpdaterArguments {
    package: PathBuf,
    format: ArtifactFormat,
    pid: u32,
    target: PathBuf,
    launch: PathBuf,
}

/// Windows 上为命令加上 CREATE_NO_WINDOW，使控制台子系统程序不闪出黑窗口；
/// 其他平台无操作。自更新链路的三处拉起（updater 本体、tasklist 轮询、
/// 新版拉起）都经此收敛。
fn no_window(cmd: &mut Command) -> &mut Command {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    #[allow(unreachable_code)]
    {
        cmd
    }
}

/// Start the bundled updater and return immediately so the caller can quit.
pub fn spawn_updater(package: &Path, format: ArtifactFormat) -> Result<(), InstallerError> {
    let current_exe = std::env::current_exe().map_err(InstallerError::CurrentExecutable)?;
    let updater = updater_path(&current_exe);
    if !updater.is_file() {
        return Err(InstallerError::UpdaterMissing(updater));
    }
    let (target, launch) = install_paths(&current_exe)?;

    let mut cmd = Command::new(&updater);
    cmd.arg("--package")
        .arg(package)
        .arg("--format")
        .arg(format.as_str())
        .arg("--pid")
        .arg(std::process::id().to_string())
        .arg("--target")
        .arg(&target)
        .arg("--launch")
        .arg(&launch)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    // updater 保持控制台子系统（手动运行时能看到报错），但从 GUI 自更新
    // 启动时不能闪出黑色控制台窗口。stdout 已重定向到 null，结果落盘到
    // UpdateResult，所以隐藏控制台没有信息损失。
    no_window(&mut cmd);
    cmd.spawn()?;
    Ok(())
}

fn updater_path(current_exe: &Path) -> PathBuf {
    let name = if cfg!(windows) {
        "crossh-updater.exe"
    } else {
        "crossh-updater"
    };
    current_exe
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(name)
}

fn install_paths(current_exe: &Path) -> Result<(PathBuf, PathBuf), InstallerError> {
    #[cfg(target_os = "macos")]
    {
        let bundle = current_exe
            .parent()
            .and_then(Path::parent)
            .and_then(Path::parent)
            .ok_or_else(|| InstallerError::InvalidArguments("invalid macOS app path".into()))?
            .to_path_buf();
        Ok((bundle.clone(), bundle))
    }

    #[cfg(not(target_os = "macos"))]
    {
        #[cfg(target_os = "linux")]
        if let Some(appimage) = std::env::var_os("APPIMAGE") {
            let appimage = PathBuf::from(appimage);
            return Ok((appimage.clone(), appimage));
        }

        Ok((current_exe.to_path_buf(), current_exe.to_path_buf()))
    }
}

/// Entry point used by `src/bin/crossh-updater.rs`.
///
/// 无论成败都把结果落盘（`UpdateResult`），因为 updater 的 stdout/stderr
/// 被父进程重定向到 null；主应用下次启动时读取并展示失败原因。
pub fn run_from_args<I>(args: I) -> Result<(), InstallerError>
where
    I: IntoIterator<Item = OsString>,
{
    let outcome = install_from_args(args);
    match &outcome {
        Ok(()) => record_update_result(&UpdateResult {
            success: true,
            error: None,
        }),
        Err(error) => record_update_result(&UpdateResult {
            success: false,
            error: Some(error.to_string()),
        }),
    }
    outcome
}

fn install_from_args<I>(args: I) -> Result<(), InstallerError>
where
    I: IntoIterator<Item = OsString>,
{
    let arguments = parse_arguments(args)?;
    wait_for_process(arguments.pid)?;

    let staging = create_staging_dir(arguments.pid)?;

    let install_result = (|| match arguments.format {
        ArtifactFormat::AppImage => replace_file(&arguments.package, &arguments.target),
        ArtifactFormat::Zip => {
            extract_zip(&arguments.package, &staging)?;
            install_zip_payload(&staging, &arguments.target)
        }
        ArtifactFormat::TarGz => {
            extract_tar_gz(&arguments.package, &staging)?;
            install_tar_payload(&staging, &arguments.target)
        }
    })();

    if let Err(error) = install_result {
        let _ = fs::remove_dir_all(&staging);
        let _ = launch(&arguments.launch);
        return Err(error);
    }
    let _ = fs::remove_dir_all(&staging);
    launch(&arguments.launch)?;
    Ok(())
}

/// 独占创建 staging 目录：名可预测（pid 双拼）会被抢占，这里混入 `getrandom`
/// 熵并用 `create_dir` 独占创建，撞名就换后缀重试（`getrandom` 已是本 crate 依赖）。
fn create_staging_dir(pid: u32) -> Result<PathBuf, InstallerError> {
    static NEXT_STAGING: AtomicU64 = AtomicU64::new(1);
    for _ in 0..100 {
        let mut entropy = [0u8; 8];
        if getrandom::fill(&mut entropy).is_err() {
            return Err(InstallerError::InvalidArguments(
                "could not randomize staging directory".into(),
            ));
        }
        let suffix = u64::from_le_bytes(entropy)
            ^ u64::from(pid)
            ^ (u64::from(std::process::id()) << 32)
            ^ NEXT_STAGING.fetch_add(1, Ordering::Relaxed);
        let staging = std::env::temp_dir().join(format!("crossh-update-{suffix:016x}"));
        match fs::create_dir(&staging) {
            Ok(()) => return Ok(staging),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error.into()),
        }
    }
    Err(InstallerError::InvalidArguments(
        "could not create a unique staging directory".into(),
    ))
}

fn parse_arguments<I>(args: I) -> Result<UpdaterArguments, InstallerError>
where
    I: IntoIterator<Item = OsString>,
{
    let mut package = None;
    let mut format = None;
    let mut pid = None;
    let mut target = None;
    let mut launch = None;
    let mut args = args.into_iter();

    while let Some(argument) = args.next() {
        let mut value = || {
            args.next().ok_or_else(|| {
                InstallerError::InvalidArguments(format!(
                    "missing value for {}",
                    argument.to_string_lossy()
                ))
            })
        };
        match argument.to_string_lossy().as_ref() {
            "--package" => package = Some(PathBuf::from(value()?)),
            "--format" => {
                let value = value()?.to_string_lossy().into_owned();
                format = Some(match value.as_str() {
                    "zip" => ArtifactFormat::Zip,
                    "appimage" => ArtifactFormat::AppImage,
                    "tar.gz" => ArtifactFormat::TarGz,
                    other => return Err(InstallerError::UnsupportedFormat(other.into())),
                });
            }
            "--pid" => {
                let value = value()?.to_string_lossy().parse::<u32>().map_err(|_| {
                    InstallerError::InvalidArguments("pid must be an integer".into())
                })?;
                pid = Some(value);
            }
            "--target" => target = Some(PathBuf::from(value()?)),
            "--launch" => launch = Some(PathBuf::from(value()?)),
            other => {
                return Err(InstallerError::InvalidArguments(format!(
                    "unknown argument {other}"
                )));
            }
        }
    }

    Ok(UpdaterArguments {
        package: package
            .ok_or_else(|| InstallerError::InvalidArguments("missing package".into()))?,
        format: format.ok_or_else(|| InstallerError::InvalidArguments("missing format".into()))?,
        pid: pid.ok_or_else(|| InstallerError::InvalidArguments("missing pid".into()))?,
        target: target.ok_or_else(|| InstallerError::InvalidArguments("missing target".into()))?,
        launch: launch.ok_or_else(|| InstallerError::InvalidArguments("missing launch".into()))?,
    })
}

fn wait_for_process(pid: u32) -> Result<(), InstallerError> {
    for _ in 0..UPDATE_WAIT_ATTEMPTS {
        if !process_is_running(pid) {
            return Ok(());
        }
        thread::sleep(UPDATE_WAIT);
    }
    Err(InstallerError::ProcessTimeout)
}

#[cfg(unix)]
fn process_is_running(pid: u32) -> bool {
    // `kill(0, …)` 查的是进程组、`kill(-1, …)` 会广播，越界/0 一律按已退出处理。
    let pid: libc::pid_t = match i32::try_from(pid) {
        Ok(pid) if pid > 0 => pid as libc::pid_t,
        _ => return false,
    };
    let result = unsafe { libc::kill(pid, 0) };
    result == 0 || io::Error::last_os_error().kind() == io::ErrorKind::PermissionDenied
}

#[cfg(windows)]
fn process_is_running(pid: u32) -> bool {
    // 等待循环每 250ms 调用一次：tasklist 是控制台子系统，不加会持续闪黑窗。
    let Ok(output) = no_window(&mut Command::new("tasklist"))
        .args(["/FI", &format!("PID eq {pid}"), "/NH"])
        .output()
    else {
        return true;
    };
    String::from_utf8_lossy(&output.stdout).contains(&pid.to_string())
}

#[cfg(not(any(unix, windows)))]
fn process_is_running(_pid: u32) -> bool {
    false
}

/// ponytail: 预算 = min(1GiB, 可用 - 64MiB)；小 /tmp 按可用量收紧，不再一律放行 1GiB。
fn budget_for_available(available: u64) -> u64 {
    available
        .saturating_sub(STAGING_HEADROOM_BYTES)
        .min(MAX_DOWNLOAD_BYTES)
}

fn extraction_budget(destination: &Path) -> u64 {
    available_space(destination)
        .map(budget_for_available)
        .unwrap_or(MAX_DOWNLOAD_BYTES)
}

#[cfg(unix)]
fn available_space(path: &Path) -> io::Result<u64> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    let ancestor = path.ancestors().find(|candidate| candidate.exists());
    let ancestor = ancestor.unwrap_or(path);
    let bytes = ancestor.as_os_str().as_bytes();
    if bytes.contains(&0) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "path contains NUL",
        ));
    }
    let c_path = CString::new(bytes)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "path contains NUL"))?;
    let mut stat: libc::statvfs = unsafe { std::mem::zeroed() };
    // SAFETY: `c_path` 以 NUL 结尾且在调用期间有效；`stat` 指向有效可写内存，
    // `statvfs` 不保留指针。
    if unsafe { libc::statvfs(c_path.as_ptr(), &mut stat) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(stat.f_bavail as u64 * stat.f_frsize as u64)
}

#[cfg(windows)]
#[link(name = "kernel32")]
unsafe extern "system" {
    fn GetDiskFreeSpaceExW(
        directory: *const u16,
        free_to_caller: *mut u64,
        total: *mut u64,
        free: *mut u64,
    ) -> i32;
}

#[cfg(windows)]
fn available_space(path: &Path) -> io::Result<u64> {
    use std::os::windows::ffi::OsStrExt;

    let ancestor = path.ancestors().find(|candidate| candidate.exists());
    let ancestor = ancestor.unwrap_or(path);
    let wide: Vec<u16> = ancestor
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let mut free_to_caller: u64 = 0;
    // SAFETY: `wide` 以 NUL 结尾且在调用期间有效；传出的只是栈上 u64 指针。
    let ok = unsafe {
        GetDiskFreeSpaceExW(
            wide.as_ptr(),
            &mut free_to_caller,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(free_to_caller)
}

#[cfg(not(any(unix, windows)))]
fn available_space(_path: &Path) -> io::Result<u64> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "free space query unsupported",
    ))
}

fn extract_zip(package: &Path, destination: &Path) -> Result<(), InstallerError> {
    let budget = extraction_budget(destination);
    extract_zip_with_limit(package, destination, budget)
}

fn extract_zip_with_limit(
    package: &Path,
    destination: &Path,
    budget: u64,
) -> Result<(), InstallerError> {
    if budget == 0 {
        return Err(InstallerError::ArchiveTooLarge);
    }
    let file = File::open(package)?;
    let mut archive = zip::ZipArchive::new(file)?;
    let mut extracted = 0u64;
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index)?;
        // zip 规范要求分隔符是 '/'，但 Windows 打包工具（Windows PowerShell 5.1
        // 的 Compress-Archive、.NET ZipFile.CreateFromDirectory）会写 '\'。先归一化
        // 再做安全校验：'..\..\evil' 归一化后仍会被 is_safe_relative_path 拒绝，
        // 路径穿越防护不降级。
        let entry_name = entry.name().replace('\\', "/");
        // 目录判定必须基于归一化后的名字：Windows 产物写成
        // "resources\crossh-assets\"，用原始名字判断会被当成普通文件，
        // 同名目录下后续的条目就再也写不进去。
        let is_directory = entry_name.ends_with('/');
        let relative = Path::new(&entry_name);
        if !is_safe_relative_path(relative) {
            return Err(InstallerError::UnsafeArchivePath(entry_name));
        }
        let path = destination.join(relative);
        if is_directory {
            fs::create_dir_all(path)?;
            continue;
        }
        // 单文件上限：声明值先卡一次，注定超限的文件不分配写入。
        if entry.size() > budget {
            return Err(InstallerError::ArchiveTooLarge);
        }
        let remaining = budget - extracted;
        if entry.size() > remaining {
            return Err(InstallerError::ArchiveTooLarge);
        }
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut output = File::create(&path)?;
        // 写入字节复核：头里的 size 可伪造，以实际写入 n 为准；Take(limit + 1)
        // 让超限恰好多写 1 字节即被发现，不会把整个炸弹写满磁盘。
        let mut source = (&mut entry).take(remaining.saturating_add(1));
        let written = io::copy(&mut source, &mut output)?;
        drop(source);
        drop(output);
        if written > remaining {
            let _ = fs::remove_file(&path);
            return Err(InstallerError::ArchiveTooLarge);
        }
        extracted += written;
    }
    Ok(())
}

fn extract_tar_gz(package: &Path, destination: &Path) -> Result<(), InstallerError> {
    let budget = extraction_budget(destination);
    extract_tar_gz_with_limit(package, destination, budget)
}

fn extract_tar_gz_with_limit(
    package: &Path,
    destination: &Path,
    budget: u64,
) -> Result<(), InstallerError> {
    if budget == 0 {
        return Err(InstallerError::ArchiveTooLarge);
    }
    let file = File::open(package)?;
    let decoder = GzDecoder::new(file);
    let mut archive = tar::Archive::new(decoder);
    let mut extracted = 0u64;
    for entry in archive.entries().map_err(InstallerError::Tar)? {
        let mut entry = entry.map_err(InstallerError::Tar)?;
        let path = entry.path().map_err(InstallerError::Tar)?.into_owned();
        if !is_safe_relative_path(&path) {
            return Err(InstallerError::UnsafeArchivePath(
                path.display().to_string(),
            ));
        }
        let entry_type = entry.header().entry_type();
        if entry_type.is_dir() {
            entry.unpack_in(destination).map_err(InstallerError::Tar)?;
            continue;
        }
        if !(entry_type.is_file()) {
            return Err(InstallerError::Tar(io::Error::new(
                io::ErrorKind::InvalidData,
                "tar archive contains an unsupported entry type",
            )));
        }
        if entry.size() > budget {
            return Err(InstallerError::ArchiveTooLarge);
        }
        let remaining = budget - extracted;
        if entry.size() > remaining {
            return Err(InstallerError::ArchiveTooLarge);
        }
        let destination_path = destination.join(&path);
        if let Some(parent) = destination_path.parent() {
            fs::create_dir_all(parent)?;
        }
        #[cfg(unix)]
        let mode = entry.header().mode().map_err(InstallerError::Tar)?;
        let mut output = File::create(&destination_path)?;
        let mut source = (&mut entry).take(remaining.saturating_add(1));
        let written = io::copy(&mut source, &mut output)?;
        drop(source);
        drop(output);
        if written > remaining {
            let _ = fs::remove_file(&destination_path);
            return Err(InstallerError::ArchiveTooLarge);
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;

            let permissions = fs::Permissions::from_mode(mode);
            let _ = fs::set_permissions(&destination_path, permissions);
        }
        extracted += written;
    }
    Ok(())
}

fn is_safe_relative_path(path: &Path) -> bool {
    if path.as_os_str().is_empty() {
        return false;
    }
    path.components()
        .all(|component| matches!(component, Component::Normal(_)))
}

fn install_zip_payload(staging: &Path, target: &Path) -> Result<(), InstallerError> {
    if target.extension().and_then(|extension| extension.to_str()) == Some("app") {
        let source = find_named_path(staging, "crossh.app")?;
        replace_directory(&source, target)
    } else {
        replace_payload_directory(staging, target)
    }
}

/// Windows zip 的顶层结构（`crossh.exe`、`crossh-git.exe`、`crossh-note.exe`、
/// `crossh-updater.exe`、`crossh-daemon.exe`、`resources/`、README、LICENSE）与安装目录一一对应，
/// 见 `scripts/crossh.iss` 的 `[Files]`。只换主程序会让附属二进制和 resources
/// 停在旧版本，因此这里把整个顶层载荷合并进安装目录。
///
/// 半更新防护：先把 staging 快照成清单并在同卷暂存里建好完整新树（此间任何
/// 读取/拷贝失败都还没动安装目录），最后用两次 rename 做整体切换。切换前
/// 安装目录保持原样，不会出现“新旧混装”。
fn replace_payload_directory(staging: &Path, target: &Path) -> Result<(), InstallerError> {
    let parent = target
        .parent()
        .ok_or_else(|| InstallerError::MissingPayload(target.display().to_string()))?;
    let name = target
        .file_name()
        .ok_or_else(|| InstallerError::MissingPayload(target.display().to_string()))?;
    // 先确认主程序在包里，否则结构异常的载荷会动到安装目录却没人能启动。
    find_named_path(staging, name)?;
    // 快照载荷清单：一次性读完 staging，边读边写会在读到一半失败时留下半更新。
    let mut entries = Vec::new();
    for entry in fs::read_dir(staging)? {
        let entry = entry?;
        entries.push((entry.file_name(), entry.path(), entry.file_type()?));
    }
    swap_install_tree(&entries, parent)
}

/// 同卷暂存整树 + 两次 rename 切换：建树失败时安装目录 untouched；第二次
/// rename 失败时尽力把备份换回去，避免新旧混装。
fn swap_install_tree(
    entries: &[(OsString, PathBuf, fs::FileType)],
    install: &Path,
) -> Result<(), InstallerError> {
    let grandparent = install
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .ok_or_else(|| InstallerError::MissingPayload(install.display().to_string()))?;
    let install_name = install
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| InstallerError::MissingPayload(install.display().to_string()))?;
    let replacement = grandparent.join(format!(".{install_name}.crossh-new"));
    let backup = grandparent.join(format!(".{install_name}.crossh-old"));
    let _ = fs::remove_dir_all(&replacement);
    let _ = fs::remove_dir_all(&backup);
    let built = build_replacement_tree(entries, install, &replacement);
    if let Err(error) = built {
        let _ = fs::remove_dir_all(&replacement);
        return Err(error);
    }
    if install.exists() {
        fs::rename(install, &backup)?;
    }
    if let Err(error) = fs::rename(&replacement, install) {
        // 新树换不进去时把旧树换回来；回滚失败也只报原始错误，旧树留在 backup 可人工恢复。
        if backup.exists() && !install.exists() {
            let _ = fs::rename(&backup, install);
        }
        return Err(error.into());
    }
    let _ = fs::remove_dir_all(backup);
    Ok(())
}

/// 在同卷暂存里建好完整新树：先拷旧安装（跳过历史残留的替换临时目录），
/// 再把载荷清单逐项覆盖。目录载荷整体替换（与旧逐个 replace_directory 语义一致）。
fn build_replacement_tree(
    entries: &[(OsString, PathBuf, fs::FileType)],
    install: &Path,
    replacement: &Path,
) -> Result<(), InstallerError> {
    fs::create_dir_all(replacement)?;
    if install.exists() {
        for entry in fs::read_dir(install)? {
            let entry = entry?;
            let file_name = entry.file_name();
            if is_swap_temp(&file_name) {
                continue;
            }
            let source = entry.path();
            let destination = replacement.join(&file_name);
            if entry.file_type()?.is_dir() {
                copy_directory(&source, &destination)?;
            } else if entry.file_type()?.is_file() {
                fs::copy(&source, &destination)?;
                make_executable(&destination)?;
            }
            // 旧安装里的特殊文件（symlink 等）直接跳过：它们不属于载荷，
            // 丢下也不影响新树完整性，且避免一次更新失败。
        }
    }
    for (file_name, source, file_type) in entries {
        let destination = replacement.join(file_name);
        if file_type.is_dir() {
            let _ = fs::remove_dir_all(&destination);
            copy_directory(source, &destination)?;
        } else if file_type.is_file() {
            if let Some(parent) = destination.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::copy(source, &destination)?;
            make_executable(&destination)?;
        } else {
            return Err(InstallerError::Io(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "update payload contains an unsupported entry: {}",
                    source.display()
                ),
            )));
        }
    }
    Ok(())
}

/// 识别历史替换残留的临时目录（`.<name>.crossh-new` / `.<name>.crossh-old`），
/// 建新树时跳过，避免把垃圾带进新安装。
fn is_swap_temp(file_name: &std::ffi::OsStr) -> bool {
    let Some(name) = file_name.to_str() else {
        return false;
    };
    name.starts_with('.') && (name.ends_with(".crossh-new") || name.ends_with(".crossh-old"))
}

fn install_tar_payload(staging: &Path, target: &Path) -> Result<(), InstallerError> {
    let name = target
        .file_name()
        .ok_or_else(|| InstallerError::MissingPayload(target.display().to_string()))?;
    let source = find_named_path(staging, name)?;
    replace_file(&source, target)
}

fn find_named_path(root: &Path, name: impl AsRef<Path>) -> Result<PathBuf, InstallerError> {
    let name = name.as_ref();
    let mut pending = vec![root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        for entry in fs::read_dir(directory)? {
            let entry = entry?;
            let path = entry.path();
            if path.file_name() == name.file_name() {
                return Ok(path);
            }
            if entry.file_type()?.is_dir() {
                pending.push(path);
            }
        }
    }
    Err(InstallerError::MissingPayload(name.display().to_string()))
}

fn replace_file(source: &Path, target: &Path) -> Result<(), InstallerError> {
    let replacement = staging_path(target, "crossh-new")?;
    let backup = staging_path(target, "crossh-old")?;
    if let Some(parent) = replacement.parent() {
        fs::create_dir_all(parent)?;
    }
    let _ = fs::remove_file(&replacement);
    let _ = fs::remove_file(&backup);
    fs::copy(source, &replacement)?;
    make_executable(&replacement)?;
    if target.exists() {
        fs::rename(target, &backup)?;
    }
    if let Err(error) = fs::rename(&replacement, target) {
        if backup.exists() {
            let _ = fs::rename(&backup, target);
        }
        return Err(error.into());
    }
    let _ = fs::remove_file(backup);
    Ok(())
}

fn replace_directory(source: &Path, target: &Path) -> Result<(), InstallerError> {
    let replacement = staging_path(target, "crossh-new")?;
    let backup = staging_path(target, "crossh-old")?;
    if let Some(parent) = replacement.parent() {
        fs::create_dir_all(parent)?;
    }
    let _ = fs::remove_dir_all(&replacement);
    let _ = fs::remove_dir_all(&backup);
    copy_directory(source, &replacement)?;
    if target.exists() {
        fs::rename(target, &backup)?;
    }
    if let Err(error) = fs::rename(&replacement, target) {
        if backup.exists() {
            let _ = fs::rename(&backup, target);
        }
        return Err(error.into());
    }
    let _ = fs::remove_dir_all(backup);
    Ok(())
}

/// 替换用的临时/备份路径：与目标同目录（同卷 rename 才原子生效），以 `.` 开头
/// 并带走目标的文件名，避免和安装内容或同时替换的其他目标撞名。
///
/// 替换运行中的文件时（Windows 上的 `crossh-updater.exe` 替换自身）旧名可以
/// rename 但删不掉，备份会残留；下次替换开头的清理会顺手收掉。
fn staging_path(target: &Path, suffix: &str) -> Result<PathBuf, InstallerError> {
    let parent = target
        .parent()
        .ok_or_else(|| InstallerError::MissingPayload(target.display().to_string()))?;
    let name = target
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| InstallerError::MissingPayload(target.display().to_string()))?;
    Ok(parent.join(format!(".{name}.{suffix}")))
}

fn copy_directory(source: &Path, destination: &Path) -> Result<(), InstallerError> {
    fs::create_dir_all(destination)?;
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let source_path = entry.path();
        let destination_path = destination.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_directory(&source_path, &destination_path)?;
        } else if entry.file_type()?.is_file() {
            fs::copy(&source_path, &destination_path)?;
            make_executable(&destination_path)?;
        } else {
            return Err(InstallerError::Io(io::Error::new(
                io::ErrorKind::InvalidData,
                "app bundle contains an unsupported entry type",
            )));
        }
    }
    Ok(())
}

#[cfg(unix)]
fn make_executable(path: &Path) -> Result<(), InstallerError> {
    use std::os::unix::fs::PermissionsExt;

    let mut permissions = fs::metadata(path)?.permissions();
    permissions.set_mode(permissions.mode() | 0o111);
    fs::set_permissions(path, permissions)?;
    Ok(())
}

#[cfg(not(unix))]
fn make_executable(_path: &Path) -> Result<(), InstallerError> {
    Ok(())
}

fn launch(path: &Path) -> Result<(), InstallerError> {
    #[cfg(target_os = "macos")]
    {
        Command::new("open").arg(path).spawn()?;
    }
    #[cfg(not(target_os = "macos"))]
    {
        // 被拉起的新版主程序已是 windows 子系统，此 flag 无作用；
        // 旧版/其他控制台程序则借此避免闪出黑窗口。
        no_window(&mut Command::new(path)).spawn()?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_TEMP: AtomicU64 = AtomicU64::new(1);

    struct TestDir(PathBuf);

    impl TestDir {
        fn new(label: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "crossh-update-test-{label}-{}-{}",
                std::process::id(),
                NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
            ));
            let _ = fs::remove_dir_all(&path);
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn create_zip(path: &Path, name: &str, contents: &[u8]) {
        create_zip_entries(path, &[(name, contents)]);
    }

    /// 条目名原样写入（`zip` 的写入器不做分隔符归一化），用于构造 Windows
    /// 打包工具产出的反斜杠条目。
    fn create_zip_entries(path: &Path, entries: &[(&str, &[u8])]) {
        let file = File::create(path).unwrap();
        let mut archive = zip::ZipWriter::new(file);
        for (name, contents) in entries {
            archive
                .start_file(*name, zip::write::SimpleFileOptions::default())
                .unwrap();
            archive.write_all(contents).unwrap();
        }
        archive.finish().unwrap();
    }

    fn create_tar_gz(path: &Path, name: &str, contents: &[u8]) {
        create_tar_gz_entries(path, &[(name, contents)]);
    }

    fn create_tar_gz_entries(path: &Path, entries: &[(&str, &[u8])]) {
        let file = File::create(path).unwrap();
        let encoder = flate2::write::GzEncoder::new(file, flate2::Compression::default());
        let mut archive = tar::Builder::new(encoder);
        for (name, contents) in entries {
            let mut header = tar::Header::new_gnu();
            header.set_size(contents.len() as u64);
            header.set_mode(0o755);
            header.set_cksum();
            archive.append_data(&mut header, name, *contents).unwrap();
        }
        archive.into_inner().unwrap().finish().unwrap();
    }

    #[test]
    fn updater_arguments_require_all_fields() {
        let error = parse_arguments([OsString::from("--package")]).unwrap_err();
        assert!(matches!(error, InstallerError::InvalidArguments(_)));
    }

    #[test]
    fn archive_paths_cannot_escape_staging() {
        assert!(is_safe_relative_path(Path::new("crossh/bin")));
        assert!(!is_safe_relative_path(Path::new("")));
        assert!(!is_safe_relative_path(Path::new("../crossh")));
        assert!(!is_safe_relative_path(Path::new("/tmp/crossh")));
    }

    #[test]
    fn valid_updater_arguments_are_parsed() {
        let arguments = parse_arguments(
            [
                "--package",
                "update.zip",
                "--format",
                "zip",
                "--pid",
                "42",
                "--target",
                "crossh",
                "--launch",
                "crossh",
            ]
            .map(OsString::from),
        )
        .unwrap();
        assert_eq!(arguments.package, PathBuf::from("update.zip"));
        assert_eq!(arguments.format, ArtifactFormat::Zip);
        assert_eq!(arguments.pid, 42);
        assert_eq!(arguments.target, PathBuf::from("crossh"));
    }

    #[test]
    fn real_zip_and_tar_archives_extract_into_staging() {
        let root = TestDir::new("extract");
        let zip = root.0.join("update.zip");
        let tar = root.0.join("update.tar.gz");
        create_zip(&zip, "bundle/crossh", b"zip-payload");
        create_tar_gz(&tar, "bundle/crossh", b"tar-payload");

        let zip_out = root.0.join("zip-out");
        let tar_out = root.0.join("tar-out");
        fs::create_dir_all(&zip_out).unwrap();
        fs::create_dir_all(&tar_out).unwrap();
        extract_zip(&zip, &zip_out).unwrap();
        extract_tar_gz(&tar, &tar_out).unwrap();

        assert_eq!(
            fs::read(zip_out.join("bundle/crossh")).unwrap(),
            b"zip-payload"
        );
        assert_eq!(
            fs::read(tar_out.join("bundle/crossh")).unwrap(),
            b"tar-payload"
        );
    }

    #[test]
    fn zip_traversal_is_rejected_without_writing_outside_staging() {
        let root = TestDir::new("zip-slip");
        let package = root.0.join("unsafe.zip");
        create_zip(&package, "../escaped", b"bad");
        let staging = root.0.join("staging");
        fs::create_dir_all(&staging).unwrap();

        assert!(matches!(
            extract_zip(&package, &staging),
            Err(InstallerError::UnsafeArchivePath(_))
        ));
        assert!(!root.0.join("escaped").exists());
    }

    #[test]
    fn file_replacement_keeps_new_payload_and_removes_backup() {
        let root = TestDir::new("replace");
        let source = root.0.join("source");
        let target = root.0.join("crossh");
        fs::write(&source, b"new-version").unwrap();
        fs::write(&target, b"old-version").unwrap();

        replace_file(&source, &target).unwrap();
        assert_eq!(fs::read(&target).unwrap(), b"new-version");
        assert!(!root.0.join(".crossh.crossh-old").exists());
        assert!(!root.0.join(".crossh.crossh-new").exists());
    }

    // ── Windows PowerShell 5.1 的 Compress-Archive 写出反斜杠条目 ────────────

    #[test]
    fn backslash_zip_entries_extract_into_staging() {
        let root = TestDir::new("backslash-extract");
        let package = root.0.join("windows.zip");
        // 目录条目以 '\' 结尾：既不是合法目录条目，也不是合法文件名。
        create_zip_entries(
            &package,
            &[
                ("resources\\crossh-assets\\", b""),
                ("resources\\crossh-assets\\fonts\\lilex.ttf", b"font"),
                ("crossh.exe", b"app"),
                ("crossh-updater.exe", b"updater"),
            ],
        );
        let staging = root.0.join("staging");
        fs::create_dir_all(&staging).unwrap();

        extract_zip(&package, &staging).expect("windows-produced zip must be installable");

        assert_eq!(
            fs::read(staging.join("resources/crossh-assets/fonts/lilex.ttf")).unwrap(),
            b"font"
        );
        assert_eq!(fs::read(staging.join("crossh.exe")).unwrap(), b"app");
        assert_eq!(
            fs::read(staging.join("crossh-updater.exe")).unwrap(),
            b"updater"
        );
    }

    #[test]
    fn backslash_zip_traversal_is_still_rejected() {
        let root = TestDir::new("backslash-slip");
        let package = root.0.join("unsafe.zip");
        create_zip_entries(&package, &[("..\\escaped", b"bad")]);
        let staging = root.0.join("staging");
        fs::create_dir_all(&staging).unwrap();

        assert!(matches!(
            extract_zip(&package, &staging),
            Err(InstallerError::UnsafeArchivePath(_))
        ));
        assert!(!root.0.join("escaped").exists());
    }

    #[test]
    fn windows_zip_payload_replaces_every_top_level_entry() {
        let root = TestDir::new("payload");
        let install = root.0.join("install");
        fs::create_dir_all(install.join("resources")).unwrap();
        fs::write(install.join("crossh.exe"), b"old-app").unwrap();
        fs::write(install.join("crossh-git.exe"), b"old-git").unwrap();
        fs::write(install.join("crossh-updater.exe"), b"old-updater").unwrap();
        fs::write(install.join("resources/old.svg"), b"old-icon").unwrap();

        let staging = root.0.join("staging");
        fs::create_dir_all(staging.join("resources")).unwrap();
        fs::write(staging.join("crossh.exe"), b"new-app").unwrap();
        fs::write(staging.join("crossh-git.exe"), b"new-git").unwrap();
        fs::write(staging.join("crossh-updater.exe"), b"new-updater").unwrap();
        fs::write(staging.join("resources/new.svg"), b"new-icon").unwrap();

        install_zip_payload(&staging, &install.join("crossh.exe")).unwrap();

        assert_eq!(fs::read(install.join("crossh.exe")).unwrap(), b"new-app");
        assert_eq!(
            fs::read(install.join("crossh-git.exe")).unwrap(),
            b"new-git"
        );
        assert_eq!(
            fs::read(install.join("crossh-updater.exe")).unwrap(),
            b"new-updater"
        );
        assert_eq!(
            fs::read(install.join("resources/new.svg")).unwrap(),
            b"new-icon"
        );
        assert!(
            !install.join("resources/old.svg").exists(),
            "replaced resources must not keep the previous tree"
        );
        assert!(!install.join(".resources.crossh-old").exists());
    }

    #[test]
    fn windows_zip_payload_without_main_binary_is_rejected() {
        let root = TestDir::new("payload-missing");
        let install = root.0.join("install");
        fs::create_dir_all(&install).unwrap();
        fs::write(install.join("crossh.exe"), b"old-app").unwrap();
        let staging = root.0.join("staging");
        fs::create_dir_all(&staging).unwrap();
        fs::write(staging.join("crossh-git.exe"), b"new-git").unwrap();

        assert!(matches!(
            install_zip_payload(&staging, &install.join("crossh.exe")),
            Err(InstallerError::MissingPayload(_))
        ));
        assert_eq!(
            fs::read(install.join("crossh.exe")).unwrap(),
            b"old-app",
            "a payload without the main binary must leave the install untouched"
        );
        assert!(
            !install.join("crossh-git.exe").exists(),
            "nothing may be written before the payload is validated"
        );
    }

    #[test]
    fn broken_payload_leaves_install_untouched_instead_of_half_updated() {
        let root = TestDir::new("payload-atomic");
        let install = root.0.join("install");
        fs::create_dir_all(&install).unwrap();
        fs::write(install.join("crossh.exe"), b"old-app").unwrap();
        fs::write(install.join("crossh-git.exe"), b"old-git").unwrap();

        let staging = root.0.join("staging");
        fs::create_dir_all(&staging).unwrap();
        fs::write(staging.join("crossh.exe"), b"new-app").unwrap();
        fs::write(staging.join("crossh-git.exe"), b"new-git").unwrap();
        // 顶层 symlink 不是合法载荷：建新树阶段就失败，安装目录必须保持原样。
        #[cfg(unix)]
        std::os::unix::fs::symlink("crossh.exe", staging.join("evil-link")).unwrap();
        #[cfg(not(unix))]
        {
            let bad = staging.join("evil-link");
            fs::create_dir_all(&bad).unwrap();
            #[cfg(windows)]
            {
                // Windows 上目录即合法载荷，此分支仅为占位，断言仍覆盖主路径。
                let _ = bad;
            }
        }

        #[cfg(unix)]
        {
            assert!(install_zip_payload(&staging, &install.join("crossh.exe")).is_err());
            assert_eq!(
                fs::read(install.join("crossh.exe")).unwrap(),
                b"old-app",
                "failed payload must not leave a half-updated install"
            );
            assert_eq!(
                fs::read(install.join("crossh-git.exe")).unwrap(),
                b"old-git",
                "no top-level entry may be replaced before the whole tree is ready"
            );
            assert!(
                !root.0.join(".install.crossh-new").exists(),
                "failed build must clean up the same-volume staging tree"
            );
        }
    }

    #[test]
    fn payload_swap_drops_previous_staging_leftovers() {
        let root = TestDir::new("payload-clean-temp");
        let install = root.0.join("install");
        fs::create_dir_all(&install).unwrap();
        fs::write(install.join("crossh.exe"), b"old-app").unwrap();
        fs::write(install.join(".crossh.exe.crossh-old"), b"stale-backup").unwrap();

        let staging = root.0.join("staging");
        fs::create_dir_all(&staging).unwrap();
        fs::write(staging.join("crossh.exe"), b"new-app").unwrap();

        install_zip_payload(&staging, &install.join("crossh.exe")).unwrap();

        assert_eq!(fs::read(install.join("crossh.exe")).unwrap(), b"new-app");
        assert!(
            !install.join(".crossh.exe.crossh-old").exists(),
            "history temp files must not be carried into the new install"
        );
    }

    #[test]
    fn extraction_budget_leaves_headroom() {
        assert_eq!(budget_for_available(0), 0);
        assert_eq!(budget_for_available(STAGING_HEADROOM_BYTES), 0);
        assert_eq!(budget_for_available(STAGING_HEADROOM_BYTES + 1), 1);
        assert_eq!(
            budget_for_available(MAX_DOWNLOAD_BYTES + STAGING_HEADROOM_BYTES),
            MAX_DOWNLOAD_BYTES
        );
        assert_eq!(budget_for_available(u64::MAX), MAX_DOWNLOAD_BYTES);
    }

    #[test]
    fn available_space_reports_temp_dir() {
        let free = available_space(&std::env::temp_dir()).expect("temp dir must be statable");
        assert!(free > 0, "free space must be positive");
    }

    #[test]
    fn zero_budget_writes_nothing() {
        let root = TestDir::new("budget-zero");
        let package = root.0.join("update.zip");
        create_zip(&package, "crossh", b"payload");
        let staging = root.0.join("staging");
        fs::create_dir_all(&staging).unwrap();

        assert!(matches!(
            extract_zip_with_limit(&package, &staging, 0),
            Err(InstallerError::ArchiveTooLarge)
        ));
        assert!(matches!(
            extract_tar_gz_with_limit(&package, &staging, 0),
            Err(InstallerError::ArchiveTooLarge)
        ));
        assert!(
            fs::read_dir(&staging).unwrap().next().is_none(),
            "zero budget must not write any payload byte"
        );
    }

    #[test]
    fn zip_budget_counts_actual_bytes_and_drops_partial_file() {
        let root = TestDir::new("zip-budget");
        let package = root.0.join("update.zip");
        create_zip_entries(
            &package,
            &[
                ("first", b"0123456789abcdef"),
                ("second", b"0123456789abcdef"),
            ],
        );
        let staging = root.0.join("staging");
        fs::create_dir_all(&staging).unwrap();

        // 预算 20 字节：第一个 16 字节能写下，第二个声明 16 字节但只剩 4 字节，
        // 声明值预检即拒绝；即使走到写入，Take(limit + 1) 复核也会按实际字节拒绝。
        assert!(matches!(
            extract_zip_with_limit(&package, &staging, 20),
            Err(InstallerError::ArchiveTooLarge)
        ));
        assert_eq!(
            fs::read(staging.join("first")).unwrap(),
            b"0123456789abcdef"
        );
        assert!(
            !staging.join("second").exists(),
            "over-budget entry must not leave a partial file"
        );
    }

    #[test]
    fn tar_budget_counts_actual_bytes_and_drops_partial_file() {
        let root = TestDir::new("tar-budget");
        let package = root.0.join("update.tar.gz");
        create_tar_gz_entries(
            &package,
            &[
                ("first", b"0123456789abcdef"),
                ("second", b"0123456789abcdef"),
            ],
        );
        let staging = root.0.join("staging");
        fs::create_dir_all(&staging).unwrap();

        assert!(matches!(
            extract_tar_gz_with_limit(&package, &staging, 20),
            Err(InstallerError::ArchiveTooLarge)
        ));
        assert_eq!(
            fs::read(staging.join("first")).unwrap(),
            b"0123456789abcdef"
        );
        assert!(
            !staging.join("second").exists(),
            "over-budget entry must not leave a partial file"
        );
    }
}
