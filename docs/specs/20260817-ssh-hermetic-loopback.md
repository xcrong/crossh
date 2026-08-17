# 为 crossh-ssh 补齐 ADR 0006 承诺的 hermetic loopback 集成测试

## 元数据

- 状态：`draft`
- 创建：2026-08-17
- 相关 ADR：docs/adr/0006-executable-testing-contracts.md
- 相关 issue / 路线图项：无
- CI 平台影响：`全部`（新增测试由 macOS 与 terminal-compat 全部 runner 执行；不依赖任何平台专属工具链）

## 背景

`crates/crossh-ssh/src/connection.rs` 中 3 个端到端测试
（`connect_real_host`、`connect_and_run_remote_command`、
`connect_and_run_ls`）全部标记 `#[ignore]`：它们依赖真实主机
`CROSSH_TEST_HOST` 与用户本机的 SSH 配置（`SshConfig::from_default_location()`
与 `default_auth_for`），CI 永不执行，也无本地 Green 依据。这直接违反
ADR 0006 第 4 条（SSH、SFTP 等使用 hermetic integration tests，不依赖
用户的 SSH 配置、凭据、网络和主机名称）。

仓库中不存在任何进程内 loopback SSH server。上层
`src/features/connections/manager.rs` 的 `acquire`（Closed/Error 状态重建
连接）同样是零测试，`docs/testing.md` 关键行为矩阵 Connection 行
「断线后可重新获取连接」无任何执行保障。

## 目标

1. 在 `crates/crossh-ssh` 内提供进程内 loopback SSH server，使连接测试
   脱离真实主机与用户 SSH 配置。
2. 以 loopback 覆盖原有 `#[ignore]` 测试的定位：host key 问答、认证、
   远程命令执行。
3. 为 `acquire` 的断线重建连接行为覆盖测试，勾选矩阵 Connection 行。
4. `cargo test -p crossh-ssh` 在无网络、无用户 SSH 配置环境下全部通过。

## 非目标

- 不实现生产环境的 SSH server 能力；loopback server 仅存在于测试代码。
- 不删除或重写现有 `#[ignore]` 测试的意图，而是以 hermetic 测试承担其
  定位；`#[ignore]` 测试是否保留由实施评审决定（保留可作为诊断工具，
  但不能计入门禁）。
- 不为每个函数建立 mock 层；只增加真实副作用边界所需的小型接口或纯
  reducer（遵循 ADR 0006「只有真实副作用边界可以增加测试用接口」）。
- 不改动 crossh-ssh 的公开 API 与 wire 格式。

## 行为契约

1. 当测试通过 loopback server 发起首次连接且服务器返回未知 host key，
   应该触发一次 host-key 问答，观察到应用发出 `NeedHostKey` 事件、消耗
   该回答（AcceptOnce）后继续握手，同一疑问不会重复触发（对应
   `docs/testing.md`「host-key/credential 应答只能消费一次」）。
2. 当测试使用正确的凭据连接 loopback server，应该认证成功，观察到连接
   进入 Connected 状态，无需任何真实主机名或用户 SSH 配置参与。
3. 当测试使用错误的凭据连接 loopback server，应该认证失败，观察到
   连接以 AuthenticationFailed（或等价错误事件）终止，服务器端无残留
   会话。
4. 当测试在 loopback 上打开远程命令执行，应该执行成功与终止两条路径，
   观察到命令输出字节、退出码与 Terminated 状态与既有 `#[ignore]` 测试
   断言的语义一致（成功回执 + 可终止的长任务）。
5. 当连接以 Closed 或 Error 状态结束后上层调用 `acquire` 重建连接，
   应该为同一目标重新建立新的可用连接，观察到重建后的连接可以再次
   打开终端或执行命令（勾选矩阵 Connection 行「断线后可重新获取连接」）。
