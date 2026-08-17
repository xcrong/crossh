# <功能/变更名称>

> 复制本文件到 `docs/specs/YYYYMMDD-<slug>.md`，填写后进入评审。
> 只描述行为与验收，不写实现方案。语言与项目文档保持一致。

## 元数据

- 状态：`draft`
- 创建：YYYY-MM-DD
- 相关 ADR：<无，或 docs/adr/00XX-*.md>
- 相关 issue / 路线图项：<无，或 issue 引用>
- CI 平台影响：`仅 macOS` | `macOS + <Linux|Windows>` | `全部` | `无（纯逻辑）`

## 背景

<为什么需要这个变更？用户痛点、现状缺陷、驱动事件。一两段即可。>

## 目标

<可验证的目标列表。每条最终都能被验收清单里的某个动作确认。>

1. ...

## 非目标

<明确排除的范围，防止实现蔓延。评审重点核对这一节。>

- ...

## 行为契约

<核心部分。每条是一条可测试行为，即将成为一条测试（命名前缀 spec_YYYYMMDD_）。>
<格式："当 <输入/前提>，应该 <行为>，观察到 <可验证结果>"。>

1. 当 ...
2. 当 ...

## 边界与错误

<失败路径、取消、乱序、重复触发、资源清理、输入分区，与 happy path 同等重要。>

- ...

## 接口与状态变更

<公开 API、设置项、持久化格式、wire 格式、进程/窗口边界。没有则写"无"。>

- ...

## 平台影响

<哪些平台行为变化？本地 macOS 无法验证的部分点名给哪个 GitHub Actions job？>

- ...

## 涉及纪律

<勾选本次变更触碰的 AGENTS.md 纪律或 ADR，并在对应行说明如何遵守。>

- [ ] Logic must not depend on UI（层级）
- [ ] Feature-owned settings
- [ ] 图标纪律（Lucide 1.27.0 官方 SVG，IconName 映射）
- [ ] 文件规模 < 2000 行（scripts/check-architecture.sh）
- [ ] 工程笔记 / ADR 同步义务
- [ ] 响应式 UI（最小窗口尺寸可用性）

## 影响模块

<将触及的 crate / feature 文件地图，帮助审阅人核对属主。>

- ...

## 验收清单

<完成后逐项打勾。最后一项全部勾选才可置为 done。>

- [ ] spec 评审通过（AI 评审 + 人批准）
- [ ] 行为契约全部固化为失败测试并确认失败原因正确（Red）
- [ ] 最小实现通过聚焦测试（Green）
- [ ] `cargo fmt --check`
- [ ] `scripts/check-architecture.sh`
- [ ] `cargo clippy --all-targets -- -D warnings`
- [ ] `cargo test --workspace`
- [ ] 声明的平台 CI job 通过（非本机平台：提交后由 Actions 验证，spec 状态
      保持 in-progress 直到通过）
- [ ] 结构性决策提炼进 ADR（如有）并登记 docs/architecture.md
- [ ] 调试根因合并进 docs/engineering-notes/（如有）
- [ ] 新增行为合并进 docs/testing.md 关键行为矩阵（如有）
- [ ] 用户可观察效果人工确认（针对 UI/交互变更）