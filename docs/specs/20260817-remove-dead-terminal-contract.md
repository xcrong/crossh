# 删除 InputCmd/SessionEvent 死终端契约与 cfg(test) 通道设施

## 元数据

- 状态：`done`
- 创建：2026-08-17
- 相关 ADR：docs/adr/0006-executable-testing-contracts.md（删除与 hermetic 测试纪律冲突的真实主机验证路径）
- 相关 issue / 路线图项：docs/plans/2026-08-17-simplification-backlog.md（S-B1）；docs/specs/20260817-ssh-hermetic-loopback.md（draft，联动）
- CI 平台影响：`无（纯逻辑）`（删除为编译器层面变更，全部平台一致，本地验证足够）

## 背景

`crates/crossh-core/src/terminal/session.rs` 的 `InputCmd`/`SessionEvent` 与
`crates/crossh-ssh/src/connection.rs` 的 cfg(test) 终端设施（`ConnCmd::OpenTerminal`、
`open_terminal_channel`、`detect_remote_shell`、`remote_shell_bootstrap_command`、
`relay_terminal`、`drive_input`）是旧"channel 内终端"路径的遗留：生产零消费，
唯一使用点是 connection.rs 的 cfg(test) 范围与 3 个 `#[ignore]` 真实主机测试。

生产远程终端走 `src/features/workspace/tabs.rs` 的 `ssh -tt` +
`remote_shell_bootstrap_command()`（crossh-core 生产版），不经过该契约。3 个
`#[ignore]` 测试依赖真实主机与用户 SSH 配置，CI 永不执行，与 ADR 0006 第 4 条
（SSH 使用 hermetic integration tests）冲突。backlog 已确认全部属实（审计报告
C-2/S-B1）。

## 目标

1. 删除 `InputCmd`/`SessionEvent` 两个死契约符号及其 re-export。
2. 删除 connection.rs 中仅被删除契约消费的 cfg(test) 终端设施。
3. 移除 3 个 `#[ignore]` 真实主机测试（设施删除后它们无法编译；其定位由
   loopback spec 的 hermetic 测试承接）。
4. 保留生产符号 `TerminalProcessInfo`（被 `terminal/title.rs` 消费）及其契约。

## 非目标

- 不实现也不代替 loopback spec（hermetic 测试另行立项，本 spec 只移除旧资产）。
- 不改动生产远程终端路径（`ssh -tt` / 生产版 bootstrap）。
- 不触碰 `remote_shell_setup_script`、`remote_shell_from_path`、`shell_quote_remote`、
  `run_remote_command` 等仍有生产消费者的符号。
- 不修改 3 个 `#[ignore]` 测试的"意图"——只是随设施移除；意图由 loopback 重建。
- 不修改历史文档（docs/archived/、docs/audit/），它们是当时的意图记录。

## 行为契约

删除类变更没有新的运行时行为，Red 阶段以静态探测失败为证（现在 rg 能命中
符号引用 → 删除后零命中），Green 后由编译器 dead-code lint 与既有测试守护。

1. 当对全仓库源码运行 `rg 'InputCmd|SessionEvent' --glob '*.rs'`，应该零命中，
   观察到符号及全部 cfg(test) 使用点（connection.rs 导入、`ConnCmd::OpenTerminal`
   变体、match 分支、relay/drive/detect/bootstrap 函数、3 个 `#[ignore]` 测试）
   已不存在。
2. 当连接器删除 cfg(test) 终端设施后定义仍引用它的符号，应该编译失败，观察到
   编译器报"unresolved reference / 未定义变体"（Green 后编译器即回归守卫）。
3. 当以 `--all-targets` 编译 workspace，应该无任何 dead_code/未使用导入警告，
   观察到 `cargo clippy --workspace --all-targets -D warnings` 零输出。
4. 当对 crossh-ssh 运行非忽略测试，应该与删除前一致全绿，观察到
   `remote_shell_quote_preserves_command_text` 与
   `remote_command_output_keeps_the_newest_complete_utf8` 仍通过，其他当前
   workspace 测试不受影响。
5. 当消费 `crossh_core::terminal::TerminalProcessInfo`（title.rs 的
   `process_display_name`），应该保持既有行为，观察到 title 相关测试保持绿，
   且 `terminal::session` 模块与 re-export 中仅剩 `TerminalProcessInfo`。

