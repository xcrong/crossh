# Crossh 项目审阅报告（2026-08-17）

审阅方式：三个并行只读 subagent（架构与分层一致性 / 测试契约覆盖 / 技术债与文档漂移），
全部对照项目权威声明（`docs/architecture.md`、`docs/testing.md`、ADR 0006、`scripts/check-architecture.sh`、AGENTS.md）核实，证据具体到文件:行号。本报告未修改生产代码。

## 总体结论

架构分层纪律执行得很好（纯逻辑 crate 零 GPUI 违规、Git 语义归属正确、解析器无重复实现、
无 TODO/FIXME/dbg 残留），但存在三类系统性风险：

1. **CI 静默排除测试**：macOS job 缺 `--workspace`、Linux/Windows 的 `--lib` 跳过根 crate 全部 186 个测试（等价的"零匹配零失败"）。
2. **ADR 0006 的 hermetic 集成测试承诺未兑现**：crossh-ssh 无 loopback server，3 个真实主机测试 `#[ignore]`；SFTP 核心行为零测试；Agent SDK 无 loopback HTTP/SSE。
3. **文档与实现漂移**：README 声称支持 `Match` 指令（代码明确不支持）、发布包清单缺 agent/updater、Git 子系统缺行为规格且 testing.md 矩阵无 Git 行。

## 一、架构与分层（审阅 A）

### 合规确认

- 8 个纯逻辑 crate 全部无 `gpui`/`gpui_platform`/`crossh-ui` 导入（命中均为注释）。
- Cargo.toml 依赖图与 architecture.md 声明一致，无反向边。
- Git 协议/仓库操作全部在 `crossh-core`（`src/` 下 `Command::new("git")` 零命中）；Git Viewer 状态在 UI 无关 session 层；settings feature-owned。
- 无超过 2000 行的文件（白名单 `terminal_element.rs` 除外）；无额外 `#[path]` 违规。

### 发现

| 编号 | 问题 | 严重度 | 证据 |
| --- | --- | --- | --- |
| A-1 | bin 级依赖边界不可表达：`src/bin/crossh-git.rs` / `crossh-agent.rs` 共享根包全部依赖，"只依赖纯 crate"无编译期强制力 | high | architecture.md:19-23,59 |
| A-2 | rule 7/8/9 与依赖方向完全无 CI 检查：新 bin 乱挂 `#[path]`、Git 语义上浮、共享组件下沉都不会被拦截；脚本不读 Cargo.toml | high | check-architecture.sh |
| A-3 | `src/shared/text_editing.rs` 跨 4 个 feature + 2 个二进制共享，是纯逻辑却躺在应用 crate，rule 1 脚本不检查 `src/shared/` | medium | src/shared/text_editing.rs |
| A-4 | `main.rs` 承担 agent spawn 与子命令分派，rule 5"assembly only"边缘违背（git 侧有 git_launcher，agent 侧没有） | low | main.rs:44-92,131-146 |
| A-5 | 白名单注释失真：terminal_element.rs 实测 2187 行，注释写 2186；fork 持续增长无预警 | low | check-architecture.sh:9-10 |

## 二、测试契约（审阅 B）

### 行为矩阵覆盖结论（390 个测试）

| 矩阵行 | 覆盖结论 |
| --- | --- |
| Settings | 已覆盖（最完整） |
| Agent | 已覆盖（逻辑层；HTTP/SSE 传输层缺 loopback） |
| Update | 已覆盖（缺失败回滚注入测试） |
| Workspace | 部分：无 GPUI 交互测试，"关闭时清理订阅和后台任务"无测试 |
| Terminal | 部分：resize、PTY 退出流程无测试 |
| Connection | 部分：断线重建无测试；SSH 无 hermetic 测试 |
| Forwarding | 部分：状态机有测试，传输层（TCP relay/SOCKS5）零测试 |
| SFTP | 未覆盖（核心行为：list/read/write/upload/download、dirty editor、保存失败、关闭确认全部无测试） |
| Git | 矩阵无此行，但实际测试最充分（crossh-core 87 个，含真实 repo 端到端） |

### 发现

| 编号 | 问题 | 严重度 | 证据 |
| --- | --- | --- | --- |
| B-1 | macOS job `cargo test --release` 缺 `--workspace`：非 virtual workspace 只选根 crate，workspace members 约 200 个测试在 PR 上不执行 | high | .github/workflows/ci.yml |
| B-2 | terminal-compat `--workspace --lib`：根 crate 无 lib target，186 个测试在 Linux/Windows 静默跳过 | high | 同上 |
| B-3 | crossh-ssh 3 个端到端测试 `#[ignore]` + 依赖真实主机/用户配置，违反 ADR 0006 第 4 条；loopback server 全仓库不存在 | high | connection.rs:connect_real_host 等 |
| B-4 | SFTP 核心行为零测试，ADR 0006 承诺的 fake backend/loopback 未实现 | high | crates/crossh-ssh/src/sftp.rs |
| B-5 | GPUI 交互测试薄弱：15 个 `#[gpui::test]` 中仅 2 处正确使用 `run_until_parked`，workspace/SFTP/forwarding 的确认/取消/快速重复/关闭路径基本缺失 | medium | testing.md 交互要求 |
| B-6 | `tests/visual_capture.rs` 无任何 CI job 使用，视觉回归资产完全失效 | medium | ci.yml / release.yml 全文 grep |
| B-7 | app_menu 测试 `dispatch_action` 后未 `run_until_parked` 直接断言，脆弱模式 | medium | app_menu.rs:deferred_menu_work_can_read_the_dispatching_window |
| B-8 | Git feature 未在矩阵声明（违反 testing.md 规则 5） | medium | testing.md:58-67 |

