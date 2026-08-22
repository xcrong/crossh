//! Pure Git worktree + diff inspection for the Git window.
//!
//! No `gpui` imports: this module only shells out to `git` and parses its
//! `--porcelain=v2` status, `--numstat` counters, and unified diff output.

pub mod command;
pub mod numstat;

use std::collections::HashMap;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

use self::command::{git_output, git_output_limited, git_result, run_git, run_git_paths};
use self::numstat::numstat_map;
use crate::git_status::{GitStatus, parse_status};

const MAX_DIFF_BYTES: u64 = 2 * 1024 * 1024;
const MAX_DIFF_LINES: usize = 10_000;

#[derive(Debug, thiserror::Error)]
pub enum GitError {
    #[error("无法执行 git：{0}")]
    Spawn(#[from] std::io::Error),
    #[error("{0}")]
    CommandFailed(String),
    #[error("差异内容超过 {MAX_DIFF_BYTES} 字节，未加载以保持界面可用")]
    DiffTooLarge,
}

/// 单个文件在索引（已暂存）或工作区（未暂存）中的变更状态。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChangeStatus {
    Modified,
    Added,
    Deleted,
    Renamed,
    Conflict,
    Untracked,
}

impl ChangeStatus {
    /// 组内排序权重：冲突/新增优先于修改，便于稳定排序。
    pub fn rank(self) -> u8 {
        match self {
            Self::Conflict => 0,
            Self::Added | Self::Renamed => 1,
            Self::Deleted => 2,
            Self::Modified => 3,
            Self::Untracked => 4,
        }
    }

    /// 列表行首的状态字形（VS Code 风格）。
    pub fn glyph(self) -> &'static str {
        match self {
            Self::Modified => "M",
            Self::Added => "A",
            Self::Deleted => "D",
            Self::Renamed => "R",
            Self::Conflict => "!",
            Self::Untracked => "?",
        }
    }
}

/// 一条（路径，暂存态）上的改动。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileChange {
    /// 当前磁盘上的路径（重命名的目标路径）。
    pub path: String,
    /// 重命名/复制的源路径。
    pub orig_path: Option<String>,
    pub status: ChangeStatus,
    /// true = 已暂存（相对 HEAD），false = 工作区未暂存（相对索引）。
    pub staged: bool,
    pub insertions: usize,
    pub deletions: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiffLineKind {
    /// `@@ -a,b +c,d @@` 的 hunk 头。
    Hunk,
    Context,
    Added,
    Removed,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiffLine {
    pub kind: DiffLineKind,
    /// 所属 Hunk 的索引；未跟踪文件的合成新增行没有 Hunk。
    pub hunk_index: Option<usize>,
    /// 旧文件行号（`-` / 上下文行有值，新增行为 None）。
    pub old_ln: Option<u32>,
    /// 新文件行号（`+` / 上下文行有值，删除行为 None）。
    pub new_ln: Option<u32>,
    pub text: String,
}

/// 一个文件的完整 diff 内容（选中行渲染用）。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FileDiff {
    pub old_path: Option<String>,
    pub new_path: Option<String>,
    pub lines: Vec<DiffLine>,
    /// 二进制文件：无文本 hunk。
    pub binary: bool,
}

/// 一次工作区扫描的变更列表与分支状态。
///
/// 两者来自同一份 porcelain 输出，避免界面层为状态栏额外启动一次 `git status`。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ChangeScan {
    pub changes: Vec<FileChange>,
    pub status: Option<GitStatus>,
}

/// 扫描目录下的全部改动（已暂存 + 未暂存），每个（路径，暂存态）一条。
pub fn list_changes(cwd: &Path) -> Result<Vec<FileChange>, GitError> {
    Ok(scan_changes(cwd)?.changes)
}

