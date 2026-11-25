use std::collections::HashMap;
use std::path::Path;

use crate::git::command::git_output;
use crate::git::numstat::numstat_map;
use crate::git::types::{ChangeScan, ChangeStatus, FileChange, GitError};
use crate::git_status::parse_status;

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
            Some(b'1' | b'2') => parse_change_record(
                record,
                &records,
                &mut index,
                &mut changes,
                &staged_counts,
                &working_counts,
            ),
            Some(b'u') => {
                if let Some((_, path)) = split_after_spaces(record, 10) {
                    push_plain_change(path, ChangeStatus::Conflict, &mut changes);
                }
            }
            Some(b'?') => {
                if let Some(space) = record.iter().position(|byte| *byte == b' ') {
                    push_plain_change(&record[space + 1..], ChangeStatus::Untracked, &mut changes);
                }
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

/// 解析 porcelain v2 的 `1`/`2` 记录，并在需要时消费其后的 rename 源路径片段。
fn parse_change_record(
    record: &[u8],
    records: &[&[u8]],
    index: &mut usize,
    changes: &mut Vec<FileChange>,
    staged_counts: &HashMap<Vec<u8>, (usize, usize)>,
    working_counts: &HashMap<Vec<u8>, (usize, usize)>,
) {
    let field_count = if record[0] == b'2' { 9 } else { 8 };
    let Some((prefix, raw_path)) = split_after_spaces(record, field_count) else {
        return;
    };
    let mut fields = prefix.split(|byte| *byte == b' ');
    let _kind = fields.next();
    let Some(xy) = fields.next() else {
        return;
    };
    let mut orig_raw = None;
    if record[0] == b'2' {
        // -z 模式下源路径是记录后的下一个 NUL 片段。
        if let Some(extra) = records.get(*index)
            && !extra.is_empty()
        {
            orig_raw = Some(*extra);
            *index += 1;
        }
    }
    let index_status = xy.first().copied().unwrap_or(b'.');
    let worktree_status = xy.get(1).copied().unwrap_or(b'.');
    if index_status != b'.' {
        changes.push(file_change(
            raw_path,
            orig_raw,
            index_status,
            true,
            staged_counts,
        ));
    }
    if worktree_status != b'.' {
        changes.push(file_change(
            raw_path,
            if is_rename(index_status) {
                orig_raw
            } else {
                None
            },
            worktree_status,
            false,
            working_counts,
        ));
    }
}

/// 追加一条无 diff 统计的未暂存变更。
fn push_plain_change(path: &[u8], status: ChangeStatus, changes: &mut Vec<FileChange>) {
    changes.push(FileChange {
        path: String::from_utf8_lossy(path).into_owned(),
        orig_path: None,
        status,
        staged: false,
        insertions: 0,
        deletions: 0,
    });
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
    raw_path: &[u8],
    orig_raw: Option<&[u8]>,
    status: u8,
    staged: bool,
    counts: &HashMap<Vec<u8>, (usize, usize)>,
) -> FileChange {
    // 原始字节透传查表，展示才 lossy：不同非 UTF8 文件名不再合流。
    let (insertions, deletions) = counts.get(raw_path).copied().unwrap_or((0, 0));
    FileChange {
        path: String::from_utf8_lossy(raw_path).into_owned(),
        orig_path: orig_raw.map(|bytes| String::from_utf8_lossy(bytes).into_owned()),
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
