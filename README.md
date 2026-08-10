# crossh

基于 [gpui](https://github.com/zed-industries/zed/tree/main/crates/gpui) 的轻量 SSH 客户端（macOS / Linux / Windows），
以 `~/.ssh/config` 为唯一真源，常驻开发工具定位：低内存、多标签、开箱即用。

## 特性

- **读取 `~/.ssh/config`（只读）**：别名列表、`Include`/通配/`Match`、`ProxyJump`、`Local/Remote/DynamicForward`、`IdentityFile`/`IdentitiesOnly` 均可解析。
- **会话池复用**：SFTP、端口转发和后台远程命令按主机共享已认证的 russh 连接；交互式终端由 Zed 的 PTY/事件循环独立管理。
- **交互式终端**：Zed `terminal` 负责 PTY、终端模拟和滚动核心；Crossh 按固定 Zed revision 本地裁剪并维护 `terminal_view` 的 `TerminalElement`，继续复用 Zed 的绘制、输入、文本选择、鼠标协议、IME 和滚动行为。本地 shell 与交互式 SSH 都走同一套视图。
- **反应式认证**：未知主机密钥弹指纹确认（可写 `known_hosts`）；加密私钥口令、密码按需弹出，凭据不回写日志。
- **SFTP**：远程浏览、上传/下载、目录递归、进度条、覆盖确认。
- **端口转发**：`-L` / `-R` / `-D`(SOCKS5)，config 驱动，UI 启停。
- **本地终端**：Zed `TerminalBuilder` 创建本地 PTY；Crossh 只叠加项目目录、当前 `cwd` 和多会话标签，Git 状态由工作区单独维护。
- **设置**：语言（zh/en）、Zed 终端字号、滚动回退行数、启动时检查更新，持久化到 `~/.config/crossh/`。
- **远程更新**：设置页从 HTTPS release manifest 检查版本，按平台下载并校验 SHA-256，再交给随应用分发的独立 updater 完成替换和重启。
- **Crossh Agent**：`crossh agent` 提供流式多协议模型对话、`read`/`grep`/`find`/`ls`/`edit`/`write`/`bash` 工具、项目 `AGENTS.md`/`CLAUDE.md`/`.pi/SYSTEM.md` 上下文、项目与用户级 `skills`/prompt templates、JSONL 会话恢复/分叉/树回退/压缩/导出、模型与思考级别切换、Markdown 输出、工具确认、取消和工作中排队。
- 常驻友好：日志裁剪（`/tmp/crossh/run.log`）、panic 现场保留、空闲内存 ~70MB。

## 构建与运行

```bash
# 需要 Xcode Command Line Tools + Rust (edition 2024)
cargo run            # 开发模式（日志同时 tee 到 stderr）
cargo run --release  # 发布模式

# 交互式 coding agent
cargo run -- agent
cargo run -- agent --continue
```

打包为未签名 `.app`（当前没有 Developer ID）：

```bash
scripts/package.sh          # 本机架构，输出 dist/crossh.app 与 dist/crossh-<version>-<arch>-macos.zip
scripts/package.sh x86_64-apple-darwin   # 指定架构（交叉编译）
open dist/crossh.app
```

三平台发布产物由 [.github/workflows/release.yml](.github/workflows/release.yml) 构建：macOS `.app` zip（aarch64/x86_64）、Linux `tar.gz` + AppImage（x86_64/aarch64）、Windows zip（x86_64）。每个 release 同时生成 `stable.json`，由 [scripts/generate-update-manifest.sh](scripts/generate-update-manifest.sh) 根据实际产物的大小和 SHA-256 自动生成。更新设计、平台替换策略和后续 Ed25519 签名计划见 [docs/remote-update-plan.md](docs/remote-update-plan.md)。

当前版本的 macOS 包不做 Apple 签名，不承诺绕过 Gatekeeper 或提供公证；远程更新只负责验证 HTTPS、目标平台、版本、文件大小和 SHA-256。

## 快捷键

| 快捷键 | 功能 |
| --- | --- |
| `Cmd/Ctrl+T` | 新终端标签（复制当前标签的目标） |
| `Cmd/Ctrl+W` | 关闭当前标签 |
| `Cmd/Ctrl+Tab` / `Cmd/Ctrl+Shift+Tab` | 切换标签 |
| `Cmd/Ctrl+1..9` | 跳到第 N 个标签 |
| 侧栏搜索框回车 | 打开主机 / 快速连接 `user@host` |

侧栏搜索支持关键词：`local` / `project`（或中文 `本地` / `项目`）直达目录视图与目录选择器。

Agent 终端内输入 `/help` 查看命令。常用命令包括 `/model`、`/thinking`、`/resume`、`/new`、`/compact`、`/export`、`/skills`、`/prompts`；`/skill NAME` 应用项目技能，`/prompt NAME [args]` 执行 prompt template。`!command` 会执行 Shell 并把输出交给模型，`!!command` 只执行不回传。写入和 Shell 工具默认由审批模型自动审核；审批模型不可用时才回退到本地确认，审批请求、结果和拒绝原因会显示在消息流中。

项目资源目录支持 `.agents/skills/<name>/SKILL.md`、`.pi/skills/<name>/SKILL.md`、`.pi/prompts/<name>.md` 和 `prompts/<name>.md`；用户级资源放在 `~/.pi/agent/`、`~/.agents/` 或 `~/.config/crossh/agent/` 对应目录。当前项目目录优先于全局同名资源。

## 架构

```
crates/
  crossh-core/                无 UI 的配置、终端契约、命令/Git 逻辑
  crossh-ssh/                 russh 连接、SFTP、转发和认证引擎
  crossh-terminal/            终端 settings/events 模型边界
  crossh-update/              manifest、下载校验、归档安装和 updater
  crossh-assets/              无 UI 的图标资源、嵌入和资源完整性校验
  crossh-ui/                  GPUI 主题、图标、菜单和通用控件
src/
  main.rs                     入口编排：窗口、快捷键、启动顺序
  infrastructure/logging.rs  日志、panic hook、日志裁剪
  features/                   GPUI feature views 和跨 crate adapters
    connections/              crossh-ssh 的 GPUI entity、连接管理和提示
    terminal/                 Zed terminal 的终端视图宿主
    sftp/                     SFTP 面板、远程编辑器和交互逻辑
    forwarding/               端口转发面板
    workspace/                外壳、侧栏、标签和 WorkspacePane 抽象
    settings/                 设置窗口与持久化编排
```

依赖方向保持单向：`crossh-core`、`crossh-assets`、`crossh-ssh`、`crossh-terminal` 和 `crossh-update` 不依赖 GPUI；`crossh-ui` 将 `crossh-assets` 适配为 GPUI 的资源源；根 package 的 GPUI feature adapter 依赖这些 crate；`workspace` 通过 `WorkspacePane` trait 消费终端、SFTP 和转发面板。可重复执行的分层检查位于 `scripts/check-architecture.sh`。

UI 图标统一放在 `crates/crossh-assets/assets/icons/`，由 `crossh-assets`
自动嵌入。图标引用必须通过 `crossh_ui::icons::IconName`，不要在业务视图中
直接写 `icons/<name>.svg`；资源包的单个测试会校验所有声明图标和嵌入文件。

技术栈：Zed `gpui`、`terminal`、`task`（UI、PTY、终端模拟和 shell 进程）；Crossh 本地裁剪的 `terminal_view` 基础（渲染和交互）以及薄宿主（生命周期、焦点和工作区边界）；`russh`（SFTP、端口转发和后台 SSH 命令）；`tokio`（2 worker 常驻）。`alacritty_terminal` / `vte` 由 Zed `terminal` 间接使用，Crossh 不再直接维护另一套生产终端实现。

## 路线图

见 [ROADMAP.md](ROADMAP.md) 和 [.kilo/plans/](.kilo/plans/)。已实现的规划外补强：设置面板、i18n、连接池生命周期。
未落地的 stretch 项：

- 命令生命周期事件和其他 Crossh 终端附加层
- SFTP 拖拽上传 / 批量 / 断点续传
- 标签拖拽排序
- config 编辑 UI、自动重连
- 多层 ProxyJump、`Match exec` / `ProxyCommand` / GSSAPI
- 多窗口、浅色主题

## 许可证与致谢

GPL-3.0-or-later。交互式终端使用 Zed 的 `terminal` 基础设施，并将 Zed `terminal_view` 的必要 `TerminalElement` 与 APCA 辅助代码按 `Cargo.toml` 中的固定 revision 本地裁剪维护，以避免引入编辑器、LSP 和工作区应用层依赖；Crossh 自有代码只提供薄宿主和工作区适配。具体依赖、源代码归属和许可见 [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md)。UI 图标为 [Lucide](https://lucide.dev/) 1.27.0 官方 SVG；应用图标为自绘。
