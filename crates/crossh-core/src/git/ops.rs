use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

use crate::git::command::{git_output_limited, git_result, run_git, run_git_paths};
use crate::git::diff::{diff_args, select_hunk_patch};
use crate::git::types::{ChangeStatus, FileChange, GitError, MAX_DIFF_BYTES};

/// 将指定工作区路径加入暂存区。
pub fn stage(cwd: &Path, paths: &[String]) -> Result<(), GitError> {
    if paths.is_empty() {
        return Ok(());
    }
    run_git_paths(cwd, &["add", "-A", "--"], paths)
}

/// 将指定路径从暂存区移回工作区。
pub fn unstage(cwd: &Path, paths: &[String]) -> Result<(), GitError> {
    if paths.is_empty() {
        return Ok(());
    }
    let has_head = Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(["rev-parse", "--verify", "HEAD"])
        .output()?
        .status
        .success();
    if has_head {
        run_git_paths(cwd, &["restore", "--staged", "--"], paths)
    } else {
        run_git_paths(cwd, &["rm", "--cached", "-r", "--"], paths)
    }
}

/// 将工作区 Diff 中指定的 Hunk 加入暂存区。
pub fn stage_hunk(cwd: &Path, entry: &FileChange, hunk_index: usize) -> Result<(), GitError> {
    if entry.staged {
        return Err(GitError::CommandFailed(
            "暂存 Hunk 需要选择工作区 Diff".to_string(),
        ));
    }
    apply_hunk(cwd, entry, false, hunk_index)
}

/// 将暂存区 Diff 中指定的 Hunk 移回工作区。
pub fn unstage_hunk(cwd: &Path, entry: &FileChange, hunk_index: usize) -> Result<(), GitError> {
    if !entry.staged {
        return Err(GitError::CommandFailed(
            "取消暂存 Hunk 需要选择暂存区 Diff".to_string(),
        ));
    }
    apply_hunk(cwd, entry, true, hunk_index)
}

/// 丢弃指定路径的工作区修改，但保留索引中的内容。
///
/// 已跟踪文件通过 `git restore --worktree` 还原；未跟踪文件通过 `git clean -f -d`
/// 删除，必要时回退到文件系统直接删除以覆盖被忽略文件的边界情况。
pub fn discard_worktree(cwd: &Path, paths: &[String]) -> Result<(), GitError> {
    if paths.is_empty() {
        return Ok(());
    }
    // 混合批次（已跟踪 + 未跟踪）下，批量 restore 会因未跟踪路径而整体失败，
    // 因此逐路径处理，确保已跟踪文件的还原不受未跟踪文件影响。
    for path in paths {
        let single = vec![path.clone()];
        let restore = run_git_paths(cwd, &["restore", "--worktree", "--"], &single);
        if restore.is_ok() {
            continue;
        }
        let clean = run_git_paths(cwd, &["clean", "-f", "-d", "--"], &single);
        if clean.is_ok() {
            let full = cwd.join(path);
            if !full.exists() {
                continue;
            }
            // `git clean` 对被忽略的未跟踪文件可能不生效，回退到直接文件系统删除。
            let fs_result = if full.is_dir() {
                std::fs::remove_dir_all(&full)
            } else {
                std::fs::remove_file(&full)
            };
            if fs_result.is_ok() && !full.exists() {
                continue;
            }
        }
        // 两种方式均未成功，返回 restore 的原始错误以保留可诊断信息。
        restore?;
    }
    Ok(())
}
/// 提交当前暂存区。
pub fn commit(cwd: &Path, message: &str) -> Result<(), GitError> {
    let message = message.trim();
    if message.is_empty() {
        return Err(GitError::CommandFailed("提交信息不能为空".to_string()));
    }
    run_git(cwd, &["commit", "-m", message])
}

/// 推送当前分支到已配置的上游；尚无上游时以 `origin` 为远程并建立跟踪。
pub fn push(cwd: &Path) -> Result<(), GitError> {
    if has_upstream(cwd)? {
        run_git(cwd, &["push"])
    } else {
        run_git(cwd, &["push", "-u", "origin", "HEAD"])
    }
}

/// 拉取上游变更并入当前分支；尚无上游时以 `origin` 的对应分支建立跟踪。
pub fn pull(cwd: &Path) -> Result<(), GitError> {
    if has_upstream(cwd)? {
        run_git(cwd, &["pull"])
    } else if let Some(branch) = current_branch(cwd)? {
        run_git(cwd, &["pull", "--set-upstream", "origin", &branch])
    } else {
        Err(GitError::CommandFailed(
            "当前不在任何分支上，无法拉取。请先切换到一个分支。".to_string(),
        ))
    }
}

fn has_upstream(cwd: &Path) -> Result<bool, GitError> {
    let output = Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args([
            "rev-parse",
            "--abbrev-ref",
            "--symbolic-full-name",
            "@{upstream}",
        ])
        .output()?;
    Ok(output.status.success())
}

/// 当前分支名；分离头指针或出错时返回 None。
fn current_branch(cwd: &Path) -> Result<Option<String>, GitError> {
    let output = Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .output()?;
    if !output.status.success() {
        return Ok(None);
    }
    let branch = String::from_utf8_lossy(&output.stdout).trim().to_string();
    Ok((branch != "HEAD").then_some(branch))
}

fn apply_hunk(
    cwd: &Path,
    entry: &FileChange,
    staged_diff: bool,
    hunk_index: usize,
) -> Result<(), GitError> {
    if matches!(
        entry.status,
        ChangeStatus::Untracked | ChangeStatus::Conflict
    ) {
        return Err(GitError::CommandFailed(
            "未跟踪文件和冲突文件暂不支持 Hunk 操作".to_string(),
        ));
    }

    let args = diff_args(entry, staged_diff);
    let output = git_output_limited(cwd, &args, MAX_DIFF_BYTES)?;
    let patch = select_hunk_patch(&output, hunk_index).ok_or_else(|| {
        GitError::CommandFailed(format!("找不到第 {} 个 Diff Hunk", hunk_index + 1))
    })?;

    let mut command = Command::new("git");
    command
        .arg("-C")
        .arg(cwd)
        .args(["apply", "--cached", "--recount"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if staged_diff {
        command.arg("--reverse");
    }
    let mut child = command.spawn()?;
    child
        .stdin
        .take()
        .expect("git apply stdin must be piped")
        .write_all(&patch)?;
    git_result(child.wait_with_output()?)
}
