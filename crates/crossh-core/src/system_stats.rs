//! 本机系统采样纯逻辑：CPU / Memory / Disk / Network
//!
//! 零 `gpui` 依赖，可被单测覆盖。采样通过 `sysinfo` 实现，
//! 计算层为纯函数（增量速率、回绕、占比、盘选择），契约见
//! `docs/specs/20260824-system-monitor-card.md`。

use std::path::{Path, PathBuf};
use std::time::Instant;

use sysinfo::{Disks, Networks, System};

/// 系统平均负载（`sysinfo::LoadAvg` 的自有副本，避免对外暴露 sysinfo 类型）
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LoadAvg {
    pub one: f64,
    pub five: f64,
    pub fifteen: f64,
}

/// 采样快照：落盘与 UI 共享的可渲染数据。
#[derive(Clone, Debug, PartialEq)]
pub struct SystemSnapshot {
    /// CPU 总占用率 0..100，首次采样无效时为 `None`（UI 显示 `--`）
    pub cpu_usage: Option<f32>,
    /// 平均负载，Windows 等平台不可用时为 `None`
    pub load_avg: Option<LoadAvg>,
    pub memory_total: u64,
    pub memory_used: u64,
    pub memory_available: u64,
    /// 已用占比 0..100
    pub memory_usage_percent: Option<f32>,
    /// 主磁盘总容量，不可用时为 `None`
    pub disk_total: Option<u64>,
    pub disk_available: Option<u64>,
    pub disk_used: Option<u64>,
    pub disk_usage_percent: Option<f32>,
    /// 网络速率 bytes/s，不可用/回绕时为 `None`
    pub network_rx_rate: Option<u64>,
    pub network_tx_rate: Option<u64>,
}

/// 计算内存/磁盘占比；总量为 0 时返回 `None`
pub fn compute_usage_percent(used: u64, total: u64) -> Option<f32> {
    if total == 0 {
        None
    } else {
        Some(used as f32 / total as f32 * 100.0)
    }
}

/// 计算网络速率；回绕或间隔非法时返回 `None`
pub fn compute_network_rates(
    prev_rx: u64,
    prev_tx: u64,
    cur_rx: u64,
    cur_tx: u64,
    elapsed_secs: f64,
) -> (Option<u64>, Option<u64>) {
    if !elapsed_secs.is_finite() || elapsed_secs <= 0.0 {
        return (None, None);
    }
    if cur_rx < prev_rx || cur_tx < prev_tx {
        return (None, None);
    }
    let rx = ((cur_rx - prev_rx) as f64 / elapsed_secs) as u64;
    let tx = ((cur_tx - prev_tx) as f64 / elapsed_secs) as u64;
    (Some(rx), Some(tx))
}

/// 系统盘挂载点（用于主盘选择）
pub fn system_mount_path() -> &'static Path {
    if cfg!(windows) {
        Path::new("C:\\")
    } else {
        Path::new("/")
    }
}

/// 在磁盘清单中选择系统盘；找不到时返回 `None`
pub fn select_system_disk(disks: &[(PathBuf, u64, u64)]) -> Option<(u64, u64)> {
    let mount = system_mount_path();
    for (path, total, available) in disks {
        if path == mount {
            return Some((*total, *available));
        }
    }
    None
}

/// 选择系统盘的通用实现（可注入挂载点，便于单测跨平台）
pub fn select_system_disk_with_mount(
    disks: &[(PathBuf, u64, u64)],
    mount: &Path,
) -> Option<(u64, u64)> {
    for (path, total, available) in disks {
        if path == mount {
            return Some((*total, *available));
        }
    }
    None
}

