//! Lightweight Git worktree inspection for the local-terminal status bar.

use std::path::Path;
use std::process::Command;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct GitStatus {
    pub branch: String,
    pub ahead: usize,
    pub behind: usize,
    pub staged: usize,
    pub modified: usize,
    pub untracked: usize,
    pub conflicts: usize,
}

impl GitStatus {
    pub(crate) fn is_clean(&self) -> bool {
        self.staged == 0 && self.modified == 0 && self.untracked == 0 && self.conflicts == 0
    }
}

pub(crate) fn inspect(cwd: &Path) -> Option<GitStatus> {
    let output = git(
        cwd,
        &[
            "status",
            "--porcelain=v2",
            "--branch",
            "-z",
            "--untracked-files=normal",
        ],
    )?;
    parse_status(&output)
}

fn git(cwd: &Path, args: &[&str]) -> Option<Vec<u8>> {
    let output = Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(args)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .output()
        .ok()?;
    output.status.success().then_some(output.stdout)
}

fn parse_status(output: &[u8]) -> Option<GitStatus> {
    let mut status = GitStatus::default();
    let mut oid = None;

    for record in output
        .split(|byte| *byte == 0)
        .filter(|record| !record.is_empty())
    {
        let record = String::from_utf8_lossy(record);
        if let Some(value) = record.strip_prefix("# branch.head ") {
            if value != "(detached)" {
                status.branch = value.to_string();
            }
            continue;
        }
        if let Some(value) = record.strip_prefix("# branch.oid ") {
            oid = Some(value.to_string());
            continue;
        }
        if let Some(value) = record.strip_prefix("# branch.ab ") {
            for part in value.split_whitespace() {
                if let Some(value) = part.strip_prefix('+') {
                    status.ahead = value.parse().unwrap_or(0);
                } else if let Some(value) = part.strip_prefix('-') {
                    status.behind = value.parse().unwrap_or(0);
                }
            }
            continue;
        }

        match record.as_bytes().first().copied() {
            Some(b'1' | b'2') => count_xy(&mut status, record.as_bytes().get(2..4)),
            Some(b'u') => status.conflicts += 1,
            Some(b'?') => status.untracked += 1,
            _ => {}
        }
    }

    if status.branch.is_empty() {
        let oid = oid?;
        status.branch = format!("detached@{}", &oid[..oid.len().min(7)]);
    }
    Some(status)
}

fn count_xy(status: &mut GitStatus, xy: Option<&[u8]>) {
    let Some([index, worktree]) = xy else {
        return;
    };
    if *index != b'.' {
        status.staged += 1;
    }
    if *worktree != b'.' {
        status.modified += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_branch_tracking_and_worktree_counts() {
        let output = b"# branch.oid abcdef123456\0# branch.head feature/status\0# branch.upstream origin/feature/status\0# branch.ab +2 -3\0\
1 M. N... 100644 100644 100644 aaa bbb staged.txt\0\
1 .M N... 100644 100644 100644 aaa bbb modified.txt\0\
2 RM N... 100644 100644 100644 aaa bbb R100 renamed.txt\0old.txt\0\
u UU N... 100644 100644 100644 100644 aaa bbb ccc conflict.txt\0\
? new.txt\0";
        let status = parse_status(output).unwrap();

        assert_eq!(status.branch, "feature/status");
        assert_eq!(status.ahead, 2);
        assert_eq!(status.behind, 3);
        assert_eq!(status.staged, 2);
        assert_eq!(status.modified, 2);
        assert_eq!(status.untracked, 1);
        assert_eq!(status.conflicts, 1);
        assert!(!status.is_clean());
    }

    #[test]
    fn labels_detached_head_with_short_oid() {
        let output = b"# branch.oid abcdef123456\0# branch.head (detached)\0";
        let status = parse_status(output).unwrap();
        assert_eq!(status.branch, "detached@abcdef1");
        assert!(status.is_clean());
    }
}
