use super::messages::{AgentToolCall, AgentToolResult};
use super::policy::{
    MAX_DIRECTORY_ENTRIES, MAX_DISCOVERED_PATHS, MAX_FILE_BYTES, MAX_FILE_SCAN_BYTES,
    MAX_LINE_BYTES, MAX_TOOL_ARGUMENT_BYTES, MAX_TOOL_OUTPUT_BYTES, TOOL_TIMEOUT,
};
use regex::Regex;
use serde_json::{Value, json};
use std::collections::VecDeque;
use std::fs;
use std::io::{BufRead, BufReader, Read};
use std::path::{Component, Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

#[cfg(unix)]
use std::os::unix::process::CommandExt;

#[cfg(unix)]
static PATCH_COUNTER: AtomicUsize = AtomicUsize::new(0);

pub(super) struct ToolControl<'a> {
    cancel: &'a AtomicBool,
    deadline: Instant,
}

impl<'a> ToolControl<'a> {
    pub(super) fn new(cancel: &'a AtomicBool) -> Self {
        Self {
            cancel,
            deadline: Instant::now() + TOOL_TIMEOUT,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct AgentToolDefinition {
    pub name: &'static str,
    pub description: &'static str,
    pub input_schema: Value,
    pub requires_approval: bool,
}

pub fn builtin_tools() -> Vec<AgentToolDefinition> {
    vec![
        AgentToolDefinition {
            name: "read",
            description: "Read a UTF-8 file inside the current workspace; prefer a workspace-relative path such as README.md",
            input_schema: json!({"type":"object","properties":{"path":{"type":"string"},"offset":{"type":["integer","null"],"minimum":1},"limit":{"type":["integer","null"],"minimum":1}},"required":["path","offset","limit"],"additionalProperties":false}),
            requires_approval: false,
        },
        AgentToolDefinition {
            name: "grep",
            description: "Search workspace files for a text or regular expression; prefer a workspace-relative path",
            input_schema: json!({"type":"object","properties":{"pattern":{"type":"string"},"path":{"type":["string","null"]},"limit":{"type":["integer","null"],"minimum":1}},"required":["pattern","path","limit"],"additionalProperties":false}),
            requires_approval: false,
        },
        AgentToolDefinition {
            name: "find",
            description: "Find files and directories in the current workspace; prefer a workspace-relative path",
            input_schema: json!({"type":"object","properties":{"pattern":{"type":["string","null"]},"path":{"type":["string","null"]},"limit":{"type":["integer","null"],"minimum":1}},"required":["pattern","path","limit"],"additionalProperties":false}),
            requires_approval: false,
        },
        AgentToolDefinition {
            name: "ls",
            description: "List entries in a workspace directory; prefer a workspace-relative path such as .",
            input_schema: json!({"type":"object","properties":{"path":{"type":["string","null"]}},"required":["path"],"additionalProperties":false}),
            requires_approval: false,
        },
        AgentToolDefinition {
            name: "write",
            description: "Create or replace a UTF-8 file inside the current workspace; prefer a workspace-relative path",
            input_schema: json!({"type":"object","properties":{"path":{"type":"string"},"content":{"type":"string"}},"required":["path","content"],"additionalProperties":false}),
            requires_approval: true,
        },
        AgentToolDefinition {
            name: "edit",
            description: "Replace one exact text occurrence in a workspace file; prefer a workspace-relative path",
            input_schema: json!({"type":"object","properties":{"path":{"type":"string"},"old_text":{"type":"string"},"new_text":{"type":"string"}},"required":["path","old_text","new_text"],"additionalProperties":false}),
            requires_approval: true,
        },
        AgentToolDefinition {
            name: "patch",
            description: "Apply a unified-diff patch to one existing UTF-8 workspace file; provide the file path separately and include @@ hunks with context lines",
            input_schema: json!({"type":"object","properties":{"path":{"type":"string"},"patch":{"type":"string"}},"required":["path","patch"],"additionalProperties":false}),
            requires_approval: true,
        },
        AgentToolDefinition {
            name: "bash",
            description: "Run a shell command in the current workspace",
            input_schema: json!({"type":"object","properties":{"command":{"type":"string"}},"required":["command"],"additionalProperties":false}),
            requires_approval: true,
        },
    ]
}

pub fn execute_tool(call: &AgentToolCall, workspace: &Path) -> AgentToolResult {
    let cancel = AtomicBool::new(false);
    execute_tool_with_cancel(call, workspace, &cancel)
}

pub fn execute_tool_with_cancel(
    call: &AgentToolCall,
    workspace: &Path,
    cancel: &AtomicBool,
) -> AgentToolResult {
    let control = ToolControl::new(cancel);
    let result = execute_tool_inner(call, workspace, &control);
    AgentToolResult {
        call_id: call.id.clone(),
        is_error: result.is_err(),
        output: truncate_output(&result.unwrap_or_else(|error| error)),
    }
}

fn execute_tool_inner(
    call: &AgentToolCall,
    workspace: &Path,
    control: &ToolControl<'_>,
) -> Result<String, String> {
    if call.arguments.len() > MAX_TOOL_ARGUMENT_BYTES {
        return Err(format!(
            "tool arguments exceed the {} KiB limit",
            MAX_TOOL_ARGUMENT_BYTES / 1024
        ));
    }
    check_cancelled(control)?;
    let args: Value = serde_json::from_str(&call.arguments)
        .map_err(|error| format!("invalid tool arguments: {error}"))?;
    match call.name.as_str() {
        "read" => {
            let path = workspace_path(workspace, required_str(&args, "path")?, false)?;
            let offset = args
                .get("offset")
                .and_then(Value::as_u64)
                .unwrap_or(1)
                .max(1) as usize;
            let limit = args
                .get("limit")
                .and_then(Value::as_u64)
                .unwrap_or(200)
                .min(2000) as usize;
            read_file_lines(&path, offset, limit, control)
        }
        "grep" => execute_grep(&args, workspace, control),
        "find" => execute_find(&args, workspace, control),
        "ls" => execute_ls(&args, workspace, control),
        "write" => {
            let path = workspace_path(workspace, required_str(&args, "path")?, true)?;
            let content = required_str(&args, "content")?;
            if content.len() as u64 > MAX_FILE_BYTES {
                return Err(format!(
                    "file content exceeds the {} MiB limit",
                    MAX_FILE_BYTES / (1024 * 1024)
                ));
            }
            check_cancelled(control)?;
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).map_err(|error| error.to_string())?;
            }
            fs::write(&path, content).map_err(|error| error.to_string())?;
            Ok(format!("wrote {}", path.display()))
        }
        "edit" => {
            let path = workspace_path(workspace, required_str(&args, "path")?, false)?;
            let mut text = read_file_string(&path, control)?;
            let old = required_str(&args, "old_text")?;
            let new = required_str(&args, "new_text")?;
            check_cancelled(control)?;
            if old.is_empty() || text.matches(old).count() != 1 {
                return Err("old_text must match exactly once".into());
            }
            text = text.replacen(old, new, 1);
            fs::write(&path, text).map_err(|error| error.to_string())?;
            Ok(format!("edited {}", path.display()))
        }
        "patch" => {
            let path = workspace_path(workspace, required_str(&args, "path")?, false)?;
            let text = read_file_string(&path, control)?;
            let patch = required_str(&args, "patch")?;
            let updated = apply_unified_patch(&text, patch)?;
            check_cancelled(control)?;
            write_file_atomically(&path, &updated)?;
            Ok(format!("patched {}", path.display()))
        }
        "bash" => {
            let command = required_str(&args, "command")?;
            let mut process = shell_command(command, workspace);
            let output = run_bounded_command(&mut process, control)?;
            Ok(format_command_output(&output))
        }
        _ => Err(format!("unknown tool: {}", call.name)),
    }
}

#[derive(Debug, PartialEq, Eq)]
struct PatchHunk {
    old_start: usize,
    old_count: usize,
    new_count: usize,
    lines: Vec<PatchLine>,
}

#[derive(Debug, PartialEq, Eq)]
enum PatchLine {
    Context(String),
    Remove(String),
    Add(String),
}

fn apply_unified_patch(original: &str, patch: &str) -> Result<String, String> {
    let hunks = parse_unified_patch(patch)?;
    let uses_crlf = original.contains("\r\n");
    let newline = if uses_crlf { "\r\n" } else { "\n" };
    let had_final_newline = original.ends_with('\n');
    let mut source_lines = if original.is_empty() {
        Vec::new()
    } else {
        original
            .split('\n')
            .map(|line| line.strip_suffix('\r').unwrap_or(line).to_string())
            .collect::<Vec<_>>()
    };
    if had_final_newline {
        source_lines.pop();
    }

    let mut result_lines = Vec::new();
    let mut source_cursor = 0;
    for hunk in hunks {
        let hunk_position = if hunk.old_count == 0 {
            hunk.old_start
        } else {
            hunk.old_start.saturating_sub(1)
        };
        if hunk_position < source_cursor || hunk_position > source_lines.len() {
            return Err(format!(
                "patch hunk starts outside the source at line {}",
                hunk.old_start
            ));
        }
        result_lines.extend(source_lines[source_cursor..hunk_position].iter().cloned());

        let mut index = hunk_position;
        for line in hunk.lines {
            match line {
                PatchLine::Context(expected) => {
                    verify_patch_line(&source_lines, index, &expected)?;
                    result_lines.push(expected);
                    index += 1;
                }
                PatchLine::Remove(expected) => {
                    verify_patch_line(&source_lines, index, &expected)?;
                    index += 1;
                }
                PatchLine::Add(text) => result_lines.push(text),
            }
        }
        source_cursor = index;
    }
    result_lines.extend(source_lines[source_cursor..].iter().cloned());

    let mut updated = result_lines.join(newline);
    if had_final_newline && !result_lines.is_empty() {
        updated.push_str(newline);
    }
    Ok(updated)
}

fn verify_patch_line(source: &[String], index: usize, expected: &str) -> Result<(), String> {
    match source.get(index) {
        Some(actual) if actual == expected => Ok(()),
        Some(_) => Err(format!(
            "patch context mismatch at source line {}",
            index + 1
        )),
        None => Err(format!(
            "patch reaches past the end at source line {}",
            index + 1
        )),
    }
}

fn parse_unified_patch(patch: &str) -> Result<Vec<PatchHunk>, String> {
    let lines = patch
        .split('\n')
        .map(|line| line.strip_suffix('\r').unwrap_or(line))
        .collect::<Vec<_>>();
    let mut hunks = Vec::new();
    let mut current = None;

    for (line_index, line) in lines.iter().enumerate() {
        if line.starts_with("@@") {
            if let Some(hunk) = current.take() {
                validate_patch_hunk(&hunk)?;
                hunks.push(hunk);
            }
            current = Some(parse_patch_hunk_header(line, line_index + 1)?);
            continue;
        }

        if let Some(hunk) = current.as_mut() {
            if let Some(text) = line.strip_prefix(' ') {
                hunk.lines.push(PatchLine::Context(text.into()));
            } else if let Some(text) = line.strip_prefix('-') {
                hunk.lines.push(PatchLine::Remove(text.into()));
            } else if let Some(text) = line.strip_prefix('+') {
                hunk.lines.push(PatchLine::Add(text.into()));
            } else if *line == "\\ No newline at end of file" {
                // The current file's newline style is preserved by the writer.
            } else if *line == "*** End Patch" {
                // Accept the common apply_patch wrapper around unified hunks.
            } else if line.is_empty() && line_index + 1 == lines.len() {
                // split('\n') produces a final empty item when the patch ends in a newline.
            } else {
                return Err(format!(
                    "invalid unified patch line {}: expected context, addition, or removal",
                    line_index + 1
                ));
            }
            continue;
        }

        if line.starts_with("--- ")
            || line.starts_with("+++ ")
            || line.starts_with("diff ")
            || line.starts_with("index ")
            || *line == "*** Begin Patch"
            || *line == "*** End Patch"
            || line.starts_with("*** Update File:")
            || (line.is_empty() && line_index + 1 == lines.len())
        {
            continue;
        }
        return Err(format!(
            "invalid unified patch line {}: expected a hunk header",
            line_index + 1
        ));
    }

    if let Some(hunk) = current {
        validate_patch_hunk(&hunk)?;
        hunks.push(hunk);
    }
    if hunks.is_empty() {
        return Err("patch does not contain a unified-diff hunk".into());
    }
    Ok(hunks)
}

fn validate_patch_hunk(hunk: &PatchHunk) -> Result<(), String> {
    let old_count = hunk
        .lines
        .iter()
        .filter(|line| matches!(line, PatchLine::Context(_) | PatchLine::Remove(_)))
        .count();
    let new_count = hunk
        .lines
        .iter()
        .filter(|line| matches!(line, PatchLine::Context(_) | PatchLine::Add(_)))
        .count();
    if old_count != hunk.old_count || new_count != hunk.new_count {
        return Err(format!(
            "patch hunk line count mismatch: expected -{}, +{} but received -{}, +{}",
            hunk.old_count, hunk.new_count, old_count, new_count
        ));
    }
    Ok(())
}

fn parse_patch_hunk_header(line: &str, line_number: usize) -> Result<PatchHunk, String> {
    let ranges = line
        .strip_prefix("@@")
        .and_then(|line| line.find("@@").map(|end| &line[..end]))
        .ok_or_else(|| format!("invalid patch hunk header on line {line_number}"))?;
    let mut ranges = ranges.split_whitespace();
    let (old_start, old_count) = parse_patch_range(
        ranges
            .next()
            .ok_or_else(|| format!("missing old range on patch line {line_number}"))?,
        '-',
        line_number,
    )?;
    let (_new_start, new_count) = parse_patch_range(
        ranges
            .next()
            .ok_or_else(|| format!("missing new range on patch line {line_number}"))?,
        '+',
        line_number,
    )?;
    Ok(PatchHunk {
        old_start,
        old_count,
        new_count,
        lines: Vec::new(),
    })
}

fn parse_patch_range(
    range: &str,
    prefix: char,
    line_number: usize,
) -> Result<(usize, usize), String> {
    let value = range
        .strip_prefix(prefix)
        .ok_or_else(|| format!("invalid patch range on line {line_number}"))?;
    let (start, count) = value.split_once(',').unwrap_or((value, "1"));
    let start = start
        .parse::<usize>()
        .map_err(|_| format!("invalid patch range on line {line_number}"))?;
    let count = count
        .parse::<usize>()
        .map_err(|_| format!("invalid patch range on line {line_number}"))?;
    Ok((start, count))
}

fn write_file_atomically(path: &Path, contents: &str) -> Result<(), String> {
    #[cfg(unix)]
    {
        let file_name = path
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or("file name is not valid UTF-8")?;
        let temp_path = path.with_file_name(format!(
            ".{file_name}.crossh-patch-{}-{}.tmp",
            std::process::id(),
            PATCH_COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let permissions = fs::metadata(path)
            .map_err(|error| error.to_string())?
            .permissions();
        let result = (|| {
            fs::write(&temp_path, contents).map_err(|error| error.to_string())?;
            fs::set_permissions(&temp_path, permissions).map_err(|error| error.to_string())?;
            fs::rename(&temp_path, path).map_err(|error| error.to_string())
        })();
        if result.is_err() {
            let _ = fs::remove_file(&temp_path);
        }
        result
    }
    #[cfg(not(unix))]
    {
        fs::write(path, contents).map_err(|error| error.to_string())
    }
}

fn execute_grep(
    args: &Value,
    workspace: &Path,
    control: &ToolControl<'_>,
) -> Result<String, String> {
    let pattern = required_str(args, "pattern")?;
    if pattern.is_empty() {
        return Err("pattern must not be empty".into());
    }
    let root = optional_workspace_path(args, "path", workspace)?;
    let limit = args
        .get("limit")
        .and_then(Value::as_u64)
        .unwrap_or(100)
        .clamp(1, 1_000) as usize;

    let mut command = Command::new("rg");
    command
        .args(["--line-number", "--no-heading", "--color", "never"])
        .arg("--glob")
        .arg("!.git/**")
        .arg("--glob")
        .arg("!target/**")
        .arg("--glob")
        .arg("!node_modules/**")
        .arg("--max-count")
        .arg(limit.to_string())
        .arg("--")
        .arg(pattern)
        .arg(&root)
        .current_dir(workspace);
    let output = match run_bounded_command(&mut command, control) {
        Ok(output) => output,
        Err(error) if error.starts_with("failed to spawn command") => {
            return grep_without_rg(pattern, &root, workspace, limit, control);
        }
        Err(error) => return Err(error),
    };
    if !output.status.success() && output.status.code() != Some(1) {
        let error = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(if error.is_empty() {
            format!("rg exited with {}", output.status)
        } else {
            error
        });
    }
    let text = String::from_utf8_lossy(&output.stdout);
    Ok(limit_lines(text.trim(), limit))
}

pub(super) fn grep_without_rg(
    pattern: &str,
    root: &Path,
    workspace: &Path,
    limit: usize,
    control: &ToolControl<'_>,
) -> Result<String, String> {
    let regex = Regex::new(pattern).map_err(|error| format!("invalid regex: {error}"))?;
    let mut output = String::new();
    let mut match_count = 0;
    for path in walk_paths(root, workspace, control)? {
        check_cancelled(control)?;
        if !path.is_file() {
            continue;
        }
        let Ok(file) = fs::File::open(&path) else {
            continue;
        };
        let mut reader = BufReader::new(file);
        let mut line = String::new();
        let mut line_number = 0;
        let mut scanned = 0_u64;
        loop {
            check_cancelled(control)?;
            line.clear();
            let read = reader
                .by_ref()
                .take((MAX_LINE_BYTES + 1) as u64)
                .read_line(&mut line)
                .map_err(|error| error.to_string())?;
            if read == 0 {
                break;
            }
            if read > MAX_LINE_BYTES {
                return Err(format!(
                    "a searched line exceeds the {} MiB limit",
                    MAX_LINE_BYTES / (1024 * 1024)
                ));
            }
            scanned = scanned.saturating_add(read as u64);
            if scanned > MAX_FILE_SCAN_BYTES {
                break;
            }
            line_number += 1;
            let text = line.trim_end_matches(['\r', '\n']);
            if regex.is_match(text) {
                let formatted = format!(
                    "{}:{}:{}\n",
                    relative_display(workspace, &path),
                    line_number,
                    text
                );
                const TRUNCATION_NOTICE: &str = "\n[output truncated]";
                if output.len().saturating_add(formatted.len()) > MAX_TOOL_OUTPUT_BYTES {
                    let available = MAX_TOOL_OUTPUT_BYTES.saturating_sub(TRUNCATION_NOTICE.len());
                    if output.is_empty() {
                        let end = formatted.floor_char_boundary(available);
                        output.push_str(&formatted[..end]);
                    }
                    output.truncate(output.floor_char_boundary(available));
                    output.push_str(TRUNCATION_NOTICE);
                    return Ok(output);
                }
                output.push_str(&formatted);
                match_count += 1;
                if match_count >= limit {
                    return Ok(output.trim_end_matches('\n').to_string());
                }
            }
        }
    }
    Ok(output.trim_end_matches('\n').to_string())
}

fn execute_find(
    args: &Value,
    workspace: &Path,
    control: &ToolControl<'_>,
) -> Result<String, String> {
    let root = optional_workspace_path(args, "path", workspace)?;
    let pattern = args.get("pattern").and_then(Value::as_str).unwrap_or("");
    let limit = args
        .get("limit")
        .and_then(Value::as_u64)
        .unwrap_or(200)
        .clamp(1, 2_000) as usize;
    let mut results = Vec::new();
    for path in walk_paths(&root, workspace, control)? {
        check_cancelled(control)?;
        let relative = relative_display(workspace, &path);
        let name = path
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or("");
        if pattern.is_empty() || name.contains(pattern) || relative.contains(pattern) {
            results.push(relative);
            if results.len() >= limit {
                break;
            }
        }
    }
    Ok(results.join("\n"))
}

fn execute_ls(args: &Value, workspace: &Path, control: &ToolControl<'_>) -> Result<String, String> {
    let path = optional_workspace_path(args, "path", workspace)?;
    let mut entries = Vec::new();
    for entry in fs::read_dir(&path).map_err(|error| error.to_string())? {
        check_cancelled(control)?;
        let entry = entry.map_err(|error| error.to_string())?;
        entries.push(entry);
        if entries.len() > MAX_DIRECTORY_ENTRIES {
            return Err(format!(
                "directory contains more than {MAX_DIRECTORY_ENTRIES} entries"
            ));
        }
    }
    entries.sort_by_key(|entry| entry.file_name());
    let mut output = Vec::new();
    for entry in entries {
        check_cancelled(control)?;
        let metadata = entry.metadata().map_err(|error| error.to_string())?;
        let kind = if metadata.is_dir() { "dir" } else { "file" };
        output.push(format!(
            "{kind}\t{}\t{}",
            metadata.len(),
            entry.file_name().to_string_lossy()
        ));
    }
    Ok(output.join("\n"))
}

fn optional_workspace_path(args: &Value, name: &str, workspace: &Path) -> Result<PathBuf, String> {
    match args.get(name).and_then(Value::as_str) {
        Some(value) if !value.is_empty() => workspace_path(workspace, value, false),
        _ => workspace.canonicalize().map_err(|error| error.to_string()),
    }
}

fn walk_paths(
    root: &Path,
    workspace: &Path,
    control: &ToolControl<'_>,
) -> Result<Vec<PathBuf>, String> {
    let workspace = workspace
        .canonicalize()
        .map_err(|error| error.to_string())?;
    let root = root.canonicalize().map_err(|error| error.to_string())?;
    if !root.starts_with(&workspace) {
        return Err("path escapes the current workspace".into());
    }
    let mut pending = vec![root.clone()];
    let mut visited = std::collections::BTreeSet::from([root.clone()]);
    let mut result = Vec::new();
    while let Some(path) = pending.pop() {
        check_cancelled(control)?;
        if should_skip_path(&path, &root) {
            continue;
        }
        result.push(path.clone());
        if result.len() > MAX_DISCOVERED_PATHS {
            return Err(format!(
                "workspace traversal exceeded {MAX_DISCOVERED_PATHS} paths"
            ));
        }
        if !path.is_dir() {
            continue;
        }
        let mut children = fs::read_dir(&path)
            .map_err(|error| error.to_string())?
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .collect::<Vec<_>>();
        children.sort();
        for child in children.into_iter().rev() {
            check_cancelled(control)?;
            let Ok(canonical) = child.canonicalize() else {
                continue;
            };
            if canonical.starts_with(&workspace) && visited.insert(canonical.clone()) {
                pending.push(canonical);
            }
        }
    }
    Ok(result)
}

fn should_skip_path(path: &Path, root: &Path) -> bool {
    if path == root {
        return false;
    }
    path.strip_prefix(root)
        .unwrap_or(path)
        .components()
        .any(|component| {
            component
                .as_os_str()
                .to_str()
                .is_some_and(|name| matches!(name, ".git" | "target" | "node_modules"))
        })
}

fn relative_display(workspace: &Path, path: &Path) -> String {
    path.strip_prefix(workspace)
        .unwrap_or(path)
        .display()
        .to_string()
}

fn limit_lines(text: &str, limit: usize) -> String {
    text.lines().take(limit).collect::<Vec<_>>().join("\n")
}

fn required_str<'a>(args: &'a Value, name: &str) -> Result<&'a str, String> {
    args.get(name)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("missing string argument: {name}"))
}

