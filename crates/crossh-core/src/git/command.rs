use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};

use crate::git::GitError;

/// Runs `git -C <cwd> <args>` and maps non-zero exit to `GitError::CommandFailed`.
pub fn run_git(cwd: &Path, args: &[&str]) -> Result<(), GitError> {
    let output = Command::new("git").arg("-C").arg(cwd).args(args).output()?;
    git_result(output)
}

/// Runs `git -C <cwd> <args> -- <paths>` and maps failure to `GitError`.
pub fn run_git_paths(cwd: &Path, args: &[&str], paths: &[String]) -> Result<(), GitError> {
    let output = Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(args)
        .args(paths)
        .output()?;
    git_result(output)
}

/// Single-path variant used by conflict resolution.
pub fn run_git_path(cwd: &Path, args: &[&str], path: &str) -> Result<(), GitError> {
    let output = Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(args)
        .arg(path)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .output()?;
    if output.status.success() {
        Ok(())
    } else {
        Err(command_error(output.status, &output.stderr))
    }
}

/// Runs `git -C <cwd> <args>` where args are owned `String`s and returns stdout.
/// Used by `git_branch` / `git_history` / `git_stash` which build `Vec<String>`.
pub fn run_git_output(cwd: &Path, args: &[String]) -> Result<Vec<u8>, GitError> {
    let output = Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(args)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .output()?;
    if output.status.success() {
        Ok(output.stdout)
    } else {
        Err(command_error(output.status, &output.stderr))
    }
}

/// Read-only helper that adds `GIT_OPTIONAL_LOCKS=0` and returns stdout.
pub fn git_output(cwd: &Path, args: &[&str]) -> Result<Vec<u8>, GitError> {
    let output = Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(args)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .output()?;
    git_stdout(output)
}

/// Best-effort variant for status probing where failure is mapped to `None`.
pub fn try_git_output(cwd: &Path, args: &[&str]) -> Option<Vec<u8>> {
    let output = Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(args)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .output()
        .ok()?;
    output.status.success().then_some(output.stdout)
}

/// Limited variant that caps stdout at `limit + 1` bytes to detect overflow.
pub fn git_output_limited(cwd: &Path, args: &[&str], limit: u64) -> Result<Vec<u8>, GitError> {
    let mut child = Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(args)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let stderr = child.stderr.take().expect("piped stderr must exist");
    let stderr_reader = std::thread::spawn(move || {
        let mut stderr_bytes = Vec::new();
        let _ = stderr.take(64 * 1024).read_to_end(&mut stderr_bytes);
        stderr_bytes
    });
    let mut stdout = child.stdout.take().expect("piped stdout must exist");
    let mut bytes = Vec::new();
    stdout.by_ref().take(limit + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > limit {
        let _ = child.kill();
        let _ = child.wait();
        let _ = stderr_reader.join();
        return Err(GitError::DiffTooLarge);
    }
    let status = child.wait()?;
    let stderr_bytes = stderr_reader.join().unwrap_or_default();
    if status.success() {
        Ok(bytes)
    } else {
        Err(command_error(status, &stderr_bytes))
    }
}

pub fn git_result(output: std::process::Output) -> Result<(), GitError> {
    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    let message = if stderr.is_empty() {
        format!("git 命令失败：{}", output.status)
    } else {
        stderr
    };
    Err(GitError::CommandFailed(message))
}

pub fn git_stdout(output: std::process::Output) -> Result<Vec<u8>, GitError> {
    if output.status.success() {
        Ok(output.stdout)
    } else {
        Err(command_error(output.status, &output.stderr))
    }
}

pub fn command_error(status: std::process::ExitStatus, stderr: &[u8]) -> GitError {
    let stderr = String::from_utf8_lossy(stderr).trim().to_string();
    GitError::CommandFailed(if stderr.is_empty() {
        format!("git 命令失败：{status}")
    } else {
        stderr
    })
}

/// Shared `field` helper: trims `\r`/`\n` on both ends.
pub fn field(value: &[u8]) -> String {
    String::from_utf8_lossy(value)
        .trim_matches(['\r', '\n'])
        .to_string()
}
