# Spec 驱动的开发循环（SDD）

Crossh 由 AI coding 高频推进，人工逐行审查和手工回归无法随变更规模线性扩展
（见 `docs/adr/0006-executable-testing-contracts.md` 的背景）。SDL 用 **spec
先行** 把"这次变更要做什么、不做什么、如何验收"固化为一份人批准、机可测的
契约，再进入已有的 TDD 循环。

- **spec 是工作单**：描述"这一次变更"的可测试行为，评审后一次性消费。
- **ADR 是永久规范**：spec 中出现的结构性决策提炼进 `docs/adr/`。
- **engineering notes 是操作记忆**：spec 实施中确认的调试根因写进
  `docs/engineering-notes/`。
- **testing.md 行为矩阵是总契约**：spec 的验收条目是可执行矩阵的增量来源。

## 变更循环

```
写 spec ──▶ spec 评审（AI 找漏洞 + 人批准）──▶ Red ──▶ Green ──▶ Refactor
   ▲                                                        │
   └───────── 评审不通过，回到写 spec ◀──────────────────────┘
                                                            ▼
                                 收尾：更新状态、提炼 ADR、补充 notes、归档
```

### 1. 写 spec

复制的模板 `docs/specs/template.md` 到 `docs/specs/YYYYMMDD-<slug>.md`，
只填行为与验收，不写实现方案。状态置 `draft`。

### 2. Spec 评审（门槛）

1. AI 按"评审清单"（见下）审 spec，指出缺口和可测试性问题。
2. 人审阅 spec 与 AI 的评审意见，批准后状态置 `approved`；未批准则修订后重审。

批准之前**不写任何实现代码**。这一步是"人只审规格、不审实现"的杠杆点。

### 3. Red（TDD 第一步）

按 `docs/testing.md` 将行为契约逐条固化为失败测试。测试名以 spec 编号为前缀
（如 `spec_20260817_sftp_batch__cancelled_upload_leaves_no_partial_state`），
使失败信息能追溯到规格。确认失败原因正确（行为未实现，而非编译/Fixture 错误）。

### 4. Green

实施满足契约的最小生产代码，运行聚焦测试至通过。

### 5. Refactor + 全量检查

保持测试绿色，清理偶然复杂度。然后执行

```sh
cargo fmt --check
scripts/check-architecture.sh
cargo clippy --all-targets -- -D warnings
cargo test --workspace
```

并运行 spec 中声明的平台 CI job。验收清单逐项打勾。

### 6. 收尾

- spec 状态置 `done`（被取代时置 `superseded`，头部注明取代者）。
- 结构性决策提炼为新 ADR（新边界、新属主、契约变更），并在
  `docs/architecture.md` 登记。
- 实施中确认的调试根因合并进 `docs/engineering-notes/`。
- 新增的可执行行为契约合并进 `docs/testing.md` 关键行为矩阵。

## Spec 生命周期

```
draft ──▶ proposed(可选) ──▶ approved ──▶ in-progress ——▶ done
                ▲                │                       │
                └── 评审驳回 ────┘                       └──▶ superseded
```

- `done`：契约全部实现且验证完成，留在 `docs/specs/` 作为变更档案。
- `superseded`：被更新的 spec 取代，头部必须写取代者路径。
- 不删除已 `done` 的 spec：它是"当时意图"的历史记录，删除会丢失可追溯性。

## 何时必须写 spec

- 用户可观察的行为变更（功能、交互、UI）。
- 跨 crate 的契约变更（wire 格式、持久化格式、设置项、公开 API）。
- 影响非本机平台行为的变更（必须在 spec 中声明 CI 平台归属）。
- 较大缺陷修复（修复策略、回归边界值得记录）。

## 何时可以跳过 spec

- 纯文档、格式化、生成物。
- 可证明行为不变的机械重构（仍须通过既有检查）。
- 单行级小修复（仍须有回归测试，按 TDD）。

拿不准时写 spec 的成本低于返工：一页纸换一次"意图对齐"。

## Spec 评审清单

AI 和人在评审一个 spec 时逐项核查：

1. **可测性**：每条行为契约是否可被测试观察到？"提升性能""更自然"这类目标
   必须附带可测指标或明确边界行为，否则退回。
2. **错误路径**：失败、取消、乱序、重复触发、资源清理是否都有契约条目？
   不得只有 happy path。
3. **非目标**：是否声明了"这次不做"？防止实现范围蔓延进未评审的领域。
4. **平台影响**：每个受影响平台都有对应 CI job 归属；本地 macOS 无法验证的
   部分是否显式点名交给 Actions runner。
5. **纪律冲突**：是否违反 `AGENTS.md` 的边界规则（logic 不依赖 UI、图标纪律、
   文件规模、feature-owned 设置）或已有 ADR？违反必须显式说明并走 ADR 流程。
6. **契约冲突**：与 `docs/testing.md` 行为矩阵、已有 spec、crate README 的
   职责声明是否矛盾？
7. **验收可观察**：验收清单是否除检查命令之外，还包含用户可观察的最终效果？
8. **影响模块**：是否列出将触及的 crate/feature，帮助审阅人核对属主？

## 文档职责边界速查

| 文档 | 职责 | 生命周期 |
| --- | --- | --- |
| `docs/specs/` | 一次性变更契约（意图 + 验收） | 消费后归档为档案 |
| `docs/adr/` | 长期结构性决策 | 永久 |
| `docs/engineering-notes/` | 调试根因与操作经验 | 永久，可合并 |
| `docs/testing.md` | 可执行行为总契约与验证责任 | 随功能演进 |
| `AGENTS.md` | 不变式与纪律 | 永久 |
| crate README | 单 crate 职责/边界/验证命令 | 随 crate 演进 |