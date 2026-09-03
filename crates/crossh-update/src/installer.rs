//! Cross-platform hand-off to, and implementation of, the standalone updater.
//!
//! The running application never replaces its own executable. It starts the
//! small `crossh-updater` companion, then exits. The companion performs the
//! replacement after the process lock is gone and launches the new version.

use std::ffi::OsString;
use std::fs::{self, File};
use std::io;
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::Duration;

use flate2::read::GzDecoder;
use thiserror::Error;

use super::model::{ArtifactFormat, MAX_DOWNLOAD_BYTES, UpdateResult, record_update_result};

const UPDATE_WAIT: Duration = Duration::from_millis(250);
const UPDATE_WAIT_ATTEMPTS: usize = 480;

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
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
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

    let staging = std::env::temp_dir().join(format!(
        "crossh-update-{}-{}",
        arguments.pid,
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&staging);
    fs::create_dir_all(&staging)?;

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
    let result = unsafe { libc::kill(pid as libc::pid_t, 0) };
    result == 0 || io::Error::last_os_error().kind() == io::ErrorKind::PermissionDenied
}

#[cfg(windows)]
fn process_is_running(pid: u32) -> bool {
    let Ok(output) = Command::new("tasklist")
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

fn extract_zip(package: &Path, destination: &Path) -> Result<(), InstallerError> {
    let file = File::open(package)?;
    let mut archive = zip::ZipArchive::new(file)?;
    let mut extracted = 0u64;
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index)?;
        let entry_name = entry.name().to_owned();
        if entry_name.contains('\\') {
            return Err(InstallerError::UnsafeArchivePath(entry_name));
        }
        let Some(relative) = entry.enclosed_name() else {
            return Err(InstallerError::UnsafeArchivePath(entry_name));
        };
        if !is_safe_relative_path(&relative) {
            return Err(InstallerError::UnsafeArchivePath(entry_name));
        }
        let path = destination.join(relative);
        if entry.is_dir() {
            fs::create_dir_all(path)?;
            continue;
        }
        extracted = extracted
            .checked_add(entry.size())
            .ok_or(InstallerError::ArchiveTooLarge)?;
        if extracted > MAX_DOWNLOAD_BYTES {
            return Err(InstallerError::ArchiveTooLarge);
        }
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut output = File::create(path)?;
        io::copy(&mut entry, &mut output)?;
    }
    Ok(())
}

fn extract_tar_gz(package: &Path, destination: &Path) -> Result<(), InstallerError> {
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
        if !(entry_type.is_file() || entry_type.is_dir()) {
            return Err(InstallerError::Tar(io::Error::new(
                io::ErrorKind::InvalidData,
                "tar archive contains an unsupported entry type",
            )));
        }
        extracted = extracted
            .checked_add(entry.size())
            .ok_or(InstallerError::ArchiveTooLarge)?;
        if extracted > MAX_DOWNLOAD_BYTES {
            return Err(InstallerError::ArchiveTooLarge);
        }
        entry.unpack_in(destination).map_err(InstallerError::Tar)?;
    }
    Ok(())
}

fn is_safe_relative_path(path: &Path) -> bool {
    path.components()
        .all(|component| matches!(component, Component::Normal(_)))
}

fn install_zip_payload(staging: &Path, target: &Path) -> Result<(), InstallerError> {
    if target.extension().and_then(|extension| extension.to_str()) == Some("app") {
        let source = find_named_path(staging, "crossh.app")?;
        replace_directory(&source, target)
    } else {
        let name = target
            .file_name()
            .ok_or_else(|| InstallerError::MissingPayload(target.display().to_string()))?;
        let source = find_named_path(staging, name)?;
        replace_file(&source, target)
    }
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
    let parent = target
        .parent()
        .ok_or_else(|| InstallerError::MissingPayload(target.display().to_string()))?;
    fs::create_dir_all(parent)?;
    let replacement = parent.join(format!(
        ".{}.crossh-new-{}",
        target
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("app"),
        std::process::id()
    ));
    let backup = parent.join(format!(
        ".{}.crossh-old",
        target
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("app")
    ));
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
    let parent = target
        .parent()
        .ok_or_else(|| InstallerError::MissingPayload(target.display().to_string()))?;
    fs::create_dir_all(parent)?;
    let replacement = parent.join(format!(".crossh-new-{}.app", std::process::id()));
    let backup = parent.join(".crossh-old.app");
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
        Command::new(path).spawn()?;
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
        let file = File::create(path).unwrap();
        let mut archive = zip::ZipWriter::new(file);
        archive
            .start_file(name, zip::write::SimpleFileOptions::default())
            .unwrap();
        archive.write_all(contents).unwrap();
        archive.finish().unwrap();
    }

    fn create_tar_gz(path: &Path, name: &str, contents: &[u8]) {
        let file = File::create(path).unwrap();
        let encoder = flate2::write::GzEncoder::new(file, flate2::Compression::default());
        let mut archive = tar::Builder::new(encoder);
        let mut header = tar::Header::new_gnu();
        header.set_size(contents.len() as u64);
        header.set_mode(0o755);
        header.set_cksum();
        archive.append_data(&mut header, name, contents).unwrap();
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
        assert!(
            !root
                .0
                .join(format!(".crossh.crossh-new-{}", std::process::id()))
                .exists()
        );
    }
}
