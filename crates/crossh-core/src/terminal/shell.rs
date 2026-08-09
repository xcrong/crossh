//! Shell snippets used to instrument an SSH PTY without changing remote shell configuration.

use std::path::Path;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RemoteShell {
    Bash,
    Zsh,
    Fish,
}

const COMMAND_STATUS_TITLE_PREFIX: &str = "crossh-command-status=";

pub fn remote_shell_from_path(path: &str) -> Option<RemoteShell> {
    let name = Path::new(path).file_name()?.to_str()?;
    match name {
        "bash" => Some(RemoteShell::Bash),
        "zsh" => Some(RemoteShell::Zsh),
        "fish" => Some(RemoteShell::Fish),
        _ => None,
    }
}

/// Return the prompt/cwd/command hooks for a remote interactive shell.
pub fn remote_shell_setup_script(shell: RemoteShell) -> &'static str {
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
__crossh_command_serial=0
__crossh_report_prompt() {
    local __crossh_status=$?
    __crossh_command_serial=$((__crossh_command_serial + 1))
    trap - DEBUG
    printf '\033]133;D;%s\007\033]0;crossh-command-status=%s:%s\007\033]133;B\007\033]7;file://localhost%s\007\033]133;A\007' "$__crossh_status" "$__crossh_status" "$__crossh_command_serial" "$PWD"
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
typeset -g __crossh_command_serial=0
__crossh_report_pwd() { printf '\033]7;file://localhost%s\007' "$PWD"; }
__crossh_report_prompt() { local __crossh_status=$?; __crossh_command_serial=$((__crossh_command_serial + 1)); printf '\033]133;D;%s\007\033]0;crossh-command-status=%s:%s\007\033]133;B\007' "$__crossh_status" "$__crossh_status" "$__crossh_command_serial"; __crossh_report_pwd; printf '\033]133;A\007'; }
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
set -g __crossh_command_serial 0
function __crossh_report_prompt --on-event fish_prompt
    set -l __crossh_status $status
    set -g __crossh_command_serial (math "$__crossh_command_serial + 1")
    printf '\033]133;D;%s\007\033]0;crossh-command-status=%s:%s\007\033]133;B\007' "$__crossh_status" "$__crossh_status" "$__crossh_command_serial"
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

pub fn shell_setup_script_for_path(path: &str) -> Option<String> {
    let shell = remote_shell_from_path(path)?;
    Some(remote_shell_setup_script(shell).to_owned())
}

pub fn command_status_from_title(title: &str) -> Option<i32> {
    let value = title.strip_prefix(COMMAND_STATUS_TITLE_PREFIX)?;
    let status = value.split_once(':').map_or(value, |(status, _)| status);
    status.parse().ok()
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

    #[test]
    fn shell_setup_script_is_deterministic_for_supported_shells() {
        for path in ["/bin/bash", "/usr/bin/zsh", "fish"] {
            let script = shell_setup_script_for_path(path).expect("supported shell");
            assert!(script.contains("crossh-command="));
            assert!(script.contains("133;D"));
        }
        assert!(shell_setup_script_for_path("/bin/sh").is_none());
    }

    #[test]
    fn command_status_title_marker_parses_the_exit_code() {
        assert_eq!(
            command_status_from_title("crossh-command-status=127:4"),
            Some(127)
        );
        assert_eq!(
            command_status_from_title("crossh-command-status=0:5"),
            Some(0)
        );
        assert_eq!(
            command_status_from_title("crossh-command-status=bad:6"),
            None
        );
        assert_eq!(command_status_from_title("ordinary-title"), None);
    }
}
