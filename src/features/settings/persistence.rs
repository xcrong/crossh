//! Settings persistence. Domain validation stays in the owning features.

use std::fs;
use std::io;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::features::updates::UpdateSettings;
use crate::features::workspace::settings::WorkspaceSettings;
use crossh_core::i18n::LanguagePreference;
use crossh_terminal::TerminalSettings;

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct SettingsSnapshot {
    pub(crate) language: LanguagePreference,
    pub(crate) terminal: TerminalSettings,
    pub(crate) updates: UpdateSettings,
    pub(crate) workspace: WorkspaceSettings,
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
        }
    }
}

pub(crate) fn load() -> SettingsSnapshot {
    let Some(path) = crossh_core::config::settings_read_path() else {
        return SettingsFile::default().into();
    };
    match fs::read_to_string(&path) {
        Ok(contents) => match toml::from_str::<SettingsFile>(&contents) {
            Ok(settings) => settings.into(),
            // 单字段类型错不应丢整份配置：按节回退，未知/错字段只影响本节。
            // 整份都解不出表结构时主文件已损坏：读 .bak 兜底（与 workspace_store 一致）。
            Err(error) => {
                if toml::from_str::<toml::Table>(&contents).is_err() {
                    log::warn!("settings corrupt, falling back to backup: {error}");
                    return load_backup(&path);
                }
                log::warn!("failed to parse settings, falling back per section: {error}");
                load_per_section(&contents)
            }
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => load_backup(&path),
        Err(error) => {
            log::warn!("failed to read settings: {error}");
            load_backup(&path)
        }
    }
}

/// 主文件缺失/损坏时的 .bak 兜底：覆盖 save_atomic 两次 rename 间崩溃的空窗。
fn load_backup(path: &Path) -> SettingsSnapshot {
    let backup = path.with_extension("toml.bak");
    match fs::read_to_string(&backup) {
        Ok(contents) => match toml::from_str::<SettingsFile>(&contents) {
            Ok(settings) => {
                log::warn!("using backup settings: {}", backup.display());
                settings.into()
            }
            Err(error) => {
                log::warn!("failed to parse backup settings, falling back per section: {error}");
                load_per_section(&contents)
            }
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            SettingsFile::default().into()
        }
        Err(error) => {
            log::warn!("failed to read backup settings: {error}");
            SettingsFile::default().into()
        }
    }
}

/// 整份解析失败时的按节回退：每节独立反序列化，失败只默认本节。
fn load_per_section(contents: &str) -> SettingsSnapshot {
    let table: toml::Table = match toml::from_str(contents) {
        Ok(table) => table,
        Err(_) => return SettingsFile::default().into(),
    };
    let language: LanguagePreference = table
        .get("language")
        .and_then(|value| value.clone().try_into().ok())
        .unwrap_or_default();
    let terminal: TerminalSettings = table.clone().try_into().unwrap_or_default();
    let updates = UpdateSettings {
        check_on_startup: table
            .get("updates_check_on_startup")
            .and_then(|value| value.as_bool())
            .unwrap_or_else(|| UpdateSettings::default().check_on_startup),
    };
    let workspace: WorkspaceSettings = table.try_into().unwrap_or_default();
    SettingsSnapshot {
        language,
        terminal: terminal.normalized(),
        updates,
        workspace: workspace.normalized(),
    }
}

pub(crate) fn save(snapshot: &SettingsSnapshot) -> std::io::Result<()> {
    let Some(path) = crossh_core::config::settings_path() else {
        return Ok(());
    };
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
        if let Err(error) = crossh_core::config::restrict_dir(parent) {
            log::warn!(
                "failed to restrict settings dir {}: {error}",
                parent.display()
            );
        }
    }
    let contents =
        toml::to_string_pretty(&SettingsFile::from(snapshot)).map_err(std::io::Error::other)?;
    save_atomic(&path, contents.as_bytes())
}

