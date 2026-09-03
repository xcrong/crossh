//! Pure Git remote inspection and fetching.
//!
//! This module owns the remote command and parsing contract for the Git
//! workbench. It deliberately has no GPUI dependency.

use std::path::Path;

use crate::git::GitError;
use crate::git::command::run_git_output;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RemoteSummary {
    pub name: String,
    pub fetch_url: Option<String>,
    pub push_url: Option<String>,
}

/// Lists configured remotes in `git remote -v` order with fetch/push URLs merged.
pub fn list_remotes(cwd: &Path) -> Result<Vec<RemoteSummary>, GitError> {
    let output = run_git_output(cwd, &["remote".to_string(), "-v".to_string()])?;
    parse_remotes(&output)
}

/// Fetches one remote with prune after validating its name.
pub fn fetch_remote(cwd: &Path, name: &str) -> Result<(), GitError> {
    validate_remote(name)?;
    run_git_output(
        cwd,
        &["fetch".to_string(), "--prune".to_string(), name.to_string()],
    )?;
    Ok(())
}

/// Fetches all remotes with prune.
pub fn fetch_all_remotes(cwd: &Path) -> Result<(), GitError> {
    run_git_output(
        cwd,
        &[
            "fetch".to_string(),
            "--prune".to_string(),
            "--all".to_string(),
        ],
    )?;
    Ok(())
}

/// Adds a remote after validating its name and URL.
pub fn add_remote(cwd: &Path, name: &str, url: &str) -> Result<(), GitError> {
    let name = name.trim();
    let url = url.trim();
    validate_remote(name)?;
    validate_remote_url(url)?;
    run_git_output(
        cwd,
        &[
            "remote".to_string(),
            "add".to_string(),
            name.to_string(),
            url.to_string(),
        ],
    )?;
    Ok(())
}

/// Removes a remote after validating its name.
pub fn remove_remote(cwd: &Path, name: &str) -> Result<(), GitError> {
    let name = name.trim();
    validate_remote(name)?;
    run_git_output(
        cwd,
        &["remote".to_string(), "remove".to_string(), name.to_string()],
    )?;
    Ok(())
}

fn parse_remotes(output: &[u8]) -> Result<Vec<RemoteSummary>, GitError> {
    let mut remotes: Vec<RemoteSummary> = Vec::new();
    for line in String::from_utf8_lossy(output).lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let mut parts = line.split_whitespace();
        let (Some(name), Some(url), Some(kind)) = (parts.next(), parts.next(), parts.next()) else {
            continue;
        };
        if name.is_empty() || url.is_empty() {
            continue;
        }
        let entry = match remotes.iter_mut().find(|entry| entry.name == name) {
            Some(entry) => entry,
            None => {
                remotes.push(RemoteSummary {
                    name: name.to_string(),
                    fetch_url: None,
                    push_url: None,
                });
                remotes.last_mut().expect("remote just pushed")
            }
        };
        match kind {
            "(fetch)" => {
                if entry.fetch_url.is_none() {
                    entry.fetch_url = Some(url.to_string());
                }
            }
            "(push)" => {
                if entry.push_url.is_none() {
                    entry.push_url = Some(url.to_string());
                }
            }
            _ => continue,
        }
    }
    Ok(remotes)
}

fn validate_remote(name: &str) -> Result<(), GitError> {
    if name.is_empty() || name.len() > 255 {
        return Err(GitError::CommandFailed("远端名称无效".to_string()));
    }
    if !name
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-' | b'/'))
    {
        return Err(GitError::CommandFailed("远端名称无效".to_string()));
    }
    if name.starts_with('-')
        || name.starts_with('/')
        || name.starts_with('.')
        || name.ends_with('/')
        || name.ends_with('.')
        || name.contains("//")
        || name.contains("..")
    {
        return Err(GitError::CommandFailed("远端名称无效".to_string()));
    }
    Ok(())
}

