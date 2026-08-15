//! Agent settings persistence shared by the GUI and the standalone CLI.
//!
//! The application writes one flat `~/.config/crossh/settings.toml` for every
//! feature. This module owns the `[agent]` section of that file; unknown fields
//! for other features are ignored so both binaries can read the same file.

use std::fs;
use std::path::PathBuf;

use serde::Deserialize;

use crate::policy::AgentSettings;

const SETTINGS_FILE_NAME: &str = "settings.toml";

#[derive(Debug, Default, Deserialize)]
struct AgentSettingsFile {
    #[serde(default)]
    agent: AgentSettings,
}

/// 读取共享 `settings.toml` 中的 agent 段。
///
/// 文件缺失、解析失败时回退默认设置，与应用的 settings persistence 语义一致；
/// GUI 调整设置后无需同步，两边读到同一份文件。
pub fn load() -> AgentSettings {
    let Some(path) = settings_path() else {
        return AgentSettings::default();
    };
    match fs::read_to_string(path) {
        Ok(contents) => match toml::from_str::<AgentSettingsFile>(&contents) {
            Ok(file) => file.agent,
            Err(error) => {
                log::warn!("failed to parse agent settings: {error}");
                AgentSettings::default()
            }
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => AgentSettings::default(),
        Err(error) => {
            log::warn!("failed to read agent settings: {error}");
            AgentSettings::default()
        }
    }
}

pub fn settings_path() -> Option<PathBuf> {
    dirs::home_dir().map(settings_path_from_home)
}

fn settings_path_from_home(home: PathBuf) -> PathBuf {
    home.join(".config").join("crossh").join(SETTINGS_FILE_NAME)
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    fn gui_style_settings_toml() -> String {
        r#"language = "English"
terminal_font_size = 18.0
terminal_show_timestamps = false
terminal_scrollback = 5000
updates_check_on_startup = true
show_host_sidebar = true
show_quick_commands = false
recent_local_dirs_max = 4

[agent]
max_tool_rounds = 60

[[agent.providers]]
id = "local"
name = "Local"
protocol = "openai-chat"
url = "http://127.0.0.1:11434/v1/chat/completions"
api_key_env = ""
api_key = ""

[[agent.providers.models]]
id = "qwen3-coder"
name = "qwen3-coder"
reasoning = true
context_window = 128000
max_tokens = 32000

[agent.active_model]
provider = "local"
model = "qwen3-coder"

[agent.reviewer_model]
provider = "local"
model = "qwen3-coder"
"#
        .into()
    }

    #[test]
    fn agent_section_is_extracted_from_the_gui_style_file() {
        let file: AgentSettingsFile = toml::from_str(&gui_style_settings_toml())
            .expect("GUI-style settings should parse with unrelated fields ignored");
        assert_eq!(file.agent.max_tool_rounds, 60);
        assert_eq!(file.agent.providers.len(), 1);
        assert_eq!(file.agent.providers[0].id, "local");
        assert_eq!(file.agent.providers[0].models.len(), 1);
        assert_eq!(file.agent.providers[0].models[0].id, "qwen3-coder");
        assert_eq!(file.agent.active_model.model, "qwen3-coder");
    }

    #[test]
    fn missing_agent_section_falls_back_to_defaults() {
        let file: AgentSettingsFile =
            toml::from_str("language = \"English\"\nterminal_font_size = 18.0")
                .expect("files with no agent section should parse");
        assert_eq!(file.agent, AgentSettings::default());
    }

    #[test]
    fn empty_string_falls_back_to_defaults() {
        let file: AgentSettingsFile = toml::from_str("").expect("empty settings should parse");
        assert_eq!(file.agent, AgentSettings::default());
    }

    #[test]
    fn settings_path_uses_the_xdg_style_crossh_directory() {
        assert_eq!(
            settings_path_from_home(PathBuf::from("/Users/example")),
            PathBuf::from("/Users/example/.config/crossh/settings.toml")
        );
    }
}
