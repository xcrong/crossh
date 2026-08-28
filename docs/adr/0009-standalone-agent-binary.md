> **Status: Removed — superseded**
> crossh-agent 子系统已在 2026-08-23 简化审计中移除 (commit 5daa8ce)，crates/crossh-agent 已删除，发布产物不再包含 crossh-agent。保留此 ADR 仅作历史归档。替代：直调形态，Workspace 侧按需直调，见 docs/adr/0015 修订。

# 0009-Standalone agent binary

## 状态

已接受

## 背景

`crossh agent` 曾经是完整 `crossh` 二进制的子命令，主进程内编译 TUI 与全部 GUI 代码。agent 的 TUI 层（`src/agent_cli.rs`）本来就不依赖 GPUI，且唯一的外部耦合是 main 入口处的设置注入；合体让主二进制携带 ratatui/TUI 代码与依赖，也让 agent 无法独立分发（远端/容器使用场景）。

## 决策

Cargo package 同时产出 `crossh` 与 `crossh-agent` 两个二进制。`crossh agent` 负责解析参数并委托同目录或 PATH 中的 `crossh-agent`（继承 stdio、透传退出码、不碰 termios）；`crossh-agent` 通过 `#[path]` 复用 `src/agent_cli.rs`，只依赖 `crossh-agent`、`crossh-ssh`、`crossh-theme` 等纯 crate，不初始化 GPUI。

设置读取收口到 `crossh_agent::load_agent_settings()`（`crates/crossh-agent/src/config.rs`）：GUI 与独立二进制共用同一个 `~/.config/crossh/settings.toml`，agent 段由纯 crate 解析，其他字段按未知字段忽略。同伴二进制查找抽到 `crossh_core::process::sibling_executable`，由 `crossh-git` 与 `crossh-agent` 两个启动器复用。

各平台发布脚本把 `crossh-agent` 与现有二进制放在同一安装目录或 bundle 内，macOS 增加嵌套 codesign（`io.crossh.app.agent`）。本地只负责 macOS arm64 的构建和验证；Linux、Windows 以及 macOS x86_64 由 GitHub Actions 构建和校验。

## 结果/代价

主二进制不再携带 TUI 代码（debug 体积 86M → 20M 对比，release 差距更大）；agent 可独立构建、独立测量、将来独立分发。代价是每个发布包携带第三个可执行文件，主程序与 `crossh-agent` 必须保持兼容版本；`crossh agent` 在开发机上需要先 `cargo build` 产生同伴二进制（或走 PATH）。

## 关联规则

- `docs/architecture.md` 边界规则 7：`crossh-git` 与 `crossh-agent` 是仅有的两个允许 `#[path]` 的独立入口
- `docs/plans/agent-binary-split.md`：迁移计划与验证清单（已随 crossh-agent 整体移除而清除，5daa8ce）

## Removal note

crossh-agent 子系统已在 2026-08-23 简化审计中移除（commit 5daa8ce）：`crates/crossh-agent` 已删除，发布产物不再包含 `crossh-agent`，`docs/plans/agent-binary-split.md` 已清除。保留此 ADR 仅作历史归档。替代：直调形态，Workspace 侧按需直调，见 `docs/adr/0015-agent-runtime-and-session-tree.md` 修订（2026-08-23）。