6. 当上述所有测试在无网络、无 `CROSSH_TEST_HOST`、无用户 SSH 配置的
   环境运行 `cargo test -p crossh-ssh`，应该全部通过，观察到零依赖
   `#[ignore]` 或名称跳过。

## 边界与错误

- 认证失败必须可复现：loopback server 需支持至少一种认证方法的
  成功/失败分支（key 或 password 视实现而定，但不得读取用户配置）。
- host key 拒绝路径必须覆盖：用户拒绝（Reject）时握手终止且无会话
  泄漏。
- 断线重建需覆盖「应答已消费过」的重复触发场景，防止重建后复用旧应答。
- loopback server 的端口与密钥必须在测试内生成（临时端口/临时目录），
  不可依赖固定端口或预置 fixture 密钥对外可见。
- 测试结束后服务器任务必须被明确关闭，不留后台任务（资源清理）。

## 接口与状态变更

- 仅在 `crates/crossh-ssh` 的测试侧新增 loopback server 设施；生产代码
  若因此增加最小接口（如连接参数构造器、可注入 host/port），须以真实
  复杂度驱动，且保持 crossh-ssh 零 gpui 依赖（Logic must not depend on
  UI 纪律）。

## 平台影响

- 新增测试为纯 Rust + tokio 逻辑，macOS 本地与 Linux/Windows
  terminal-compat runner 均执行；无平台专属行为。
- Linux/Windows 的实际执行由 terminal-compat job 验证；本地 macOS 负责
  本机执行与平台无关逻辑的验证。

## 涉及纪律

- [x] Logic must not depend on UI（层级）：crossh-ssh 保持零 gpui 导入；
      loopback server 与测试仅依赖 tokio/async-channel 等既有依赖
- [ ] Feature-owned settings
- [ ] 图标纪律（Lucide 1.27.0 官方 SVG，IconName 映射）
- [ ] 文件规模 < 2000 行（scripts/check-architecture.sh）：新增测试设施
      若使 connection.rs 或 sftp.rs 超限，应拆出独立测试模块文件
- [x] 工程笔记 / ADR 同步义务：loopback 设施落地后提炼进 ADR 0006 或
      新 ADR；调试根因合并进 docs/engineering-notes/（如有）
- [ ] 响应式 UI（最小窗口尺寸可用性）

## 影响模块

- `crates/crossh-ssh/src/connection.rs`（3 个 `#[ignore]` 测试的替代或
  并存；host key/认证/命令回执路径的 hermetic 覆盖）
- `crates/crossh-ssh/src/sftp.rs`（若共享 loopback 设施则只读复用，不属
  本次目标）
- `src/features/connections/manager.rs`（`acquire` 断线重建的测试）
- `docs/testing.md`（Connection 行「断线后可重新获取连接」勾选）

## 验收清单

- [ ] spec 评审通过（AI 评审 + 人批准）
- [ ] 行为契约全部固化为失败测试并确认失败原因正确（Red）：无 loopback
      设施时测试因「无法连接」或「无服务器」失败，而非编译错误
- [ ] 最小实现通过聚焦测试（Green）：loopback server + 生产最小接口
- [ ] `cargo fmt --check`
- [ ] `scripts/check-architecture.sh`
- [ ] `cargo clippy --all-targets -- -D warnings`
- [ ] `cargo test --workspace`
- [ ] `cargo test -p crossh-ssh` 在无网络、无用户 SSH 配置环境全部通过
      （含新增测试，不含 `#[ignore]`）
- [ ] 声明的平台 CI job 通过（terminal-compat 在 Linux/Windows 执行
      新增测试）
- [ ] 结构性决策提炼进 ADR（如有）并登记 docs/architecture.md
- [ ] 调试根因合并进 docs/engineering-notes/（如有）
- [ ] 新增行为合并进 docs/testing.md 关键行为矩阵（如有）：
      Connection 行「断线后可重新获取连接」标注已覆盖
- [ ] 用户可观察效果人工确认（针对 UI/交互变更）：不适用