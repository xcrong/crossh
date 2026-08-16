//! Pure Git stash inspection and operations.
//!
//! This module owns the command and parsing contract for the Git workbench.
//! It deliberately has no GPUI dependency.

use std::path::Path;
use std::process::Command;

use crate::git::GitError;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StashSummary {
    pub selector: String,
    pub id: String,
    pub date: String,
    pub message: String,
}

pub fn list_stashes(cwd: &Path) -> Result<Vec<StashSummary>, GitError> {
    let output = run_git(
        cwd,
        &[
            "stash".to_string(),
            "list".to_string(),
            "--format=%gd%x00%H%x00%ci%x00%gs".to_string(),
        ],
    )?;
    parse_stashes(&output)
}

pub fn push_stash(cwd: &Path) -> Result<(), GitError> {
    run_git(
        cwd,
        &[
            "stash".to_string(),
            "push".to_string(),
            "--include-untracked".to_string(),
        ],
    )?;
    Ok(())
}

pub fn apply_stash(cwd: &Path, selector: &str) -> Result<(), GitError> {
    run_stash_operation(cwd, "apply", selector, true)
}

pub fn pop_stash(cwd: &Path, selector: &str) -> Result<(), GitError> {
    run_stash_operation(cwd, "pop", selector, true)
}

pub fn drop_stash(cwd: &Path, selector: &str) -> Result<(), GitError> {
    run_stash_operation(cwd, "drop", selector, false)
}

fn run_stash_operation(
    cwd: &Path,
    operation: &str,
    selector: &str,
    restore_index: bool,
) -> Result<(), GitError> {
    validate_selector(selector)?;
    let mut args = vec!["stash".to_string(), operation.to_string()];
    if restore_index {
        args.push("--index".to_string());
    }
    args.push(selector.to_string());
    run_git(cwd, &args)?;
    Ok(())
}

fn parse_stashes(output: &[u8]) -> Result<Vec<StashSummary>, GitError> {
    let mut entries = Vec::new();
    for line in output.split(|byte| *byte == b'\n') {
        if line.is_empty() {
            continue;
        }
        let fields = line.split(|byte| *byte == 0).collect::<Vec<_>>();
        if fields.len() < 4 {
            return Err(GitError::CommandFailed(
                "Git Stash 列表格式无效".to_string(),
            ));
        }
        let selector = field(fields[0]);
        let id = field(fields[1]);
        if selector.is_empty() || id.is_empty() {
            return Err(GitError::CommandFailed(
                "Git Stash 列表格式无效".to_string(),
            ));
        }
        entries.push(StashSummary {
            selector,
            id,
            date: field(fields[2]),
            message: field(fields[3]),
        });
    }
    Ok(entries)
}

fn validate_selector(selector: &str) -> Result<(), GitError> {
    let Some(index) = selector
        .strip_prefix("stash@{")
        .and_then(|value| value.strip_suffix('}'))
    else {
        return Err(GitError::CommandFailed("Stash 引用无效".to_string()));
    };
    if index.is_empty() || !index.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(GitError::CommandFailed("Stash 引用无效".to_string()));
    }
    Ok(())
}

fn run_git(cwd: &Path, args: &[String]) -> Result<Vec<u8>, GitError> {
    let output = Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(args)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .output()?;
    if output.status.success() {
        return Ok(output.stdout);
    }
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    Err(GitError::CommandFailed(if stderr.is_empty() {
        format!("git 命令失败：{}", output.status)
    } else {
        stderr
    }))
}

fn field(value: &[u8]) -> String {
    String::from_utf8_lossy(value)
        .trim_matches(['\r', '\n'])
        .to_string()
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;
    use std::process::Command;

    use super::*;

    #[test]
    fn lists_and_applies_a_stash_with_untracked_files() {
        let dir = repository();
        write_commit(&dir, "note.txt", "base\n", "initial");
        fs::write(dir.join("note.txt"), "changed\n").unwrap();
        fs::write(dir.join("untracked.txt"), "new\n").unwrap();

        push_stash(&dir).unwrap();

        let entries = list_stashes(&dir).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].selector, "stash@{0}");
        assert!(entries[0].message.contains("initial"));
        assert_eq!(fs::read_to_string(dir.join("note.txt")).unwrap(), "base\n");
        assert!(!dir.join("untracked.txt").exists());

        apply_stash(&dir, &entries[0].selector).unwrap();

        assert_eq!(
            fs::read_to_string(dir.join("note.txt")).unwrap(),
            "changed\n"
        );
        assert_eq!(
            fs::read_to_string(dir.join("untracked.txt")).unwrap(),
            "new\n"
        );
        assert_eq!(list_stashes(&dir).unwrap().len(), 1);
    }

    #[test]
    fn pops_and_drops_only_valid_stash_selectors() {
        let dir = repository();
        write_commit(&dir, "note.txt", "base\n", "initial");
        fs::write(dir.join("note.txt"), "changed\n").unwrap();
        push_stash(&dir).unwrap();

        pop_stash(&dir, "stash@{0}").unwrap();
        assert!(list_stashes(&dir).unwrap().is_empty());
        assert_eq!(
            fs::read_to_string(dir.join("note.txt")).unwrap(),
            "changed\n"
        );

        fs::write(dir.join("note.txt"), "again\n").unwrap();
        push_stash(&dir).unwrap();
        assert!(drop_stash(&dir, "stash@{x}").is_err());
        assert!(drop_stash(&dir, "--version").is_err());
        drop_stash(&dir, "stash@{0}").unwrap();
        assert!(list_stashes(&dir).unwrap().is_empty());
    }

    #[test]
    fn rejects_invalid_git_directories() {
        let dir = tempfile::tempdir().unwrap();
        assert!(list_stashes(dir.path()).is_err());
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
