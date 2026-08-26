---
name: find-simplifications
description: Use when the user asks to find simplification candidates in the crossh repo, run a simplification audit, or asks whether something can be deleted/folded/replaced. Turn broad "find things to simplify" requests into an evidence-backed audit pass delivered in-session; persist a report under docs/audit/ only when unresolved decisions need recording. Covers dead or test-only code, duplicated representations, speculative generality, unused seams, defensive machinery without an owner, and hand-rolled code where a maintained crate or stdlib already exists. Do not use for correctness reviews or spec work (those are separate flows).
---

# 简化扫描（Find Simplifications）

把宽泛的"找简化"请求变成一轮有证据的审计：结论当场交付，仅当存在需要产品输入或跨会话跟进的未决项时才落盘到 `docs/audit/`，消化后随文档清理删除——代码是唯一真相，过期文档是干扰。这是指导不是清单：跟随代码、保持判断力，**少量经得起推敲的候选胜过一堆薄弱的猜测**。

## 触发时机

纯手动触发。典型时机：没有进行中的开发任务时、关闭一个 spec 之后、用户明确要求"跑一轮简化扫描"。每次触发只做一轮，不要常驻后台。饱和退出：单轮只发现 P3/信息级候选记一次饱和；连续两轮饱和即进入冷却，直到相关区域出现实质性提交后再扫。

## 第一步：先建"何为故意"的意图基线

改任何东西之前先读，防止简化掉架构意图：

- `AGENTS.md` 全文（工程规则、响应式要求、大小约束）。
- `docs/architecture.md` 与全部 `docs/adr/`（入链按编号递增读）。
- `docs/engineering-notes/README.md`，只读与当前症状匹配的主题。

在 crossh 中被视为"有意为之、默认不动"的受保护表面：

- ADR 裁决过的边界：logic/UI 分层（0002/0003）、feature-owned settings（0004）、workspace 的 trait 组合（0007）、bin 拆分与依赖方向（0008/0009/0010）、终端/SSH/更新的归属（0005/0011/0013）。
- `scripts/check-architecture.sh` 白名单（`terminal_element.rs` 超长文件是有意保留的）以及脚本本身声明的规则。
- 固定 Zed/GPUI revision 依赖、固定 Lucide 1.27.0 图标资产（替换必须走官方源）。
- engineering-notes 里"hard-won 的防御模式"（如 SSH 生命周期、路径逃逸防护的 `select!` 结构、known_hosts 决策链路）。删除或折叠它们需要比记录更强的证据。

## 什么算强候选

强候选 = 删除/折叠/降级一个真实存在的东西，且证据表明当前设计成本 > 收益：

- 公开方法、事件、配置键、订阅、helper、crate 依赖、测试工件没有任何生产消费者。
- 只有测试或文档在消费，且被钉住的行为不承重。
- 两份表示镜像同一事实（尤其跨 crate：`src/` 与 `crates/*/src/` 之间、settings 与运行时状态之间）。
- 一个 seam 的方法没有消费者（trait 的全部实现都不用某个方法）。
- 投机性泛化：多会话/会话加载、后台任务名册、实时 registry 失效、mid-turn 转向等没有产品主人的设计。
- 手写代码复刻了 crates.io 上维护良好的包或 stdlib 已提供的能力，且替换后净删除（实现 + 专属测试 + 文档，减去残留胶水）。
- 简化的新行为略微不同，但依然合理且更易解释。

薄候选（不足以单独立项，但可收集进报告）：删一个 typo、单次运行工具的输出、"这里看起来复杂"却没有调用点证据的抱怨。

## 广泛扫描：锚点 + 并行分片

1. **先跑死代码锚点**（Rust 侧没有 knip 等价物）：
   - `cargo clippy --workspace --all-targets 2>&1 | rg "dead_code|unused"`——把每个命中当候选起点，但记住 `pub` 跨 crate 的表面 clippy 看不到，必须手动验证。
   - `rg "allow\(dead_code\)|allow\(unused"` src crates——审计先例已发现过 `agent_cli.rs` 的 `allow(unused_imports)`。
   - `cargo test --workspace --no-run 2>&1 | rg -i "warning"` 顺带确认无警告面。
2. **按领域分片并行**（每个 subagent 一个域，要求证据到 `文件:行号`，不要猜测）：
   - 根 crate 的 terminal/SSH/Git/workspace feature 与 `src/shared/`、`src/bin/`。
   - `crates/crossh-ssh`、`crossh-core`（协议、session、引擎）。
   - `crates/crossh-ui` 与 `src/features/`（视图、渲染、订阅、命令）。
   - settings、updater、infrastructure（日志、错误、toaster）、assets/dist/scripts。
