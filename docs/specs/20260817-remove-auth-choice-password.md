# 删除 AuthChoice::Password 零构造变体

## 元数据

- 状态：`done`
- 创建：2026-08-17
- 相关 ADR：无（公共 API 收缩，不改变边界/属主；风险已在审计中接受）
- 相关 issue / 路线图项：`docs/plans/2026-08-17-simplification-backlog.md` S-B2
- CI 平台影响：`无（纯逻辑）`

## 背景

审计发现 `AuthChoice::Password` 全仓库零构造点：`default_auth_for` 只产出
`Key`/`Agent`（session.rs:43,71），`authenticate` 中对应 match 分支是不可达
分支，`#[allow(dead_code)]`（session.rs:11）正是用于压制 never-constructed
警告。真实密码认证走 `request_credential(CredentialKind::Password)` 兜底
（connection.rs:602-611），不受删除影响。删除该变体可移除死契约、收窄公共
API 面。

## 目标

1. 删除 `AuthChoice::Password` 变体与其唯一 match 分支，且不留任何编译警告。
2. 密码认证语义保持不变：仍只通过 UI 凭据兜底路径。
3. 为「候选生成不含 Password」与「密码兜底请求往返」补固定契约测试。

## 非目标

- 不引入"注入密码"的新 API（未来 UI 需要时另行设计，spec 中说明回滚点）。
- 不改动 `request_credential` 的实现语义（300s 超时、取消返回 None 等不变）。
- 不动归档文档 `docs/archived/`。

## 行为契约

1. 当 `default_auth_for` 从任意 `HostConfig` 推导认证候选时，返回的
   `Vec<AuthChoice>` 中不会出现 `Password` 变体；候选顺序（显式密钥 → 默认
   密钥 → agent）与 `Key`/`Agent` 构造语义不变。
2. 当候选耗尽且认证仍未成功时，`authenticate` 进入兜底：向 UI 发送
   `ConnEvent::NeedCredential { kind: CredentialKind::Password, .. }`；UI 经
   oneshot 回复密码时拿到该密码并尝试认证，事件通道关闭（UI 不可达/取消）时
   返回 `None`，密码重试语义不变。
3. 编译契约：任何构造 `AuthChoice::Password` 的代码不再编译；`authenticate`
   的 match 穷尽性由编译器 + clippy 门禁保证（无 `_` 通配分支掩盖缺失）。

## 边界与错误

- `request_credential` 的 300s 超时兜底保持不变；本次不写等待超时的测试
  （行为未被本次变更触碰，且 300s 等待不可入 CI 测试）。
- 空候选列表（无密钥、无 agent）：`authenticate` 直接进入密码兜底，行为不变。

## 接口与状态变更

- `crates/crossh-ssh` 公开 API：`AuthChoice` 删除 `Password` 变体（公共 API
  变更，审计已接受该风险；回滚点：加回变体 + match 分支）。
- re-export 保持不变：`AuthChoice`、`default_auth_for` 仍从 lib.rs 导出
  （`src/features/connections/manager.rs:9`、`entity.rs:10` 消费类型本身）。
- 无 wire / 持久化格式变化。

## 平台影响

- 无。`crossh-ssh` 为纯逻辑 crate；本地 macOS 全量可验证，CI 通用
  check/test job 覆盖即可，无需声明专门平台 job。

## 涉及纪律

- [x] Logic must not depend on UI（层级）：`crossh-ssh` 无 gpui 依赖，本次
      变更不引入
- [x] 其余纪律（图标、设置、文件规模、响应式）不适用，标注无

## 影响模块

- `crates/crossh-ssh/src/session.rs`（变体定义 + `#[allow(dead_code)]` 移除）
- `crates/crossh-ssh/src/connection.rs`（match 分支删除 + 契约测试）
- `crates/crossh-ssh/src/lib.rs`（核对导出不变）
- `crates/crossh-ssh/README.md`（核对公开入口说明不变）
- `docs/plans/2026-08-17-simplification-backlog.md`（完成后标注）

## 验收清单

- [x] spec 评审通过（AI 评审 + 人批准）
- [x] 契约测试先行落地并通过（删除前即绿，固定现有语义；删除任务无新行为，
      不产生 Red 阶段）
- [x] 删除变体 + 分支 + allow 豁免，`rg` 全仓库零 `AuthChoice::Password`
- [x] `cargo fmt --check`
- [x] `scripts/check-architecture.sh`
- [x] `cargo clippy --all-targets -- -D warnings`
- [x] `cargo test --workspace`
- [x] 声明的平台 CI job：无（纯逻辑）
- [x] 结构性决策提炼进 ADR（如有）：无（已有审计记录，无新边界）
- [x] 调试根因合并进 docs/engineering-notes/（如有）：无
- [x] 新增行为合并进 docs/testing.md 关键行为矩阵（如有）：Connection 行追加
      认证候选与密码兜底契约
- [x] 用户可观察效果人工确认：无需（内部 API 收缩，无 UI 变化）
