//! Pure Git local branch inspection and switching.
//!
//! This module owns the branch command and parsing contract for the Git
//! workbench. It deliberately has no GPUI dependency.

use std::path::Path;

use crate::git::GitError;
use crate::git::command::{field, run_git_output};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BranchSummary {
    pub name: String,
    pub current: bool,
    pub upstream: Option<String>,
    pub ahead: usize,
    pub behind: usize,
    pub upstream_gone: bool,
    pub commit: String,
    pub subject: String,
}

/// Lists local branches, with the current branch first and then recent branches.
pub fn list_branches(cwd: &Path) -> Result<Vec<BranchSummary>, GitError> {
    let output = run_git_output(
        cwd,
        &[
            "for-each-ref".to_string(),
            "--sort=-committerdate".to_string(),
            "--format=%(HEAD)%00%(refname:short)%00%(upstream:short)%00%(upstream:track)%00%(objectname:short)%00%(subject)%00".to_string(),
            "refs/heads".to_string(),
        ],
    )?;
    parse_branches(&output)
}

/// Switches to an existing local branch after validating its ref name.
pub fn switch_branch(cwd: &Path, name: &str) -> Result<(), GitError> {
    run_git_output(
        cwd,
        &[
            "check-ref-format".to_string(),
            "--branch".to_string(),
            name.to_string(),
        ],
    )?;
    run_git_output(
        cwd,
        &[
            "switch".to_string(),
            "--quiet".to_string(),
            "--".to_string(),
            name.to_string(),
        ],
    )?;
    Ok(())
}

fn parse_branches(output: &[u8]) -> Result<Vec<BranchSummary>, GitError> {
    let mut branches = Vec::new();
    for fields in output
        .split(|byte| *byte == 0)
        .collect::<Vec<_>>()
        .chunks(6)
    {
        if fields.len() < 6 {
            continue;
        }
        let name = field(fields[1]);
        if name.is_empty() {
            continue;
        }
        let track = field(fields[3]);
        let (ahead, behind, upstream_gone) = parse_tracking(&track);
        branches.push(BranchSummary {
            name,
            current: field(fields[0]).trim() == "*",
            upstream: (!fields[2].is_empty()).then(|| field(fields[2])),
            ahead,
            behind,
            upstream_gone,
            commit: field(fields[4]),
            subject: field(fields[5]),
        });
    }
    branches.sort_by(|left, right| {
        right.current.cmp(&left.current).then_with(|| {
            left.name
                .to_ascii_lowercase()
                .cmp(&right.name.to_ascii_lowercase())
        })
    });
    Ok(branches)
}

fn parse_tracking(value: &str) -> (usize, usize, bool) {
    let value = value.trim();
    if value == "[gone]" {
        return (0, 0, true);
    }
    (
        tracking_count(value, "ahead "),
        tracking_count(value, "behind "),
        false,
    )
}

fn tracking_count(value: &str, marker: &str) -> usize {
    value
        .split_once(marker)
        .and_then(|(_, rest)| {
            rest.split(|character| [',', ']', ' '].contains(&character))
                .next()
        })
        .and_then(|count| count.parse().ok())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::process::Command;

    use super::*;

    #[test]
    fn lists_current_branch_and_tracking_counts() {
        let dir = repository();
        write_commit(&dir, "README.md", "first\n", "first commit");
        run(&dir, &["branch", "-M", "main"]);
        run(&dir, &["branch", "feature/history"]);
        run(&dir, &["branch", "release/v1"]);
        run(&dir, &["switch", "-q", "feature/history"]);
        run(&dir, &["branch", "--set-upstream-to=main"]);
        write_commit(&dir, "feature.txt", "feature\n", "feature commit");

        let branches = list_branches(&dir).unwrap();

        assert_eq!(branches[0].name, "feature/history");
        assert!(branches[0].current);
        assert_eq!(branches[0].upstream.as_deref(), Some("main"));
        assert_eq!(branches[0].ahead, 1);
        assert_eq!(branches[0].behind, 0);
        assert!(branches.iter().any(|branch| branch.name == "release/v1"));
    }

    #[test]
    fn switches_only_to_a_valid_existing_branch() {
        let dir = repository();
        write_commit(&dir, "README.md", "first\n", "first commit");
        run(&dir, &["branch", "feature/history"]);

        switch_branch(&dir, "feature/history").unwrap();
        assert_eq!(current_branch(&dir), "feature/history");
        assert!(switch_branch(&dir, "--version").is_err());
        assert!(switch_branch(&dir, "missing/branch").is_err());
    }

    #[test]
    fn parses_tracking_ahead_behind_and_gone_states() {
        assert_eq!(parse_tracking("[ahead 2, behind 3]"), (2, 3, false));
        assert_eq!(parse_tracking("[gone]"), (0, 0, true));
        assert_eq!(parse_tracking(""), (0, 0, false));
    }

    fn repository() -> std::path::PathBuf {
        let dir = tempfile::tempdir().unwrap().keep();
        run(&dir, &["init", "-q"]);
        run(&dir, &["config", "user.email", "test@crossh.local"]);
        run(&dir, &["config", "user.name", "Crossh Test"]);
        dir
    }

    fn write_commit(dir: &Path, path: &str, content: &str, subject: &str) {
        std::fs::write(dir.join(path), content).unwrap();
        run(dir, &["add", "-A"]);
        run(dir, &["commit", "-qm", subject]);
    }

    fn current_branch(dir: &Path) -> String {
        let output = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(["branch", "--show-current"])
            .output()
            .unwrap();
        assert!(output.status.success());
        String::from_utf8_lossy(&output.stdout).trim().to_string()
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
