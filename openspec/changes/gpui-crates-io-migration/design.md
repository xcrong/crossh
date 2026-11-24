# Design（占位版）

> 迁移方案设计尚未开始。本文件只记录占位期的决策；技术决策在官方新版源码
> 落地、proposal 补全时一并写入。

## Context

见 proposal.md - Why。本变更当前处于等待期，不做任何实施。

## Goals / Non-Goals

- Goals：立住门禁与观察动作；定义"何时把占位补全"的触发器。
- Non-Goals：迁移实施本身（换源、启动代码、回归）——全部后置，不在本版设计。

## Decisions

- **D1：等待而非现在切。** registry 现版落后约一年且无 `gpui_platform`，
  现在切的成本覆盖不了收益。
- **D2：观察全手动。** 不设后台盯梢；12 月由人跑检查并记录结论。
- **D3：补全触发器 = 官方新版 + 源码已可对照。** 触发后先补全 proposal
  （版本与对应 commit、启动 API 映射、文件清单），再补 design/tasks，
  然后 `review.md`，最后实施。顺序不可跳。

## Risks / Trade-offs

- [Risk] 官方跳票 → Mitigation：门禁日期由用户延长，继续等。
- [Risk] 占位期间 pin 继续前移 → Mitigation：补全时以当时 pin 为基线重算落差。

## Migration Plan

后置。触发补全时根据新版源码编写（含试切、回滚策略）。

## Open Questions

以下全部等新版源码落地后回答，回答时同步补全本文件与 tasks：

- Q1：新版本号、发布时间、对应 commit 是什么？
- Q2：`gpui-platform`（或等价物）是否存在？启动 API 如何等价平移？
- Q3：`[patch.crates-io]` 去留？feature 对齐清单？