/// 扫描目录下的变更与分支状态。
pub fn scan_changes(cwd: &Path) -> Result<ChangeScan, GitError> {
    let output = git_output(
        cwd,
        // 逐个列出未追踪文件（而非折叠成 `dir/`），使其真正可见。
        &[
            "status",
            "--porcelain=v2",
            "--branch",
            "-z",
            "--untracked-files=all",
        ],
    )?;
    let staged_counts = numstat_map(&git_output(cwd, &["diff", "--cached", "--numstat", "-z"])?);
    let working_counts = numstat_map(&git_output(cwd, &["diff", "--numstat", "-z"])?);

    let mut changes = Vec::new();
    let records = output.split(|byte| *byte == 0).collect::<Vec<_>>();
    let mut index = 0;
    while index < records.len() {
        let record = records[index];
        index += 1;
        if record.is_empty() || record.first() == Some(&b'#') {
            continue;
        }

        match record.first().copied() {
            Some(b'1' | b'2') => {
                let field_count = if record[0] == b'2' { 9 } else { 8 };
                let Some((prefix, path)) = split_after_spaces(record, field_count) else {
                    continue;
                };
                let mut fields = prefix.split(|byte| *byte == b' ');
                let _kind = fields.next();
                let Some(xy) = fields.next() else {
                    continue;
                };
                let path = String::from_utf8_lossy(path).into_owned();
                let mut orig_path = None;
                if record[0] == b'2' {
                    // -z 模式下源路径是记录后的下一个 NUL 片段。
                    if let Some(extra) = records.get(index)
                        && !extra.is_empty()
                    {
                        orig_path = Some(String::from_utf8_lossy(extra).into_owned());
                        index += 1;
                    }
                }
                let index_status = xy.first().copied().unwrap_or(b'.');
                let worktree_status = xy.get(1).copied().unwrap_or(b'.');
                if index_status != b'.' {
                    changes.push(file_change(
                        &path,
                        orig_path.clone(),
                        index_status,
                        true,
                        &staged_counts,
                    ));
                }
                if worktree_status != b'.' {
                    changes.push(file_change(
                        &path,
                        if is_rename(index_status) {
                            orig_path.clone()
                        } else {
                            None
                        },
                        worktree_status,
                        false,
                        &working_counts,
                    ));
                }
            }
            Some(b'u') => {
                let Some((_, path)) = split_after_spaces(record, 10) else {
                    continue;
                };
                changes.push(FileChange {
                    path: String::from_utf8_lossy(path).into_owned(),
                    orig_path: None,
                    status: ChangeStatus::Conflict,
                    staged: false,
                    insertions: 0,
                    deletions: 0,
                });
            }
            Some(b'?') => {
                let Some(space) = record.iter().position(|byte| *byte == b' ') else {
                    continue;
                };
                changes.push(FileChange {
                    path: String::from_utf8_lossy(&record[space + 1..]).into_owned(),
                    orig_path: None,
                    status: ChangeStatus::Untracked,
                    staged: false,
                    insertions: 0,
                    deletions: 0,
                });
            }
            _ => {}
        }
    }

    changes.sort_by_key(|entry| (!entry.staged, entry.status.rank(), entry.path.clone()));
    Ok(ChangeScan {
        changes,
        status: parse_status(&output),
    })
}

