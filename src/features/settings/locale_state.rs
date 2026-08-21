//! settings feature 拥有 locale 全局态并注入；i18n 纯逻辑位于 `crate::shared::i18n`。

use gpui::{BorrowAppContext, Global};

use crate::shared::i18n::{LanguagePreference, Locale, set_locale};

/// GPUI 应用态：当前语言偏好与解析后的 locale。
#[derive(Clone, Debug, PartialEq)]
pub struct I18nState {
    pub preference: LanguagePreference,
    pub locale: Locale,
}

impl Global for I18nState {}

/// 启动时初始化 locale 全局态。
pub(crate) fn init<C: BorrowAppContext>(cx: &mut C, preference: LanguagePreference) {
    set_locale(preference);
    let locale = preference.resolve();
    cx.set_global(I18nState { preference, locale });
}

/// 运行时切换语言。
pub(crate) fn set_language<C: BorrowAppContext>(cx: &mut C, preference: LanguagePreference) {
    set_locale(preference);
    let locale = preference.resolve();
    cx.update_global::<I18nState, _>(|state, _| {
        *state = I18nState { preference, locale };
    });
}
