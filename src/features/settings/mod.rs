//! User settings feature.

pub(crate) mod persistence;
pub(crate) mod window;

use gpui::BorrowAppContext;

use crate::shared::i18n;

pub(crate) use persistence::{SettingsSnapshot, load, save};
pub(crate) use window::{
    SettingsSection, is_settings_window_open, open_settings_section, toggle_settings,
};

/// Load persisted feature settings and initialize the locale global during boot.
pub(crate) fn init<C: BorrowAppContext>(cx: &mut C) {
    let snapshot = load();
    i18n::init(cx, snapshot.language);
}
