//! 状态栏更新指示器（检查 → 下载 → 重启）的外壳侧行为。
//!
//! 状态与网络行为归 `features::updates`；本模块只把指示器点击翻译成动作，
//! 并在用户主动发起的检查 / 下载落地时补一次 toast。

use gpui::Context;

use crate::features::updates::{UpdateNoticeKind, UpdateStatus};
use crate::features::workspace::toaster::{ToastNotice, ToastTone};
use crossh_core::i18n;

use super::AppShell;

impl AppShell {
    /// 指示器点击：按当前状态检查 / 下载 / 重启安装。
    pub(crate) fn activate_update_indicator(&mut self, cx: &mut Context<Self>) {
        let status = self
            .updates
            .read_with(cx, |updates, _app| updates.status().clone());
        match status {
            // 进行中：按钮同时是 disabled，这里只做防御。
            UpdateStatus::Checking | UpdateStatus::Downloading { .. } => {}
            UpdateStatus::Available(_) => {
                self.update_notice.request(UpdateNoticeKind::Downloaded);
                self.updates.update(cx, |updates, cx| updates.download(cx));
            }
            UpdateStatus::Ready { .. } => self.install_downloaded_update(cx),
            UpdateStatus::Idle | UpdateStatus::UpToDate | UpdateStatus::Failed(_) => {
                self.update_notice.request(UpdateNoticeKind::UpToDate);
                self.updates.update(cx, |updates, cx| updates.check(cx));
            }
        }
    }

    /// 更新状态变化：刷新外壳（状态栏指示器），并结算用户主动操作的提示。
    pub(crate) fn sync_update_status(&mut self, cx: &mut Context<Self>) {
        let status = self
            .updates
            .read_with(cx, |updates, _app| updates.status().clone());
        let notice = self.update_notice.resolve(&status);
        let toast = match notice {
            Some(UpdateNoticeKind::UpToDate) => Some(ToastNotice::new(
                i18n::text("settings.updates_up_to_date"),
                ToastTone::Success,
            )),
            Some(UpdateNoticeKind::Downloaded) => Some(ToastNotice::new(
                i18n::text("toast.update_downloaded"),
                ToastTone::Success,
            )),
            None => None,
        };
        if let Some(toast) = toast {
            self.show_toast(toast, cx);
        }
        cx.notify();
    }

    /// 安装已下载的更新并退出，由 updater 子进程替换应用本体。
    fn install_downloaded_update(&mut self, cx: &mut Context<Self>) {
        match self.updates.update(cx, |updates, _cx| updates.install()) {
            Ok(()) => self.quit_for_update(cx),
            Err(error) => {
                self.updates
                    .update(cx, |updates, _cx| updates.set_failed(error));
                cx.notify();
            }
        }
    }
}