## 三、技术债与文档漂移（审阅 C）

| 编号 | 问题 | 严重度 | 证据 |
| --- | --- | --- | --- |
| C-1 | README 声称支持 `Match` 指令，代码明确不支持（`Match exec` 属计划排除项），顶层文档承诺未实现能力 | high | README.md:8 vs ssh_config.rs:4-5 |
| C-2 | `crossh-core::terminal::session` 生产无消费者（InputCmd/SessionEvent 仅测试消费），旧"channel 内终端"路径遗留 | medium | terminal/session.rs、connection.rs cfg(test) |
| C-3 | 两份 remote shell bootstrap 生成逻辑独立维护（shell.rs 生产版 vs connection.rs 测试版），测试路径不能验证生产路径 | medium | shell.rs:332 vs connection.rs:741 |
| C-4 | 主 crate `assets` 直接依赖未被引用（实际使用者是 crossh-ui） | medium | Cargo.toml:28 |
| C-5 | README 发布包清单未随 ADR 0009 更新（缺 crossh-agent、crossh-updater） | medium | README.md:38-40 vs package.sh:36 |
| C-6 | Git 子系统完全无行为规格文档，且矩阵无 Git 行 | medium | src/features/git/ + crossh-core git 模块 |
| C-7 | SSH 连接/认证/host-key 决策无当前行为规格（安全敏感路径） | medium | crates/crossh-ssh/src/connection.rs |
| C-8 | 小项：agent_cli.rs:45 `allow(unused_imports)`、archived 内部引用文件名不符、plans 行号过时、workspace/shell.rs 逼近 2000 行、`SETTINGS_FILE_NAME` 常量双处定义 | low | 各处 |

## 处置 Backlog

按优先级排序，每项给出处置方式。处置 = **spec**（走新 SDD 流程）/ 直接修（豁免清单内或小型）/ 决策（需人裁定）。

| 优先级 | 编号 | 问题 | 处置 | 对应 spec / 说明 |
| --- | --- | --- | --- | --- |
| P0 | B-1, B-2 | CI 全量测试门禁静默排除 | spec | `docs/specs/20260817-ci-full-test-gates.md`（draft） |
| P0 | B-3, B-4 | SSH / SFTP hermetic 测试未兑现 ADR 0006 | spec | `docs/specs/20260817-ssh-hermetic-loopback.md`、`docs/specs/20260817-sftp-core-contracts.md`（draft） |
| P0 | C-1 | README `Match` 声明与实现冲突 | 直接修 | 文档修正（豁免清单）；或裁定实现 `Match`（需 ADR） |
| P1 | B-8, C-6 | Git 子系统入矩阵 + 行为规格 | 直接修 | testing.md 补 Git 行；Git 行为契约文档 |
| P1 | C-5, C-4, C-2, C-3 | 文档与死契约清理 | 直接修 | README 补发布清单；移除冗余依赖；删除或恢复 session 契约；统一 bootstrap |
| P1 | B-5 | GPUI 交互测试补强 | spec | 后续拆分（workspace 关闭清理 / forwarding 交互等） |
| P1 | A-1, A-2 | bin 依赖边界与规则 7/8/9 无 CI 检查 | 决策 | 增强 check-architecture.sh + 评估 workspace 拆 bin crate |
| P2 | A-3 | text_editing 迁移 | 决策 | 移入 crossh-core 或新增检查规则 |
| P2 | B-6 | visual_capture 启用或移除 | 决策 | 需要人裁定 |
| P2 | B-7, C-8, A-4, A-5 | 低危清理项 | 直接修 | 随缘处理，不排期 |

## 与 SDD 工作流的衔接

- 首批 3 个 spec（CI 门禁、SSH loopback、SFTP 契约）处于 `draft`，等待按 `docs/specs/README.md` 流程评审（AI 评审 + 人批准）后进入 TDD。
- 直接修类问题不需要 spec，但仍需遵守既有检查（fmt/architecture/clippy/test）。
- 决策类问题（bin 依赖边界、text_editing 归属、visual_capture）建议各写一份短 spec 或在 issue 中裁定后补 ADR。
- 本报告的 backlog 表是"待办档案"，每完成一项可把处置结论标记进表；长期处置记录仍以 spec/ADR 为准。