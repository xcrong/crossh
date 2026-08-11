# 0004-feature-owned-settings

## 状态

已接受

## 背景

终端、更新、workspace 和 agent 设置分别影响不同 feature 的行为。把所有字段集中到一个全局 owner 会让持久化结构反过来决定功能边界，也会让独立 feature 难以演进。

## 决策

设置的类型、默认值、规范化和行为 setter 归属各自 feature；`src/features/settings/persistence.rs` 只负责读取文件、组合 `SettingsSnapshot` 并保存快照，不拥有 feature 语义。主窗口 `AppShell` 仍是运行时设置的唯一真源。

## 结果/代价

feature 可以独立验证自己的设置约束，持久化层保持薄；代价是组合快照需要显式维护字段映射，新增设置必须同时更新所属 feature 和 persistence adapter。

## 关联规则

- `AGENTS.md` 的 Each feature ships its own settings
- `docs/architecture.md` 的 Boundary Rules 第 4 条
- `src/features/settings/persistence.rs`