fn check_cancelled(control: &ToolControl<'_>) -> Result<(), String> {
    if control.cancel.load(Ordering::Relaxed) {
        Err("tool execution cancelled".into())
    } else if Instant::now() >= control.deadline {
        Err(format!(
            "tool execution timed out after {} seconds",
            TOOL_TIMEOUT.as_secs()
        ))
    } else {
        Ok(())
    }
}

fn read_file_lines(
    path: &Path,
    offset: usize,
    limit: usize,
    control: &ToolControl<'_>,
) -> Result<String, String> {
    let file = fs::File::open(path).map_err(|error| error.to_string())?;
    let mut reader = BufReader::new(file);
    let mut line = String::new();
    let mut line_number = 0;
    let mut scanned = 0_u64;
    let mut output = String::new();
    let mut returned = 0;
    let end_line = offset.saturating_add(limit.saturating_sub(1));
    while returned < limit {
        check_cancelled(control)?;
        line.clear();
        let read = reader
            .by_ref()
            .take((MAX_LINE_BYTES + 1) as u64)
            .read_line(&mut line)
            .map_err(|error| error.to_string())?;
        if read == 0 {
            break;
        }
        if read > MAX_LINE_BYTES {
            return Err(format!(
                "a read line exceeds the {} MiB limit",
                MAX_LINE_BYTES / (1024 * 1024)
            ));
        }
        scanned = scanned.saturating_add(read as u64);
        if scanned > MAX_FILE_SCAN_BYTES {
            return Err(format!(
                "read scan exceeded the {} MiB limit",
                MAX_FILE_SCAN_BYTES / (1024 * 1024)
            ));
        }
        line_number += 1;
        if line_number < offset {
            continue;
        }
        let text = line.trim_end_matches(['\r', '\n']);
        let formatted = format!("{line_number}: {text}\n");
        if output.len() + formatted.len() > MAX_TOOL_OUTPUT_BYTES {
            return Err("read output exceeded the 64 KiB limit".into());
        }
        output.push_str(&formatted);
        returned += 1;
        if line_number >= end_line {
            break;
        }
    }
    Ok(output.trim_end_matches('\n').to_string())
}