fn validate_remote_url(url: &str) -> Result<(), GitError> {
    if url.is_empty() || url.len() > 2048 {
        return Err(GitError::CommandFailed("远端地址无效".to_string()));
    }
    if url.starts_with('-') || url.bytes().any(|byte| byte.is_ascii_control()) {
        return Err(GitError::CommandFailed("远端地址无效".to_string()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;
    use std::process::Command;

    use super::*;

    #[test]
    fn merges_fetch_and_push_urls_in_config_order() {
        let output = b"origin\tgit@github.com:crossh/crossh.git (fetch)\n\
            origin\tgit@github.com:crossh/crossh.git (push)\n\
            upstream\thttps://github.com/upstream/crossh.git (fetch)\n\
            upstream\thttps://github.com/upstream/push.git (push)\n";
        let remotes = parse_remotes(output).unwrap();
        assert_eq!(remotes.len(), 2);
        assert_eq!(remotes[0].name, "origin");
        assert_eq!(
            remotes[0].fetch_url.as_deref(),
            Some("git@github.com:crossh/crossh.git")
        );
        assert_eq!(
            remotes[0].push_url.as_deref(),
            Some("git@github.com:crossh/crossh.git")
        );
        assert_eq!(remotes[1].name, "upstream");
        assert_eq!(
            remotes[1].fetch_url.as_deref(),
            Some("https://github.com/upstream/crossh.git")
        );
        assert_eq!(
            remotes[1].push_url.as_deref(),
            Some("https://github.com/upstream/push.git")
        );
    }

    #[test]
    fn skips_blank_and_malformed_lines() {
        let output = b"\norigin\tgit@local:repo.git (fetch)\nmalformed-line\n\n";
        let remotes = parse_remotes(output).unwrap();
        assert_eq!(remotes.len(), 1);
        assert_eq!(remotes[0].name, "origin");
        assert_eq!(remotes[0].fetch_url.as_deref(), Some("git@local:repo.git"));
        assert_eq!(remotes[0].push_url, None);
        assert!(parse_remotes(b"").unwrap().is_empty());
    }

    #[test]
    fn rejects_flag_like_and_path_like_remote_names() {
        for name in [
            "",
            "--all",
            "-origin",
            "origin remote",
            "/origin",
            ".origin",
            "origin/",
            "origin.",
            "a//b",
            "a..b",
            "origin;rm",
        ] {
            assert!(validate_remote(name).is_err(), "{name:?}");
        }
        assert!(validate_remote("origin").is_ok());
        assert!(validate_remote("team/upstream-1.2").is_ok());
    }

    #[test]
    fn lists_and_fetches_a_local_remote() {
        let origin = repository();
        write_commit(&origin, "note.txt", "base\n", "initial");
        let work = repository();
        write_commit(&work, "note.txt", "work\n", "work");
        run(
            &work,
            &["remote", "add", "origin", &origin.to_string_lossy()],
        );

        let remotes = list_remotes(&work).unwrap();
        assert_eq!(remotes.len(), 1);
        assert_eq!(remotes[0].name, "origin");
        assert_eq!(
            remotes[0].fetch_url.as_deref(),
            Some(origin.to_string_lossy().as_ref())
        );

        fetch_remote(&work, "origin").unwrap();
        fetch_all_remotes(&work).unwrap();
        assert!(fetch_remote(&work, "--all").is_err());
    }

    #[test]
    fn adds_lists_and_removes_a_remote() {
        let origin = repository();
        let work = repository();
        let url = origin.to_string_lossy().to_string();

        add_remote(&work, "origin", &url).unwrap();
        // 重复添加同名远端应当失败。
        assert!(add_remote(&work, "origin", &url).is_err());

        let remotes = list_remotes(&work).unwrap();
        assert_eq!(remotes.len(), 1);
        assert_eq!(remotes[0].name, "origin");

        remove_remote(&work, "origin").unwrap();
        assert!(list_remotes(&work).unwrap().is_empty());
        // 删除不存在的远端应当失败。
        assert!(remove_remote(&work, "origin").is_err());
    }

    #[test]
    fn rejects_invalid_remote_names_and_urls() {
        let work = repository();
        assert!(add_remote(&work, "--all", "/tmp/x").is_err());
        assert!(add_remote(&work, "origin", "").is_err());
        assert!(add_remote(&work, "origin", "-snapshot").is_err());
        assert!(add_remote(&work, "origin", "a\nb").is_err());
        assert!(add_remote(&work, "  ", "/tmp/x").is_err());
        assert!(remove_remote(&work, "--all").is_err());
        assert!(remove_remote(&work, "").is_err());
        assert!(validate_remote_url("https://github.com/crossh/crossh.git").is_ok());
        assert!(validate_remote_url("git@github.com:crossh/crossh.git").is_ok());
    }

    #[test]
    fn rejects_invalid_git_directories() {
        let dir = tempfile::tempdir().unwrap();
        assert!(list_remotes(dir.path()).is_err());
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
