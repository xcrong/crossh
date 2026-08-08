//! Settings persistence. Domain validation stays in the owning features.

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::features::terminal::settings::TerminalSettings;
use crate::features::workspace::settings::WorkspaceSettings;
use crate::shared::i18n::LanguagePreference;

const SETTINGS_FILE_NAME: &str = "settings.toml";

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct SettingsSnapshot {
    pub(crate) language: LanguagePreference,
    pub(crate) terminal: TerminalSettings,
    pub(crate) workspace: WorkspaceSettings,
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct SettingsFile {
    #[serde(default)]
    language: LanguagePreference,
    #[serde(flatten)]
    terminal: TerminalSettings,
    #[serde(flatten)]
    workspace: WorkspaceSettings,
}

impl From<SettingsFile> for SettingsSnapshot {
    fn from(file: SettingsFile) -> Self {
        Self {
            language: file.language,
            terminal: file.terminal.normalized(),
            workspace: file.workspace.normalized(),
        }
    }
}

impl From<&SettingsSnapshot> for SettingsFile {
    fn from(snapshot: &SettingsSnapshot) -> Self {
        Self {
            language: snapshot.language,
            terminal: snapshot.terminal.clone().normalized(),
            workspace: snapshot.workspace.clone().normalized(),
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
    dirs::home_dir().map(|home| settings_path_from_home(&home))
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
            workspace: WorkspaceSettings {
                recent_dirs: vec![PathBuf::from("/a"), PathBuf::from("/b")],
                recent_dirs_max: 2,
            },
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

        let decoded: SettingsSnapshot = toml::from_str::<SettingsFile>(&encoded)
            .expect("settings should deserialize")
            .into();
        assert_eq!(decoded, snapshot);
    }

    #[test]
    fn legacy_language_only_settings_use_feature_defaults() {
        let snapshot: SettingsSnapshot = toml::from_str::<SettingsFile>("language = \"English\"")
            .expect("legacy settings should deserialize")
            .into();
        assert_eq!(snapshot.language, LanguagePreference::English);
        assert_eq!(snapshot.terminal, TerminalSettings::default());
        assert_eq!(snapshot.workspace, WorkspaceSettings::default());
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
            crate::features::terminal::settings::MAX_FONT_SIZE
        );
        assert_eq!(
            snapshot.terminal.scrollback,
            crate::features::terminal::settings::MAX_SCROLLBACK
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
