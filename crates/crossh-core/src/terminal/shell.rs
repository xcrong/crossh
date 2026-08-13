//! Shell snippets used to instrument interactive PTYs without changing shell configuration.

use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use std::env;
use std::ffi::OsStr;
use std::fs;
use std::io;
use std::path::Path;
use tempfile::{Builder as TempDirBuilder, TempDir};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RemoteShell {
    Bash,
    Zsh,
    Fish,
}

const COMMAND_STATUS_TITLE_PREFIX: &str = "crossh-command-status=";
const COMMAND_TITLE_PREFIX: &str = "crossh-command=";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShellCommandMarker {
    pub command: String,
    pub cwd: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShellPromptMarker {
    pub status: i32,
    pub cwd: String,
}

/// Owns the temporary startup configuration for one local interactive shell.
/// Dropping this value removes the generated files.
#[derive(Debug)]
pub struct LocalShellEnvironment {
    program: String,
    args: Vec<String>,
    env: Vec<(String, String)>,
    use_system_shell: bool,
    _temp_dir: Option<TempDir>,
}

impl LocalShellEnvironment {
    pub fn create(path: &str) -> io::Result<Option<Self>> {
        Self::create_with_zdotdir(path, env::var_os("ZDOTDIR").as_deref())
    }

    pub fn program(&self) -> &str {
        &self.program
    }

    pub fn args(&self) -> &[String] {
        &self.args
    }

    pub fn env(&self) -> &[(String, String)] {
        &self.env
    }

    pub fn use_system_shell(&self) -> bool {
        self.use_system_shell
    }

    fn create_with_zdotdir(
        path: &str,
        original_zdotdir: Option<&OsStr>,
    ) -> io::Result<Option<Self>> {
        let Some(shell) = remote_shell_from_path(path) else {
            return Ok(None);
        };
        let setup = shell_setup_script(shell);

        let mut environment = match shell {
            RemoteShell::Bash => {
                let temp_dir = new_shell_temp_dir()?;
                let rc_path = temp_dir.path().join("crossh.bash");
                fs::write(
                    &rc_path,
                    format!(
                        r#"builtin set +o posix
builtin unset ENV
if [[ ${{CROSSH_BASH_LOGIN:-0}} == 1 ]]; then
    if [[ -r /etc/profile ]]; then builtin source /etc/profile; fi
    for __crossh_profile in "$HOME/.bash_profile" "$HOME/.bash_login" "$HOME/.profile"; do
        if [[ -r "$__crossh_profile" ]]; then
            builtin source "$__crossh_profile"
            break
        fi
    done
else
    for __crossh_bashrc in /etc/bash.bashrc /etc/bash/bashrc; do
        if [[ -r "$__crossh_bashrc" ]]; then
            builtin source "$__crossh_bashrc"
            break
        fi
    done
    if [[ -r "$HOME/.bashrc" ]]; then builtin source "$HOME/.bashrc"; fi
fi
builtin unset CROSSH_BASH_LOGIN __crossh_profile __crossh_bashrc
{setup}
"#
                    ),
                )?;
                Self {
                    program: path.to_owned(),
                    args: vec![
                        "--rcfile".to_string(),
                        path_to_string(&rc_path)?,
                        "-i".to_string(),
                    ],
                    env: vec![(
                        "CROSSH_BASH_LOGIN".to_string(),
                        u8::from(cfg!(target_os = "macos")).to_string(),
                    )],
                    use_system_shell: false,
                    _temp_dir: Some(temp_dir),
                }
            }
            RemoteShell::Zsh => {
                let temp_dir = new_shell_temp_dir()?;
                let integration_path = temp_dir.path().join("crossh-integration.zsh");
                fs::write(&integration_path, zsh_deferred_setup_script())?;

                let user_zdotdir = match original_zdotdir {
                    Some(zdotdir) => format!(
                        "builtin export ZDOTDIR={}",
                        shell_quote(&os_str_to_string(zdotdir)?),
                    ),
                    None => "builtin unset ZDOTDIR".to_string(),
                };
                fs::write(
                    temp_dir.path().join(".zshenv"),
                    format!(
                        r#"{user_zdotdir}
if [[ -r "${{ZDOTDIR:-$HOME}}/.zshenv" ]]; then
    builtin source "${{ZDOTDIR:-$HOME}}/.zshenv"
fi
builtin source {}
"#,
                        shell_quote(&path_to_string(&integration_path)?),
                    ),
                )?;
                Self {
                    program: path.to_owned(),
                    args: Vec::new(),
                    env: vec![("ZDOTDIR".to_string(), path_to_string(temp_dir.path())?)],
                    use_system_shell: true,
                    _temp_dir: Some(temp_dir),
                }
            }
            RemoteShell::Fish => {
                let temp_dir = new_shell_temp_dir()?;
                let data_dir = temp_dir.path().join("share");
                let conf_dir = data_dir.join("fish/vendor_conf.d");
                fs::create_dir_all(&conf_dir)?;
                fs::write(conf_dir.join("crossh.fish"), format!("{setup}\n"))?;
                let mut data_dirs = path_to_string(&data_dir)?;
                if let Some(existing) = std::env::var_os("XDG_DATA_DIRS") {
                    data_dirs.push(':');
                    data_dirs.push_str(&os_str_to_string(&existing)?);
                }
                Self {
                    program: path.to_owned(),
                    args: Vec::new(),
                    env: vec![("XDG_DATA_DIRS".to_string(), data_dirs)],
                    use_system_shell: true,
                    _temp_dir: Some(temp_dir),
                }
            }
        };
        if let Some(binary_dir) = env::current_exe()?.parent() {
            let mut paths = vec![binary_dir.to_path_buf()];
            if let Some(existing) = env::var_os("PATH") {
                paths.extend(env::split_paths(&existing));
            }
            let path = env::join_paths(paths)
                .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
            environment
                .env
                .push(("PATH".to_string(), os_str_to_string(&path)?));
        }
        Ok(Some(environment))
    }
}

fn new_shell_temp_dir() -> io::Result<TempDir> {
    TempDirBuilder::new().prefix("crossh-shell-").tempdir()
}

fn path_to_string(path: &Path) -> io::Result<String> {
    os_str_to_string(path.as_os_str())
}

fn os_str_to_string(value: &OsStr) -> io::Result<String> {
    value
        .to_str()
        .map(ToOwned::to_owned)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "shell path is not valid UTF-8"))
}

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
    if [ "${__crossh_capture_enabled:-1}" != 1 ]; then
        return
    fi
    case "$BASH_COMMAND" in
        __crossh_*|trap\ *) return ;;
    esac
    local __crossh_command=$BASH_COMMAND
    local __crossh_encoded __crossh_cwd_encoded
    __crossh_encoded=$(printf '%s' "$__crossh_command" | base64 | tr -d '\n')
    __crossh_cwd_encoded=$(printf '%s' "$PWD" | base64 | tr -d '\n')
    printf '\033]0;crossh-command=%s:%s\007\033]1337;crossh-command=%s:%s\007\033]133;C\007' "$__crossh_encoded" "$__crossh_cwd_encoded" "$__crossh_encoded" "$__crossh_cwd_encoded"
}
__crossh_original_prompt_command=$PROMPT_COMMAND
__crossh_command_serial=0
__crossh_report_prompt() {
    local __crossh_status=$?
    __crossh_command_serial=$((__crossh_command_serial + 1))
    trap - DEBUG
    local __crossh_cwd_encoded
    __crossh_cwd_encoded=$(printf '%s' "$PWD" | base64 | tr -d '\n')
    printf '\033]133;D;%s\007\033]0;crossh-command-status=%s:%s:%s\007\033]133;B\007\033]7;file://localhost%s\007\033]133;A\007' "$__crossh_status" "$__crossh_status" "$__crossh_command_serial" "$__crossh_cwd_encoded" "$PWD"
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
__crossh_report_prompt() { local __crossh_status=$?; __crossh_command_serial=$((__crossh_command_serial + 1)); local __crossh_cwd_encoded; __crossh_cwd_encoded=$(printf '%s' "$PWD" | base64 | tr -d '\n'); printf '\033]133;D;%s\007\033]0;crossh-command-status=%s:%s:%s\007\033]133;B\007' "$__crossh_status" "$__crossh_status" "$__crossh_command_serial" "$__crossh_cwd_encoded"; __crossh_report_pwd; printf '\033]133;A\007'; }
__crossh_report_command() {
    if [[ ${__crossh_capture_enabled:-1} -ne 1 ]]; then
        return
    fi
    local __crossh_encoded __crossh_cwd_encoded
    __crossh_encoded=$(printf '%s' "$1" | base64 | tr -d '\n')
    __crossh_cwd_encoded=$(printf '%s' "$PWD" | base64 | tr -d '\n')
    printf '\033]0;crossh-command=%s:%s\007\033]1337;crossh-command=%s:%s\007\033]133;C\007' "$__crossh_encoded" "$__crossh_cwd_encoded" "$__crossh_encoded" "$__crossh_cwd_encoded"
}
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
    set -l __crossh_cwd_encoded (printf '%s' "$PWD" | base64 | tr -d '\n')
    printf '\033]133;D;%s\007\033]0;crossh-command-status=%s:%s:%s\007\033]133;B\007' "$__crossh_status" "$__crossh_status" "$__crossh_command_serial" "$__crossh_cwd_encoded"
    __crossh_report_pwd
    printf '\033]133;A\007'
end
function __crossh_report_command --on-event fish_preexec
    if set -q __crossh_capture_enabled
        if test "$__crossh_capture_enabled" != 1
            return
        end
    end
    set -l __crossh_encoded (printf '%s' "$argv[1]" | base64 | tr -d '\n')
    set -l __crossh_cwd_encoded (printf '%s' "$PWD" | base64 | tr -d '\n')
    printf '\033]0;crossh-command=%s:%s\007\033]1337;crossh-command=%s:%s\007\033]133;C\007' "$__crossh_encoded" "$__crossh_cwd_encoded" "$__crossh_encoded" "$__crossh_cwd_encoded"
end
function __crossh_report_cwd --on-variable PWD
    __crossh_report_pwd
end
"#
        }
    }
}