fn read_file_string(path: &Path, control: &ToolControl<'_>) -> Result<String, String> {
    let metadata = fs::metadata(path).map_err(|error| error.to_string())?;
    if metadata.len() > MAX_FILE_BYTES {
        return Err(format!(
            "file exceeds the {} MiB limit",
            MAX_FILE_BYTES / (1024 * 1024)
        ));
    }
    let mut file = fs::File::open(path).map_err(|error| error.to_string())?;
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    let mut chunk = [0_u8; 32 * 1024];
    loop {
        check_cancelled(control)?;
        let read = file.read(&mut chunk).map_err(|error| error.to_string())?;
        if read == 0 {
            break;
        }
        if bytes.len().saturating_add(read) as u64 > MAX_FILE_BYTES {
            return Err(format!(
                "file exceeds the {} MiB limit",
                MAX_FILE_BYTES / (1024 * 1024)
            ));
        }
        bytes.extend_from_slice(&chunk[..read]);
    }
    String::from_utf8(bytes).map_err(|error| format!("file is not valid UTF-8: {error}"))
}

struct CommandOutput {
    status: ExitStatus,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

fn format_command_output(output: &CommandOutput) -> String {
    let mut text = String::from_utf8_lossy(&output.stdout).into_owned();
    if !output.stderr.is_empty() {
        if !text.is_empty() {
            text.push('\n');
        }
        text.push_str(&String::from_utf8_lossy(&output.stderr));
    }
    format!("exit status: {}\n{text}", output.status)
}

fn shell_command(command: &str, workspace: &Path) -> Command {
    let mut process = if cfg!(windows) {
        let mut process = Command::new("powershell");
        process.args(["-NoProfile", "-Command", command]);
        process
    } else {
        let mut process = Command::new("sh");
        process.args(["-lc", command]);
        process
    };
    process.current_dir(workspace);
    process
}

fn run_bounded_command(
    process: &mut Command,
    control: &ToolControl<'_>,
) -> Result<CommandOutput, String> {
    process
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(unix)]
    process.process_group(0);
    let mut child = process
        .spawn()
        .map_err(|error| format!("failed to spawn command: {error}"))?;
    let stdout = match child.stdout.take() {
        Some(stdout) => stdout,
        None => {
            kill_child(&mut child);
            let _ = child.wait();
            return Err("command stdout was not captured".into());
        }
    };
    let stderr = match child.stderr.take() {
        Some(stderr) => stderr,
        None => {
            kill_child(&mut child);
            let _ = child.wait();
            return Err("command stderr was not captured".into());
        }
    };
    let buffer = Arc::new(Mutex::new(CommandOutputBuffer::default()));
    let bytes = Arc::new(AtomicUsize::new(0));
    let exceeded = Arc::new(AtomicBool::new(false));
    let stdout_thread = spawn_output_reader(
        stdout,
        OutputStream::Stdout,
        buffer.clone(),
        bytes.clone(),
        exceeded.clone(),
    );
    let stderr_thread = spawn_output_reader(
        stderr,
        OutputStream::Stderr,
        buffer.clone(),
        bytes.clone(),
        exceeded.clone(),
    );
    let status = loop {
        if control.cancel.load(Ordering::Relaxed) {
            kill_child(&mut child);
            let _ = child.wait();
            join_output_reader(stdout_thread);
            join_output_reader(stderr_thread);
            return Err("tool execution cancelled".into());
        }
        if exceeded.load(Ordering::Relaxed) {
            kill_child(&mut child);
            let _ = child.wait();
            join_output_reader(stdout_thread);
            join_output_reader(stderr_thread);
            return Err(format!(
                "command output exceeded the {} KiB limit",
                MAX_TOOL_OUTPUT_BYTES / 1024
            ));
        }
        if Instant::now() >= control.deadline {
            kill_child(&mut child);
            let _ = child.wait();
            join_output_reader(stdout_thread);
            join_output_reader(stderr_thread);
            return Err(format!(
                "command timed out after {} seconds",
                TOOL_TIMEOUT.as_secs()
            ));
        }
        match child.try_wait() {
            Ok(Some(status)) => {
                // A shell can exit while a background descendant still owns
                // stdout or stderr. Close that process group before joining
                // the readers so the tool cannot wait forever for EOF.
                kill_child(&mut child);
                break status;
            }
            Ok(None) => {}
            Err(error) => {
                kill_child(&mut child);
                let _ = child.wait();
                join_output_reader(stdout_thread);
                join_output_reader(stderr_thread);
                return Err(error.to_string());
            }
        }
        thread::sleep(Duration::from_millis(20));
    };
    join_output_reader(stdout_thread);
    join_output_reader(stderr_thread);
    let output = buffer
        .lock()
        .map_err(|_| "command output lock was poisoned".to_string())?
        .clone();
    Ok(CommandOutput {
        status,
        stdout: output.stdout,
        stderr: output.stderr,
    })
}

