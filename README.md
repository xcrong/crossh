# crossh

基于 [gpui](https://github.com/zed-industries/zed/tree/main/crates/gpui) 的本地优先终端工作环境（macOS / Linux / Windows），
以项目目录组织多会话本地终端为核心，Git Viewer、Note 为独立二进制。

## 特性

- **本地终端 / 项目管理**：以项目目录组织多会话本地终端（`LocalSession` / `LocalDir` 一等公民，见 `src/features/workspace/view.rs`），Zed `TerminalBuilder` 创建本地 PTY，Crossh 叠加项目目录、当前 `cwd`、多会话标签与 Git 状态联动；侧栏以项目为核心，隐藏主机分组（见 `src/features/workspace/sidebar.rs:188`）。
- **Git Viewer**：独立二进制 `crossh-git`，提供变更列表、staging/unstage、commit、push/pull 与远端分歧同步，状态栏实时展示 Git 状态，`cmd-r` 刷新。
- **Note**：独立二进制 `crossh-note`，本地 SQLite（WAL + FTS5 + 触发器同步，见 `crates/crossh-note/src/lib.rs`）存储，支持全文检索、标签、置顶，零 `gpui` 依赖的纯逻辑层。
- **系统监视 / Scratch 终端**：系统指标常驻面板与随手 Scratch 终端，作为工作区内置视图，不阻塞主工作区。
- **设置与常驻友好**：语言（zh/en）、Zed 终端字号、滚动回退行数、启动时检查更新，持久化到 `~/.config/crossh/`；日志裁剪（`/tmp/crossh/run.log`）、panic 现场保留、空闲内存 ~70MB。
- **远程更新**：设置页从 HTTPS release manifest 检查版本，按平台下载并校验 SHA-256 与 Ed25519 签名（缺失/无效签名一律拒绝，），再交给随应用分发的独立 updater 完成替换和重启。

## 构建与运行

```bash
# 需要 Xcode Command Line Tools + Rust (edition 2024)
cargo run            # 开发模式（日志同时 tee 到 stderr）
cargo run --release  # 发布模式

# 打开当前目录的 Git Viewer；也可以传入一个目录
crossh git
crossh git ~/Code/draw-backend

# 在已运行实例中打开项目（无实例时启动新实例）；裸 `crossh` 仅聚焦已有实例
crossh ~/Code/draw-backend
# 与子命令同名的目录（git/note/help）用 ./ 或 -- 转义
crossh ./git
```

Git Viewer 提供变更列表、staging/unstage、commit、push/pull 与刷新，状态栏同步显示与远端的分歧；`cmd-r` 刷新。

发布包会同时包含 `crossh`、`crossh-git`、`crossh-updater` 和共享的 `crossh-assets/` 资源目录（三平台一致，见 `scripts/package.sh` / `package-linux.sh` / `package-windows.ps1`）。`crossh git`
会优先启动安装目录旁边的 `crossh-git`，所有子程序共用同一份字体、图标和主题资源，
因此 Git Viewer 不需要加载完整的终端和工作区功能。

打包为未签名 `.app`（当前没有 Developer ID）：

```bash
scripts/package.sh          # 本机架构，输出 dist/crossh.app 与 dist/crossh-<version>-<arch>-macos.zip
scripts/package.sh x86_64-apple-darwin   # 指定架构（交叉编译）
open dist/crossh.app
```

三平台发布产物由 [.github/workflows/release.yml](.github/workflows/release.yml) 构建：macOS `.app` zip（aarch64/x86_64）、Linux `tar.gz` + AppImage + `AppImage.tar.gz`（AppImage 与 `install.sh` 一键安装包）+ `.deb` / `.rpm` 发行版原生包（x86_64/aarch64）、Windows zip + Inno Setup 安装程序 `*-setup.exe`（x86_64，aarch64 为 optional experimental）。安装程序与 zip 内容一致，默认 per-user 安装到 `%LOCALAPPDATA%\Programs\crossh`（免 UAC，开始菜单 + 卸载器，可选加 PATH/桌面快捷方式）；后续自更新仍走 zip 通道原地替换 exe，无需重跑安装程序。每个 release 同时生成 `stable.json`，由 [scripts/generate-update-manifest.sh](scripts/generate-update-manifest.sh) 根据实际产物的大小、SHA-256 与 Ed25519 签名自动生成。更新设计、平台替换策略与签名校验（v0.16.4 已落地）见 [docs/remote-update-plan.md](docs/remote-update-plan.md)。
当前版本的 macOS 包不做 Apple 签名，Windows 安装程序也不做代码签名，不承诺绕过 Gatekeeper / SmartScreen 或提供公证；远程更新负责验证 HTTPS、目标平台、版本、文件大小、SHA-256 与 manifest Ed25519 签名。

## 快捷键

| 快捷键 | 功能 |
| `Cmd/Ctrl+T` | 新终端标签（复制当前标签的目标） |
| `Cmd/Ctrl+W` | 关闭当前标签 |
| `Cmd/Ctrl+Tab` / `Cmd/Ctrl+Shift+Tab` | 切换标签 |
| `Cmd/Ctrl+1..9` | 跳到第 N 个标签 |
| 侧栏搜索框回车 | 打开项目 |

```
crates/
  crossh-core/                无 UI 的配置、终端契约、命令/Git 逻辑
  crossh-terminal/            终端 settings/events 模型边界
  crossh-update/              manifest、下载校验、归档安装和 updater
  crossh-assets/              无 UI 的图标资源、嵌入和资源完整性校验
  crossh-ui/                  GPUI 主题、调色板、图标、菜单和通用控件
  crossh-ui-component/        通用 GPUI 控件（按钮、徽章、头像、分隔线等）
src/
  main.rs                     入口编排：窗口、快捷键、启动顺序
  infrastructure/logging.rs  日志、panic hook、日志裁剪
  features/                   GPUI feature views 和跨 crate adapters
    terminal/                 Zed terminal 的终端视图宿主
    git/                      Git Viewer 窗口与变更操作
    workspace/                外壳、侧栏、标签和分栏状态
    settings/                 设置窗口与持久化编排
    updates/                  更新状态机与设置页入口
```

依赖方向保持单向：`crossh-core`、`crossh-assets`、`crossh-terminal` 和 `crossh-update` 不依赖 GPUI；`crossh-ui`（含调色板 `palette`）将 `crossh-assets` 适配为 GPUI 的资源源；根 package 的 GPUI feature adapter 依赖这些 crate；`workspace` 直接管理本地终端实体、活动视图与分栏状态。可重复执行的分层检查位于 `scripts/check-architecture.sh`。

UI 图标统一放在 `crates/crossh-assets/assets/icons/`，由 `crossh-assets`
自动嵌入。图标引用必须通过 `crossh_ui::icons::IconName`，不要在业务视图中
直接写 `icons/<name>.svg`；资源包的单个测试会校验所有声明图标和嵌入文件。

技术栈：Zed `gpui`、`terminal`、`task`（UI、PTY、终端模拟和 shell 进程）；Crossh 本地裁剪的 `terminal_view` 基础（渲染和交互）以及薄宿主（生命周期、焦点和工作区边界）；`tokio`（2 worker 常驻）；`crossh-note`（`rusqlite` bundled + SQLite WAL + FTS5 + 触发器同步，见 `crates/crossh-note/src/lib.rs`）提供本地笔记持久化与全文检索，`crossh-git` / `crossh-note` / `crossh-updater` 均为独立二进制。`alacritty_terminal` / `vte` 由 Zed `terminal` 间接使用，Crossh 不再直接维护另一套生产终端实现。
## 路线图

终端能力已冻结：后续终端变更以 bug 修复驱动，不再规划新协议能力。已实现的规划外补强：设置面板、i18n。
未落地的 stretch 项（均与终端协议无关）：

- 标签拖拽排序
- 多窗口、浅色主题

## 许可证与致谢

GPL-3.0-or-later。交互式终端使用 Zed 的 `terminal` 基础设施，并将 Zed `terminal_view` 的必要 `TerminalElement` 与 APCA 辅助代码按 `Cargo.toml` 中的固定 revision 本地裁剪维护，以避免引入编辑器、LSP 和工作区应用层依赖；Crossh 自有代码只提供薄宿主和工作区适配。具体依赖、源代码归属和许可见 [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md)。UI 图标为 [Lucide](https://lucide.dev/) 1.27.0 官方 SVG；应用图标为自绘。
