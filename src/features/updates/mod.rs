//! Crossh self-update feature: state, settings, and user actions.

use std::path::PathBuf;
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};
use std::time::Duration;

use gpui::{Context, Task};
use semver::Version;
use serde::{Deserialize, Serialize};

use crate::app::bootstrap::runtime;
use crossh_update::{
    DEFAULT_MANIFEST_URL, UpdateCandidate, UpdateError, UpdateTarget,
    download_artifact_with_progress, fetch_manifest, spawn_updater, take_update_result,
};
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub(crate) struct UpdateSettings {
    #[serde(default = "default_check_on_startup")]
    pub(crate) check_on_startup: bool,
}

impl Default for UpdateSettings {
    fn default() -> Self {
        Self {
            check_on_startup: true,
        }
    }
}

fn default_check_on_startup() -> bool {
    true
}

#[derive(Clone, Debug)]
pub(crate) enum UpdateStatus {
    Idle,
    Checking,
    UpToDate,
    Available(UpdateCandidate),
    Downloading {
        candidate: UpdateCandidate,
        downloaded: u64,
        total: u64,
    },
    Ready {
        candidate: UpdateCandidate,
        package: PathBuf,
    },
    Failed(String),
}

pub(crate) struct UpdateController {
    settings: UpdateSettings,
    status: UpdateStatus,
    task: Option<Task<()>>,
    startup_check_pending: bool,
}

impl UpdateController {
    pub(crate) fn new(settings: UpdateSettings) -> Self {
        // 上次安装（updater 子进程）失败时，把结果带到本次启动的状态里，
        // 否则失败被 null 掉的 stdout/stderr 吞掉，用户看到的是「应用
        // 重启成了旧版本」而没有任何说明。有失败结果时跳过自动检查，
        // 优先展示失败原因，避免立刻被「检查中…」覆盖。
        let (status, startup_check_pending) = match take_update_result() {
            Some(result) if !result.success => (
                UpdateStatus::Failed(result.error.unwrap_or_else(|| "update failed".to_string())),
                false,
            ),
            _ => (UpdateStatus::Idle, settings.check_on_startup),
        };
        Self {
            settings,
            status,
            task: None,
            startup_check_pending,
        }
    }

    pub(crate) fn status(&self) -> &UpdateStatus {
        &self.status
    }

    pub(crate) fn set_settings(&mut self, settings: UpdateSettings) {
        if self.settings == settings {
            return;
        }
        if settings.check_on_startup && !self.settings.check_on_startup {
            self.startup_check_pending = true;
        }
        self.settings = settings;
    }

    pub(crate) fn take_startup_check(&mut self) -> bool {
        if self.startup_check_pending {
            self.startup_check_pending = false;
            true
        } else {
            false
        }
    }

    pub(crate) fn start_startup_check(&mut self, cx: &mut Context<Self>) {
        if self.take_startup_check() {
            self.check(cx);
        }
    }

    pub(crate) fn check(&mut self, cx: &mut Context<Self>) {
        if matches!(
            self.status,
            UpdateStatus::Checking | UpdateStatus::Downloading { .. }
        ) {
            return;
        }
        let Some(target) = UpdateTarget::current() else {
            self.status = UpdateStatus::Failed("current platform is not supported".into());
            cx.notify();
            return;
        };
        let current = Version::parse(env!("CARGO_PKG_VERSION"))
            .expect("CARGO_PKG_VERSION must be valid semver");
        let manifest_url = option_env!("CROSSH_UPDATE_MANIFEST_URL")
            .unwrap_or(DEFAULT_MANIFEST_URL)
            .to_owned();
        self.status = UpdateStatus::Checking;
        cx.notify();
        let task = cx.spawn(async move |weak, cx| {
            let result = runtime()
                .spawn(async move {
                    let manifest = fetch_manifest(&manifest_url).await?;
                    manifest
                        .candidate(&current, target)
                        .map_err(UpdateError::from)
                })
                .await;
            let result = match result {
                Ok(Ok(candidate)) => Ok(candidate),
                Ok(Err(error)) => Err(error.to_string()),
                Err(error) => Err(format!("update task failed: {error}")),
            };
            let _ = weak.update(cx, |this, cx| {
                this.task = None;
                this.status = match result {
                    Ok(Some(candidate)) => UpdateStatus::Available(candidate),
                    Ok(None) => UpdateStatus::UpToDate,
                    Err(error) => UpdateStatus::Failed(error),
                };
                cx.notify();
            });
        });
        self.task = Some(task);
    }