/// 构造快照：落库所有派生占比字段
#[allow(clippy::too_many_arguments)]
pub fn build_snapshot(
    cpu_usage: Option<f32>,
    load_avg: Option<LoadAvg>,
    mem_total: u64,
    mem_used: u64,
    mem_available: u64,
    disk_total: Option<u64>,
    disk_available: Option<u64>,
    network_rates: (Option<u64>, Option<u64>),
) -> SystemSnapshot {
    let memory_usage_percent = compute_usage_percent(mem_used, mem_total);
    let (disk_used, disk_usage_percent) = match (disk_total, disk_available) {
        (Some(total), Some(avail)) if total >= avail => {
            let used = total - avail;
            (Some(used), compute_usage_percent(used, total))
        }
        (Some(_total), None) => (None, None),
        (Some(_total), Some(_)) => (None, None),
        _ => (None, None),
    };
    SystemSnapshot {
        cpu_usage,
        load_avg,
        memory_total: mem_total,
        memory_used: mem_used,
        memory_available: mem_available,
        memory_usage_percent,
        disk_total,
        disk_available,
        disk_used,
        disk_usage_percent,
        network_rx_rate: network_rates.0,
        network_tx_rate: network_rates.1,
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct SystemMonitorState {
    pub visible: bool,
    pub generation: u64,
    pub snapshot: Option<SystemSnapshot>,
}

impl Default for SystemMonitorState {
    fn default() -> Self {
        Self::new()
    }
}

impl SystemMonitorState {
    pub fn new() -> Self {
        Self {
            visible: false,
            generation: 0,
            snapshot: None,
        }
    }

    /// 切换显隐；每次切换递增代数以失效旧任务
    pub fn toggle(&mut self) {
        self.visible = !self.visible;
        self.generation = self.generation.wrapping_add(1);
        if !self.visible {
            self.snapshot = None;
        }
    }

    /// 尝试应用采样；代数不匹配或已隐藏时拒绝
    pub fn apply_snapshot(&mut self, snapshot: SystemSnapshot, expected_generation: u64) -> bool {
        if self.generation != expected_generation || !self.visible {
            return false;
        }
        self.snapshot = Some(snapshot);
        true
    }

    pub fn should_sample(&self) -> bool {
        self.visible
    }
}

/// 对 `sysinfo` 的有状态采样器；持有 `System` 以计算 CPU 差值与网络速率
pub struct SystemSampler {
    system: System,
    disks: Disks,
    networks: Networks,
    prev_rx: Option<u64>,
    prev_tx: Option<u64>,
    prev_instant: Option<Instant>,
    first_cpu: bool,
}

impl Default for SystemSampler {
    fn default() -> Self {
        Self::new()
    }
}

impl SystemSampler {
    pub fn new() -> Self {
        let mut system = System::new();
        // 建立 CPU 基线；首次 usage 无效
        system.refresh_cpu_usage();
        Self {
            system,
            disks: Disks::new_with_refreshed_list(),
            networks: Networks::new_with_refreshed_list(),
            prev_rx: None,
            prev_tx: None,
            prev_instant: None,
            first_cpu: true,
        }
    }

    /// 执行一次采样；`now` 由调用方注入，便于测试时间推进
    pub fn sample(&mut self, now: Instant) -> SystemSnapshot {
        self.system.refresh_cpu_usage();
        self.system.refresh_memory();
        self.disks.refresh(false);
        self.networks.refresh(false);

        let cpu_usage = if self.first_cpu {
            self.first_cpu = false;
            None
        } else {
            Some(self.system.global_cpu_usage())
        };

        let load = System::load_average();
        let load_avg = Some(LoadAvg {
            one: load.one,
            five: load.five,
            fifteen: load.fifteen,
        });
        // Windows 下 load_average 可能固定为 0；调用方按需要可视为不可用，
        // 这里保持 Some，由 UI 决定是否展示占位。

        let mem_total = self.system.total_memory();
        let mem_used = self.system.used_memory();
        let mem_available = self.system.available_memory();

        let disk_pair = {
            let list: Vec<(PathBuf, u64, u64)> = self
                .disks
                .list()
                .iter()
                .map(|d| {
                    (
                        d.mount_point().to_path_buf(),
                        d.total_space(),
                        d.available_space(),
                    )
                })
                .collect();
            select_system_disk(&list)
        };
        let (disk_total, disk_available) = match disk_pair {
            Some((t, a)) => (Some(t), Some(a)),
            None => (None, None),
        };

        let cur_rx: u64 = self.networks.values().map(|d| d.total_received()).sum();
        let cur_tx: u64 = self.networks.values().map(|d| d.total_transmitted()).sum();
        let rates = match (self.prev_rx, self.prev_tx, self.prev_instant) {
            (Some(prx), Some(ptx), Some(prev_time)) => {
                let elapsed = now.duration_since(prev_time).as_secs_f64();
                compute_network_rates(prx, ptx, cur_rx, cur_tx, elapsed)
            }
            _ => (None, None),
        };
        self.prev_rx = Some(cur_rx);
        self.prev_tx = Some(cur_tx);
        self.prev_instant = Some(now);

        build_snapshot(
            cpu_usage,
            load_avg,
            mem_total,
            mem_used,
            mem_available,
            disk_total,
            disk_available,
            rates,
        )
    }
}

#[cfg(test)]
mod tests {
    #![allow(non_snake_case)]
    use super::*;
    use std::path::Path;

    #[test]
    fn spec_20260824_system_monitor_card__compute_network_rates_increments() {
        let (rx, tx) = compute_network_rates(1000, 2000, 3000, 5000, 2.0);
        assert_eq!(rx, Some(1000));
        assert_eq!(tx, Some(1500));
    }

    #[test]
    fn spec_20260824_system_monitor_card__network_wraparound_shows_placeholder() {
        let (rx, tx) = compute_network_rates(5000, 5000, 1000, 6000, 1.0);
        assert_eq!(rx, None);
        assert_eq!(tx, None);
        let (rx2, _) = compute_network_rates(1000, 1000, 2000, 500, 1.0);
        assert_eq!(rx2, None);
    }

    #[test]
    fn spec_20260824_system_monitor_card__network_zero_elapsed_shows_placeholder() {
        let (rx, tx) = compute_network_rates(1000, 1000, 2000, 2000, 0.0);
        assert_eq!(rx, None);
        assert_eq!(tx, None);
    }

    #[test]
    fn spec_20260824_system_monitor_card__memory_usage_percent() {
        assert_eq!(compute_usage_percent(50, 100), Some(50.0));
        assert_eq!(compute_usage_percent(0, 0), None);
    }

    #[test]
    fn spec_20260824_system_monitor_card__disk_selection_system_path() {
        let disks = vec![
            (PathBuf::from("/"), 100, 40),
            (PathBuf::from("/Volumes/Data"), 200, 100),
        ];
        assert_eq!(
            select_system_disk_with_mount(&disks, Path::new("/")),
            Some((100, 40))
        );
        assert_eq!(
            select_system_disk_with_mount(&disks, Path::new("/missing")),
            None
        );
    }

    #[test]
    fn spec_20260824_system_monitor_card__build_snapshot_disk_unavailable_placeholder() {
        let snap = build_snapshot(None, None, 16_000, 8_000, 8_000, None, None, (None, None));
        assert_eq!(snap.disk_total, None);
        assert_eq!(snap.disk_used, None);
        assert_eq!(snap.disk_usage_percent, None);
    }

    #[test]
    fn spec_20260824_system_monitor_card__build_snapshot_memory_percent() {
        let snap = build_snapshot(None, None, 100, 60, 40, Some(100), Some(40), (None, None));
        assert!((snap.memory_usage_percent.unwrap() - 60.0).abs() < 0.001);
        assert_eq!(snap.disk_used, Some(60));
        assert!((snap.disk_usage_percent.unwrap() - 60.0).abs() < 0.001);
    }

    #[test]
    fn spec_20260824_system_monitor_card__default_hidden_not_persisted() {
        let state = SystemMonitorState::new();
        assert!(!state.visible);
        assert_eq!(state.generation, 0);
        assert!(state.snapshot.is_none());
    }

    #[test]
    fn spec_20260824_system_monitor_card__toggle_changes_generation_and_visibility() {
        let mut state = SystemMonitorState::new();
        state.toggle();
        assert!(state.visible);
        assert_eq!(state.generation, 1);
        state.toggle();
        assert!(!state.visible);
        assert_eq!(state.generation, 2);
        assert!(state.snapshot.is_none());
    }

    #[test]
    fn spec_20260824_system_monitor_card__generation_expired_write_rejected() {
        let mut state = SystemMonitorState::new();
        state.toggle(); // visible, gen 1
        let expected = state.generation;
        let snap = build_snapshot(Some(10.0), None, 100, 50, 50, None, None, (None, None));
        assert!(state.apply_snapshot(snap.clone(), expected));
        assert!(state.snapshot.is_some());
        // 隐藏后代数递增，旧代数写入应被拒绝
        state.toggle(); // hidden, gen 2
        assert!(!state.apply_snapshot(snap.clone(), expected));
        // 即使再次推进周期，代数不变，快照保持 None（契约 5：不再递增）
        assert_eq!(state.generation, 2);
        assert!(state.snapshot.is_none());
        // 再次显示，新代数才能写入
        state.toggle(); // visible, gen 3
        assert!(state.apply_snapshot(snap, 3));
        assert!(state.snapshot.is_some());
    }

    #[test]
    fn spec_20260824_system_monitor_card__apply_rejected_when_hidden() {
        let mut state = SystemMonitorState::new();
        let snap = build_snapshot(Some(5.0), None, 100, 10, 90, None, None, (None, None));
        // 未 visible 时即使用匹配代数也拒绝
        assert!(!state.apply_snapshot(snap, 0));
    }

    #[test]
    fn spec_20260824_system_monitor_card__compute_network_rates_deterministic_injection() {
        // 契约 4：注入两次采样，速率随输入变化
        let (rx1, _) = compute_network_rates(0, 0, 2_000, 0, 2.0);
        let (rx2, _) = compute_network_rates(0, 0, 4_000, 0, 2.0);
        assert_eq!(rx1, Some(1_000));
        assert_eq!(rx2, Some(2_000));
        assert_ne!(rx1, rx2);
    }
}
