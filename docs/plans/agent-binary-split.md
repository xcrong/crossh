# crossh-agent 独立二进制拆分计划

状态：**已完结**（2026-08-15）。Phase 0-3 已执行完毕并收录为 ADR 0009；依赖图、ownership 与边界规则 7 已同步到 `docs/architecture.md`。Phase 4（独立分发）仍未启动，待 §5 信号触发后另起计划。

## 1. 背景与现状盘点（已核实）

`crossh agent` 现在是同一二进制的子命令（`src/main.rs:55-75`），但已经是"准独立"状态：

- `src/agent_cli.rs`（1866 行）**零 gpui、零 `crate::`/`features` 引用**，只依赖 `crossh-agent`、`crossh-theme`、`crossh-ssh`（仅 `ssh_runtime`）、ratatui/crossterm/tui-markdown/unicode-width、tokio。
- 唯一的外部耦合是 `main.rs:69` 从 `features::settings::load()` 注入 `AgentSettings`。
- `settings.toml` 是扁平合并文件（`persistence.rs`），`agent` 是其中的独立 table，结构由纯 crate `crossh-agent` 的 `AgentSettings` 定义；serde 默认忽略未知字段，因此"只读 agent 段"与 GUI 的完整读写天然兼容。
- `src/bin/` 自动发现 bin target（`crossh-git`、`crossh-updater` 先例）；`agent_cli.rs` 用 `#[path]` 引用 `agent_cli_render.rs` 和测试模块，相对路径与 crate 根无关，可直接被 bin 目标复用。
- 打包脚本显式列出二进制：`package.sh:36/43-45/87-88`、`package-linux.sh:32/40/54-56`、`package-windows.ps1:26/63-65`，新增二进制需三处同步。

拆分后形态：

```text
crossh（GUI 主程序；`crossh agent` → spawn 同目录/ PATH 的 crossh-agent）
  -> 不再编译 agent_cli 模块，主二进制不携带 ratatui/TUI 代码

crossh-agent（独立 TUI 二进制，src/bin/crossh-agent.rs）
  -> crossh-agent、crossh-ssh（仅 ssh_runtime）、crossh-theme、ratatui/crossterm/...
```

## 2. 不变契约（任何阶段不得破坏）

- TUI 功能、CLI 参数（`-c/-r/-m/--thinking/--no-session`）、帮助文本逐字不变。
- 会话 JSONL 格式与位置不变（由 `crossh-agent` crate 负责，天然保持）。
- `settings.toml` 文件格式与路径（`~/.config/crossh/settings.toml`）不变，GUI 与 agent 共用同一文件。
- `crossh agent` 用户可见行为保持不变。

## 3. 阶段划分

### Phase 0 — 设置读取收口（可立即做，零行为变化）

1. 在 `crates/crossh-agent` 新增 `settings::load()`：读 `~/.config/crossh/settings.toml`，仅取 `agent` table，未知字段忽略，缺失/解析失败回退默认值——语义与 `persistence.rs:69-89` 一致。
2. `main.rs:69` 改为调用它，消除 agent 对 `features::settings::persistence` 的依赖点，此后**全程序只有一份 agent 设置读取实现**。
3. 回归契约测试：用 GUI 风格完整 settings.toml（含 terminal/workspace 等无关字段）解析出 agent 段，断言与 GUI 解析结果一致。

### Phase 1 — 产出独立二进制（核心拆分，行为不变）

1. 新增 `src/bin/crossh-agent.rs`：`#[path = "../agent_cli.rs"] mod agent_cli;`，main() 只做设置加载 + `parse_options` + `run_with_options`（拷贝自 main.rs:55-75 的精简版）。
2. 无需 `[[bin]]`（`src/bin/` 自动发现）；root package 的 ratatui 等依赖保留不动。
3. 双轨运行期：`main.rs` 保留 `mod agent_cli`，`crossh agent` 与 `crossh-agent` 并存可 A/B 对比。
4. 验收：`cargo build --release --bin crossh-agent` 成功并记录体积（预期远小于完整二进制）；TUI 冒烟正常；`cargo clippy --workspace --all-targets -D warnings` 绿。

### Phase 2 — main.rs 委托（架构落地，行为不变）