#[derive(Clone, Default)]
struct CommandOutputBuffer {
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

#[derive(Clone, Copy)]
enum OutputStream {
    Stdout,
    Stderr,
}

fn spawn_output_reader<R: Read + Send + 'static>(
    reader: R,
    stream: OutputStream,
    buffer: Arc<Mutex<CommandOutputBuffer>>,
    bytes: Arc<AtomicUsize>,
    exceeded: Arc<AtomicBool>,
) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        let mut reader = reader;
        let mut chunk = [0_u8; 8 * 1024];
        while let Ok(read) = reader.read(&mut chunk) {
            if read == 0 {
                break;
            }
            let start = bytes.fetch_add(read, Ordering::Relaxed);
            if start >= MAX_TOOL_OUTPUT_BYTES {
                exceeded.store(true, Ordering::Relaxed);
                continue;
            }
            let keep = read.min(MAX_TOOL_OUTPUT_BYTES - start);
            if keep < read {
                exceeded.store(true, Ordering::Relaxed);
            }
            if let Ok(mut output) = buffer.lock() {
                match stream {
                    OutputStream::Stdout => output.stdout.extend_from_slice(&chunk[..keep]),
                    OutputStream::Stderr => output.stderr.extend_from_slice(&chunk[..keep]),
                }
            }
        }
    })
}

