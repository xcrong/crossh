use super::*;

const SLASH_COMMANDS: &[(&str, &str)] = &[
    ("/help", "Show commands and shortcuts"),
    ("/hotkeys", "Show commands and shortcuts"),
    ("/model", "List or switch provider/model"),
    ("/thinking", "Set reasoning level"),
    ("/tools", "Show available tools"),
    ("/skills", "List project skills"),
    ("/skill", "Apply a skill to a request"),
    ("/prompts", "List prompt templates"),
    ("/prompt", "Run a prompt template"),
    ("/new", "Start a fresh session"),
    ("/clear", "Start a fresh session"),
    ("/continue", "Resume the most recent session"),
    ("/resume", "List or resume a saved session"),
    ("/tree", "List or rewind conversation tree"),
    ("/fork", "Branch the current conversation"),
    ("/clone", "Branch the current conversation"),
    ("/name", "Set or show session name"),
    ("/session", "Show session and context details"),
    ("/stats", "Show session and context details"),
    ("/compact", "Compact older conversation context"),
    ("/reload", "Reload project instructions and resources"),
    ("/export", "Export the session as Markdown"),
    ("/quit", "Quit"),
    ("/exit", "Quit"),
];

#[derive(Clone, Debug)]
pub(super) struct SlashCandidate {
    pub(super) insert: String,
    pub(super) display: String,
    /// 候选说明（自动补全浮层展示用）
    pub(super) desc: String,
}

pub(super) fn slash_candidates(app: &App) -> Vec<SlashCandidate> {
    let trimmed = app.input.trim_start();
    let slash_content = if let Some(rest) = trimmed.strip_prefix('/') {
        rest
    } else if let Some(rest) = trimmed.strip_prefix('、') {
        rest
    } else {
        return Vec::new();
    };
    if app.input.contains('\n') {
        return Vec::new();
    }
    let mut parts = slash_content.splitn(2, |c: char| c.is_whitespace());
    let cmd_prefix = parts.next().unwrap_or("");
    let arg_opt = parts.next();
    if arg_opt.is_none() {
        let lower = cmd_prefix.to_ascii_lowercase();
        let mut out = Vec::new();
        for (name, desc) in SLASH_COMMANDS {
            let bare = name.trim_start_matches('/').to_ascii_lowercase();
            if lower.is_empty()
                || bare.starts_with(&lower)
                || name.to_ascii_lowercase().starts_with(&format!("/{lower}"))
            {
                out.push(SlashCandidate {
                    insert: (*name).to_string(),
                    display: (*name).to_string(),
                    desc: (*desc).to_string(),
                });
            }
        }
        out.sort_by(|a, b| a.display.cmp(&b.display));
        out.dedup_by(|a, b| a.display == b.display);
        return out.into_iter().take(8).collect();
    }
    let cmd = cmd_prefix.to_ascii_lowercase();
    let arg_prefix = arg_opt.unwrap_or("").trim_start().to_ascii_lowercase();
    match cmd.as_str() {
        "skill" => {
            let mut out = Vec::new();
            for skill in &app.skills {
                if arg_prefix.is_empty() || skill.name.to_ascii_lowercase().starts_with(&arg_prefix)
                {
                    out.push(SlashCandidate {
                        insert: skill.name.clone(),
                        display: skill.name.clone(),
                        desc: skill.description().to_string(),
                    });
                }
            }
            out.sort_by(|a, b| a.display.cmp(&b.display));
            out.into_iter().take(8).collect()
        }
        "prompt" => {
            let mut out = Vec::new();
            for prompt in &app.prompts {
                if arg_prefix.is_empty()
                    || prompt.name.to_ascii_lowercase().starts_with(&arg_prefix)
                {
                    out.push(SlashCandidate {
                        insert: prompt.name.clone(),
                        display: prompt.name.clone(),
                        desc: prompt.description().to_string(),
                    });
                }
            }
            out.sort_by(|a, b| a.display.cmp(&b.display));
            out.into_iter().take(8).collect()
        }
        "model" => {
            let mut out = Vec::new();
            for provider in &app.settings.providers {
                for model in &provider.models {
                    let full = format!("{}/{}", provider.id, model.id);
                    if arg_prefix.is_empty()
                        || full.to_ascii_lowercase().starts_with(&arg_prefix)
                        || model.id.to_ascii_lowercase().starts_with(&arg_prefix)
                    {
                        out.push(SlashCandidate {
                            insert: full.clone(),
                            display: full.clone(),
                            desc: if model.reasoning {
                                "reasoning".into()
                            } else {
                                provider.name.clone()
                            },
                        });
                    }
                }
            }
            out.sort_by(|a, b| a.display.cmp(&b.display));
            out.into_iter().take(8).collect()
        }
        "thinking" => ALL_THINKING_LEVELS
            .iter()
            .filter(|level| arg_prefix.is_empty() || level.label().starts_with(&arg_prefix))
            .map(|level| SlashCandidate {
                insert: level.label().to_string(),
                display: level.label().to_string(),
                desc: "reasoning level".into(),
            })
            .take(8)
            .collect(),
        _ => Vec::new(),
    }
}

pub(super) fn apply_slash_completion(app: &mut App, candidate: &SlashCandidate) {
    let trimmed = app.input.trim_start();
    let leading_len = app.input.len() - trimmed.len();
    let leading = app.input[..leading_len].to_string();
    let slash_char = trimmed.chars().next().unwrap_or('/');
    let slash_str = slash_char.to_string();
    let content = if trimmed.len() >= slash_str.len() {
        &trimmed[slash_str.len()..]
    } else {
        ""
    };
    let has_arg = content.chars().any(|c| c.is_whitespace());
    let new_input = if !has_arg {
        let needs_space = matches!(
            candidate.insert.as_str(),
            "/skill"
                | "/prompt"
                | "/model"
                | "/thinking"
                | "/resume"
                | "/tree"
                | "/name"
                | "/export"
        );
        if needs_space {
            format!(
                "{}{}{} ",
                leading,
                slash_str,
                candidate.insert.trim_start_matches('/')
            )
        } else {
            format!(
                "{}{}{}",
                leading,
                slash_str,
                candidate.insert.trim_start_matches('/')
            )
        }
    } else {
        let mut parts = content.splitn(2, |c: char| c.is_whitespace());
        let cmd = parts.next().unwrap_or("");
        format!("{}{}{} {}", leading, slash_str, cmd, candidate.insert)
    };
    app.input = new_input;
    app.input_cursor = app.input.len();
    app.history_cursor = None;
    app.slash_selected = 0;
}