// ponytail: 与 workspace_store::save_atomic 同模式的最小复制（tmp+sync+bak+rename+目录同步），
// 不跨 crate 抽公共 helper；上限是两处副本，第三处再统一。
fn save_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let tmp = path.with_extension("toml.tmp");
    {
        use std::io::Write;
        let mut options = fs::OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&tmp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
    }
    if path.is_file() {
        let backup = path.with_extension("toml.bak");
        // Windows 的 rename 不覆盖目标文件；先删旧备份。
        let _ = fs::remove_file(&backup);
        fs::rename(path, &backup)?;
    }
    fs::rename(&tmp, path)?;
    if let Err(error) = crossh_core::config::restrict_file(path) {
        log::warn!(
            "failed to restrict settings file {}: {error}",
            path.display()
        );
    }
    #[cfg(unix)]
    if let Some(parent) = path.parent()
        && let Ok(dir) = fs::File::open(parent)
        && let Err(error) = dir.sync_all()
    {
        log::warn!("failed to sync settings dir {}: {error}", parent.display());
    }
    Ok(())
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
                editor_command: Some("zed".into()),
                focus_follows_mouse: false,
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
                .any(|line| line == "show_host_sidebar = false")
        );
        assert!(
            encoded
                .lines()
                .any(|line| line == "editor_command = \"zed\"")
        );
        assert!(
            encoded
                .lines()
                .any(|line| line == "updates_check_on_startup = true")
        );
        assert!(!encoded.contains("recent_local_dirs"));
        assert!(!encoded.contains("pinned_local_tabs"));

        let decoded: SettingsSnapshot = toml::from_str::<SettingsFile>(&encoded)
            .expect("settings should deserialize")
            .into();
        assert_eq!(decoded, snapshot);
    }

    #[test]
    fn legacy_settings_keys_are_ignored() {
        let snapshot: SettingsSnapshot = toml::from_str::<SettingsFile>(
            "language = \"English\"\nrecent_local_dirs = [\"/a\"]\nrecent_local_dirs_max = 10\npinned_local_tabs = []\n",
        )
        .expect("legacy settings should deserialize")
        .into();
        assert_eq!(snapshot.workspace, WorkspaceSettings::default());
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
    }

    #[test]
    fn feature_settings_are_normalized_independently() {
        let snapshot: SettingsSnapshot = toml::from_str::<SettingsFile>(
            "terminal_font_size = 200.0\nterminal_scrollback = 4294967295",
        )
        .expect("settings should deserialize")
        .into();
        assert_eq!(snapshot.terminal.font_size, crossh_terminal::MAX_FONT_SIZE);
        assert_eq!(
            snapshot.terminal.scrollback,
            crossh_terminal::MAX_SCROLLBACK
        );
    }

    #[test]
    fn corrupt_terminal_field_falls_back_per_section() {
        // terminal_scrollback 类型错：terminal 节回默认，其他节保留。
        let snapshot = super::load_per_section(
            "language = \"zh-CN\"\nterminal_scrollback = \"huge\"\nshow_host_sidebar = false\n",
        );
        assert_eq!(snapshot.language, LanguagePreference::SimplifiedChinese);
        assert_eq!(snapshot.terminal, TerminalSettings::default());
        assert!(!snapshot.workspace.show_host_sidebar);
    }

    #[test]
    fn spec_20260820_open_project_in_editor_default_editor_settings_stay_out_of_toml() {
        let snapshot = SettingsSnapshot {
            language: LanguagePreference::English,
            terminal: TerminalSettings::default(),
            updates: UpdateSettings::default(),
            workspace: WorkspaceSettings::default(),
        };
        let encoded = toml::to_string(&SettingsFile::from(&snapshot)).expect("should serialize");
        assert!(
            !encoded.contains("editor_command"),
            "未配置的命令不得写入 settings.toml"
        );
        assert!(
            !encoded.contains("editor_priority"),
            "检测候选列表是代码常量，不得写入 settings.toml"
        );
    }

    #[test]
    fn spec_20260820_open_project_in_editor_legacy_priority_field_is_ignored() {
        let snapshot: SettingsSnapshot = toml::from_str::<SettingsFile>(
            "editor_command = \"/usr/bin/zed\"\neditor_priority = [\"code\", \"zed\"]",
        )
        .expect("legacy settings with editor_priority should deserialize")
        .into();
        assert_eq!(
            snapshot.workspace,
            WorkspaceSettings {
                editor_command: Some("/usr/bin/zed".into()),
                ..WorkspaceSettings::default()
            },
            "旧配置里的检测候选列表被忽略，不影响归一化结果"
        );
    }
}