fn join_output_reader(thread: thread::JoinHandle<()>) {
    let _ = thread.join();
}

fn kill_child(child: &mut Child) {
    #[cfg(unix)]
    {
        let process_group = -(child.id() as i32);
        // The child is placed in its own process group before spawn so shell
        // descendants are terminated together with the command wrapper.
        unsafe {
            libc::kill(process_group, libc::SIGKILL);
        }
    }
    let _ = child.kill();
}

fn workspace_path(workspace: &Path, value: &str, allow_missing: bool) -> Result<PathBuf, String> {
    let input = Path::new(value);
    let workspace = workspace
        .canonicalize()
        .map_err(|error| error.to_string())?;
    if !input.is_absolute()
        && input
            .components()
            .any(|part| matches!(part, Component::ParentDir))
    {
        return Err(
            "path must stay inside the current workspace; use a workspace-relative path".into(),
        );
    }
    let path = if input.is_absolute() {
        input.to_path_buf()
    } else {
        workspace.join(input)
    };
    if path.exists() {
        let canonical = path.canonicalize().map_err(|error| error.to_string())?;
        if !canonical.starts_with(&workspace) {
            return Err(
                "path must stay inside the current workspace; use a workspace-relative path".into(),
            );
        }
        Ok(canonical)
    } else if allow_missing {
        let parent = path.parent().unwrap_or(&workspace);
        let existing = parent
            .ancestors()
            .find(|path| path.exists())
            .ok_or("no existing parent")?;
        let existing = existing.canonicalize().map_err(|error| error.to_string())?;
        if !existing.starts_with(&workspace) {
            return Err(
                "path must stay inside the current workspace; use a workspace-relative path".into(),
            );
        }
        // 逐组件校验并解析路径中的所有符号链接：链接必须逐跳跟随（目标
        // 内部可能还有链接，形成多跳链），每一跳的目标都必须落在工作区内，
        // 悬空链按词法解析同样逐跳校验；否则写入会经由链接链逃逸出工作区。
        let relative = path.strip_prefix(&existing).map_err(|_| {
            "path must stay inside the current workspace; use a workspace-relative path".to_string()
        })?;
        let mut queue: VecDeque<PathBuf> = relative
            .components()
            .map(|component| PathBuf::from(component.as_os_str()))
            .collect();
        let mut cursor = existing;
        let mut hops = 0usize;
        const MAX_SYMLINK_HOPS: usize = 40;
        while let Some(component) = queue.pop_front() {
            let next = if component.is_absolute() {
                // 链接目标为绝对路径（或根），游标落回该路径再继续。
                component
            } else {
                cursor.join(&component)
            };
            match fs::symlink_metadata(&next) {
                Ok(metadata) if metadata.file_type().is_symlink() => {
                    hops += 1;
                    if hops > MAX_SYMLINK_HOPS {
                        return Err("too many symlink hops while resolving path".into());
                    }
                    let target = fs::read_link(&next).map_err(|error| error.to_string())?;
                    let resolved = if target.is_absolute() {
                        target
                    } else {
                        next.parent().unwrap_or(&cursor).join(target)
                    };
                    let resolved = lexically_normalize(&resolved);
                    if !resolved.starts_with(&workspace) {
                        return Err(
                            "path must stay inside the current workspace; use a workspace-relative path"
                                .into(),
                        );
                    }
                    if resolved == next {
                        return Err("cyclic symlink while resolving path".into());
                    }
                    // 目标可能仍含链接组件，切回队列逐组件重新解析。
                    for target_component in resolved.components().rev() {
                        queue.push_front(PathBuf::from(target_component.as_os_str()));
                    }
                }
                Ok(_) => {
                    cursor = next;
                }
                Err(_) => {
                    // 悬空（组件或链目标不存在）：剩余组件不可能再经由链接
                    // 逃逸，原样保留即可。
                    queue.push_front(next);
                    break;
                }
            }
        }
        while let Some(component) = queue.pop_front() {
            cursor = cursor.join(component);
        }
        Ok(cursor)
    } else {
        Err("path does not exist".into())
    }
}

/// 不访问文件系统的路径规范化：解析 `.` 与 `..`，供悬空链接目标校验。
fn lexically_normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() {
                    out.push(component.as_os_str());
                }
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

fn truncate_output(text: &str) -> String {
    if text.len() <= MAX_TOOL_OUTPUT_BYTES {
        text.to_string()
    } else {
        format!(
            "{}\n[output truncated]",
            &text[..text.floor_char_boundary(MAX_TOOL_OUTPUT_BYTES)]
        )
    }
}
