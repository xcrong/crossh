use std::path::Path;

use crate::git::command::git_output_limited;
use crate::git::types::{
    ChangeStatus, DiffLine, DiffLineKind, FileChange, FileDiff, GitError, MAX_DIFF_BYTES,
    MAX_DIFF_LINES,
};

/// 读取某个文件（按暂存态）的统一 diff。
pub fn diff(cwd: &Path, entry: &FileChange, staged: bool) -> Result<Option<FileDiff>, GitError> {
    if entry.status == ChangeStatus::Untracked {
        return untracked_diff(cwd, entry);
    }
    let args = diff_args(entry, staged);
    let output = git_output_limited(cwd, &args, MAX_DIFF_BYTES)?;
    parse_diff(&output)
}

pub(crate) fn diff_args(entry: &FileChange, staged: bool) -> Vec<&str> {
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

pub(crate) fn select_hunk_patch(output: &[u8], hunk_index: usize) -> Option<Vec<u8>> {
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

pub(crate) fn parse_diff(output: &[u8]) -> Result<Option<FileDiff>, GitError> {
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

pub(crate) fn parse_hunk_header(line: &str) -> Option<(u32, u32)> {
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
