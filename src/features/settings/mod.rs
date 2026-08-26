//! User settings feature.

pub(crate) mod persistence;
pub(crate) mod window;

pub(crate) use persistence::{SettingsSnapshot, load, save};
pub(crate) use window::{
    SettingsSection, is_settings_window_open, open_settings_section, toggle_settings,
};

/// Load persisted feature settings and initialize the locale during boot.
pub(crate) fn init() {
    let snapshot = load();
    crate::shared::i18n::set_locale(snapshot.language);
}
