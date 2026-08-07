//! Pure Git worktree + diff inspection for the Git window.
//!
//! No `gpui` imports: this module only shells out to `git` and parses its
//! `--porcelain=v2` status, `--numstat` counters, and unified diff output.

use std::collections::HashMap;
use std::path::Path;
use std::process::Command;

/// 单个文件在索引（已暂存）或工作区（未暂存）中的变更状态。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ChangeStatus {
    Modified,
    Added,
    Deleted,
    Renamed,
    Conflict,
    Untracked,
}

impl ChangeStatus {
    /// 组内排序权重：冲突/新增优先于修改，便于稳定排序。
    pub(crate) fn rank(self) -> u8 {
        match self {
            Self::Conflict => 0,
            Self::Added | Self::Renamed => 1,
            Self::Deleted => 2,
            Self::Modified => 3,
            Self::Untracked => 4,
        }
    }

    /// 列表行首的状态字形（VS Code 风格）。
    pub(crate) fn glyph(self) -> &'static str {
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
pub(crate) struct FileChange {
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
pub(crate) enum DiffLineKind {
    /// `@@ -a,b +c,d @@` 的 hunk 头。
    Hunk,
    Context,
    Added,
    Removed,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct DiffLine {
    pub kind: DiffLineKind,
    /// 旧文件行号（`-` / 上下文行有值，新增行为 None）。
    pub old_ln: Option<u32>,
    /// 新文件行号（`+` / 上下文行有值，删除行为 None）。
    pub new_ln: Option<u32>,
    pub text: String,
}

/// 一个文件的完整 diff 内容（选中行渲染用）。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct FileDiff {
    pub old_path: Option<String>,
    pub new_path: Option<String>,
    pub lines: Vec<DiffLine>,
    /// 二进制文件：无文本 hunk。
    pub binary: bool,
}

/// 扫描目录下的全部改动（已暂存 + 未暂存），每个（路径，暂存态）一条。
pub(crate) fn list_changes(cwd: &Path) -> Vec<FileChange> {
    let Some(output) = git(
        cwd,
        // 逐个列出未追踪文件（而非折叠成 `dir/`），使其真正可见。
        &["status", "--porcelain=v2", "-z", "--untracked-files=all"],
    ) else {
        return Vec::new();
    };
    let staged_counts = numstat_map(&git(cwd, &["diff", "--cached", "--numstat"]));
    let working_counts = numstat_map(&git(cwd, &["diff", "--numstat"]));

    let mut changes = Vec::new();
    let records: Vec<Vec<u8>> = output
        .split(|byte| *byte == 0)
        .map(|slice| slice.to_vec())
        .collect();
    let mut index = 0;
    while index < records.len() {
        let record = records[index].to_vec();
        index += 1;
        if record.is_empty() || record.first() == Some(&b'#') {
            continue;
        }

        match record.first().copied() {
            Some(b'1' | b'2') => {
                let fields = whitespace_fields(&record);
                if fields.len() < 8 {
                    continue;
                }
                let xy = fields[1].to_string();
                // type 2（重命名/复制）记录在 hH hI 之后多一个 `R<score>` 字段。
                let path_start = if record[0] == b'2' && (xy.contains('R') || xy.contains('C')) {
                    9
                } else {
                    8
                };
                if fields.len() < path_start {
                    continue;
                }
                let path = fields[path_start..].join(" ");
                let mut orig_path = None;
                if xy.contains('R') || xy.contains('C') {
                    // -z 模式下源路径是记录后的下一个 NUL 片段。
                    if let Some(extra) = records.get(index)
                        && !extra.is_empty()
                        && !matches!(extra[0], b'1' | b'2' | b'u' | b'?')
                    {
                        orig_path = Some(String::from_utf8_lossy(extra).into_owned());
                        index += 1;
                    }
                }
                let bytes = xy.as_bytes();
                let index_status = bytes.first().copied().unwrap_or(b'.');
                let worktree_status = bytes.get(1).copied().unwrap_or(b'.');
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
                let fields = whitespace_fields(&record);
                if fields.len() < 10 {
                    continue;
                }
                changes.push(FileChange {
                    path: fields[10..].join(" "),
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

    changes.sort_by_key(|entry| (entry.staged, entry.status.rank(), entry.path.clone()));
    changes
}

/// 读取某个文件（按暂存态）的统一 diff。
pub(crate) fn diff(cwd: &Path, entry: &FileChange, staged: bool) -> Option<FileDiff> {
    if entry.status == ChangeStatus::Untracked {
        return untracked_diff(cwd, entry);
    }
    let mut args: Vec<&str> = vec!["diff", "--no-color", "--unified=3"];
    if staged {
        args.push("--cached");
    }
    args.push("--");
    // 重命名必须同时给源与目标路径，否则 git 会把新文件当作全新文件比较。
    if let Some(orig) = entry.orig_path.as_deref() {
        args.push(orig);
    }
    args.push(&entry.path);
    let output = git(cwd, &args)?;
    parse_diff(&output)
}

/// 未跟踪文件没有可 diff 的基线：把整个文件内容当作新增行呈现。
fn untracked_diff(cwd: &Path, entry: &FileChange) -> Option<FileDiff> {
    let path = cwd.join(&entry.path);
    if !path.is_file() {
        return None;
    }
    let bytes = std::fs::read(&path).ok()?;
    let Ok(text) = String::from_utf8(bytes) else {
        return Some(FileDiff {
            binary: true,
            ..FileDiff::default()
        });
    };
    let mut lines: Vec<&str> = text.split('\n').collect();
    if text.ends_with('\n') {
        lines.pop();
    }
    let mut produced = Vec::new();
    for (index, line) in lines.into_iter().enumerate() {
        produced.push(DiffLine {
            kind: DiffLineKind::Added,
            old_ln: None,
            new_ln: Some(index as u32 + 1),
            text: line.to_string(),
        });
    }
    Some(FileDiff {
        old_path: None,
        new_path: Some(entry.path.clone()),
        lines: produced,
        ..FileDiff::default()
    })
}

/// 运行 git 命令并返回 stdout（失败时返回 None）。
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

/// 解析 `--numstat` 输出为 path -> (insertions, deletions)。
fn numstat_map(output: &Option<Vec<u8>>) -> HashMap<String, (usize, usize)> {
    let mut map = HashMap::new();
    let Some(output) = output else {
        return map;
    };
    let text = String::from_utf8_lossy(output);
    for line in text.lines() {
        if line.is_empty() {
            continue;
        }
        let mut parts = line.splitn(3, '\t');
        let Some(added) = parts.next() else { continue };
        let Some(deleted) = parts.next() else {
            continue;
        };
        // 重命名显示为 `old => new`；按新路径索引（与条目 path 对齐）。
        let path = parts.next().unwrap_or("");
        let key = path
            .split_once(" => ")
            .map(|(_, new)| new.to_string())
            .unwrap_or_else(|| path.to_string());
        let insertions = added.parse().unwrap_or(0);
        let deletions = deleted.parse().unwrap_or(0);
        map.insert(key, (insertions, deletions));
    }
    map
}

/// 解析统一 diff 输出为逐行结构。
fn parse_diff(output: &[u8]) -> Option<FileDiff> {
    let text = String::from_utf8_lossy(output);
    if text.contains("Binary files") && !text.contains("@@ ") {
        return Some(FileDiff {
            binary: true,
            ..FileDiff::default()
        });
    }
    let mut diff = FileDiff::default();
    let mut old_ln = 0u32;
    let mut new_ln = 0u32;
    for line in text.lines() {
        if let Some((old_start, new_start)) = parse_hunk_header(line) {
            old_ln = old_start;
            new_ln = new_start;
            diff.lines.push(DiffLine {
                kind: DiffLineKind::Hunk,
                old_ln: None,
                new_ln: None,
                text: line.to_string(),
            });
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
                old_ln: None,
                new_ln: Some(new_ln),
                text: content.to_string(),
            });
            new_ln += 1;
        } else if let Some(content) = line.strip_prefix('-') {
            diff.lines.push(DiffLine {
                kind: DiffLineKind::Removed,
                old_ln: Some(old_ln),
                new_ln: None,
                text: content.to_string(),
            });
            old_ln += 1;
        } else if let Some(content) = line.strip_prefix(' ') {
            diff.lines.push(DiffLine {
                kind: DiffLineKind::Context,
                old_ln: Some(old_ln),
                new_ln: Some(new_ln),
                text: content.to_string(),
            });
            old_ln += 1;
            new_ln += 1;
        }
    }
    if diff.lines.is_empty() && !diff.binary {
        return None;
    }
    Some(diff)
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
fn whitespace_fields(record: &[u8]) -> Vec<String> {
    String::from_utf8_lossy(record)
        .split_whitespace()
        .map(str::to_string)
        .collect()
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
        let bytes = String::from("2\t1\trenamed.txt\n0\t0\tmvsim.txt => moved.txt\n-\t-\tb.bin\n")
            .into_bytes();
        let map = numstat_map(&Some(bytes));
        assert_eq!(map.get("renamed.txt"), Some(&(2, 1)));
        assert_eq!(map.get("moved.txt"), Some(&(0, 0)));
        assert_eq!(map.get("b.bin"), Some(&(0, 0)));
        assert_eq!(map.get("mvsim.txt"), None);
    }

    #[test]
    fn parses_unified_diff_into_typed_lines() {
        let text = b"diff --git a/sample.txt b/sample.txt\nindex 111..222 100644\n\
--- a/sample.txt\n+++ b/sample.txt\n@@ -1,4 +1,4 @@\n a\n-b\n+B\n c\n";
        let diff = parse_diff(text).unwrap();
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
    fn parses_new_file_hunk_headers() {
        let text = b"--- /dev/null\n+++ b/new.txt\n@@ -0,0 +1,2 @@\n+x\n+y\n";
        let diff = parse_diff(text).unwrap();
        assert_eq!(diff.old_path, None);
        assert_eq!(diff.lines[1].kind, DiffLineKind::Added);
        assert_eq!(diff.lines[1].new_ln, Some(1));
    }

    #[test]
    fn detects_binary_diffs() {
        let diff = parse_diff(b"Binary files a/x and b/y differ\n").unwrap();
        assert!(diff.binary);
        assert!(diff.lines.is_empty());
    }

    #[test]
    fn drops_metadata_only_diffs() {
        let text = b"diff --git a/mv.txt b/mv.txt\nsimilarity index 100%\nrename from mv.txt\nrename to moved.txt\n";
        assert_eq!(parse_diff(text), None);
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

        let changes = list_changes(&dir);
        assert!(changes.iter().any(|entry| entry.path == "renamed.txt"));
        assert!(changes.iter().any(|entry| entry.path == "staged-only.txt"));

        let untracked = changes
            .iter()
            .find(|entry| entry.path == "untracked/note.txt")
            .expect("untracked file should be listed individually");
        assert_eq!(untracked.status, ChangeStatus::Untracked);
        assert!(!untracked.staged);
        let untracked_diff = diff(&dir, untracked, false).expect("untracked content diff");
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
        let rename_diff = diff(&dir, renamed, renamed.staged).expect("rename diff");
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
        let added_diff = diff(&dir, added, true).expect("staged add diff");
        assert!(added_diff.lines.iter().any(|line| line.text == "x"));
        assert_eq!(added_diff.old_path, None);

        let _ = fs::remove_dir_all(&dir);
    }
}
