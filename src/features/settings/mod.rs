//! User settings feature.

pub(crate) mod locale_state;
pub(crate) mod persistence;
pub(crate) mod window;

use gpui::BorrowAppContext;

pub(crate) use persistence::{SettingsSnapshot, load, save};
pub(crate) use window::{
    SettingsSection, is_settings_window_open, open_settings_section, toggle_settings,
};

/// Load persisted feature settings and initialize the locale global during boot.
pub(crate) fn init<C: BorrowAppContext>(cx: &mut C) {
    let snapshot = load();
    locale_state::init(cx, snapshot.language);
}