    pub(crate) fn download(&mut self, cx: &mut Context<Self>) {
        let UpdateStatus::Available(candidate) = &self.status else {
            return;
        };
        let candidate = candidate.clone();
        let artifact = candidate.artifact.clone();
        let version = candidate.version.to_string();
        let target = candidate.target.key().to_owned();
        let total = artifact.size;
        // tokio 下载任务只写原子计数，GPUI 任务轮询读并 notify；回调不碰 GPUI（AsyncApp 非 Send）。
        // ponytail: 120ms 轮询而非 watch 通道，进度粒度 120ms；觉得卡再换事件驱动。
        let downloaded = Arc::new(AtomicU64::new(0));
        let downloaded_for_task = downloaded.clone();
        self.status = UpdateStatus::Downloading {
            candidate: candidate.clone(),
            downloaded: 0,
            total,
        };
        cx.notify();
        let task = cx.spawn(async move |weak, cx| {
            let join = runtime().spawn(async move {
                download_artifact_with_progress(&artifact, &version, &target, |got, _| {
                    downloaded_for_task.store(got, Ordering::Relaxed);
                })
                .await
            });
            loop {
                if join.is_finished() {
                    break;
                }
                let got = downloaded.load(Ordering::Relaxed);
                let _ = weak.update(cx, |this, cx| {
                    if let UpdateStatus::Downloading {
                        downloaded: current,
                        ..
                    } = &mut this.status
                        && *current != got
                    {
                        *current = got;
                        cx.notify();
                    }
                });
                cx.background_executor()
                    .timer(Duration::from_millis(120))
                    .await;
            }
            let result = match join.await {
                Ok(Ok(package)) => Ok(package),
                Ok(Err(error)) => Err(error.to_string()),
                Err(error) => Err(format!("download task failed: {error}")),
            };
            let _ = weak.update(cx, |this, cx| {
                this.task = None;
                this.status = match result {
                    Ok(package) => UpdateStatus::Ready { candidate, package },
                    Err(error) => UpdateStatus::Failed(error),
                };
                cx.notify();
            });
        });
        self.task = Some(task);
    }

    pub(crate) fn install(&mut self) -> Result<(), String> {
        let UpdateStatus::Ready { candidate, package } = &self.status else {
            return Err("no downloaded update is ready".into());
        };
        spawn_updater(package, candidate.artifact.format).map_err(|error| error.to_string())?;
        log::info!(
            "starting updater for Crossh {} from {}",
            candidate.version,
            package.display()
        );
        Ok(())
    }

    pub(crate) fn set_failed(&mut self, error: String) {
        self.status = UpdateStatus::Failed(error);
    }
}

/// 用户主动发起的更新动作：异步结果落地后补一次提示。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum UpdateNoticeKind {
    /// 点击检查：落地为「已是最新」时提示。
    UpToDate,
    /// 点击下载：下载完成时提示可以重启安装。
    Downloaded,
}

/// 等待中的提示槽位。
///
/// 检查与下载都是异步的：点击只发起请求，结果稍后由 [`UpdateController`]
/// 落地。自动检查（启动时、菜单）不经过这里，避免每次启动都弹提示。
/// 同一时刻至多一个等待项（点击是串行的，后一次覆盖前一次）。
#[derive(Debug, Default)]
pub(crate) struct UpdateNotice {
    pending: Option<UpdateNoticeKind>,
}

impl UpdateNotice {
    /// 用户发起一次检查 / 下载，等结果落地。
    pub(crate) fn request(&mut self, kind: UpdateNoticeKind) {
        self.pending = Some(kind);
    }

