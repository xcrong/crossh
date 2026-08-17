# 0006-executable-testing-contracts

## 状态

已接受

## 背景

Crossh 由 AI coding 高频推进，人工逐行审查和手工回归无法随变更规模线性扩展。现有测试对解析、字符串和局部渲染算法已有较好覆盖，但 workspace 状态迁移、GPUI 交互、SSH/SFTP/转发、终端协议和更新安装等跨边界行为仍可能在测试未执行或只执行零个匹配用例时通过 CI。

覆盖率只能说明代码是否被执行，不能单独证明行为正确。应用需要把架构边界、状态不变量、外部协议契约和关键用户流程转化为自动执行且无法静默跳过的约束。

## 决策

Crossh 采用分层的可执行测试契约：

1. 行为变更和缺陷修复默认采用 Red-Green-Refactor。先运行能够因目标行为缺失而失败的契约测试，再实施最小变更使其通过，最后在测试持续通过的前提下重构。测试是可执行的行为说明，不只是覆盖率输入。
2. 纯逻辑和状态机使用普通 Rust 测试，覆盖输入规范化、状态迁移、错误分支和不变量。能脱离 GPUI 的逻辑不得为了测试而引入 GPUI。
3. GPUI entity、action、焦点、事件订阅和异步回调使用 `#[gpui::test]`。触发延迟工作的测试必须运行 executor 直至 parked，并验证最终状态和无重入 panic。
4. SSH、SFTP、转发、HTTP/SSE、PTY 和 updater 使用 hermetic integration tests。测试依赖 loopback server、临时目录或冻结 fixture，不依赖用户的 SSH 配置、凭据、网络和主机名称。
5. 终端控制字节 fixture 必须同时按完整 buffer、单字节和确定性分块方式 replay，并比较最终 screen/mode 等语义状态。
6. CI 中关键测试使用显式 integration-test target。目标不存在、无法编译或未发现预期测试时必须失败，不允许依靠可匹配零项仍返回成功的名称过滤器。
7. 覆盖率用于发现盲区和约束新增代码，不作为正确性的唯一指标。关键行为契约、失败路径和竞态测试优先于全局百分比。
8. 生产事故和已确认回归必须先固化为失败测试或架构检查，再合入修复。
9. 本地开发环境只承担 macOS 行为和平台无关逻辑的验证。Linux、Windows 以及各自的 PTY、进程、路径和安装行为由 GitHub Actions 的原生 runner 验证；对应 job 通过前不得宣称该平台已经验证。
10. 测试和 Agent 工作直接使用当前 checkout 与默认 `target/`。除非用户明确要求，不创建 Git worktree、仓库副本或独立构建缓存来并行实施。

为支持确定性测试，只有真实副作用边界可以增加小型接口或纯 reducer，例如 connection、update、forwarding 和 SFTP 的 command/event 边界。接口必须由真实复杂度驱动，不能为每个函数建立 mock 层。

## 结果/代价

变更将由机器可执行的行为契约保护，CI 能识别空跑，异步成功、失败、取消和乱序可以稳定复现。测试还承担稳定的行为说明，使 AI Agent 可以从失败信息定位受破坏的边界，并在编码前获得明确约束，而不依赖人工完成全面回归。

代价是需要维护 fixture、fake backend、loopback 服务和少量测试专用构造器；跨平台协议测试也会增加 CI 时间，而且 Linux/Windows 的最终反馈晚于本地 macOS 验证。测试替身必须保持在副作用边界，避免测试实现反过来污染生产架构。

## 关联规则

- `AGENTS.md` 的 Logic must not depend on UI
- `AGENTS.md` 的 Zed / GPUI Dependency Source
- `docs/testing.md`
- `.github/workflows/ci.yml`
- `scripts/check-architecture.sh`

2026-08-17 修订：CI 门禁曾有两处静默排除（macOS job 缺 `--workspace`、
terminal-compat 的 `--workspace --lib` 跳过根 crate bin 测试），由
`docs/specs/20260817-ci-full-test-gates.md` 修复；此后全量门禁命令统一为
`cargo test --release --workspace`。
