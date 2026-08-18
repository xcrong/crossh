use gpui::{AnyElement, Context, IntoElement};

use super::AppShell;
use super::toaster::{TOAST_DURATION, ToastNotice, ToastTone};
use crossh_ui_component::{Toast, ToastTone as VisualToastTone, Toaster};

fn visual_tone(tone: ToastTone) -> VisualToastTone {
    match tone {
        ToastTone::Info => VisualToastTone::Info,
        ToastTone::Success => VisualToastTone::Success,
        ToastTone::Warning => VisualToastTone::Warning,
        ToastTone::Error => VisualToastTone::Error,
    }
}

impl AppShell {
    pub(crate) fn show_toast(&mut self, notice: ToastNotice, cx: &mut Context<Self>) {
        let toast_id = self.workspace.toaster.show(notice);
        // 覆盖句柄即取消前一个 toast 的计时器（toaster 单活动 toast，
        // 旧 dismiss 本就失效）；句柄保留到任务完成或 AppShell 销毁。
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

    pub(crate) fn render_toaster(&self) -> Option<AnyElement> {
        let active = self.workspace.toaster.active()?;
        Some(
            Toaster::new(
                Toast::new(active.notice.message.clone()).tone(visual_tone(active.notice.tone)),
            )
            .into_any_element(),
        )
    }
}