    /// 用最新状态结算等待中的提示；返回应补哪条提示（无则 `None`）。
    pub(crate) fn resolve(&mut self, status: &UpdateStatus) -> Option<UpdateNoticeKind> {
        let kind = self.pending?;
        let landed = match (kind, status) {
            // 结果尚未落地（仍在检查 / 下载），继续等待。
            (UpdateNoticeKind::UpToDate, UpdateStatus::Checking)
            | (UpdateNoticeKind::Downloaded, UpdateStatus::Downloading { .. }) => return None,
            // 落地成用户等待的那个结果：提示一次。
            (UpdateNoticeKind::UpToDate, UpdateStatus::UpToDate)
            | (UpdateNoticeKind::Downloaded, UpdateStatus::Ready { .. }) => true,
            // 其它结果（发现新版本 / 失败）：指示器图标已表达状态，不再补 toast。
            _ => false,
        };
        self.pending = None;
        landed.then_some(kind)
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use semver::Version;

    use super::{UpdateCandidate, UpdateNotice, UpdateNoticeKind, UpdateStatus, UpdateTarget};
    use crossh_update::{ArtifactFormat, UpdateArtifact};

    fn candidate() -> UpdateCandidate {
        UpdateCandidate {
            version: Version::new(0, 2, 0),
            notes: String::new(),
            release_url: None,
            artifact: UpdateArtifact {
                url: "https://example.com/crossh.zip".into(),
                filename: "crossh.zip".into(),
                format: ArtifactFormat::Zip,
                sha256: "0".repeat(64),
                size: 1024,
            },
            target: UpdateTarget::MacosAarch64,
        }
    }

    #[test]
    fn notice_fires_once_for_interactive_up_to_date() {
        let mut notice = UpdateNotice::default();
        // 自动检查不经过 request，绝不弹提示。
        assert_eq!(notice.resolve(&UpdateStatus::UpToDate), None);

        notice.request(UpdateNoticeKind::UpToDate);
        // 点击后结果未落地：继续等待。
        assert_eq!(notice.resolve(&UpdateStatus::Checking), None);
        // 落地为已是最新：弹一次。
        assert_eq!(
            notice.resolve(&UpdateStatus::UpToDate),
            Some(UpdateNoticeKind::UpToDate)
        );
        // 同一次请求不重复弹。
        assert_eq!(notice.resolve(&UpdateStatus::UpToDate), None);
    }

    #[test]
    fn notice_fires_once_for_interactive_download() {
        let mut notice = UpdateNotice::default();
        notice.request(UpdateNoticeKind::Downloaded);
        // 下载进度变化不属于落地，继续等待。
        assert_eq!(
            notice.resolve(&UpdateStatus::Downloading {
                candidate: candidate(),
                downloaded: 512,
                total: 1024,
            }),
            None
        );
        // 下载完成：提示可以重启安装。
        assert_eq!(
            notice.resolve(&UpdateStatus::Ready {
                candidate: candidate(),
                package: PathBuf::from("/tmp/crossh.zip"),
            }),
            Some(UpdateNoticeKind::Downloaded)
        );
        assert_eq!(
            notice.resolve(&UpdateStatus::Ready {
                candidate: candidate(),
                package: PathBuf::from("/tmp/crossh.zip"),
            }),
            None
        );
    }

    #[test]
    fn notice_drops_when_interactive_action_finds_update_or_fails() {
        let mut notice = UpdateNotice::default();

        notice.request(UpdateNoticeKind::UpToDate);
        assert_eq!(notice.resolve(&UpdateStatus::Available(candidate())), None);
        assert_eq!(notice.resolve(&UpdateStatus::UpToDate), None);

        notice.request(UpdateNoticeKind::UpToDate);
        assert_eq!(notice.resolve(&UpdateStatus::Failed("boom".into())), None);
        assert_eq!(notice.resolve(&UpdateStatus::UpToDate), None);

        notice.request(UpdateNoticeKind::Downloaded);
        assert_eq!(notice.resolve(&UpdateStatus::Failed("boom".into())), None);
    }

    #[test]
    fn later_action_replaces_the_pending_notice() {
        let mut notice = UpdateNotice::default();
        notice.request(UpdateNoticeKind::UpToDate);
        // 用户改点下载：等待项被替换，检查落地不再补提示。
        notice.request(UpdateNoticeKind::Downloaded);
        assert_eq!(notice.resolve(&UpdateStatus::UpToDate), None);
    }
}
