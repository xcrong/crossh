//! 本机系统采样纯逻辑：CPU / Memory / Disk / Network
//!
//! 零 `gpui` 依赖，可被单测覆盖。采样通过 `sysinfo` 实现，
//! 计算层为纯函数（增量速率、回绕、占比、盘选择），契约见
//! `docs/specs/20260824-system-monitor-card.md`。

use std::collections::HashMap;
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

/// 单盘快照：用于多盘列表与每盘 I/O 速率展示。
#[derive(Clone, Debug, PartialEq)]
pub struct DiskSnapshot {
    /// 挂载点字符串（如 `/`、`D:\`、`/Volumes/Data`），作为 `mount_point` 的可序列化形式
    pub mount_point: String,
    pub total_space: u64,
    pub used_space: Option<u64>,
    pub usage_percent: Option<f32>,
    /// 读速率 bytes/s，不可用/回绕/首帧时为 `None`
    pub read_rate: Option<u64>,
    /// 写速率 bytes/s
    pub write_rate: Option<u64>,
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
    /// 已用占比 0..100
    pub memory_usage_percent: Option<f32>,
    /// 多盘明细（按挂载点排序）；为空时表示本帧未采集到可用磁盘
    pub disks: Vec<DiskSnapshot>,
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

/// 判断挂载点是否为面向用户的可见磁盘（过滤系统合成卷）
fn is_visible_mount(mount: &str) -> bool {
    if cfg!(target_os = "macos") {
        // macOS 上 APFS 会在 /System/Volumes/* 下暴露多个合成卷（Preboot/VM/Data 等），
        // 与根卷共享同一容器且会造成“物理盘数量虚增”；只保留用户可见的挂载。
        mount == "/" || mount.starts_with("/Volumes/")
    } else if cfg!(windows) {
        true
    } else {
        // Linux：白名单默认拦截，仅放行根与用户外挂物理盘。
        // 不额外放行 /home —— Btrfs 同盘子卷（/home、/var/log 等）会与 / 同设备同容量导致重复显示；
        // 独立物理分区的 /home 如需支持，后续应通过“设备去重”而非扩大白名单解决。
        mount == "/" || mount.starts_with("/mnt/") || mount.starts_with("/media/")
    }
}

/// 判断文件系统是否为面向用户的可见磁盘（过滤虚拟/合成文件系统）
/// 仅 Linux 生效，macOS/Windows 始终返回 true。
fn is_visible_filesystem(fs: &str) -> bool {
    if cfg!(target_os = "macos") || cfg!(windows) {
        return true;
    }
    let fs = fs.to_ascii_lowercase();
    if fs.starts_with("fuse") {
        return false;
    }
    if fs.starts_with("cgroup") {
        return false;
    }
    // 常见虚拟/合成文件系统黑名单
    !matches!(
        fs.as_str(),
        "tmpfs"
            | "devtmpfs"
            | "squashfs"
            | "overlay"
            | "efivarfs"
            | "proc"
            | "sysfs"
            | "devpts"
            | "selinuxfs"
            | "securityfs"
            | "debugfs"
            | "tracefs"
            | "fusectl"
            | "configfs"
            | "ramfs"
            | "nsfs"
            | "bpf"
            | "pstore"
            | "autofs"
            | "mqueue"
            | "hugetlbfs"
            | "binfmt_misc"
            | "rpc_pipefs"
            | "nfsd"
    )
}

/// 构造快照（含多盘明细）
pub fn build_snapshot_with_disks(
    cpu_usage: Option<f32>,
    load_avg: Option<LoadAvg>,
    mem_total: u64,
    mem_used: u64,
    disks: Vec<DiskSnapshot>,
    network_rates: (Option<u64>, Option<u64>),
) -> SystemSnapshot {
    let memory_usage_percent = compute_usage_percent(mem_used, mem_total);
    SystemSnapshot {
        cpu_usage,
        load_avg,
        memory_total: mem_total,
        memory_used: mem_used,
        memory_usage_percent,
        disks,
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
}

/// 对 `sysinfo` 的有状态采样器；持有 `System` 以计算 CPU 差值与网络/磁盘速率
pub struct SystemSampler {
    system: System,
    disks: Disks,
    networks: Networks,
    prev_rx: Option<u64>,
    prev_tx: Option<u64>,
    prev_disk_io: HashMap<String, (u64, u64)>,
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
            prev_disk_io: HashMap::new(),
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

        // 收集多盘 I/O 与容量数据（含文件系统类型用于二次过滤）
        let raw_disks: Vec<(String, String, u64, u64, u64, u64)> = self
            .disks
            .list()
            .iter()
            .map(|d| {
                let mount = d.mount_point().to_string_lossy().to_string();
                let fs = d.file_system().to_string_lossy().to_string();
                let usage = d.usage();
                (
                    mount,
                    fs,
                    d.total_space(),
                    d.available_space(),
                    usage.total_read_bytes,
                    usage.total_written_bytes,
                )
            })
            .collect();

        let elapsed = self
            .prev_instant
            .map(|prev| now.duration_since(prev).as_secs_f64());
        let mut disks: Vec<DiskSnapshot> = Vec::new();
        for (mount, fs, total, avail, cur_read, cur_write) in &raw_disks {
            if *total == 0 {
                continue;
            }
            if !is_visible_mount(mount) {
                continue;
            }
            if !is_visible_filesystem(fs) {
                continue;
            }
            let (read_rate, write_rate) = match (self.prev_disk_io.get(mount), elapsed) {
                (Some((pr, pw)), Some(el)) => {
                    compute_network_rates(*pr, *pw, *cur_read, *cur_write, el)
                }
                _ => (None, None),
            };
            let (used, pct) = if *total >= *avail {
                let u = *total - *avail;
                (Some(u), compute_usage_percent(u, *total))
            } else {
                (None, None)
            };
            disks.push(DiskSnapshot {
                mount_point: mount.clone(),
                total_space: *total,
                used_space: used,
                usage_percent: pct,
                read_rate,
                write_rate,
            });
        }
        disks.sort_by(|a, b| a.mount_point.cmp(&b.mount_point));

        let cur_rx: u64 = self.networks.values().map(|d| d.total_received()).sum();
        let cur_tx: u64 = self.networks.values().map(|d| d.total_transmitted()).sum();
        let rates = match (self.prev_rx, self.prev_tx, self.prev_instant) {
            (Some(prx), Some(ptx), Some(prev_time)) => {
                let elapsed = now.duration_since(prev_time).as_secs_f64();
                compute_network_rates(prx, ptx, cur_rx, cur_tx, elapsed)
            }
            _ => (None, None),
        };
        // 更新状态：磁盘与网络共享同一时间基准
        for (mount, _, _, _, cur_read, cur_write) in &raw_disks {
            self.prev_disk_io
                .insert(mount.clone(), (*cur_read, *cur_write));
        }
        self.prev_rx = Some(cur_rx);
        self.prev_tx = Some(cur_tx);
        self.prev_instant = Some(now);

        build_snapshot_with_disks(cpu_usage, load_avg, mem_total, mem_used, disks, rates)
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
        // zero elapsed also yields placeholder
        let (rx3, tx3) = compute_network_rates(1000, 1000, 2000, 2000, 0.0);
        assert_eq!(rx3, None);
        assert_eq!(tx3, None);
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
    fn spec_20260824_system_monitor_card__build_snapshot_memory_percent() {
        let snap = build_snapshot_with_disks(None, None, 100, 60, Vec::new(), (None, None));
        assert!((snap.memory_usage_percent.unwrap() - 60.0).abs() < 0.001);
        assert!(snap.disks.is_empty());
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
        let snap = build_snapshot_with_disks(Some(10.0), None, 100, 50, Vec::new(), (None, None));
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
        let snap = build_snapshot_with_disks(Some(5.0), None, 100, 10, Vec::new(), (None, None));
        // 未 visible 时即使用匹配代数也拒绝
        assert!(!state.apply_snapshot(snap, 0));
    }

    #[test]
    fn disk_io__build_snapshot_with_disks_multi() {
        let d1 = DiskSnapshot {
            mount_point: "/".to_string(),
            total_space: 100,
            used_space: Some(60),
            usage_percent: Some(60.0),
            read_rate: Some(1024),
            write_rate: Some(2048),
        };
        let d2 = DiskSnapshot {
            mount_point: "/Volumes/Data".to_string(),
            total_space: 200,
            used_space: Some(100),
            usage_percent: Some(50.0),
            read_rate: None,
            write_rate: None,
        };
        let snap = build_snapshot_with_disks(
            None,
            None,
            100,
            60,
            vec![d1.clone(), d2.clone()],
            (None, None),
        );
        assert_eq!(snap.disks.len(), 2);
        assert_eq!(snap.disks[0].mount_point, "/");
        assert_eq!(snap.disks[0].read_rate, Some(1024));
        assert_eq!(snap.disks[1].mount_point, "/Volumes/Data");
    }

    #[test]
    fn disk_io__build_snapshot_without_disks_empty() {
        let snap = build_snapshot_with_disks(None, None, 100, 60, Vec::new(), (None, None));
        assert!(snap.disks.is_empty());
    }

    #[test]
    fn linux__is_visible_mount_whitelist() {
        // 仅在 Linux 分支生效；macOS/Windows 分支该测试不适用，但当前 CI 为 Linux。
        if cfg!(target_os = "macos") || cfg!(windows) {
            return;
        }
        // 白名单放行
        assert!(is_visible_mount("/"));
        assert!(is_visible_mount("/mnt/data"));
        assert!(is_visible_mount("/mnt/nas/photos"));
        assert!(is_visible_mount("/media/usb"));
        assert!(is_visible_mount("/media/user/disk1"));
        // 严格拦截（即使历史黑名单未覆盖）
        assert!(!is_visible_mount("/home"));
        assert!(!is_visible_mount("/var/log"));
        assert!(!is_visible_mount("/var/cache"));
        assert!(!is_visible_mount("/var/tmp/flatpak-cache-abc"));
        assert!(!is_visible_mount("/run/user/1000"));
        assert!(!is_visible_mount("/run/media/user/usb"));
        assert!(!is_visible_mount("/tmp"));
        assert!(!is_visible_mount("/snap/chromium/123"));
        assert!(!is_visible_mount("/boot"));
        assert!(!is_visible_mount("/boot/efi"));
        assert!(!is_visible_mount("/sys/fs/cgroup"));
        assert!(!is_visible_mount("/proc/self"));
        assert!(!is_visible_mount("/dev/shm"));
        // 边界：前缀必须带斜杠
        assert!(!is_visible_mount("/mnt"));
        assert!(!is_visible_mount("/media"));
        assert!(!is_visible_mount("/mntdata"));
        assert!(!is_visible_mount("/mediadata"));
        // 空与根相似路径
        assert!(!is_visible_mount(""));
        assert!(!is_visible_mount("/var"));
    }

    #[test]
    fn linux__is_visible_filesystem_filters_virtual() {
        if cfg!(target_os = "macos") || cfg!(windows) {
            return;
        }
        // 真实物理文件系统应放行
        assert!(is_visible_filesystem("ext4"));
        assert!(is_visible_filesystem("btrfs"));
        assert!(is_visible_filesystem("xfs"));
        assert!(is_visible_filesystem("ntfs"));
        assert!(is_visible_filesystem("exfat"));
        assert!(is_visible_filesystem("vfat")); // /boot/efi 是 vfat 但已由挂载点过滤，/media 下的 vfat 仍可见
        assert!(is_visible_filesystem("apfs")); // 允许透过，macOS 分支本就不过滤
        // 虚拟/合成文件系统应拦截
        assert!(!is_visible_filesystem("tmpfs"));
        assert!(!is_visible_filesystem("devtmpfs"));
        assert!(!is_visible_filesystem("squashfs"));
        assert!(!is_visible_filesystem("overlay"));
        assert!(!is_visible_filesystem("efivarfs"));
        assert!(!is_visible_filesystem("cgroup"));
        assert!(!is_visible_filesystem("cgroup2"));
        assert!(!is_visible_filesystem("proc"));
        assert!(!is_visible_filesystem("sysfs"));
        // fuse 家族（大小写不敏感）
        assert!(!is_visible_filesystem("fuse.revokefs-fuse"));
        assert!(!is_visible_filesystem("fuse.portal"));
        assert!(!is_visible_filesystem("fuseblk"));
        assert!(!is_visible_filesystem("FUSE.REVOKES-FUSE"));
        // 其余常见虚拟 FS
        assert!(!is_visible_filesystem("ramfs"));
        assert!(!is_visible_filesystem("nsfs"));
        assert!(!is_visible_filesystem("bpf"));
        assert!(!is_visible_filesystem("autofs"));
    }

    #[test]
    fn linux__combined_mount_and_fs_filter_covers_manajaro_case() {
        // Manjaro 实测 7 条：btrfs 同容量 4 条（/、/var/log、/var/cache、/home）+ vfat /boot/efi + 2×fuse
        if cfg!(target_os = "macos") || cfg!(windows) {
            return;
        }
        let cases = vec![
            ("/", "btrfs", true),
            ("/var/log", "btrfs", false),
            ("/var/cache", "btrfs", false),
            ("/home", "btrfs", false),
            ("/boot/efi", "vfat", false),
            ("/run/user/1000/doc", "fuse.revokefs-fuse", false),
            ("/run/user/1000/doc2", "fuse.portal", false),
            // 合法外挂
            ("/mnt/data", "ext4", true),
            ("/media/usb", "exfat", true),
            ("/media/usb", "vfat", true),
            // 挂载点合法但文件系统虚拟 → 仍应过滤
            ("/mnt/data", "tmpfs", false),
            ("/media/usb", "fuseblk", false),
        ];
        for (mount, fs, expected) in cases {
            let visible = is_visible_mount(mount) && is_visible_filesystem(fs);
            assert_eq!(
                visible, expected,
                "mount={mount} fs={fs} expected visible={expected}"
            );
        }
    }
}
