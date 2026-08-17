# Crossh 测试契约

本文是 ADR 0006 的执行说明。目标不是承诺“没有 bug”，而是让每次变更面对快速、确定、不可静默跳过的行为约束。

## Spec 驱动的测试来源

TDD 的起点是 `docs/specs/` 中已批准的 spec（见 `docs/specs/README.md` 与
ADR 0012）：spec 的行为契约条目就是测试的输入，测试名带 spec 编号前缀
（`spec_YYYYMMDD_<slug>__<行为>`），失败信息可追溯到规格。spec 评审只审
行为与验收，不审实现；一旦批准，按下方 TDD 循环执行。

新增可执行行为在收尾时合并进本文的关键行为矩阵。

## 默认开发范式：TDD

任何改变应用行为的功能、修复或重构，默认使用 Red-Green-Refactor：

1. **Red**：先用测试描述预期的可观察行为，运行它，并确认它因为目标行为尚未实现而失败，而不是因为测试本身无法编译、fixture 错误或环境缺失。
2. **Green**：实施满足该契约所需的最小生产代码，运行聚焦测试直到通过，再运行受影响模块的测试。
3. **Refactor**：清理生产代码和测试中的重复或偶然复杂度，期间保持测试为绿色，最后运行与变更风险相称的整套检查。

测试同时是行为说明，因此测试名称、输入、操作和断言应让读者无需阅读实现就能理解契约。优先断言公开输出、状态迁移、协议数据、持久化结果和用户可观察效果；不要默认绑定私有字段、函数调用次数、渲染树形状或内部步骤。

已确认的回归必须先由失败测试复现，再实施修复，并永久保留该测试。若产品契约有意改变，应同时修改测试及相关文档或 ADR，不能通过弱化断言来掩盖变化。

纯文档、格式化、生成物以及可证明不改变行为的机械重构可以不新增测试，但必须执行已有检查。无法在本机执行的平台专属测试应与实现一起加入，由对应 GitHub Actions runner 完成验证；交付说明必须明确本地没有观察到该测试的 Red/Green 阶段。

## 测试层次

### 纯逻辑测试

- 配置、协议、路径、Unicode 和序列化的输入分区与错误分支。
- workspace、forwarding、SFTP、connection 和 update 的状态迁移与不变量。
- 随机操作序列结束后，索引、引用、pending/active 集合和 dirty 状态必须一致。

### GPUI 行为测试

- 通过真实 entity、action、键盘事件、焦点和订阅驱动行为。
- 异步或 deferred callback 后调用 `run_until_parked()`。
- 断言用户可观察状态，不绑定无关的 element 树实现细节。
- 每个交互组件至少覆盖确认、取消、快速重复操作和关闭路径。

### Hermetic 集成测试

- Terminal：冻结控制字节 fixture、真实本地 PTY smoke。
- SSH：进程内 loopback server，覆盖 host key、认证、command、断线。
- SFTP/forwarding：loopback transport 或窄 fake backend，覆盖成功、失败、取消和乱序。
- Agent SDK：loopback HTTP/SSE，覆盖 chunk 边界、UTF-8、错误响应和中断。
- Updater：临时目录中的真实 zip/tar、checksum、替换和回滚。

### 发布验证

- 每个平台验证发布物包含主程序、updater 和必要资源。
- updater 安装 smoke 只能操作临时副本，不覆盖开发环境中的 Crossh。

## 关键行为矩阵

| 功能 | 必须受保护的行为 |
| --- | --- |
| Workspace | 打开/切换/关闭 tab，active index 合法，关闭时清理订阅和后台任务；启动/同步时清理失效最近目录，竞态点击不打开根目录 |
| Terminal | chunk 边界等价，alternate screen/mouse/keyboard mode，resize，退出和通知 |
| Connection | 连接状态，host-key/credential 应答只能消费一次，断线后可重新获取连接 |
| SFTP | list/read/write/upload/download，dirty editor，保存失败，关闭确认 |
| Forwarding | start/stop，启动后立即停止，旧回执不得重新激活，关闭 pane 停止全部规则 |
| Settings | 默认值、规范化、持久化迁移，provider/model 引用修复，Unicode/IME 输入 |
| Agent | provider wire contract，SSE 分块，tool call 聚合，取消和 workspace 路径隔离 |
| Update | manifest 校验，大小/checksum，归档安全，原子替换和失败回滚 |

## CI 规则

1. PR 必须运行 format、architecture、Clippy、全量普通测试和显式 integration-test target。
2. 不使用可能匹配零项仍成功的过滤命令充当关键门禁。
3. macOS 运行完整应用测试；Linux/Windows 运行完整逻辑测试和各自平台测试。
4. 慢速 loopback、fuzz、mutation 和发布安装验证可进入定时任务，但失败必须可追踪。
5. 新增 feature 必须在本矩阵中声明行为契约，或在 ADR 中说明为何现有契约已经覆盖。

## 验证责任边界

- 本地开发只对 macOS 行为和可在 macOS 执行的平台无关逻辑负责。
- Linux、Windows 及其 PTY、路径、进程、安装行为由 GitHub Actions 中对应 runner 负责，不要求本机安装模拟器、交叉编译工具链或兼容层。
- 影响 Linux/Windows 的变更必须同步增加或调整 CI 测试。在对应 Actions job 通过前，只能说明“已提交 CI 验证”，不能说明该平台已经验证通过。
- 本机因 `cfg` 跳过的平台测试必须在交付说明中明确列出，并指出负责执行它的 CI job。
- 测试执行直接使用当前 checkout 和默认 `target/`。除非用户明确要求，不创建 Git worktree、仓库副本或独立构建缓存。

## 本地验证

```sh
cargo fmt --check
scripts/check-architecture.sh
cargo clippy --all-targets -- -D warnings
cargo test --workspace
```

终端兼容测试使用独立 target，确保文件缺失时立即失败：

```sh
cargo test --test terminal_replay -- --test-threads=1
```
