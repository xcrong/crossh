use std::collections::HashMap;

/// Parses `--numstat -z` output where each record is `added<TAB>deleted<TAB>path`
/// and renames are encoded as an empty `path` followed by `old` and `new` NUL records.
fn parse_numstat_entries<F>(output: &[u8], mut on_entry: F)
where
    F: FnMut(&[u8], &[u8], &[u8], Option<&[u8]>),
{
    let records = output.split(|b| *b == 0).collect::<Vec<_>>();
    let mut index = 0;
    while let Some(record) = records.get(index) {
        index += 1;
        if record.is_empty() {
            continue;
        }
        let mut parts = record.splitn(3, |b| *b == b'\t');
        let Some(added) = parts.next() else { continue };
        let Some(deleted) = parts.next() else {
            continue;
        };
        let Some(path) = parts.next() else { continue };
        if path.is_empty() {
            let old = records.get(index).copied();
            let new = records.get(index + 1).copied();
            if let (Some(old), Some(new)) = (old, new) {
                index += 2;
                on_entry(added, deleted, new, Some(old));
            }
        } else {
            on_entry(added, deleted, path, None);
        }
    }
}

/// Builds `path -> (insertions, deletions)` map for working-tree scans.
///
/// Keys are raw `-z` bytes (`Vec<u8>`) so distinct non-UTF8 filenames that share
/// the same lossy display never collapse into one entry; lossy conversion
/// happens only at display time in `scan.rs`.
pub fn numstat_map(output: &[u8]) -> HashMap<Vec<u8>, (usize, usize)> {
    let mut map = HashMap::new();
    parse_numstat_entries(output, |added, deleted, path, _old| {
        let key = path.to_vec();
        let insertions = parse_count(added);
        let deletions = parse_count(deleted);
        map.insert(key, (insertions, deletions));
    });
    map
}

/// Parses per-commit file changes for `git show --numstat -z`.
pub fn parse_numstat_vec(output: &[u8]) -> Vec<crate::git_history::CommitFileChange> {
    let mut out = Vec::new();
    parse_numstat_entries(output, |added, deleted, path, old| {
        let binary = added == b"-" || deleted == b"-";
        out.push(crate::git_history::CommitFileChange {
            path: String::from_utf8_lossy(path).into_owned(),
            old_path: old.map(|v| String::from_utf8_lossy(v).into_owned()),
            insertions: parse_count(added),
            deletions: parse_count(deleted),
            binary,
        });
    });
    out
}

pub fn parse_count(value: &[u8]) -> usize {
    std::str::from_utf8(value)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_numstat_with_renames_and_binaries() {
        let bytes = b"2\t1\trenamed.txt\0-\t-\tb.bin\x000\t0\t\0old name\0new name\0";
        let map = numstat_map(bytes);
        assert_eq!(map.get(b"renamed.txt".as_slice()), Some(&(2, 1)));
        assert_eq!(map.get(b"new name".as_slice()), Some(&(0, 0)));
        assert_eq!(map.get(b"b.bin".as_slice()), Some(&(0, 0)));
    }

    #[test]
    fn non_utf8_paths_do_not_merge_counts() {
        // `a\xff` 与 `a\xfe` 的 lossy 显示同为 `a�`，但原始字节不同，统计必须各自保留。
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"2\t1\t");
        bytes.extend_from_slice(b"a\xff");
        bytes.push(0);
        bytes.extend_from_slice(b"3\t4\t");
        bytes.extend_from_slice(b"a\xfe");
        bytes.push(0);
        let map = numstat_map(&bytes);
        assert_eq!(map.len(), 2);
        assert_eq!(map.get(b"a\xff".as_slice()), Some(&(2, 1)));
        assert_eq!(map.get(b"a\xfe".as_slice()), Some(&(3, 4)));
    }
}
