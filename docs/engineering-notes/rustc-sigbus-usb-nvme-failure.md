# rustc SIGBUS 与外置盘 I/O 故障

## 症状

- `cargo build/test/clippy` 大规模编译时 rustc 崩溃：`error: rustc interrupted by SIGBUS`，且崩溃点在编译**第三方依赖**（如 gpui）而非工作区代码。
- 编译期间整机僵死：访达/终端/Dock 全部转圈无响应，只能强制断电；或系统自动重启。
- `cargo test` 无输出挂长时间后失败，错误行只有 `warning: build failed, waiting for other jobs to finish...`。

## 根因

仓库与 `target/` 位于外置盘（本例：NVMe 装在 Realtek RTL9210 USB 桥接盒，挂载点 `/Volumes/BookDrive`）。盘体/桥接盒出现物理级读失败：

```
kernel: [RTL9210 NVME] I/O error! Opcode 0x28 (READ) ... retry 1..5
kernel: dart-usb1: DART error: PTE invalid exception on write @AppleT8110DART.cpp  → 内核 panic
```

rustc 用 mmap 读取 `target/debug/deps/*.rmeta`，磁盘读失败直接表现为用户态 SIGBUS；USB 块设备的卡死 I/O 在 macOS 上不可中断，重试风暴期间全系统阻塞在磁盘调用上，最终可由 IOMMU（DART）异常升级为内核 panic。

## 判定规则

看到 SIGBUS 先分清两层：

1. **SIGBUS 且堆栈指向 rustc 读依赖元数据 / 同时段 kernel log 有目标盘的 I/O error retry** → 存储故障，不是代码问题。换内置盘验证即可自证。
2. SIGBUS 且 kernel log 干净、可稳定复现于同一文件 → 才考虑工具链/内存问题（`rustup update`、换 toolchain、跑硬件诊断）。

## 处置

- 立即备份数据（趁盘还能挂载）；SMART 经 USB 桥通常不透传（`diskutil info` 显示 Not Supported），不能作为健康依据。
- 排查顺序：换线/换口 → 换桥接盒或直连（RTL9210 有已知固件问题，查固件更新）→ 盘体接 PCIe/别的盒子测。
- 硬件解决前不要在该卷上跑重型构建；临时方案是把构建缓存指到内置盘（`~/.cargo/config.toml` 的 `[build] target-dir`），源码可留原卷。

## 验证方法

`log show --last 3h --predicate 'eventMessage CONTAINS "I/O" AND eventMessage CONTAINS "error"' | grep -iE "disk|usb|nvme"` 能看到对应盘的错误风暴；panic 报告在 `/Library/Logs/DiagnosticReports/panic-full-*.panic`。

## 关键词

`SIGBUS`, `rustc`, `RTL9210`, `NVMe`, `USB`, `I/O error`, `DART panic`, `卡死`, `强制重启`, `外置盘`, `target 目录`
