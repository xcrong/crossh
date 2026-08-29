//! Crossh self-update feature: state, settings, and user actions.

use std::path::PathBuf;

use gpui::{Context, Task};
use semver::Version;
use serde::{Deserialize, Serialize};

use crate::infrastructure::runtime::runtime;
use crossh_update::{
    DEFAULT_MANIFEST_URL, UpdateCandidate, UpdateError, UpdateTarget, download_artifact,
    fetch_manifest, spawn_updater, take_update_result,
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
    Downloading(UpdateCandidate),
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
            UpdateStatus::Checking | UpdateStatus::Downloading(_)
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
        self.status = UpdateStatus::Downloading(candidate.clone());
        cx.notify();
        let task = cx.spawn(async move |weak, cx| {
            let result = runtime()
                .spawn(async move { download_artifact(&artifact, &version, &target).await })
                .await;
            let result = match result {
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