/// 读取某个文件（按暂存态）的统一 diff。
pub fn diff(cwd: &Path, entry: &FileChange, staged: bool) -> Result<Option<FileDiff>, GitError> {
    if entry.status == ChangeStatus::Untracked {
        return untracked_diff(cwd, entry);
    }
    let args = diff_args(entry, staged);
    let output = git_output_limited(cwd, &args, MAX_DIFF_BYTES)?;
    parse_diff(&output)
}

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
/// 调用方应先确认路径是已跟踪的工作区改动；未跟踪文件不属于该操作。
pub fn discard_worktree(cwd: &Path, paths: &[String]) -> Result<(), GitError> {
    if paths.is_empty() {
        return Ok(());
    }
    run_git_paths(cwd, &["restore", "--worktree", "--"], paths)
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

fn diff_args(entry: &FileChange, staged: bool) -> Vec<&str> {
    let mut args = vec!["diff", "--no-color", "--no-ext-diff", "--unified=3"];
    if staged {
        args.push("--cached");
    }
    args.push("--");
    // 重命名必须同时给源与目标路径，否则 Git 会把新文件当作全新文件比较。
    if let Some(orig) = entry.orig_path.as_deref() {
        args.push(orig);
    }
    args.push(&entry.path);
    args
}

/// 从单文件 unified diff 中保留文件头和指定 Hunk，生成可供 `git apply` 使用的补丁。
fn select_hunk_patch(output: &[u8], hunk_index: usize) -> Option<Vec<u8>> {
    let mut hunk_ranges = Vec::new();
    let mut offset = 0;
    for line in output.split_inclusive(|byte| *byte == b'\n') {
        if line.starts_with(b"@@ ") {
            hunk_ranges.push(offset);
        }
        offset += line.len();
    }
    let header_end = *hunk_ranges.first()?;
    let start = *hunk_ranges.get(hunk_index)?;
    let end = hunk_ranges
        .get(hunk_index + 1)
        .copied()
        .unwrap_or(output.len());
    let mut patch = output[..header_end].to_vec();
    patch.extend_from_slice(&output[start..end]);
    Some(patch)
}

/// 未跟踪文件没有可 diff 的基线：把整个文件内容当作新增行呈现。
fn untracked_diff(cwd: &Path, entry: &FileChange) -> Result<Option<FileDiff>, GitError> {
    let path = cwd.join(&entry.path);
    if !path.is_file() {
        return Ok(None);
    }
    if std::fs::metadata(&path)?.len() > MAX_DIFF_BYTES {
        return Err(GitError::DiffTooLarge);
    }
    let bytes = std::fs::read(&path)?;
    let Ok(text) = String::from_utf8(bytes) else {
        return Ok(Some(FileDiff {
            binary: true,
            ..FileDiff::default()
        }));
    };
    let mut lines: Vec<&str> = text.split('\n').collect();
    if text.ends_with('\n') {
        lines.pop();
    }
    let mut produced = Vec::new();
    if lines.len() > MAX_DIFF_LINES {
        return Err(GitError::DiffTooLarge);
    }
    for (index, line) in lines.into_iter().enumerate() {
        produced.push(DiffLine {
            kind: DiffLineKind::Added,
            hunk_index: None,
            old_ln: None,
            new_ln: Some(index as u32 + 1),
            text: line.to_string(),
        });
    }
    Ok(Some(FileDiff {
        old_path: None,
        new_path: Some(entry.path.clone()),
        lines: produced,
        ..FileDiff::default()
    }))
}

/// 解析统一 diff 输出为逐行结构。
fn parse_diff(output: &[u8]) -> Result<Option<FileDiff>, GitError> {
    let text = String::from_utf8_lossy(output);
    if text.contains("Binary files") && !text.contains("@@ ") {
        return Ok(Some(FileDiff {
            binary: true,
            ..FileDiff::default()
        }));
    }
    let mut diff = FileDiff::default();
    let mut old_ln = 0u32;
    let mut new_ln = 0u32;
    let mut current_hunk_index = None;
    for line in text.lines() {
        if let Some((old_start, new_start)) = parse_hunk_header(line) {
            old_ln = old_start;
            new_ln = new_start;
            current_hunk_index = Some(
                diff.lines
                    .iter()
                    .filter(|line| line.kind == DiffLineKind::Hunk)
                    .count(),
            );
            diff.lines.push(DiffLine {
                kind: DiffLineKind::Hunk,
                hunk_index: current_hunk_index,
                old_ln: None,
                new_ln: None,
                text: line.to_string(),
            });
            if diff.lines.len() > MAX_DIFF_LINES {
                return Err(GitError::DiffTooLarge);
            }
            continue;
        }
        // 头部行只可能在第一个 hunk 之前出现：之后以 `---`/`+++` 开头的内容行
        // 都是被删除/新增的真实文本。
        if diff.lines.is_empty() {
            if let Some(content) = line.strip_prefix("--- a/") {
                diff.old_path = Some(content.to_string());
                continue;
            }
            if line.starts_with("--- /dev/null") {
                diff.old_path = None;
                continue;
            }
            if let Some(content) = line.strip_prefix("+++ b/") {
                diff.new_path = Some(content.to_string());
                continue;
            }
        }
        if let Some(content) = line.strip_prefix('+') {
            diff.lines.push(DiffLine {
                kind: DiffLineKind::Added,
                hunk_index: current_hunk_index,
                old_ln: None,
                new_ln: Some(new_ln),
                text: content.to_string(),
            });
            new_ln += 1;
        } else if let Some(content) = line.strip_prefix('-') {
            diff.lines.push(DiffLine {
                kind: DiffLineKind::Removed,
                hunk_index: current_hunk_index,
                old_ln: Some(old_ln),
                new_ln: None,
                text: content.to_string(),
            });
            old_ln += 1;
        } else if let Some(content) = line.strip_prefix(' ') {
            diff.lines.push(DiffLine {
                kind: DiffLineKind::Context,
                hunk_index: current_hunk_index,
                old_ln: Some(old_ln),
                new_ln: Some(new_ln),
                text: content.to_string(),
            });
            old_ln += 1;
            new_ln += 1;
        }
        if diff.lines.len() > MAX_DIFF_LINES {
            return Err(GitError::DiffTooLarge);
        }
    }
    if diff.lines.is_empty() && !diff.binary {
        return Ok(None);
    }
    Ok(Some(diff))
}

/// `@@ -a[,b] +c[,d] @@` → (old_start, new_start)。失败返回 None。
fn parse_hunk_header(line: &str) -> Option<(u32, u32)> {
    let rest = line.strip_prefix("@@ -")?;
    let (old_spec, rest) = rest.split_once(' ')?;
    let new_spec = rest.strip_prefix('+')?;
    let new_spec = new_spec.split(' ').next()?;
    let old_start = parse_count(old_spec);
    let new_start = parse_count(new_spec);
    Some((old_start, new_start))
}

fn parse_count(spec: &str) -> u32 {
    spec.split(',').next().unwrap_or("1").parse().unwrap_or(1)
}

/// 拆出 v2 状态记录的所有空白字段。
fn split_after_spaces(record: &[u8], spaces: usize) -> Option<(&[u8], &[u8])> {
    let mut seen = 0;
    for (index, byte) in record.iter().enumerate() {
        if *byte == b' ' {
            seen += 1;
            if seen == spaces {
                return Some((&record[..index], &record[index + 1..]));
            }
        }
    }
    None
}

fn is_rename(status: u8) -> bool {
    matches!(status, b'R' | b'C')
}

fn file_change(
    path: &str,
    orig_path: Option<String>,
    status: u8,
    staged: bool,
    counts: &HashMap<String, (usize, usize)>,
) -> FileChange {
    let (insertions, deletions) = counts.get(path).copied().unwrap_or((0, 0));
    FileChange {
        path: path.to_string(),
        orig_path,
        status: match status {
            b'A' => ChangeStatus::Added,
            b'D' => ChangeStatus::Deleted,
            b'R' | b'C' => ChangeStatus::Renamed,
            b'U' => ChangeStatus::Conflict,
            _ => ChangeStatus::Modified,
        },
        staged,
        insertions,
        deletions,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_numstat_with_renames_and_binaries() {
        let bytes = b"2\t1\trenamed.txt\0-\t-\tb.bin\x000\t0\t\0old name\0new name\0";
        let map = numstat_map(bytes);
        assert_eq!(map.get("renamed.txt"), Some(&(2, 1)));
        assert_eq!(map.get("new name"), Some(&(0, 0)));
        assert_eq!(map.get("b.bin"), Some(&(0, 0)));
        assert_eq!(map.get("mvsim.txt"), None);
    }

    #[test]
    fn parses_unified_diff_into_typed_lines() {
        let text = b"diff --git a/sample.txt b/sample.txt\nindex 111..222 100644\n\
--- a/sample.txt\n+++ b/sample.txt\n@@ -1,4 +1,4 @@\n a\n-b\n+B\n c\n";
        let diff = parse_diff(text).unwrap().unwrap();
        assert_eq!(diff.new_path.as_deref(), Some("sample.txt"));
        let kinds: Vec<DiffLineKind> = diff.lines.iter().map(|line| line.kind).collect();
        assert_eq!(
            kinds,
            vec![
                DiffLineKind::Hunk,
                DiffLineKind::Context,
                DiffLineKind::Removed,
                DiffLineKind::Added,
                DiffLineKind::Context,
            ]
        );
        assert_eq!(diff.lines[2].old_ln, Some(2));
        assert_eq!(diff.lines[2].new_ln, None);
        assert_eq!(diff.lines[3].new_ln, Some(2));
    }

    #[test]
    fn stages_and_unstages_one_hunk_without_touching_other_hunks() {
        use std::fs;

        let dir = tempfile::tempdir().unwrap();
        let run = |args: &[&str]| {
            let output = Command::new("git")
                .arg("-C")
                .arg(dir.path())
                .args(args)
                .output()
                .unwrap();
            assert!(output.status.success(), "{args:?}: {:?}", output.stderr);
        };
        run(&["init", "-q"]);
        run(&["config", "user.email", "test@crossh.local"]);
        run(&["config", "user.name", "Crossh Test"]);

        let baseline = (1..=20)
            .map(|line| format!("line {line}"))
            .collect::<Vec<_>>()
            .join("\n")
            + "\n";
        fs::write(dir.path().join("note.txt"), baseline).unwrap();
        run(&["add", "-A"]);
        run(&["commit", "-qm", "init"]);

        let mut changed = (1..=20)
            .map(|line| format!("line {line}"))
            .collect::<Vec<_>>();
        changed[1] = "changed two".to_string();
        changed[17] = "changed eighteen".to_string();
        fs::write(dir.path().join("note.txt"), changed.join("\n") + "\n").unwrap();

        let working = list_changes(dir.path())
            .unwrap()
            .into_iter()
            .find(|change| !change.staged)
            .expect("working change should exist");
        let file_diff = diff(dir.path(), &working, false)
            .unwrap()
            .expect("working diff should exist");
        assert_eq!(
            file_diff
                .lines
                .iter()
                .filter(|line| line.kind == DiffLineKind::Hunk)
                .count(),
            2
        );

        stage_hunk(dir.path(), &working, 1).unwrap();

        let staged_text = String::from_utf8(
            Command::new("git")
                .arg("-C")
                .arg(dir.path())
                .args(["show", ":note.txt"])
                .output()
                .unwrap()
                .stdout,
        )
        .unwrap();
        assert_eq!(staged_text.lines().nth(1), Some("line 2"));
        assert_eq!(staged_text.lines().nth(17), Some("changed eighteen"));

        let staged = list_changes(dir.path())
            .unwrap()
            .into_iter()
            .find(|change| change.staged && change.path == "note.txt")
            .expect("staged hunk should be listed");
        unstage_hunk(dir.path(), &staged, 0).unwrap();

        let cached = Command::new("git")
            .arg("-C")
            .arg(dir.path())
            .args(["diff", "--cached", "--quiet"])
            .output()
            .unwrap();
        assert!(
            cached.status.success(),
            "index should be clean after unstage"
        );
        assert!(
            list_changes(dir.path())
                .unwrap()
                .iter()
                .any(|change| !change.staged && change.path == "note.txt")
        );
    }

    #[test]
    fn parses_new_file_hunk_headers() {
        let text = b"--- /dev/null\n+++ b/new.txt\n@@ -0,0 +1,2 @@\n+x\n+y\n";
        let diff = parse_diff(text).unwrap().unwrap();
        assert_eq!(diff.old_path, None);
        assert_eq!(diff.lines[1].kind, DiffLineKind::Added);
        assert_eq!(diff.lines[1].new_ln, Some(1));
    }

    #[test]
    fn detects_binary_diffs() {
        let diff = parse_diff(b"Binary files a/x and b/y differ\n")
            .unwrap()
            .unwrap();
        assert!(diff.binary);
        assert!(diff.lines.is_empty());
    }

    #[test]
    fn drops_metadata_only_diffs() {
        let text = b"diff --git a/mv.txt b/mv.txt\nsimilarity index 100%\nrename from mv.txt\nrename to moved.txt\n";
        assert!(parse_diff(text).unwrap().is_none());
    }

    #[test]
    fn oversized_text_diff_is_rejected_before_rendering() {
        let mut text = String::from("@@ -1 +1 @@\n");
        for _ in 0..=MAX_DIFF_LINES {
            text.push_str(" line\n");
        }

        assert!(matches!(
            parse_diff(text.as_bytes()),
            Err(GitError::DiffTooLarge)
        ));
    }

    #[test]
    fn invalid_git_directory_returns_an_error_instead_of_an_empty_change_list() {
        let dir = tempfile::tempdir().unwrap();

        assert!(list_changes(dir.path()).is_err());
    }

    #[test]
    fn real_status_preserves_whitespace_paths_and_numstat_counts() {
        let dir = tempfile::tempdir().unwrap();
        let run = |args: &[&str]| {
            let output = Command::new("git")
                .arg("-C")
                .arg(dir.path())
                .args(args)
                .output()
                .unwrap();
            assert!(output.status.success(), "{args:?}: {:?}", output.stderr);
        };
        run(&["init", "-q"]);
        run(&["config", "user.email", "test@crossh.local"]);
        run(&["config", "user.name", "Crossh Test"]);
        let path = "space  and tab.txt";
        std::fs::write(dir.path().join(path), "one\ntwo\n").unwrap();
        run(&["add", "-A"]);

        let changes = list_changes(dir.path()).expect("status should load");
        let change = changes
            .iter()
            .find(|change| change.path == path)
            .expect("whitespace path should be preserved");
        assert_eq!(change.insertions, 2);
        assert!(change.staged);
    }

    #[test]
    fn combined_scan_includes_branch_status_without_a_second_status_command() {
        let dir = tempfile::tempdir().unwrap();
        let run = |args: &[&str]| {
            let output = Command::new("git")
                .arg("-C")
                .arg(dir.path())
                .args(args)
                .output()
                .unwrap();
            assert!(output.status.success(), "{args:?}: {:?}", output.stderr);
        };
        run(&["init", "-q"]);
        run(&["checkout", "-qb", "scan-status"]);
        std::fs::write(dir.path().join("pending.txt"), "pending\n").unwrap();

        let scan = scan_changes(dir.path()).expect("scan should load");

        assert_eq!(
            scan.status.as_ref().map(|status| status.branch.as_str()),
            Some("scan-status")
        );
        assert!(
            scan.changes
                .iter()
                .any(|change| change.path == "pending.txt" && !change.staged)
        );
    }

    #[test]
    fn oversized_untracked_file_is_rejected_before_reading() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("large.txt");
        let file = std::fs::File::create(&path).unwrap();
        file.set_len(MAX_DIFF_BYTES + 1).unwrap();
        let entry = FileChange {
            path: "large.txt".into(),
            orig_path: None,
            status: ChangeStatus::Untracked,
            staged: false,
            insertions: 0,
            deletions: 0,
        };

        assert!(matches!(
            diff(dir.path(), &entry, false),
            Err(GitError::DiffTooLarge)
        ));
    }

    #[test]
    fn covers_a_real_repo_end_to_end() {
        use std::fs;
        use std::sync::atomic::{AtomicU64, Ordering};

        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "crossh-git-logic-{}",
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();

        let run = |args: &[&str]| {
            let output = Command::new("git")
                .arg("-C")
                .arg(&dir)
                .args(args)
                .output()
                .expect("git should run");
            assert!(output.status.success(), "{args:?}");
        };
        run(&["init", "-q"]);
        run(&["config", "user.email", "t@t"]);
        run(&["config", "user.name", "t"]);

        fs::write(dir.join("a.txt"), "one\ntwo\nthree\n").unwrap();
        run(&["add", "-A"]);
        run(&["commit", "-qm", "init"]);

        fs::write(dir.join("a.txt"), "one\nTWO\nthree\nfour\n").unwrap();
        run(&["add", "a.txt"]);
        run(&["mv", "a.txt", "renamed.txt"]);
        fs::write(dir.join("renamed.txt"), "one\nTWO\nthree\nfour\nfive\n").unwrap();
        fs::write(dir.join("staged-only.txt"), "x\ny\n").unwrap();
        run(&["add", "staged-only.txt"]);
        fs::create_dir_all(dir.join("untracked")).unwrap();
        fs::write(dir.join("untracked/note.txt"), "hello\nworld\n").unwrap();

        let changes = list_changes(&dir).expect("status should load");
        assert!(changes.iter().any(|entry| entry.path == "renamed.txt"));
        assert!(changes.iter().any(|entry| entry.path == "staged-only.txt"));

        let untracked = changes
            .iter()
            .find(|entry| entry.path == "untracked/note.txt")
            .expect("untracked file should be listed individually");
        assert_eq!(untracked.status, ChangeStatus::Untracked);
        assert!(!untracked.staged);
        let untracked_diff = diff(&dir, untracked, false)
            .expect("untracked diff should load")
            .expect("untracked file should have a diff");
        assert!(!untracked_diff.binary);
        assert_eq!(
            untracked_diff.new_path.as_deref(),
            Some("untracked/note.txt")
        );
        assert!(
            untracked_diff
                .lines
                .iter()
                .any(|line| line.kind == DiffLineKind::Added)
        );
        assert!(untracked_diff.lines.iter().any(|line| line.text == "hello"));
        assert_eq!(untracked_diff.lines.len(), 2);

        assert!(changes.iter().any(|entry| {
            entry.path == "renamed.txt" && entry.staged && entry.status == ChangeStatus::Renamed
        }));
        assert!(changes.iter().any(|entry| {
            entry.path == "renamed.txt" && !entry.staged && entry.status == ChangeStatus::Modified
        }));
        let renamed = changes
            .iter()
            .find(|entry| entry.path == "renamed.txt" && entry.staged)
            .expect("staged rename entry");
        assert_eq!(renamed.orig_path.as_deref(), Some("a.txt"));
        let rename_diff = diff(&dir, renamed, renamed.staged)
            .expect("rename diff should load")
            .expect("rename should have a diff");
        assert!(
            rename_diff
                .lines
                .iter()
                .any(|line| line.kind == DiffLineKind::Added)
        );

        let added = changes
            .iter()
            .find(|entry| entry.path == "staged-only.txt")
            .expect("staged entry");
        assert_eq!(added.status, ChangeStatus::Added);
        assert!(added.staged);
        assert_eq!(added.insertions, 2);
        let added_diff = diff(&dir, added, true)
            .expect("staged add diff should load")
            .expect("staged file should have a diff");
        assert!(added_diff.lines.iter().any(|line| line.text == "x"));
        assert_eq!(added_diff.old_path, None);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn stages_unstages_and_commits_real_changes() {
        use std::fs;

        let dir = tempfile::tempdir().unwrap();
        let run = |args: &[&str]| {
            let output = Command::new("git")
                .arg("-C")
                .arg(dir.path())
                .args(args)
                .output()
                .unwrap();
            assert!(output.status.success(), "{args:?}: {:?}", output.stderr);
        };
        run(&["init", "-q"]);
        run(&["config", "user.email", "test@crossh.local"]);
        run(&["config", "user.name", "Crossh Test"]);

        fs::write(dir.path().join("note.txt"), "first\n").unwrap();
        stage(dir.path(), &["note.txt".to_string()]).unwrap();
        assert!(
            list_changes(dir.path())
                .expect("status should load")
                .iter()
                .any(|change| change.path == "note.txt" && change.staged)
        );

        unstage(dir.path(), &["note.txt".to_string()]).unwrap();
        assert!(
            list_changes(dir.path())
                .expect("status should load")
                .iter()
                .any(|change| change.path == "note.txt" && !change.staged)
        );

        stage(dir.path(), &["note.txt".to_string()]).unwrap();
        commit(dir.path(), "add note").unwrap();
        assert!(
            list_changes(dir.path())
                .expect("status should load")
                .is_empty()
        );

        fs::write(dir.path().join("note.txt"), "first\nsecond\n").unwrap();
        stage(dir.path(), &["note.txt".to_string()]).unwrap();
        unstage(dir.path(), &["note.txt".to_string()]).unwrap();
        assert!(
            list_changes(dir.path())
                .expect("status should load")
                .iter()
                .any(|change| change.path == "note.txt" && !change.staged)
        );

        fs::remove_file(dir.path().join("note.txt")).unwrap();
        stage(dir.path(), &["note.txt".to_string()]).unwrap();
        assert!(
            list_changes(dir.path())
                .expect("status should load")
                .iter()
                .any(|change| {
                    change.path == "note.txt"
                        && change.staged
                        && change.status == ChangeStatus::Deleted
                })
        );
        assert!(commit(dir.path(), "   ").is_err());
    }

    #[test]
    fn restores_tracked_worktree_changes_without_touching_the_index() {
        use std::fs;

        let dir = tempfile::tempdir().unwrap();
        let run = |args: &[&str]| {
            let output = Command::new("git")
                .arg("-C")
                .arg(dir.path())
                .args(args)
                .output()
                .unwrap();
            assert!(output.status.success(), "{args:?}: {:?}", output.stderr);
        };
        run(&["init", "-q"]);
        run(&["config", "user.email", "test@crossh.local"]);
        run(&["config", "user.name", "Crossh Test"]);
        fs::write(dir.path().join("note.txt"), "base\n").unwrap();
        run(&["add", "-A"]);
        run(&["commit", "-qm", "init"]);

        fs::write(dir.path().join("note.txt"), "staged\n").unwrap();
        stage(dir.path(), &["note.txt".to_string()]).unwrap();
        fs::write(dir.path().join("note.txt"), "working\n").unwrap();

        discard_worktree(dir.path(), &["note.txt".to_string()]).unwrap();

        assert_eq!(
            fs::read_to_string(dir.path().join("note.txt"))
                .unwrap()
                .replace("\r\n", "\n"),
            "staged\n"
        );
        assert!(
            list_changes(dir.path())
                .unwrap()
                .iter()
                .any(|change| change.path == "note.txt" && change.staged)
        );
        assert!(
            !list_changes(dir.path())
                .unwrap()
                .iter()
                .any(|change| change.path == "note.txt" && !change.staged)
        );
    }

    #[test]
    fn pushes_to_origin_and_follows_the_upstream_afterwards() {
        use std::fs;

        let dir = tempfile::tempdir().unwrap();
        let run_in = |base: &Path, args: &[&str]| {
            let output = Command::new("git")
                .arg("-C")
                .arg(base)
                .args(args)
                .output()
                .unwrap();
            assert!(output.status.success(), "{args:?}: {:?}", output.stderr);
        };

        let local = dir.path().join("local");
        fs::create_dir_all(&local).unwrap();
        run_in(&local, &["init", "-q"]);
        let branch = String::from_utf8_lossy(
            &Command::new("git")
                .arg("-C")
                .arg(&local)
                .args(["symbolic-ref", "--short", "HEAD"])
                .output()
                .unwrap()
                .stdout,
        )
        .trim()
        .to_string();
        assert!(!branch.is_empty(), "local repo should be on a branch");
        run_in(
            dir.path(),
            &["init", "-q", "--bare", "-b", &branch, "remote.git"],
        );
        run_in(&local, &["config", "user.email", "test@crossh.local"]);
        run_in(&local, &["config", "user.name", "Crossh Test"]);
        run_in(&local, &["remote", "add", "origin", "../remote.git"]);
        fs::write(local.join("note.txt"), "first\n").unwrap();
        run_in(&local, &["add", "-A"]);
        run_in(&local, &["commit", "-qm", "init"]);

        push(&local).expect("first push should create the upstream on origin");
        assert!(
            Command::new("git")
                .arg("-C")
                .arg(&local)
                .args(["rev-parse", "@{upstream}"])
                .output()
                .unwrap()
                .status
                .success(),
            "first push should record origin tracking"
        );

        fs::write(local.join("note.txt"), "first\nsecond\n").unwrap();
        run_in(&local, &["add", "-A"]);
        run_in(&local, &["commit", "-qm", "second"]);

        push(&local).expect("second push should follow the recorded upstream");
        let head = |base: &Path| {
            String::from_utf8_lossy(
                &Command::new("git")
                    .arg("-C")
                    .arg(base)
                    .args(["rev-parse", "HEAD"])
                    .output()
                    .unwrap()
                    .stdout,
            )
            .trim()
            .to_string()
        };
        assert_eq!(
            head(&local),
            head(&dir.path().join("remote.git")),
            "remote HEAD should reach the pushed commit"
        );
    }

    #[test]
    fn pulls_from_origin_and_sets_upstream_when_missing() {
        use std::fs;

        let dir = tempfile::tempdir().unwrap();
        let run_in = |base: &Path, args: &[&str]| {
            let output = Command::new("git")
                .arg("-C")
                .arg(base)
                .args(args)
                .output()
                .unwrap();
            assert!(output.status.success(), "{args:?}: {:?}", output.stderr);
        };
        let head = |base: &Path| {
            String::from_utf8_lossy(
                &Command::new("git")
                    .arg("-C")
                    .arg(base)
                    .args(["rev-parse", "HEAD"])
                    .output()
                    .unwrap()
                    .stdout,
            )
            .trim()
            .to_string()
        };

        let local = dir.path().join("local");
        fs::create_dir_all(&local).unwrap();
        run_in(&local, &["init", "-q"]);
        let branch = String::from_utf8_lossy(
            &Command::new("git")
                .arg("-C")
                .arg(&local)
                .args(["symbolic-ref", "--short", "HEAD"])
                .output()
                .unwrap()
                .stdout,
        )
        .trim()
        .to_string();
        run_in(
            dir.path(),
            &["init", "-q", "--bare", "-b", &branch, "remote.git"],
        );
        run_in(&local, &["config", "user.email", "test@crossh.local"]);
        run_in(&local, &["config", "user.name", "Crossh Test"]);
        run_in(&local, &["remote", "add", "origin", "../remote.git"]);
        fs::write(local.join("base.txt"), "base\n").unwrap();
        run_in(&local, &["add", "-A"]);
        run_in(&local, &["commit", "-qm", "base"]);
        run_in(&local, &["push", "-u", "origin", &branch]);

        run_in(dir.path(), &["clone", "-q", "remote.git", "worker"]);
        let worker = dir.path().join("worker");
        run_in(&worker, &["config", "user.email", "test@crossh.local"]);
        run_in(&worker, &["config", "user.name", "Crossh Test"]);
        fs::write(worker.join("from-worker.txt"), "mine\n").unwrap();
        run_in(&worker, &["add", "-A"]);
        run_in(&worker, &["commit", "-qm", "worker change"]);
        run_in(&worker, &["push"]);

        pull(&local).expect("pull should merge the upstream commit");
        assert_eq!(
            head(&local),
            head(&worker),
            "local should reach worker's commit after pull"
        );
        assert!(
            Command::new("git")
                .arg("-C")
                .arg(&local)
                .args(["rev-parse", "@{upstream}"])
                .output()
                .unwrap()
                .status
                .success(),
            "upstream should still resolve after pull"
        );

        run_in(&worker, &["checkout", "-qb", "topic"]);
        fs::write(worker.join("topic.txt"), "topic\n").unwrap();
        run_in(&worker, &["add", "-A"]);
        run_in(&worker, &["commit", "-qm", "topic change"]);
        run_in(&worker, &["push", "-u", "origin", "topic"]);
        run_in(&local, &["checkout", "-qb", "topic"]);

        pull(&local).expect("pull without upstream should fetch origin/<branch> and track it");
        assert_eq!(
            head(&local),
            head(&worker),
            "local topic should reach the remote topic commit"
        );
        assert!(
            Command::new("git")
                .arg("-C")
                .arg(&local)
                .args(["rev-parse", "@{upstream}"])
                .output()
                .unwrap()
                .status
                .success(),
            "pull without upstream should record tracking"
        );
    }

    #[test]
    fn detached_head_refuses_to_pull() {
        use std::fs;

        let dir = tempfile::tempdir().unwrap();
        let run_in = |base: &Path, args: &[&str]| {
            let output = Command::new("git")
                .arg("-C")
                .arg(base)
                .args(args)
                .output()
                .unwrap();
            assert!(output.status.success(), "{args:?}: {:?}", output.stderr);
        };
        run_in(dir.path(), &["init", "-q"]);
        run_in(dir.path(), &["config", "user.email", "test@crossh.local"]);
        run_in(dir.path(), &["config", "user.name", "Crossh Test"]);
        fs::write(dir.path().join("x.txt"), "x\n").unwrap();
        run_in(dir.path(), &["add", "-A"]);
        run_in(dir.path(), &["commit", "-qm", "x"]);
        run_in(dir.path(), &["checkout", "-q", "--detach"]);

        assert!(
            pull(dir.path()).is_err(),
            "detached HEAD should refuse to pull"
        );
    }
}
