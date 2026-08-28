use std::time::Duration;

pub(crate) const TOAST_DURATION: Duration = Duration::from_secs(2);

/// Toast 语气。四语气为 toaster 契约；`Info`（`Default` 构造）
/// 与 `Warning` 目前为预留语气，生产仅构造 `Success`/`Error`，测试覆盖全语气。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum ToastTone {
    #[default]
    Info,
    Success,
    /// 契约预留语气；构造点只在 cfg(test) 测试中，故需豁免。
    #[allow(dead_code)]
    Warning,
    Error,
}

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
        if self.active.as_ref().is_some_and(|toast| toast.id == id) {
            self.active = None;
            true
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::{TOAST_DURATION, ToastNotice, ToastTone, ToasterState};

    #[test]
    fn spec_20260817_workspace_status_path_copy_toast_supports_all_notification_tones() {
        let mut toaster = ToasterState::default();

        for tone in [
            ToastTone::Info,
            ToastTone::Success,
            ToastTone::Warning,
            ToastTone::Error,
        ] {
            toaster.show(ToastNotice::new("message", tone));
            assert_eq!(toaster.active().map(|toast| toast.notice.tone), Some(tone));
        }
    }

    #[test]
    fn spec_20260817_workspace_status_path_copy_toast_replaces_one_active_notice() {
        let mut toaster = ToasterState::default();

        let first = toaster.show(ToastNotice::new("first", ToastTone::Info));
        let second = toaster.show(ToastNotice::new("second", ToastTone::Success));

        assert_ne!(first, second);
        assert_eq!(
            toaster.active().map(|toast| toast.notice.message.as_str()),
            Some("second")
        );
    }

    #[test]
    fn spec_20260817_workspace_status_path_copy_toast_ignores_stale_dismissals() {
        let mut toaster = ToasterState::default();

        let first = toaster.show(ToastNotice::new("first", ToastTone::Info));
        let second = toaster.show(ToastNotice::new("second", ToastTone::Success));

        assert!(!toaster.dismiss(first));
        assert_eq!(toaster.active().map(|toast| toast.id), Some(second));
        assert!(toaster.dismiss(second));
        assert!(toaster.active().is_none());
    }

    #[test]
    fn spec_20260817_workspace_status_path_copy_toast_uses_two_second_default_duration() {
        assert_eq!(TOAST_DURATION, Duration::from_secs(2));
    }
}