fn shell_setup_script(shell: RemoteShell) -> String {
    let (capture_disabled, capture_enabled) = match shell {
        RemoteShell::Bash | RemoteShell::Zsh => {
            ("__crossh_capture_enabled=0", "__crossh_capture_enabled=1")
        }
        RemoteShell::Fish => (
            "set -g __crossh_capture_enabled 0",
            "set -g __crossh_capture_enabled 1",
        ),
    };
    format!(
        "{capture_disabled}\n{}\n{capture_enabled}",
        remote_shell_setup_script(shell)
    )
}

fn zsh_deferred_setup_script() -> String {
    let setup = shell_setup_script(RemoteShell::Zsh);
    format!(
        r#"builtin typeset -ga precmd_functions
precmd_functions+=(__crossh_deferred_init)
__crossh_deferred_init() {{
    builtin emulate -L zsh -o no_aliases
    builtin typeset -ga precmd_functions
    precmd_functions=(${{precmd_functions:#__crossh_deferred_init}})
    {setup}
    __crossh_report_prompt
    builtin unfunction __crossh_deferred_init
}}
"#
    )
}

/// Start a remote interactive shell with Crossh hooks without changing the
/// user's shell configuration files.
pub fn remote_shell_bootstrap_command() -> String {
    let bash_rc = format!(
        "source ~/.bashrc\n{}",
        remote_shell_setup_script(RemoteShell::Bash)
    );
    let zsh_integration = zsh_deferred_setup_script();
    let zshenv = r#"if [[ ${CROSSH_USER_ZDOTDIR_SET:-0} == 1 ]]; then
    builtin export ZDOTDIR="$CROSSH_USER_ZDOTDIR"
else
    builtin unset ZDOTDIR
fi
builtin unset CROSSH_USER_ZDOTDIR CROSSH_USER_ZDOTDIR_SET
if [[ -r "${ZDOTDIR:-$HOME}/.zshenv" ]]; then
    builtin source "${ZDOTDIR:-$HOME}/.zshenv"
fi
builtin source "$CROSSH_ZSH_INTEGRATION"
builtin unset CROSSH_ZSH_INTEGRATION
    "#;
    let fish_setup = remote_shell_setup_script(RemoteShell::Fish);
    let bash_rc_encoded = BASE64.encode(bash_rc.as_bytes());
    let zsh_integration_encoded = BASE64.encode(zsh_integration.as_bytes());
    let zshenv_encoded = BASE64.encode(zshenv.as_bytes());
    let fish_setup_encoded = BASE64.encode(fish_setup.as_bytes());
    let selector = format!(
        "case \"${{SHELL##*/}}\" in\n\
bash)\n\
    d=$(mktemp -d \"${{TMPDIR:-/tmp}}/crossh-shell.XXXXXX\") || exit 1\n\
    trap 'rm -rf \"$d\"' 0\n\
    printf %s {} | base64 -d > \"$d/.bashrc\"\n\
    bash --rcfile \"$d/.bashrc\" -i\n\
    status=$?\n\
    exit \"$status\"\n\
    ;;\n\
zsh)\n\
    d=$(mktemp -d \"${{TMPDIR:-/tmp}}/crossh-shell.XXXXXX\") || exit 1\n\
    trap 'rm -rf \"$d\"' 0\n\
    if [ \"${{ZDOTDIR+x}}\" = x ]; then CROSSH_USER_ZDOTDIR_SET=1 CROSSH_USER_ZDOTDIR=$ZDOTDIR; else CROSSH_USER_ZDOTDIR_SET=0 CROSSH_USER_ZDOTDIR=; fi\n\
    export CROSSH_USER_ZDOTDIR_SET CROSSH_USER_ZDOTDIR\n\
    printf %s {} | base64 -d > \"$d/crossh-integration.zsh\"\n\
    printf %s {} | base64 -d > \"$d/.zshenv\"\n\
    CROSSH_ZSH_INTEGRATION=\"$d/crossh-integration.zsh\" ZDOTDIR=\"$d\" zsh -i\n\
    status=$?\n\
    exit \"$status\"\n\
    ;;\n\
fish)\n\
    d=$(mktemp -d \"${{TMPDIR:-/tmp}}/crossh-shell.XXXXXX\") || exit 1\n\
    trap 'rm -rf \"$d\"' 0\n\
    printf %s {} | base64 -d > \"$d/config.fish\"\n\
    fish --init-command \"source $d/config.fish\" -i\n\
    status=$?\n\
    exit \"$status\"\n\
    ;;\n\
*)\n\
    \"${{SHELL:-/bin/sh}}\" -i\n\
    status=$?\n\
    exit \"$status\"\n\
    ;;\n\
esac",
        bash_rc_encoded, zsh_integration_encoded, zshenv_encoded, fish_setup_encoded,
    );
    // SSH servers pass the remote command through the user's login shell. Do
    // not embed the multi-line selector in another layer of shell quoting:
    // OpenSSH joins command arguments before the server parses them, and the
    // nested quotes can be interpreted differently by `/bin/sh` variants.
    // The payload is shell-safe base64, so only a small, portable decoder is
    // parsed remotely. Decode into a temporary script before sourcing it;
    // piping directly into `sh` would steal the SSH PTY's stdin and make an
    // interactive shell exit immediately on EOF.
    let encoded = BASE64.encode(selector.as_bytes());
    format!(
        "d=$(mktemp -d \"${{TMPDIR:-/tmp}}/crossh-bootstrap.XXXXXX\") || exit 1; trap 'rm -rf \"$d\"' 0; printf %s {encoded} | base64 -d > \"$d/boot.sh\" || exit 1; . \"$d/boot.sh\""
    )
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

/// Decode the command and working directory emitted through the title channel.
pub fn command_marker_from_title(title: &str) -> Option<ShellCommandMarker> {
    let marker = title.strip_prefix(COMMAND_TITLE_PREFIX)?;
    let (command, cwd) = marker.split_once(':')?;
    Some(ShellCommandMarker {
        command: decode_marker_field(command)?,
        cwd: decode_marker_field(cwd)?,
    })
}

/// Decode the prompt status and current working directory title marker.
pub fn prompt_marker_from_title(title: &str) -> Option<ShellPromptMarker> {
    let marker = title.strip_prefix(COMMAND_STATUS_TITLE_PREFIX)?;
    let mut fields = marker.splitn(3, ':');
    let status = fields.next()?.parse().ok()?;
    fields.next()?.parse::<u64>().ok()?;
    let cwd = decode_marker_field(fields.next()?)?;
    Some(ShellPromptMarker { status, cwd })
}

fn decode_marker_field(encoded: &str) -> Option<String> {
    String::from_utf8(BASE64.decode(encoded).ok()?).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    use std::io::Write;
    #[cfg(unix)]
    use std::process::Command;
    #[cfg(unix)]
    use std::process::Stdio;

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
            assert!(setup.contains("\\033]0;crossh-command="));
            assert!(setup.contains("133;A"));
        }
    }

    #[test]
    fn shell_setup_script_is_deterministic_for_supported_shells() {
        for shell in [RemoteShell::Bash, RemoteShell::Zsh, RemoteShell::Fish] {
            let script = shell_setup_script(shell);
            assert!(script.contains("crossh-command="));
            assert!(script.contains("133;D"));
        }
    }

    #[test]
    fn command_title_marker_decodes_command_and_cwd() {
        let command = BASE64.encode("echo hi");
        let cwd = BASE64.encode("/tmp/my project");
        assert_eq!(
            command_marker_from_title(&format!("crossh-command={command}:{cwd}")),
            Some(ShellCommandMarker {
                command: "echo hi".to_string(),
                cwd: "/tmp/my project".to_string(),
            })
        );
        assert_eq!(command_marker_from_title("ordinary-title"), None);
        assert_eq!(command_marker_from_title("crossh-command=not-base64"), None);
    }

    #[test]
    fn local_setup_suppresses_hook_registration_commands() {
        let setup = shell_setup_script(RemoteShell::Bash);
        assert!(setup.starts_with("__crossh_capture_enabled=0\n"));
        assert!(setup.contains("__crossh_capture_enabled=1"));
        assert!(!setup.contains("__crossh_skip_next_command"));
    }

    #[test]
    fn local_bash_environment_loads_user_config_and_cleans_up() {
        let environment = LocalShellEnvironment::create_with_zdotdir("/bin/bash", None)
            .unwrap()
            .unwrap();
        let temp_path = environment._temp_dir.as_ref().unwrap().path().to_path_buf();
        let rc_path = temp_path.join("crossh.bash");
        let rc = fs::read_to_string(&rc_path).unwrap();

        assert_eq!(environment.program(), "/bin/bash");
        assert_eq!(environment.args()[0], "--rcfile");
        assert_eq!(environment.args()[1], rc_path.to_string_lossy());
        assert_eq!(environment.args()[2], "-i");
        assert_eq!(environment.env()[0].0, "CROSSH_BASH_LOGIN");
        let binary_dir = env::current_exe().unwrap().parent().unwrap().to_path_buf();
        let injected_path = environment
            .env()
            .iter()
            .find(|(name, _)| name == "PATH")
            .map(|(_, value)| value)
            .unwrap();
        assert_eq!(
            env::split_paths(OsStr::new(injected_path)).next(),
            Some(binary_dir)
        );
        assert!(!environment.use_system_shell());
        assert!(
            rc.find("$HOME/.bash_profile").unwrap() < rc.find("__crossh_report_command").unwrap()
        );

        drop(environment);
        assert!(!temp_path.exists());
    }

    #[test]
    fn local_zsh_environment_restores_custom_zdotdir() {
        let original_zdotdir = OsStr::new("/tmp/custom zsh");
        let environment =
            LocalShellEnvironment::create_with_zdotdir("/bin/zsh", Some(original_zdotdir))
                .unwrap()
                .unwrap();
        let temp_path = environment._temp_dir.as_ref().unwrap().path().to_path_buf();
        let zshenv = fs::read_to_string(temp_path.join(".zshenv")).unwrap();
        let integration = fs::read_to_string(temp_path.join("crossh-integration.zsh")).unwrap();

        assert!(environment.args().is_empty());
        assert!(environment.use_system_shell());
        assert_eq!(environment.env()[0].0, "ZDOTDIR");
        assert_eq!(environment.env()[0].1, temp_path.to_string_lossy());
        assert!(zshenv.contains("export ZDOTDIR='/tmp/custom zsh'"));
        assert!(zshenv.contains("${ZDOTDIR:-$HOME}/.zshenv"));
        assert!(integration.contains("precmd_functions+=(__crossh_deferred_init)"));
        assert!(integration.contains("__crossh_report_command"));

        drop(environment);
        assert!(!temp_path.exists());
    }

    #[test]
    fn local_fish_environment_uses_vendor_configuration() {
        let environment = LocalShellEnvironment::create_with_zdotdir("/usr/bin/fish", None)
            .unwrap()
            .unwrap();
        let temp_path = environment._temp_dir.as_ref().unwrap().path().to_path_buf();
        let integration =
            fs::read_to_string(temp_path.join("share/fish/vendor_conf.d/crossh.fish")).unwrap();

        assert_eq!(environment.program(), "/usr/bin/fish");
        assert!(environment.args().is_empty());
        assert!(environment.use_system_shell());
        assert_eq!(environment.env()[0].0, "XDG_DATA_DIRS");
        assert!(
            environment.env()[0]
                .1
                .starts_with(&temp_path.join("share").to_string_lossy().into_owned())
        );
        assert!(integration.contains("__crossh_report_command"));
    }

    #[test]
    fn remote_bootstrap_selects_supported_shells() {
        let command = remote_shell_bootstrap_command();
        assert!(command.starts_with("d=$(mktemp -d "));
        assert!(command.contains("base64 -d > \"$d/boot.sh\""));
        assert!(command.contains(". \"$d/boot.sh\""));
        assert!(command.contains("base64 -d"));
    }

    #[cfg(unix)]
    #[test]
    fn local_bash_environment_captures_commands_without_echoing_setup() {
        let user_home = TempDirBuilder::new()
            .prefix("crossh-bash-home-")
            .tempdir()
            .unwrap();
        fs::write(user_home.path().join(".bashrc"), "").unwrap();
        let environment = LocalShellEnvironment::create_with_zdotdir("/bin/bash", None)
            .unwrap()
            .unwrap();
        let mut child = Command::new(environment.program())
            .args(environment.args())
            .env("HOME", user_home.path())
            .envs(environment.env().iter().cloned())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(b"printf '%s' 'echo hi'\nexit\n")
            .unwrap();
        let output = child.wait_with_output().unwrap();
        assert!(output.status.success());

        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        let marker = stdout
            .split("\x1b]0;crossh-command=")
            .nth(1)
            .and_then(|marker| marker.split('\x07').next())
            .unwrap_or_else(|| {
                panic!("bash hook should emit a command title: stdout={stdout:?} stderr={stderr:?}")
            });
        let marker = command_marker_from_title(&format!("crossh-command={marker}"))
            .expect("bash marker should decode");
        assert_eq!(marker.command, "printf '%s' 'echo hi'".to_string());
        assert_eq!(
            marker.cwd,
            std::env::current_dir().unwrap().to_string_lossy()
        );
        assert!(!stdout.contains("__crossh_report_command() {"));
        assert!(!stderr.contains("__crossh_report_command() {"));
        assert!(!stderr.contains("function>"));
    }

    #[cfg(unix)]
    #[test]
    fn local_zsh_environment_captures_commands_without_echoing_setup() {
        let user_config = TempDirBuilder::new()
            .prefix("crossh-zsh-home-")
            .tempdir()
            .unwrap();
        fs::write(user_config.path().join(".zshrc"), "").unwrap();
        let Ok(version) = Command::new("zsh").arg("-c").arg("exit 0").output() else {
            return;
        };
        assert!(version.status.success());

        let environment = LocalShellEnvironment::create_with_zdotdir(
            "/bin/zsh",
            Some(user_config.path().as_os_str()),
        )
        .unwrap()
        .unwrap();
        let mut child = Command::new(environment.program())
            .args(environment.args())
            .arg("-i")
            .envs(environment.env().iter().cloned())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(b"printf '%s' 'echo hi'\nexit\n")
            .unwrap();
        let output = child.wait_with_output().unwrap();
        assert!(output.status.success());

        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        let marker = stdout
            .split("\x1b]0;crossh-command=")
            .nth(1)
            .and_then(|marker| marker.split('\x07').next())
            .unwrap_or_else(|| panic!("zsh hook should emit a command title: {stdout:?}"));
        let marker = command_marker_from_title(&format!("crossh-command={marker}"))
            .expect("zsh marker should decode");
        assert_eq!(marker.command, "printf '%s' 'echo hi'".to_string());
        assert_eq!(
            marker.cwd,
            std::env::current_dir().unwrap().to_string_lossy()
        );
        assert!(!stdout.contains("__crossh_report_command() {"));
        assert!(!stderr.contains("__crossh_report_command() {"));
        assert!(!stderr.contains("function>"));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn local_zsh_preserves_login_startup_and_default_history_path() {
        let user_home = TempDirBuilder::new()
            .prefix("crossh-zsh-login-home-")
            .tempdir()
            .unwrap();
        fs::write(
            user_home.path().join(".zprofile"),
            "print -r -- crossh-zprofile-loaded\n",
        )
        .unwrap();
        fs::write(
            user_home.path().join(".zshrc"),
            "print -r -- crossh-zshrc-loaded\n",
        )
        .unwrap();
        let environment = LocalShellEnvironment::create_with_zdotdir("/bin/zsh", None)
            .unwrap()
            .unwrap();
        let integration_dir = environment._temp_dir.as_ref().unwrap().path().to_path_buf();
        let mut child = Command::new(environment.program())
            .args(["-l", "-i"])
            .env("HOME", user_home.path())
            .envs(environment.env().iter().cloned())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(b"print -r -- crossh-histfile=$HISTFILE\nexit\n")
            .unwrap();
        let output = child.wait_with_output().unwrap();
        assert!(output.status.success());

        let stdout = String::from_utf8_lossy(&output.stdout);
        let zprofile = stdout.find("crossh-zprofile-loaded").unwrap();
        let zshrc = stdout.find("crossh-zshrc-loaded").unwrap();
        assert!(zprofile < zshrc);
        assert!(
            stdout.contains(&format!("crossh-histfile={}/", user_home.path().display())),
            "unexpected zsh startup output: {stdout:?}"
        );
        assert!(!stdout.contains(&integration_dir.to_string_lossy().into_owned()));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn local_bash_loads_login_profile_once() {
        let user_home = TempDirBuilder::new()
            .prefix("crossh-bash-login-home-")
            .tempdir()
            .unwrap();
        fs::write(
            user_home.path().join(".bash_profile"),
            "printf '%s\\n' crossh-bash-profile-loaded\n",
        )
        .unwrap();
        fs::write(
            user_home.path().join(".bashrc"),
            "printf '%s\\n' crossh-bashrc-loaded\n",
        )
        .unwrap();
        let environment = LocalShellEnvironment::create_with_zdotdir("/bin/bash", None)
            .unwrap()
            .unwrap();
        let mut child = Command::new(environment.program())
            .args(environment.args())
            .env("HOME", user_home.path())
            .envs(environment.env().iter().cloned())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child.stdin.take().unwrap().write_all(b"exit\n").unwrap();
        let output = child.wait_with_output().unwrap();
        assert!(output.status.success());

        let stdout = String::from_utf8_lossy(&output.stdout);
        assert_eq!(stdout.matches("crossh-bash-profile-loaded").count(), 1);
        assert!(!stdout.contains("crossh-bashrc-loaded"));
    }

    #[cfg(unix)]
    #[test]
    fn remote_bootstrap_is_valid_for_the_posix_launcher() {
        let command = remote_shell_bootstrap_command();
        let output = Command::new("sh")
            .args(["-n", "-c", &remote_shell_bootstrap_command()])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "remote shell bootstrap is invalid: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(!command.contains('\n'));
        let encoded = command
            .split("printf %s ")
            .nth(1)
            .and_then(|value| value.split(" | base64 -d >").next())
            .expect("bootstrap should use the encoded remote payload");
        let decoded = String::from_utf8(BASE64.decode(encoded).unwrap()).unwrap();
        assert!(decoded.contains("case \"${SHELL##*/}\" in"));
        let decoded_check = Command::new("sh")
            .args(["-n", "-c", &decoded])
            .output()
            .unwrap();
        assert!(
            decoded_check.status.success(),
            "decoded bootstrap is invalid: {}\n{}",
            decoded,
            String::from_utf8_lossy(&decoded_check.stderr)
        );
    }

    #[test]
    fn prompt_title_marker_parses_status_and_cwd() {
        let cwd = BASE64.encode("/tmp/next");
        assert_eq!(
            prompt_marker_from_title(&format!("crossh-command-status=127:4:{cwd}")),
            Some(ShellPromptMarker {
                status: 127,
                cwd: "/tmp/next".to_string(),
            })
        );
        assert_eq!(prompt_marker_from_title("crossh-command-status=0:5"), None);
        assert_eq!(
            prompt_marker_from_title("crossh-command-status=bad:6:eA=="),
            None
        );
        assert_eq!(prompt_marker_from_title("ordinary-title"), None);
    }
}
