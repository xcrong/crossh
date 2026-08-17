# 为 SFTP 核心行为补齐测试（矩阵 SFTP 行全覆盖）

## 元数据

- 状态：`draft`
- 创建：2026-08-17
- 相关 ADR：docs/adr/0006-executable-testing-contracts.md
- 相关 issue / 路线图项：无
- CI 平台影响：`全部`（新增测试由 macOS 与 terminal-compat 全部 runner 执行；不依赖真实网络或主机）

## 背景

`docs/testing.md` 关键行为矩阵 SFTP 行
（list/read/write/upload/download，dirty editor，保存失败，关闭确认）
当前完全没有执行保障：

1. `crates/crossh-ssh/src/sftp.rs` 的 `run_sftp_worker`、`read_file`、
   `write_file`、`list_dir`、`download`、`upload` 均无任何测试（无 fake
   backend、无 loopback transport；`cfg(test)` 下仅一个
   `format_bytes` 工具函数测试）。
2. `src/features/sftp/` 现有测试只覆盖编辑器工具函数（logic.rs、
   end_caret.rs）与零散 helper（view.rs）；dirty editor、保存失败、
   关闭确认、list/read/write/upload/download 全部无测试。
3. ADR 0006 第 4 条承诺的「SFTP：loopback transport 或窄 fake backend」
   未实现，导致 SFTP 的核心行为在 CI 上零执行、零保护。

## 目标

1. 为 `crates/crossh-ssh/src/sftp.rs` 的 worker 与核心操作提供窄 fake
   backend（或 loopback transport），覆盖成功、失败、取消与乱序回执。
2. 为 SFTP 编辑器交互的 dirty editor、保存失败、关闭确认补测试。
3. 全部测试可在无网络、无主机依赖的环境运行；勾选矩阵 SFTP 行。

## 非目标

- 不实现生产环境的 SFTP backend 替换能力；测试替身只存在于测试代码或
  明确的副作用边界接口。
- 不为每个函数建立 mock 层；仅增加真实复杂度驱动的窄接口（遵循
  ADR 0006 第 10 条「接口必须由真实复杂度驱动」）。
- 不改动 SFTP wire 格式、公开 API 与用户可见交互设计；只补齐测试与
  必要的测试注入点。
- 不把 crossh-ssh 引入 gpui，也不把 gpui 依赖引入纯逻辑层。

## 行为契约

1. 当后端目录中存在文件且调用 `list_dir`，应该返回目录项列表，观察到
   文件名、类型与大小与后端真实内容一致。
2. 当读取存在的文件路径，应该返回其字节内容，观察到内容与后端存储
   字节一致；当读取不存在的路径，应该返回明确错误，观察到无部分
   输出残留。
3. 当写入远端路径，应该持久化内容，观察到后续 `read_file` 或后端
   存储可直接读回相同字节；覆盖写入时旧内容被替换。
4. 当调用 `upload` 上传本地文件到不存在目标，应该成功，观察到远端
   字节与本地一致且可被 `read_file` 读回；当目标目录不存在或权限
   拒绝，应该返回明确错误，观察到远端无残留部分文件。
5. 当调用 `download` 下载远端文件到本地路径，应该成功，观察到本地
   文件字节与远端一致；当远端不存在或本地不可写，应该返回明确错误，
   观察到不会留下损坏的半写文件。
6. 当传输过程中发出取消请求，应该停止推进，观察到取消后无新数据
   落盘、无新的成功/失败回执（或单一显式 Cancelled 回执），状态机
   回到可复用状态。
7. 当回执以乱序到达（如慢操作的回执晚于后续快操作），应该按请求
   id/句柄归位，观察到每个回执只结算其对应操作，不错误结算其他
   操作（对应矩阵「乱序」与 ADR 0006 的确定性回执要求）。
8. 当编辑器存在 dirty 修改时用户尝试关闭，应该弹出关闭确认，观察到
   确认后丢弃修改并关闭、取消后保持编辑内容与焦点不变。
9. 当远端文件保存失败（写入被后端拒绝），应该向用户提示保存失败，
   观察到编辑器保持 dirty 状态、错误信息可见，且不出现「已保存」的
   假象。

## 边界与错误

- 失败路径与 happy path 同等覆盖：远端路径不存在、权限不足、后端
  连接断开、本地目标不可写。
- 取消必须覆盖进行中挂起与已派发未回执两种时机。
- 乱序回执必须覆盖「旧回执不得激活新状态」的重复触发场景。
- 资源清理：每个测试结束后 backend/服务器句柄与临时目录被移除，不
  留后台任务。
- 测试只做真实副作用边界：文件落盘行为在临时目录中验证，不触碰用户
  真实目录。

## 接口与状态变更

- 无生产 wire/API/设置项变更；允许为测试注入窄 backend trait 或构造
  器参数，保持 crossh-ssh 零 gpui。

## 平台影响

- 新增测试为纯 Rust + tokio 逻辑 + 临时目录，macOS 本地与
  Linux/Windows terminal-compat runner 均执行；无平台专属行为。
- Linux/Windows 的实际执行由 terminal-compat job 验证；本地 macOS
  负责本机执行与平台无关逻辑的验证（路径分隔符差异属于实测范围，
  归属 Actions runner）。

## 涉及纪律

- [x] Logic must not depend on UI（层级）：crossh-ssh 保持零 gpui 导入；
      fake backend 与测试仅依赖既有依赖
- [ ] Feature-owned settings
- [ ] 图标纪律（Lucide 1.27.0 官方 SVG，IconName 映射）
- [ ] 文件规模 < 2000 行（scripts/check-architecture.sh）：新增测试若使
      sftp.rs 超限，应拆出独立测试模块文件
- [x] 工程笔记 / ADR 同步义务：fake backend 设施落地后提炼进 ADR 0006
      或新 ADR；调试根因合并进 docs/engineering-notes/（如有）
- [ ] 响应式 UI（最小窗口尺寸可用性）

## 影响模块

- `crates/crossh-ssh/src/sftp.rs`（`run_sftp_worker`、`read_file`、
  `write_file`、`list_dir`、`download`、`upload` + 测试侧 fake backend）
- `src/features/sftp/`（dirty editor 关闭确认、保存失败提示；现有
  logic.rs / end_caret.rs / view.rs 测试保留）
- `docs/testing.md`（SFTP 行勾选「已覆盖」）

## 验收清单

- [ ] spec 评审通过（AI 评审 + 人批准）
- [ ] 行为契约全部固化为失败测试并确认失败原因正确（Red）：无 fake
      backend 时测试因「无后端可连」失败，而非编译错误
- [ ] 最小实现通过聚焦测试（Green）：fake backend + 最小生产注入点
- [ ] `cargo fmt --check`
- [ ] `scripts/check-architecture.sh`
- [ ] `cargo clippy --all-targets -- -D warnings`
- [ ] `cargo test --workspace`
- [ ] 声明的平台 CI job 通过（terminal-compat 在 Linux/Windows 执行
      新增测试）
- [ ] 结构性决策提炼进 ADR（如有）并登记 docs/architecture.md
- [ ] 调试根因合并进 docs/engineering-notes/（如有）
- [ ] 新增行为合并进 docs/testing.md 关键行为矩阵（如有）：SFTP 行
      标注「已覆盖」
- [ ] 用户可观察效果人工确认（针对 UI/交互变更）：保存失败提示与
      关闭确认若涉及视觉呈现，需人工/截图确认