//! Settings persistence. Domain validation stays in the owning features.

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::features::updates::UpdateSettings;
use crate::features::workspace::settings::WorkspaceSettings;
use crate::shared::i18n::LanguagePreference;
use crossh_agent::AgentSettings;
use crossh_terminal::settings::TerminalSettings;

const SETTINGS_FILE_NAME: &str = "settings.toml";

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct SettingsSnapshot {
    pub(crate) language: LanguagePreference,
    pub(crate) terminal: TerminalSettings,
    pub(crate) updates: UpdateSettings,
    pub(crate) workspace: WorkspaceSettings,
    pub(crate) agent: AgentSettings,
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct SettingsFile {
    #[serde(default)]
    language: LanguagePreference,
    #[serde(flatten)]
    terminal: TerminalSettings,
    #[serde(default = "default_updates_check_on_startup")]
    updates_check_on_startup: bool,
    #[serde(flatten)]
    workspace: WorkspaceSettings,
    #[serde(default)]
    agent: AgentSettings,
}

fn default_updates_check_on_startup() -> bool {
    UpdateSettings::default().check_on_startup
}

impl From<SettingsFile> for SettingsSnapshot {
    fn from(file: SettingsFile) -> Self {
        Self {
            language: file.language,
            terminal: file.terminal.normalized(),
            updates: UpdateSettings {
                check_on_startup: file.updates_check_on_startup,
            },
            workspace: file.workspace.normalized(),
            agent: file.agent,
        }
    }
}

impl From<&SettingsSnapshot> for SettingsFile {
    fn from(snapshot: &SettingsSnapshot) -> Self {
        Self {
            language: snapshot.language,
            terminal: snapshot.terminal.clone().normalized(),
            updates_check_on_startup: snapshot.updates.check_on_startup,
            workspace: snapshot.workspace.clone().normalized(),
            agent: snapshot.agent.clone(),
        }
    }
}

pub(crate) fn load() -> SettingsSnapshot {
    let Some(path) = settings_path() else {
        return SettingsFile::default().into();
    };
    match fs::read_to_string(path) {
        Ok(contents) => match toml::from_str::<SettingsFile>(&contents) {
            Ok(settings) => settings.into(),
            Err(error) => {
                log::warn!("failed to parse settings: {error}");
                SettingsFile::default().into()
            }
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            SettingsFile::default().into()
        }
        Err(error) => {
            log::warn!("failed to read settings: {error}");
            SettingsFile::default().into()
        }
    }
}

pub(crate) fn save(snapshot: &SettingsSnapshot) -> std::io::Result<()> {
    let Some(path) = settings_path() else {
        return Ok(());
    };
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let contents =
        toml::to_string_pretty(&SettingsFile::from(snapshot)).map_err(std::io::Error::other)?;
    fs::write(path, contents)
}

fn settings_path() -> Option<PathBuf> {
    #[cfg(test)]
    if let Some(path) = test_settings_path() {
        return Some(path);
    }
    dirs::home_dir().map(|home| settings_path_from_home(&home))
}

/// 测试隔离：把设置读写重定向到指定路径（`None` 恢复默认目录）。
/// `thread_local` 保证并行测试互不干扰；重定向只作用于当前测试线程。
#[cfg(test)]
pub(crate) fn set_test_settings_path(path: Option<PathBuf>) {
    TEST_SETTINGS_PATH.with(|cell| *cell.borrow_mut() = path);
}

#[cfg(test)]
thread_local! {
    static TEST_SETTINGS_PATH: std::cell::RefCell<Option<PathBuf>> = const {
        std::cell::RefCell::new(None)
    };
}

#[cfg(test)]
fn test_settings_path() -> Option<PathBuf> {
    TEST_SETTINGS_PATH.with(|cell| cell.borrow().clone())
}

fn settings_path_from_home(home: &Path) -> PathBuf {
    home.join(".config").join("crossh").join(SETTINGS_FILE_NAME)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn language_and_feature_settings_round_trip_as_flat_toml() {
        let snapshot = SettingsSnapshot {
            language: LanguagePreference::SimplifiedChinese,
            terminal: TerminalSettings {
                show_timestamps: false,
                font_size: 18.0,
                scrollback: 5000,
                ..TerminalSettings::default()
            },
            updates: UpdateSettings::default(),
            workspace: WorkspaceSettings {
                show_host_sidebar: false,
                show_quick_commands: false,
                recent_dirs: vec![PathBuf::from("/a"), PathBuf::from("/b")],
                recent_dirs_max: 2,
                pinned_local_tabs: vec![
                    crate::features::workspace::settings::PinnedLocalTab {
                        pin_id: 1,
                        project_dir: PathBuf::from("/a"),
                        cwd: PathBuf::from("/a"),
                        custom_name: Some("work".into()),
                    },
                    crate::features::workspace::settings::PinnedLocalTab {
                        pin_id: 2,
                        project_dir: PathBuf::from("/b"),
                        cwd: PathBuf::from("/b"),
                        custom_name: None,
                    },
                ],
            },
            agent: AgentSettings::default(),
        };
        let encoded =
            toml::to_string(&SettingsFile::from(&snapshot)).expect("settings should serialize");
        assert!(encoded.lines().any(|line| line == "language = \"zh-CN\""));
        assert!(
            encoded
                .lines()
                .any(|line| line == "terminal_font_size = 18.0")
        );
        assert!(
            encoded
                .lines()
                .any(|line| line == "recent_local_dirs_max = 2")
        );
        assert!(
            encoded
                .lines()
                .any(|line| line == "show_host_sidebar = false")
        );
        assert!(
            encoded
                .lines()
                .any(|line| line == "show_quick_commands = false")
        );
        assert!(
            encoded
                .lines()
                .any(|line| line == "updates_check_on_startup = true")
        );
        assert!(encoded.contains("pin_id = 1"));
        assert!(encoded.contains("custom_name = \"work\""));

        let decoded: SettingsSnapshot = toml::from_str::<SettingsFile>(&encoded)
            .expect("settings should deserialize")
            .into();
        assert_eq!(decoded, snapshot);
    }

    #[test]
    fn legacy_settings_without_pinned_tabs_default_to_an_empty_list() {
        let snapshot: SettingsSnapshot = toml::from_str::<SettingsFile>("language = \"English\"")
            .expect("legacy settings should deserialize")
            .into();
        assert!(snapshot.workspace.pinned_local_tabs.is_empty());
    }

    #[test]
    fn legacy_language_only_settings_use_feature_defaults() {
        let snapshot: SettingsSnapshot = toml::from_str::<SettingsFile>("language = \"English\"")
            .expect("legacy settings should deserialize")
            .into();
        assert_eq!(snapshot.language, LanguagePreference::English);
        assert_eq!(snapshot.terminal, TerminalSettings::default());
        assert_eq!(snapshot.updates, UpdateSettings::default());
        assert_eq!(snapshot.workspace, WorkspaceSettings::default());
        assert_eq!(snapshot.agent, AgentSettings::default());
    }

    #[test]
    fn feature_settings_are_normalized_independently() {
        let snapshot: SettingsSnapshot = toml::from_str::<SettingsFile>(
            "terminal_font_size = 200.0\nterminal_scrollback = 4294967295\nrecent_local_dirs_max = 0",
        )
        .expect("settings should deserialize")
        .into();
        assert_eq!(
            snapshot.terminal.font_size,
            crossh_terminal::settings::MAX_FONT_SIZE
        );
        assert_eq!(
            snapshot.terminal.scrollback,
            crossh_terminal::settings::MAX_SCROLLBACK
        );
        assert_eq!(
            snapshot.workspace.recent_dirs_max,
            crate::features::workspace::settings::MIN_RECENT_DIRS_MAX
        );
    }

    #[test]
    fn settings_path_uses_the_xdg_style_crossh_directory() {
        assert_eq!(
            settings_path_from_home(Path::new("/Users/example")),
            PathBuf::from("/Users/example/.config/crossh/settings.toml")
        );
    }
}