## 边界与错误

- `TerminalProcessInfo` 是硬性保留符号，删除范围内不得触碰。
- cfg(test) 版 `remote_shell_bootstrap_command`（connection.rs:740-762）不在
  backlog 删除清单的原始列表内，但它是 open_terminal_channel 的唯一调用方链
  成员，设施删除后成为 cfg(test) 死代码，必须一并移除；S-B4 的"测试版统一"
  对象随之消失，S-B4 剩余价值仅剩生产版内部 refactor，需在 backlog 中注明。
- `use russh::{ChannelReadHalf, ChannelWriteHalf}` 与
  `use crossh_core::terminal::{InputCmd, RemoteShell, SessionEvent, ...}`
  两处 cfg(test) 导入随设施移除，避免未使用导入警告。
- 3 个 `#[ignore]` 测试删除后，"真实主机人工验证"路径临时消失；可接受依据：
  CI 永不执行它们（无门禁价值）、ADR 0006 立场、loopback spec 将重建等价定位。
  若评审认为人工验证路径必须保留，应驳回本 spec 的契约 1 中该删除面并先落地
  loopback。
- 历史归档文档（docs/archived/crossh-remaining-features-plan.md 等）中含
  `InputCmd`/`SessionEvent` 描述，属历史记录，不在 rg 验收范围内修改。

## 接口与状态变更

- 公开 API 变更：`crossh_core::terminal` 不再导出 `InputCmd`、`SessionEvent`；
  `crossh-ssh`（public API）本身从未导出这些符号，无外部影响。
- `ConnCmd` 移除 `OpenTerminal` 变体（cfg(test) only，非公开 API 的一部分）。

## 平台影响

- 纯编译器层面变更，macOS 本地验证即代表全部平台；无平台专属行为，无需指定
  Actions job，既有 CI 天然覆盖。

## 涉及纪律

- [x] Logic must not depend on UI（层级）：删除后 crossh-ssh 保持零 gpui 导入，
      无新增依赖
- [ ] Feature-owned settings
- [ ] 图标纪律（Lucide 1.27.0 官方 SVG，IconName 映射）
- [x] 文件规模 < 2000 行（scripts/check-architecture.sh）：connection.rs 净缩短，
      无超限风险
- [ ] 工程笔记 / ADR 同步义务：无新调试根因；无新结构性边界（收尾既有审计
      发现），不新增 ADR；backlog S-B4 联动注记更新
- [ ] 响应式 UI（最小窗口尺寸可用性）

## 影响模块

- `crates/crossh-core/src/terminal/session.rs`（删 `InputCmd`/`SessionEvent`，
  留 `TerminalProcessInfo`）
- `crates/crossh-core/src/terminal/mod.rs`（re-export 同步）
- `crates/crossh-ssh/src/connection.rs`（cfg(test) 终端设施 + 3 个 `#[ignore]`
  测试移除；其余代码零改动）
- `docs/plans/2026-08-17-simplification-backlog.md`（S-B1 标记完成；S-B4 联动
  注记）

## 验收清单

- [x] spec 评审通过（AI 评审 + 人批准）
- [x] 行为契约全部固化为失败测试并确认失败原因正确（Red）：契约 1/2/3 以静态
      探测与编译器失败为证（删除类变更，无运行时行为测试；属 AGENTS.md 豁免
      范围，仍以既有测试守护）——Red 阶段 rg 44 处引用命中，Green 后零命中
- [x] 最小实现通过聚焦测试（Green）：`cargo test -p crossh-ssh -p crossh-core`
      全绿
- [x] `cargo fmt --check`
- [x] `scripts/check-architecture.sh`
- [x] `cargo clippy --all-targets -- -D warnings`
- [x] `cargo test --workspace`
- [x] 声明的平台 CI job 通过：不适用（无平台专属行为）
- [x] 结构性决策提炼进 ADR（如有）并登记 docs/architecture.md：无
- [x] 调试根因合并进 docs/engineering-notes/（如有）：无
- [x] 新增行为合并进 docs/testing.md 关键行为矩阵（如有）：无
- [x] 用户可观察效果人工确认（针对 UI/交互变更）：不适用
- [x] backlog S-B1 标记完成，S-B4 联动注记同步