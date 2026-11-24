# Proposal（占位版）

> 本 proposal 目前是占位：只锁定"等什么、观察什么、何时补全"。迁移的技术细节
> （版本、启动 API、文件清单）必须等官方新版源码落地后才能写，见"补全触发器"。

## Why

crossh 的 `gpui` / `gpui_platform` / `gpui_macros` 钉在 Zed 整仓 git rev 上，
构建重、更新靠人工、供应链不透明。官方正在把 gpui 推向 crates.io 独立发布，
但当前 registry 版（`0.2.2`，2025-10-22）落后约一年且缺 `gpui_platform`，
现在切等于回退。策略：等官方新一轮发布，条件成熟再切。

## What Changes

- **门禁**：2026-12-15 之前不做任何切换实施。
- **观察**：12 月期间手动检查官方状态（见 tasks 1.2），结论记入本变更。
- **补全触发器**：官方发布新版且经源码确认可用后，把本占位补全为完整方案
  （版本号、启动 API 映射、文件清单、验证方式），然后走 `review.md` 评审，
  再实施。本次占位不预写这些细节。
- **明确不做**：任何后台自动盯梢（全部手动、透明）；等待期内的常规 rev
  跟随更新不属于本变更。

## Capabilities

### New Capabilities

无。

### Modified Capabilities

无。本变更为纯依赖源迁移，不改变任何用户可观测行为；已在 `.openspec.yaml`
设置 `skip_specs: true`。

## Impact

待定，发布后根据源码补全。目前仅提示涉及面（`gpui` 相关依赖与启动代码），
不作为实施依据。
