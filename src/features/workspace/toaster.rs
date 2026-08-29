use std::time::Duration;

use gpui::IntoElement;

pub(crate) use crossh_ui_component::ToastTone;
pub(crate) const TOAST_DURATION: Duration = Duration::from_secs(2);

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ToastNotice {
    pub(crate) message: String,
    pub(crate) tone: ToastTone,
}

impl ToastNotice {
    pub(crate) fn new(message: impl Into<String>, tone: ToastTone) -> Self {
        Self {
            message: message.into(),
            tone,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ActiveToast {
    pub(crate) id: u64,
    pub(crate) notice: ToastNotice,
}

#[derive(Debug, Default)]
pub(crate) struct ToasterState {
    active: Option<ActiveToast>,
    next_id: u64,
}

impl ToasterState {
    pub(crate) fn show(&mut self, notice: ToastNotice) -> u64 {
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1);
        self.active = Some(ActiveToast { id, notice });
        id
    }

    pub(crate) fn active(&self) -> Option<&ActiveToast> {
        self.active.as_ref()
    }

    pub(crate) fn dismiss(&mut self, id: u64) -> bool {
        if self.active.as_ref().is_some_and(|active| active.id == id) {
            self.active = None;
            true
        } else {
            false
        }
    }
}

impl super::shell::AppShell {
    pub(crate) fn show_toast(&mut self, notice: ToastNotice, cx: &mut gpui::Context<Self>) {
        let toast_id = self.workspace.toaster.show(notice);
        self.workspace._toast_task = Some(cx.spawn(async move |weak, cx| {
            cx.background_executor().timer(TOAST_DURATION).await;
            let _ = weak.update(cx, |this, cx| {
                if this.workspace.toaster.dismiss(toast_id) {
                    cx.notify();
                }
            });
        }));
        cx.notify();
    }

    pub(crate) fn render_toaster(&self) -> Option<gpui::AnyElement> {
        let active = self.workspace.toaster.active()?;
        Some(
            crossh_ui_component::Toaster::new(
                crossh_ui_component::Toast::new(active.notice.message.clone())
                    .tone(active.notice.tone),
            )
            .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::{TOAST_DURATION, ToastNotice, ToastTone, ToasterState};

    #[test]
    fn spec_20260817_workspace_status_path_copy_toast_supports_all_notification_tones() {
        let mut state = ToasterState::default();
        for tone in [ToastTone::Info, ToastTone::Success, ToastTone::Warning] {
            state.show(ToastNotice::new("copied", tone));
            assert_eq!(state.active().unwrap().notice.tone, tone);
        }
    }

    #[test]
    fn spec_20260817_workspace_status_path_copy_toast_replaces_one_active_notice() {
        let mut state = ToasterState::default();
        state.show(ToastNotice::new("first", ToastTone::Info));
        state.show(ToastNotice::new("second", ToastTone::Info));
        assert_eq!(state.active().unwrap().notice.message, "second");
    }

    #[test]
    fn spec_20260817_workspace_status_path_copy_toast_ignores_stale_dismissals() {
        let mut state = ToasterState::default();
        let first = state.show(ToastNotice::new("first", ToastTone::Info));
        state.show(ToastNotice::new("second", ToastTone::Info));
        assert!(!state.dismiss(first));
        assert!(state.active().is_some());
    }

    #[test]
    fn spec_20260817_workspace_status_path_copy_toast_uses_two_second_default_duration() {
        assert_eq!(TOAST_DURATION, Duration::from_secs(2));
    }
}
