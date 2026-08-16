//! Pure Git commit history and commit detail inspection.
//!
//! This module owns the command and parsing contract for the Git workbench.
//! It deliberately has no GPUI dependency.

use std::path::Path;
use std::process::Command;

use crate::git::GitError;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommitSummary {
    pub id: String,
    pub short_id: String,
    pub author: String,
    pub date: String,
    pub subject: String,
    pub parents: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HistoryRefKind {
    LocalBranch,
    RemoteBranch,
    Tag,
    Head,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HistoryRef {
    pub name: String,
    pub target: String,
    pub kind: HistoryRefKind,
    pub current: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HistorySnapshot {
    pub entries: Vec<CommitSummary>,
    pub refs: Vec<HistoryRef>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommitFileChange {
    pub path: String,
    pub old_path: Option<String>,
    pub insertions: usize,
    pub deletions: usize,
    pub binary: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommitDetail {
    pub summary: CommitSummary,
    pub body: String,
    pub files: Vec<CommitFileChange>,
}

pub const DEFAULT_HISTORY_LIMIT: usize = 200;
const MAX_HISTORY_LIMIT: usize = 500;
const RECORD_SEPARATOR: u8 = 0x1e;

pub fn list_history(cwd: &Path, limit: usize) -> Result<HistorySnapshot, GitError> {
    if limit == 0 {
        return Ok(HistorySnapshot {
            entries: Vec::new(),
            refs: Vec::new(),
        });
    }
    let limit = limit.min(MAX_HISTORY_LIMIT);
    let output = run_git(
        cwd,
        &[
            "log".to_string(),
            "--all".to_string(),
            "HEAD".to_string(),
            "--topo-order".to_string(),
            "--no-color".to_string(),
            "--no-decorate".to_string(),
            "--date=iso-strict".to_string(),
            "--format=%x1e%H%x00%h%x00%an%x00%aI%x00%s%x00%P".to_string(),
            "-n".to_string(),
            limit.to_string(),
        ],
    )?;
    Ok(HistorySnapshot {
        entries: parse_history(&output)?,
        refs: list_refs(cwd)?,
    })
}

pub fn show_commit(cwd: &Path, id: &str) -> Result<CommitDetail, GitError> {
    validate_revision(id)?;
    let metadata = run_git(
        cwd,
        &[
            "show".to_string(),
            "-s".to_string(),
            "--no-color".to_string(),
            "--no-decorate".to_string(),
            "--date=iso-strict".to_string(),
            "--format=%H%x00%h%x00%an%x00%aI%x00%s%x00%P%x00%B".to_string(),
            id.to_string(),
            "--".to_string(),
        ],
    )?;
    let summary_and_body = parse_detail_metadata(&metadata)?;
    let numstat = run_git(
        cwd,
        &[
            "show".to_string(),
            "--no-color".to_string(),
            "--no-ext-diff".to_string(),
            "--format=".to_string(),
            "--numstat".to_string(),
            "--find-renames".to_string(),
            "--find-copies".to_string(),
            "-z".to_string(),
            id.to_string(),
            "--".to_string(),
        ],
    )?;
    Ok(CommitDetail {
        summary: summary_and_body.0,
        body: summary_and_body.1,
        files: parse_numstat(&numstat),
    })
}

fn run_git(cwd: &Path, args: &[String]) -> Result<Vec<u8>, GitError> {
    let output = Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(args)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .output()?;
    if output.status.success() {
        Ok(output.stdout)
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        Err(GitError::CommandFailed(if stderr.is_empty() {
            format!("git 命令失败：{}", output.status)
        } else {
            stderr
        }))
    }
}

fn list_refs(cwd: &Path) -> Result<Vec<HistoryRef>, GitError> {
    let output = run_git(
        cwd,
        &[
            "for-each-ref".to_string(),
            "--sort=-committerdate".to_string(),
            "--format=%(refname)%00%(refname:short)%00%(objectname)%00%(*objectname)%00%(HEAD)"
                .to_string(),
            "refs/heads".to_string(),
            "refs/remotes".to_string(),
            "refs/tags".to_string(),
        ],
    )?;
    let mut refs = parse_refs(&output);

    // Detached HEAD does not have a branch decoration, but it is still useful
    // to show the selected commit as the active revision in the graph.
    if !refs.iter().any(|reference| reference.current)
        && let Ok(head) = run_git(cwd, &["rev-parse".to_string(), "HEAD".to_string()])
    {
        let target = field(&head);
        if !target.is_empty() {
            refs.push(HistoryRef {
                name: "HEAD".to_string(),
                target,
                kind: HistoryRefKind::Head,
                current: true,
            });
        }
    }
    Ok(refs)
}

fn parse_refs(output: &[u8]) -> Vec<HistoryRef> {
    let mut refs = Vec::new();
    for line in output.split(|byte| *byte == b'\n') {
        let fields = line.split(|byte| *byte == 0).collect::<Vec<_>>();
        if fields.len() < 5 {
            continue;
        }
        let full_name = field(fields[0]);
        let name = field(fields[1]);
        let object = field(fields[2]);
        let peeled = field(fields[3]);
        let target = if peeled.is_empty() { object } else { peeled };
        if name.is_empty() || target.is_empty() {
            continue;
        }
        let kind = if full_name.starts_with("refs/heads/") {
            HistoryRefKind::LocalBranch
        } else if full_name.starts_with("refs/remotes/") {
            HistoryRefKind::RemoteBranch
        } else if full_name.starts_with("refs/tags/") {
            HistoryRefKind::Tag
        } else {
            continue;
        };
        refs.push(HistoryRef {
            name,
            target,
            kind,
            current: field(fields[4]) == "*",
        });
    }
    refs
}

fn parse_history(output: &[u8]) -> Result<Vec<CommitSummary>, GitError> {
    let mut entries = Vec::new();
    for record in output.split(|byte| *byte == RECORD_SEPARATOR).skip(1) {
        if record.is_empty() {
            continue;
        }
        let fields = record.split(|byte| *byte == 0).collect::<Vec<_>>();
        if fields.len() < 6 {
            return Err(GitError::CommandFailed("Git 提交历史格式无效".to_string()));
        }
        entries.push(CommitSummary {
            id: field(fields[0]),
            short_id: field(fields[1]),
            author: field(fields[2]),
            date: field(fields[3]),
            subject: field(fields[4]),
            parents: field(fields[5])
                .split_whitespace()
                .map(str::to_string)
                .collect(),
        });
    }
    Ok(entries)
}

fn parse_detail_metadata(output: &[u8]) -> Result<(CommitSummary, String), GitError> {
    let mut fields = output.splitn(7, |byte| *byte == 0);
    let id = fields.next().map(field).unwrap_or_default();
    let short_id = fields.next().map(field).unwrap_or_default();
    let author = fields.next().map(field).unwrap_or_default();
    let date = fields.next().map(field).unwrap_or_default();
    let subject = fields.next().map(field).unwrap_or_default();
    let parents = fields
        .next()
        .map(field)
        .unwrap_or_default()
        .split_whitespace()
        .map(str::to_string)
        .collect();
    let body = fields.next().map(field).unwrap_or_default();
    if id.is_empty() || subject.is_empty() {
        return Err(GitError::CommandFailed("Git 提交详情格式无效".to_string()));
    }
    Ok((
        CommitSummary {
            id,
            short_id,
            author,
            date,
            subject,
            parents,
        },
        body,
    ))
}

fn parse_numstat(output: &[u8]) -> Vec<CommitFileChange> {
    let records = output.split(|byte| *byte == 0).collect::<Vec<_>>();
    let mut files = Vec::new();
    let mut index = 0;
    while let Some(record) = records.get(index) {
        index += 1;
        if record.is_empty() {
            continue;
        }
        let mut fields = record.splitn(3, |byte| *byte == b'\t');
        let Some(added) = fields.next() else { continue };
        let Some(deleted) = fields.next() else {
            continue;
        };
        let Some(path) = fields.next() else { continue };
        let (path, old_path) = if path.is_empty() {
            let Some(old_path) = records.get(index) else {
                continue;
            };
            let Some(new_path) = records.get(index + 1) else {
                continue;
            };
            index += 2;
            (
                String::from_utf8_lossy(new_path).into_owned(),
                Some(String::from_utf8_lossy(old_path).into_owned()),
            )
        } else {
            (String::from_utf8_lossy(path).into_owned(), None)
        };
        files.push(CommitFileChange {
            path,
            old_path,
            insertions: parse_count(added),
            deletions: parse_count(deleted),
            binary: added == b"-" || deleted == b"-",
        });
    }
    files
}

fn parse_count(value: &[u8]) -> usize {
    std::str::from_utf8(value)
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(0)
}

fn field(value: &[u8]) -> String {
    String::from_utf8_lossy(value)
        .trim_end_matches(['\r', '\n'])
        .to_string()
}

fn validate_revision(id: &str) -> Result<(), GitError> {
    if id.is_empty() || id.len() > 64 || !id.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(GitError::CommandFailed("提交 ID 无效".to_string()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::process::Command;

    use super::*;

    #[test]
    fn lists_history_in_newest_first_order() {
        let dir = repository();
        write_commit(&dir, "README.md", "first\n", "first commit", None);
        write_commit(
            &dir,
            "README.md",
            "second\n",
            "second commit",
            Some("body line"),
        );

        let entries = list_history(&dir, 10).unwrap();

        assert_eq!(entries.entries.len(), 2);
        assert_eq!(entries.entries[0].subject, "second commit");
        assert_eq!(entries.entries[1].subject, "first commit");
        assert_eq!(entries.entries[0].short_id.len(), 7);
        assert_eq!(
            entries.entries[0].parents,
            vec![entries.entries[1].id.clone()]
        );
    }

    #[test]
    fn shows_commit_body_and_numstat_for_whitespace_paths() {
        let dir = repository();
        write_commit(&dir, "README.md", "first\n", "first commit", None);
        std::fs::write(dir.join("space  name.txt"), "new\n").unwrap();
        std::fs::write(dir.join("README.md"), "second\n").unwrap();
        run(&dir, &["add", "-A"]);
        run(&dir, &["commit", "-qm", "second commit", "-m", "body line"]);

        let summary = list_history(&dir, 1).unwrap().entries.remove(0);
        let detail = show_commit(&dir, &summary.id).unwrap();

        assert_eq!(detail.summary, summary);
        assert!(detail.body.contains("body line"));
        assert!(detail.files.iter().any(|file| {
            file.path == "README.md" && file.insertions == 1 && file.deletions == 1
        }));
        assert!(detail.files.iter().any(|file| {
            file.path == "space  name.txt" && file.insertions == 1 && file.deletions == 0
        }));
    }

    #[test]
    fn rejects_non_hex_revisions_before_spawning_git() {
        let dir = repository();

        assert!(show_commit(&dir, "HEAD~1").is_err());
        assert!(show_commit(&dir, "--version").is_err());
    }

    #[test]
    fn lists_local_and_tag_refs_for_all_history() {
        let dir = repository();
        write_commit(&dir, "README.md", "first\n", "first commit", None);
        run(&dir, &["branch", "feature"]);
        run(&dir, &["tag", "v1"]);

        let snapshot = list_history(&dir, 10).unwrap();

        assert!(
            snapshot.refs.iter().any(|reference| {
                reference.name == "v1" && reference.kind == HistoryRefKind::Tag
            }),
            "refs: {:?}",
            snapshot.refs
        );
        assert!(snapshot.refs.iter().any(|reference| {
            reference.name == "feature" && reference.kind == HistoryRefKind::LocalBranch
        }));
        assert!(snapshot
            .refs
            .iter()
            .any(|reference| reference.current && reference.kind == HistoryRefKind::LocalBranch));
        assert_eq!(snapshot.entries.len(), 1);
    }

    #[test]
    fn includes_detached_head_in_history_and_refs() {
        let dir = repository();
        write_commit(&dir, "README.md", "first\n", "first commit", None);
        run(&dir, &["switch", "--detach", "HEAD"]);

        let snapshot = list_history(&dir, 10).unwrap();

        assert_eq!(snapshot.entries.len(), 1);
        assert!(snapshot.refs.iter().any(|reference| {
            reference.name == "HEAD" && reference.kind == HistoryRefKind::Head && reference.current
        }));
    }

    fn repository() -> std::path::PathBuf {
        let dir = tempfile::tempdir().unwrap().keep();
        run(&dir, &["init", "-q"]);
        run(&dir, &["config", "user.email", "test@crossh.local"]);
        run(&dir, &["config", "user.name", "Crossh Test"]);
        dir
    }

    fn write_commit(dir: &Path, path: &str, content: &str, subject: &str, body: Option<&str>) {
        std::fs::write(dir.join(path), content).unwrap();
        run(dir, &["add", "-A"]);
        let mut args = vec!["commit", "-qm", subject];
        if let Some(body) = body {
            args.extend(["-m", body]);
        }
        run(dir, &args);
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
