# 修复 CI 全量测试门禁的静默排除问题

## 元数据

- 状态：`in-progress`（本机验证已完成，Linux/Windows CI 验证待 Actions）
- 创建：2026-08-17
- 相关 ADR：docs/adr/0006-executable-testing-contracts.md
- 相关 issue / 路线图项：无
- CI 平台影响：`全部`

## 背景

2026-08-17 项目审阅发现 `.github/workflows/ci.yml` 的全量测试门禁存在两处
静默排除，导致本应在每个 PR 上执行的测试实际从未运行：

1. macOS `check` job 的 Test 步骤执行 `cargo test --release`。本仓库根 crate
   是 `[package]`（非 virtual workspace）且未设置 default-members，该命令
   只选择根 crate，不会选择 workspace members。根 crate 的 186 个测试在 PR
   上运行，而 crossh-core（87）、crossh-agent（40）、crossh-update（15）、
   crossh-ssh（8）、crossh-ai-sdk（5）等约 200 个 workspace member 测试在
   macOS 检查上完全不执行。
2. terminal-compat job 使用 `cargo test --release --workspace --lib`，限定
   只在 lib target 中选测试。根 crate 没有 lib target（无 `src/lib.rs`），
   其 186 个测试全部挂在 bin target 上，因此 `--lib` 在 Linux/Windows 上
   静默跳过它们。

这违反 `docs/testing.md` CI 规则 1（PR 必须运行全量普通测试）和规则 3
（macOS 运行完整应用测试；Linux/Windows 运行完整逻辑测试）。

## 目标

1. 每个 PR 的 macOS 检查执行根 crate 与全部 workspace members 的普通测试。
2. 每个 PR 的 Linux/Windows terminal-compat job 不再静默跳过根 crate 的
   bin target 测试。
3. 门禁命令保持简单可审，不引入可能匹配零项仍成功的过滤器。

## 非目标

- 不引入新的测试过滤、名称匹配或排除列表。
- 不调整测试本身的组织结构（不为此新增 lib target、不移动测试）。
- 不改变定时任务、发布安装验证或其他 CI job 的职责。

## 行为契约

1. 当 macOS `check` job 执行测试步骤，应该选中根 crate 与全部 workspace
   members 的测试，观察到命令形式为 `cargo test --release --workspace`，
   且该 job 在 PR 上实际执行的测试数量覆盖根 crate 的 186 个测试与
   workspace members 的约 200 个测试（合计 390+）。
2. 当 terminal-compat job 在 Linux/Windows runner 上执行
   `cargo test --release --workspace`（或显式含 `--bins`），应该选中根
   crate bin target 上的测试，观察到根 crate 的 186 个测试不再被 `--lib`
   静默排除，且 workspace members 的 lib 测试仍被选中执行。
3. 当本地以等效命令解析 workspace 测试选择（如
   `cargo test --release --workspace -- --list`），应该覆盖全部成员 crate，
   观察到已注册测试数量 ≥ 390。
4. 当上述命令无法编译或 target 缺失，应该让 CI job 失败，观察到不出现在
   可匹配零项的过滤后面（符合 ADR 0006 第 6 条）。

## 边界与错误

- 不依赖 shell 通配符或动态拼写成员名；`--workspace` 由 Cargo 依据
  `Cargo.toml` 的 `[workspace] members` 稳定解析。
- 若个别 workspace member 在特定平台天然无法编译（如平台专属依赖），该
  情况必须在 issue 中显式声明并经评审，不能在门禁中加过滤器掩盖。
- `cargo test --release --workspace` 的执行时间可能长于现状，不作为回退
  到排除策略的理由；慢速测试的归属按 `docs/testing.md` CI 规则 4 处理。

## 接口与状态变更

- 无公开 API、设置项或持久化格式变更；仅 `.github/workflows/ci.yml` 的
  门禁命令变更。

## 平台影响

- macOS：`check` job 的 Test 步骤由 `cargo test --release` 改为
  `cargo test --release --workspace`。
- Linux/Windows：terminal-compat job 的 "Run workspace library tests"
  步骤由 `cargo test --release --workspace --lib` 改为
  `cargo test --release --workspace`（或显式含 `--bins`）；如需保留 lib
  限定语义，须同时显式列出根 crate 的 bin target，不允许静默跳过。
- 本机 macOS 只能验证命令等价性；Linux/Windows 的实际执行由
  `terminal-compat` job 的 runner 验证，spec 保持 `in-progress` 直至该
  job 通过。

## 涉及纪律

- [x] Logic must not depend on UI（层级）：不涉及代码，仅 CI 命令。
- [ ] Feature-owned settings
- [ ] 图标纪律（Lucide 1.27.0 官方 SVG，IconName 映射）
- [ ] 文件规模 < 2000 行（scripts/check-architecture.sh）
- [x] 工程笔记 / ADR 同步义务：修复合入后在收尾阶段提炼新 ADR 或在 ADR
  0006 中追加修订，登记 docs/architecture.md。
- [ ] 响应式 UI（最小窗口尺寸可用性）

## 影响模块

- `.github/workflows/ci.yml`（macOS `check` job Test 步骤、
  `terminal-compat` job 测试步骤）
- `Cargo.toml`（只读依据：`[package]` 根 crate 与 `[workspace] members`
  的现状，本次不修改）
- 验收时对照 `docs/testing.md` CI 规则 1、2、3

## 验收清单

- [x] spec 评审通过（AI 评审 + 人批准）
- [x] 行为契约固化为失败确认（Red）：`cargo test -- --list` 仅选中 200 个，
      `cargo test --workspace -- --list` 选中 401 个，确认 workspace members
      测试被静默排除
- [x] 最小实现通过聚焦测试（Green）：ci.yml 两处命令修正，
      `cargo test --workspace` 本机全量通过
- [x] `scripts/check-architecture.sh`（CI 配置变更不影响，仍执行）
- [ ] `cargo test --release --workspace -- --list` 统计的已注册测试 ≥ 390
      （本机以 debug 模式验证等价选择逻辑：401 个）
- [ ] 声明的平台 CI job 通过（macOS 全量 + Linux/Windows 由 Actions runner
      验证，spec 保持 in-progress 直到通过）
- [ ] 结构性决策提炼进 ADR（如有）并登记 docs/architecture.md：本项为
      ADR 0006 执行缺口修复，在 ADR 0006 关联规则中补充本次修订说明