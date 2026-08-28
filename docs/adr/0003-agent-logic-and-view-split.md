# 0003-agent-logic-and-view-split

> Note: crossh-agent 已移除，本 ADR 的 GPUI/CLI 双形态约束仅保留设计思路，当前仅 git/note 保留独立二进制

## 状态

已接受

## 背景

Agent 需要同时服务于 GPUI 应用和独立的终端 CLI。工具执行、会话持久化、上下文发现和 provider 适配不应依赖 ratatui 或窗口事件，否则无法在不同呈现层复用。

## 决策

将 agent 消息、策略、工具执行、provider wire adapter 和会话放在无 UI 的 `crossh-agent`；`src/agent_cli.rs` 只负责参数、交互循环和 CLI 呈现。两层通过 crate-root 的公共数据类型、事件和函数通信。

## 结果/代价

逻辑可以独立测试，CLI 呈现可以调整而不改变 agent 协议；代价是 CLI 需要把流式事件、确认和取消显式桥接到自己的事件循环中。

## 关联规则

- `AGENTS.md` 的 Logic must not depend on UI
- `AGENTS.md` 的 Split vertical features, then split logic and view inside each
- `docs/architecture.md` 的 Crate Ownership
