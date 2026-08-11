# 0005-standalone-updater

## 状态

已接受

## 背景

运行中的应用不能可靠地覆盖自己的可执行文件，更新包也必须在安装前经过来源、大小和完整性校验。更新失败时需要保留旧版本并允许应用恢复启动。

## 决策

更新由独立的 `crossh-updater` 二进制完成：应用通过 `crossh-update` 获取 HTTPS manifest，校验 manifest 和下载大小，下载后计算 SHA-256，再启动 updater 等待旧进程退出并替换目标。updater 只依赖 `crossh-update`，不通过 `#[path]` 引入应用源码。

## 结果/代价

应用与自更新过程解耦，校验和失败不会安装不完整包；代价是发布物必须同时包含 updater，并需要处理跨平台安装路径、进程等待和归档安全路径。

## 关联规则

- `AGENTS.md` 的 Logic must not depend on UI
- `docs/architecture.md` 的 updater boundary rule
- `crates/crossh-update/src/client.rs`
- `crates/crossh-update/src/installer.rs`
