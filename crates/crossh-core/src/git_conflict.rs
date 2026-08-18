//! Pure Git conflict resolution operations.
//!
//! This module deliberately contains no GPUI imports. It turns an explicit
//! user choice into a bounded Git command and stages the resolved path.

use std::path::Path;
use std::process::Command;

use crate::git::GitError;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConflictResolution {
    Ours,
    Theirs,
    MarkResolved,
}

pub fn resolve_conflict(
    cwd: &Path,
    path: &str,
    resolution: ConflictResolution,
) -> Result<(), GitError> {
    if path.is_empty() || path == "." {
        return Err(GitError::CommandFailed("冲突路径无效".to_string()));
    }
    if matches!(
        resolution,
        ConflictResolution::Ours | ConflictResolution::Theirs
    ) {
        let side = match resolution {
            ConflictResolution::Ours => "--ours",
            ConflictResolution::Theirs => "--theirs",
            ConflictResolution::MarkResolved => unreachable!(),
        };
        run_git_paths(cwd, &["checkout", side, "--"], path)?;
    }
    run_git_paths(cwd, &["add", "--"], path)
}

fn run_git_paths(cwd: &Path, args: &[&str], path: &str) -> Result<(), GitError> {
    let output = Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(args)
        .arg(path)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .output()?;
    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    Err(GitError::CommandFailed(if stderr.is_empty() {
        format!("git 命令失败：{}", output.status)
    } else {
        stderr
    }))
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;
    use std::process::Command;

    use crate::git::{ChangeStatus, list_changes};

    use super::*;

    #[test]
    fn resolves_a_real_merge_conflict_with_each_explicit_choice() {
        for (resolution, expected) in [
            (ConflictResolution::Ours, "main\n"),
            (ConflictResolution::Theirs, "feature\n"),
            (
                ConflictResolution::MarkResolved,
                "<<<<<<< HEAD\nmain\n=======\nfeature\n>>>>>>> feature\n",
            ),
        ] {
            let dir = repository();
            write_commit(&dir, "conflict.txt", "base\n", "initial");
            run(&dir, &["switch", "-c", "feature"]);
            write_commit(&dir, "conflict.txt", "feature\n", "feature change");
            run(&dir, &["switch", "main"]);
            write_commit(&dir, "conflict.txt", "main\n", "main change");

            let merge = Command::new("git")
                .arg("-C")
                .arg(&dir)
                .args(["merge", "feature"])
                .output()
                .unwrap();
            assert!(!merge.status.success());
            let conflict = list_changes(&dir)
                .unwrap()
                .into_iter()
                .find(|change| change.path == "conflict.txt")
                .expect("merge conflict should be listed");
            assert_eq!(conflict.status, ChangeStatus::Conflict);

            resolve_conflict(&dir, "conflict.txt", resolution).unwrap();

            assert_eq!(
                fs::read_to_string(dir.join("conflict.txt"))
                    .unwrap()
                    .replace("\r\n", "\n"),
                expected
            );
            assert!(
                !list_changes(&dir)
                    .unwrap()
                    .iter()
                    .any(|change| change.status == ChangeStatus::Conflict)
            );
        }
    }

    #[test]
    fn keeps_paths_after_the_separator_and_rejects_empty_paths() {
        let dir = repository();
        write_commit(&dir, "note.txt", "base\n", "initial");
        fs::write(dir.join("note.txt"), "changed\n").unwrap();
        assert!(resolve_conflict(&dir, "", ConflictResolution::Ours).is_err());
        assert!(resolve_conflict(&dir, ".", ConflictResolution::Theirs).is_err());
    }

    fn repository() -> std::path::PathBuf {
        let dir = tempfile::tempdir().unwrap().keep();
        run(&dir, &["init", "-q"]);
        run(&dir, &["branch", "-M", "main"]);
        run(&dir, &["config", "user.email", "test@crossh.local"]);
        run(&dir, &["config", "user.name", "Crossh Test"]);
        dir
    }

    fn write_commit(dir: &Path, path: &str, content: &str, subject: &str) {
        fs::write(dir.join(path), content).unwrap();
        run(dir, &["add", "-A"]);
        run(dir, &["commit", "-qm", subject]);
    }

    fn run(dir: &Path, args: &[&str]) {
        let output = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .output()
            .unwrap();
        assert!(output.status.success(), "{args:?}: {:?}", output.stderr);
    }
}
