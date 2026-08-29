//! Pure Git worktree + diff inspection for the Git window.
//!
//! No `gpui` imports: this module only shells out to `git` and parses its
//! `--porcelain=v2` status, `--numstat` counters, and unified diff output.

pub mod command;
pub mod diff;
pub mod numstat;
pub mod ops;
pub mod scan;
pub mod types;

#[cfg(test)]
use self::diff::parse_diff;
#[cfg(test)]
use self::numstat::numstat_map;
pub use types::{
    ChangeScan, ChangeStatus, DiffLine, DiffLineKind, FileChange, FileDiff, GitError, MAX_DIFF_BYTES,
    MAX_DIFF_LINES,
};
pub use scan::scan_changes;
pub use diff::diff;
pub use ops::{
    commit, discard_worktree, pull, push, stage, stage_hunk, unstage, unstage_hunk,
};

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::process::Command;

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

        assert!(scan_changes(dir.path()).is_err());
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

        let changes = scan_changes(&dir).expect("status should load").changes;
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
            scan_changes(dir.path())
                .expect("status should load")
                .changes
                .iter()
                .any(|change| change.path == "note.txt" && change.staged)
        );

        unstage(dir.path(), &["note.txt".to_string()]).unwrap();
        assert!(
            scan_changes(dir.path())
                .expect("status should load")
                .changes
                .iter()
                .any(|change| change.path == "note.txt" && !change.staged)
        );

        stage(dir.path(), &["note.txt".to_string()]).unwrap();
        commit(dir.path(), "add note").unwrap();
        assert!(
            scan_changes(dir.path())
                .expect("status should load")
                .changes
                .is_empty()
        );

        fs::write(dir.path().join("note.txt"), "first\nsecond\n").unwrap();
        stage(dir.path(), &["note.txt".to_string()]).unwrap();
        unstage(dir.path(), &["note.txt".to_string()]).unwrap();
        assert!(
            scan_changes(dir.path())
                .expect("status should load")
                .changes
                .iter()
                .any(|change| change.path == "note.txt" && !change.staged)
        );

        fs::remove_file(dir.path().join("note.txt")).unwrap();
        stage(dir.path(), &["note.txt".to_string()]).unwrap();
        assert!(
            scan_changes(dir.path())
                .expect("status should load")
                .changes
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
            scan_changes(dir.path())
                .unwrap()
                .changes
                .iter()
                .any(|change| change.path == "note.txt" && change.staged)
        );
        assert!(
            !scan_changes(dir.path())
                .unwrap()
                .changes
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

