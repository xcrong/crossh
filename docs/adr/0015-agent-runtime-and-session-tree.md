# 0015: Agent Runtime 与 Session 树边界

- 状态：`approved`
- 日期：2026-08-21
- 前置：0002-logic-ui-layering, 0003-agent-logic-and-view-split, 0012-spec-driven

## 背景

`crossh-agent` 原为线性 `Vec<Message>` + 单文件 `agent_cli.rs:1713行` 单体，无法支持 pi-agent 的分支/回溯、多模式复用与可嵌入 SDK。`crossh-ai-sdk` 已是 provider-neutral 适配层（单一事实来源），需在上层补齐 Runtime 隔离与树形 Session。

## 决策

1. **Session 树**：引入 `SessionEntry{id, parentId, timestamp, type}` 与 `CURRENT_SESSION_VERSION=3`，`AgentSession` 通过 `tree_entries_from_messages` 迁移旧线性文件；`SessionManager` trait 提供 `FsSessionManager`（文件）与 `InMemorySessionManager`（SDK/测试）双实现，`fork(entryId)` 仅共享祖先。

2. **Runtime 隔离**：新增 `AgentSessionServices{cwd, manager}` 与 `AgentSessionRuntime{session, services, session_path}`，`switch/new/fork` 统一走 `teardown→create→apply`，失败不污染当前 runtime。

3. **EventBus/队列**：`EventBus` 转发 `AgentSessionEvent{queue_update, compaction_start/end, entry_appended, ...}`；`MessageQueue{steering, followUp}` 实现 `Enter=steering / Alt+Enter=followUp / Esc恢复` 语义（对齐 pi 的 `steeringMode/followUpMode`）。

4. **Compaction**：`threshold(75%)/overflow(100%)` 双阶段，`summarize_for_compaction` 生成 `CompactionResult{summary, tokens_before, first_kept_entry_id}`，历史不丢。

5. **Crate 边界**：`crossh-agent` 仍禁 `gpui/ratatui`，仅依赖 `crossh-ai-sdk`；TUI 仅在 `src/agent_cli` 与 `src/bin/crossh-agent` 中。

## 后果

- `src/agent_cli.rs` 将拆为 `app/runtime/commands`（每文件 <800 行），`crossh-agent` 可被 `print/json/rpc/SDK` 复用。
- 旧 `kind: session/message` 文件仍可读，新写入为 `type: session/message + id/parentId` 树形。
- 下一步：接 `print(-p) / --mode json` 与 `thinking_level_change/model_change` 条目落盘。

## 修订（2026-08-23：移除未接线的抽象层）

落地验证发现：生产路径（`src/agent_cli*`）始终直调 `session.rs` 自由函数（create/load/save/list/latest_session），从未迁移到 `AgentSessionRuntime`/`SessionManager` 抽象；`EventBus` 只有 emit 半边接线，无任何生产订阅者。经裁定（2026-08-23 简化审计 + 用户授权）："当前直调形态即正式契约"。

- 删除 `runtime.rs`（AgentSessionRuntime/AgentSessionServices）、`manager.rs`（SessionManager trait、FsSessionManager、InMemorySessionManager 及 fork 双实现）。
- 删除 `EventBus` 与 `AgentSessionEvent` 全部变体及生产侧落空 emit 点；保留并收敛 `MessageQueue{steering, follow_up}`（steering/followUp/Esc 恢复语义不变，为唯一在用接口）。
- 会话树契约保留：`SessionEntry{id, parentId}` 与 `tree_entries_from_messages` 不变；版本常量收敛为 `session.rs` 私有 `SESSION_VERSION` 单源。

本修订将决策 1/2/3 中涉及 Runtime 隔离与 EventBus 的部分标记为**已被取代**；决策 1 的会话树与决策 4/5 继续有效。

## 验证

- `spec_20260821_agent_runtime_*` 9 项 Red→Green 已通过，`cargo test -p crossh-agent 52 passed`，`check-architecture.sh passed`。