1. `main.rs` 的 agent 分支改为 spawn 委托：镜像 `features/git/cli.rs:50-61` 的 sibling 优先 + PATH 回退查找，**继承 stdio**（子进程直接操作同一终端；launcher 不得碰 termios），`wait` 后透传退出码。
2. 可选收敛：把 sibling 查找逻辑从 `features/git/cli.rs` 抽到 `crossh-core`（纯 crate），git 与 agent 两个启动器复用。
3. 删除 `main.rs` 的 `mod agent_cli`（及 `agent_cli_render` 在 GUI 侧的编译）。
4. 更新架构文档：`docs/architecture.md` 的依赖图与边界规则（规则 7 扩展为"`crossh-git` 与 `crossh-agent` 是仅有的两个允许 `#[path]` 的独立入口"）；同步 `scripts/check-architecture.sh`。
5. 错误提示：sibling 与 PATH 均找不到时，报错提示安装 `crossh-agent`。
6. 测试：纯测试覆盖查找/回退函数；`agent_cli_tests` 随 bin target 由 `cargo test` 覆盖；对比清单跑一遍 `-c/-r/-m/--thinking/--no-session` 与退出码。

### Phase 3 — 打包/发布/CI（三平台脚本同步）

1. `scripts/package.sh`：36 行 build 列表加 `--bin crossh-agent`；43-45 行复制进 `MacOS/`；87-88 行加 `codesign --sign - --identifier "$BUNDLE_ID.agent"`。
2. `scripts/package-linux.sh`：32 行 build 列表、40 行 tar.gz 复制、54-56 行 AppDir 复制。
3. `scripts/package-windows.ps1`：26 行 build 列表、65 行复制 `crossh-agent.exe`。
4. 检查 `generate-update-manifest.sh` 与 `crossh-update`：manifest 面向归档产物（zip/tar.gz），二进制随包更新，预计无需改动；若含逐文件清单则补。
5. `release.yml` 走 package 脚本，一般无需改；本地只跑 macOS arm64 打包，Linux/Windows 由 CI 验证（遵守 AGENTS.md 平台规则）。

### Phase 4 — 独立分发（§5 信号触发后）

- 新增单 `crossh-agent` 二进制的独立产物（zip/tar.gz 逐平台）、README 安装说明；这是从"随包"走向"独立安装"的最终形态，届时同步更新稳定版本 manifest。

## 4. 风险与对策

| 风险 | 对策 |
| --- | --- |
| crossh 与 crossh-agent 版本错配 | 打包脚本保证同版本同产物；sibling 优先于 PATH，降低错配概率（ADR 0008 已承认同类代价） |
| 子进程 TTY/进程组语义变化 | 子进程继承 stdio，raw mode 由子进程自己启用，Ctrl-C 在 raw 模式下本就是按键流，与今天逐字一致；launcher 不碰 termios |
| 设置读取分叉 | Phase 0 收口为单一实现，杜绝 |
| 开发期 `cargo run` 不便 | `cargo build` 默认构建全部 bin；文档写明 `cargo build && cargo run -- agent` 或直接 `cargo run --bin crossh-agent` |
| 拆分窗口期双份编译 | 只影响编译时间、不影响产物，Phase 2 后消除 |

## 5. 触发信号（何时启动 Phase 1-3）

出现任一信号即启动；在此之前保持合体为默认姿势（Phase 0 可先行）：

- **信号 A（独立分发）**：agent 需要被推到远端机器/容器使用（scp 体积敏感），或用户需单独下载 agent 二进制。
- **信号 B（依赖分歧）**：agent 长出 GUI 不需要的依赖树，且与 gpui 侧依赖开始互相拉扯。
- **信号 C（生命周期独立）**：agent 变常驻会话服务（server 模式），需要独立于 GUI 的升级/重启/崩溃隔离。

## 6. 验收清单（按阶段勾选）

- Phase 0：`cargo test --workspace` 绿；新增纯测试通过；`crossh agent` 冒烟不变。
- Phase 1：`crossh-agent` 可独立构建运行；体积对比记录；clippy 绿。
- Phase 2：委托后 `crossh agent` 参数/退出码对比一致；缺失二进制时报错友好；`check-architecture.sh` 绿；架构文档更新。
- Phase 3：macOS 产物含 `crossh-agent` 且 `codesign --verify` 通过；Linux/Windows 脚本改动合入；手动 `workflow_dispatch` 触发 mac job 验证产物。
- Phase 4：独立产物 + 安装文档（仅在触发后）。

执行完成后：新增 ADR（参照 0008 格式：背景/决策/结果代价）+ `docs/architecture.md` 决策记录索引更新，并将本计划文档标记为已完结。