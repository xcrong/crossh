# 0012-Spec-driven development loop

## 状态

已接受

## 背景

Crossh 由 AI coding 高频推进，变更速度快于人工审阅能力。ADR 0006 已确立
"可执行测试契约"：行为变更默认 Red-Green-Refactor，测试承担行为说明。但测试
只能约束"实现是否符合测试"，「这次变更究竟要做什么、不做什么、如何验收」在
原有流程中没有受控的载体：

- 意图只存在于会话上下文或 PR 描述，下一个会话不再可靠获取；
- 结构性决策有 ADR，一次性变更决策没有档案；
- 平台影响、纪律冲突、验收标准散落在各处，评审时无法一次性核对；
- 范围蔓延没有"非目标"这道闸门，未评审的行为可能混入实现。

需要一种在 AGENTS.md 纪律、ADR、engineering notes、testing.md 行为矩阵之上
的"变更层文档"：spec。

## 决策

Crossh 的开发循环改为 **spec 先行（SDD）+ TDD 执行**：

1. 行为变更默认先写 spec 到 `docs/specs/YYYYMMDD-<slug>.md`（复制
   `docs/specs/template.md`），只描述背景、目标、非目标、行为契约、边界与
   错误、接口与状态变更、平台影响、涉及纪律、影响模块和验收清单。
2. Spec 状态机：`draft → proposed(可选) → approved → in-progress → done`
   或被 `superseded`。只有 `approved` 之后才允许进入实现。
3. Spec 评审门槛：AI 按评审清单找漏洞（可测性、错误路径、非目标、平台影响、
   纪律冲突、契约冲突、验收可观察性），人批准。评审不通过不得实现。
4. 行为契约条目即 TDD 的测试来源，测试名以 spec 编号为前缀，失败信息可
   追溯到规格。
5. 收尾义务：spec 置 `done`/`superseded`；结构性决策提炼为新 ADR 并在
   `docs/architecture.md` 登记；调试根因合并进 `docs/engineering-notes/`；
   新增可执行行为合并进 `docs/testing.md` 关键行为矩阵。`done` 的 spec 默认
   保留为变更档案；但当其所属子系统被整体移除、或内容已与代码漂移失去档案
   价值时，应随之清除——代码是唯一真相，过期文档是干扰（2026-08-26 修订）。
6. 豁免清单：纯文档、格式化、生成物、可证明行为不变的机械重构、单行级小
   修复（仍需回归测试）。拿不准时写 spec。

## 结果/代价

人的精力和处理能力集中到"写意图 + 审规格"两件高杠杆动作上：意图以 spec
形式跨会话持久，没有上下文就丢不了；实现前评审规格，错误在成本最低的阶段
被拦截；一旦批准，AI 可以独立完成 Red-Green-Refactor 和全量检查，人工不再
需要逐行审实现。spec 同时解决了解释成本：新会话读一页 spec 就能对齐预期。

代价是每个行为变更多一份文档与一次评审门槛；spec 本身可能写错或与实现
漂移，因此评审清单、收尾义务和"superseded 取代链"是流程的一部分而非可选项。
为避免文档体系膨胀，spec 只承载"这一次变更"，长期规范仍归 ADR 与纪律文档。

## 关联规则

- `AGENTS.md` 的 Test-Driven Development
- `AGENTS.md` 的 Engineering Rules 与 Size/Language/ADR Discipline
- `docs/testing.md`
- `docs/adr/0006-executable-testing-contracts.md`
- `docs/architecture.md`（决策记录索引）