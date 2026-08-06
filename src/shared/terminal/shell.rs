//! Shell snippets used to instrument an SSH PTY without changing remote shell configuration.

use std::path::Path;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RemoteShell {
    Bash,
    Zsh,
    Fish,
}

pub(crate) fn remote_shell_from_path(path: &str) -> Option<RemoteShell> {
    let name = Path::new(path).file_name()?.to_str()?;
    match name {
        "bash" => Some(RemoteShell::Bash),
        "zsh" => Some(RemoteShell::Zsh),
        "fish" => Some(RemoteShell::Fish),
        _ => None,
    }
}

/// Return the prompt/cwd/command hooks for a remote interactive shell.
pub(crate) fn remote_shell_setup_script(shell: RemoteShell) -> &'static str {
    match shell {
        RemoteShell::Bash => {
            r#"__crossh_report_command() {
    case "$BASH_COMMAND" in
        __crossh_*|trap\ *) return ;;
    esac
    local __crossh_command=$BASH_COMMAND
    local __crossh_encoded
    __crossh_encoded=$(printf '%s' "$__crossh_command" | base64 | tr -d '\n')
    printf '\033]1337;crossh-command=%s\007\033]133;C\007' "$__crossh_encoded"
}
__crossh_original_prompt_command=$PROMPT_COMMAND
__crossh_report_prompt() {
    local __crossh_status=$?
    trap - DEBUG
    printf '\033]133;D;%s\007\033]133;B\007\033]7;file://localhost%s\007\033]133;A\007' "$__crossh_status" "$PWD"
    if [ -n "$__crossh_original_prompt_command" ]; then
        eval "$__crossh_original_prompt_command"
    fi
    trap '__crossh_report_command' DEBUG
}
PROMPT_COMMAND=__crossh_report_prompt
trap '__crossh_report_command' DEBUG
"#
        }
        RemoteShell::Zsh => {
            r#"autoload -Uz add-zsh-hook
__crossh_report_pwd() { printf '\033]7;file://localhost%s\007' "$PWD"; }
__crossh_report_prompt() { local __crossh_status=$?; printf '\033]133;D;%s\007\033]133;B\007' "$__crossh_status"; __crossh_report_pwd; printf '\033]133;A\007'; }
__crossh_report_command() { local __crossh_encoded; __crossh_encoded=$(printf '%s' "$1" | base64 | tr -d '\n'); printf '\033]1337;crossh-command=%s\007\033]133;C\007' "$__crossh_encoded"; }
add-zsh-hook precmd __crossh_report_prompt
add-zsh-hook preexec __crossh_report_command
add-zsh-hook chpwd __crossh_report_pwd
"#
        }
        RemoteShell::Fish => {
            r#"function __crossh_report_pwd
    printf '\033]7;file://localhost%s\007' "$PWD"
end
function __crossh_report_prompt --on-event fish_prompt
    set -l __crossh_status $status
    printf '\033]133;D;%s\007\033]133;B\007' "$__crossh_status"
    __crossh_report_pwd
    printf '\033]133;A\007'
end
function __crossh_report_command --on-event fish_preexec
    set -l __crossh_encoded (printf '%s' "$argv[1]" | base64 | tr -d '\n')
    printf '\033]1337;crossh-command=%s\007\033]133;C\007' "$__crossh_encoded"
end
function __crossh_report_cwd --on-variable PWD
    __crossh_report_pwd
end
"#
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_supported_remote_shells() {
        assert_eq!(remote_shell_from_path("/bin/bash"), Some(RemoteShell::Bash));
        assert_eq!(
            remote_shell_from_path("/usr/bin/zsh"),
            Some(RemoteShell::Zsh)
        );
        assert_eq!(remote_shell_from_path("fish"), Some(RemoteShell::Fish));
        assert_eq!(remote_shell_from_path("/bin/sh"), None);
    }

    #[test]
    fn setup_scripts_contain_protocol_markers() {
        for shell in [RemoteShell::Bash, RemoteShell::Zsh, RemoteShell::Fish] {
            let setup = remote_shell_setup_script(shell);
            assert!(setup.contains("crossh-command="));
            assert!(setup.contains("133;A"));
        }
    }
}