3. 从最大的生产代码差异开始，不要被第一个好候选拦住。只扫"未使用的符号"会漏掉重复生命周期和防御性机制集中的文件。

## 证明或否决每个候选

对每个符号/行为，先分类消费者：

- **生产语料**：`src/`、`crates/*/src/`、bin 入口、loader/配置路径、运行期脚本。
- **非生产语料**：`tests/`、`#[cfg(test)]`、docs、sketch/示例、snapshot 资产。
- **模糊语料**：`examples/`、`dist/`、CI workflow——先看用法再归类。

先 `rg` 再读调用点。Rust 特有陷阱：`pub` API 可能被其它 crate 消费（跨 crate 引用）、`cfg(test)` 消费者不算生产、trait 方法可能只通过泛型调用。`cargo test` 覆盖率不能代替对公共接口的理解。
历史报告不具权威惯性：往轮的发现、否决与裁定只是线索，每轮必须以当前文件现状重新取证；与旧结论冲突时以代码为准（先例：2026-08-23 对 `list_changes` 的否决在 08-26 被逐行翻案）。

否决或降级：

- 存在生产调用者 → 那是 feature 决策，不是清理。
- 已被 ADR 或 engineering-note 明确辩护，且新证据没有打败原理由。
- 删除会引发无关连锁改动，却没有减少公共 API 或必需行为。
- 想法正确但太小 → 写进报告低优先级栏或加内联 TODO（带稳定标签，如 `TODO(dead-default)`），不单独立项。

## 信任与生命周期边界（GPUI 特有）

对每个防御性拷贝、顺延、校验器、回调捕获，回答"值从哪来、下一步归谁"。同进程 typed call 通常借用只读值；解析器、配置加载、队列、PTY/SSH 远端、持久化文件则拥有或校验自己的数据。

特别留意 GPUI 生命周期表面，它们是 crossh 简化富矿：

- `cx.subscribe` / `cx.observe` / `cx.on_action` 订阅了一个永远不触发的事件或已不存在的实体。
- `WeakEntity` 升级后没有使用方；background task 无人 join/取消/替换。
- 事件枚举的变体、`SharedString` 与 `String` 的无谓混用、settings 键没有读方。
- 多个机制镜像同一 liveness/settlement 事实（sentinels、readiness promise、取消路径、disposer、状态 flag）→ 提议合并为一个事务或生命周期控制器；但保留保护同步发布与回滚、回调隔离、首终结仲裁、进程/worker 所有权、dispose-to-quiescence 的独立机制。

## 手写 vs 依赖

引入依赖是合法的简化手段，不是例外。在 crossh 语境下，先问：这个协议解析器、重试/退避循环、glob 匹配、diff 引擎，是否已有维护良好的 crate 或 stdlib 覆盖？然后：

- 读手写实现，指明包覆盖的确切表面；包不覆盖的残余语义不算账在 swap 里。
- 诚实核查包的健康度（维护、采纳、传递足迹），stdlib 优先。
- 检查 Cargo.toml 的依赖口味：被 ADR 或既有决策钉住的依赖（Zed/GPUI revision、lucide）不在 swap 范围。
- 权衡净删除量：一个把同样复杂度换个位置的 wrapper 不是胜利。

## 产出：复用现有体系，不新建提案树

| 候选规模 | 处置 |
| --- | --- |
| 文档漂移、死依赖清理、一行修复 | 直接修（豁免清单内），随报告一起交付或用独立 PR |
| 需要删除/折叠行为的变更 | `docs/specs/` SDD 流程（先 spec 后 TDD），在报告中注明对应 spec 草案 |
| 结构性决策（归属、边界、删除受保护设计） | 写 ADR 提议，报告里标注"建议 ADR" |

扫描结论默认不落盘。确需持久化时写 `docs/audit/yyyy-mm-dd-simplification-audit.md`，沿用既往结构（历史样例见 git 提交记录）：

- 头部：触发原因、扫描方式（分片清单、锚点命令）、总体结论。
- 发现表：`编号 / 问题 / 严重度 / 证据（文件:行号）`，每条附消费者分类（生产 / 非生产 / 模糊）与最强反方论证。
- 处置 backlog 与未决项清单。

报告里的未决项清零后，该报告随下一轮文档清理一并删除，不作为常驻档案。

## 验证与收尾

- **硬门禁**：直接修批次必须在提交前跑完整 `cargo test --workspace` 且全绿。测试被环境阻塞（磁盘故障、超时、崩溃）时，改动保持在未提交状态等待重跑——禁止"结果待回填"式提交。
- 纯文档变更轮只需 `git diff --check`。
- 不顺手大改生产代码：候选按处置表走对应流程。
- 未决项当场移交：转 draft spec / ADR 草案 / 产品问题清单；若已落盘且未决项随后清零，报告本身随下轮清理删除。