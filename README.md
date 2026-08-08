# crossh

基于 [gpui](https://github.com/zed-industries/zed/tree/main/crates/gpui) 的轻量 SSH 客户端（macOS / Linux / Windows），
以 `~/.ssh/config` 为唯一真源，常驻开发工具定位：低内存、多标签、开箱即用。

## 特性

- **读取 `~/.ssh/config`（只读）**：别名列表、`Include`/通配/`Match`、`ProxyJump`、`Local/Remote/DynamicForward`、`IdentityFile`/`IdentitiesOnly` 均可解析。
- **会话池复用**：同主机共享一条已认证连接，多标签/SFTP/转发不重复认证；全部标签关闭后自动断开。
- **交互式终端**：russh + alacritty_terminal + vte，256 色/真彩、宽字符、鼠标协议、IME 输入法、文本选择、URL 点击跳转。
- **反应式认证**：未知主机密钥弹指纹确认（可写 `known_hosts`）；加密私钥口令、密码按需弹出，凭据不回写日志。
- **SFTP**：远程浏览、上传/下载、目录递归、进度条、覆盖确认。
- **端口转发**：`-L` / `-R` / `-D`(SOCKS5)，config 驱动，UI 启停。
- **本地终端**：项目目录视图（OSC 7 追踪当前 `cwd`，更新状态栏和 Git 状态；会话归属不随 `cd` 改变）、多会话标签。
- **设置**：语言（zh/en）、终端字号、时间戳 gutter、滚动回退行数，持久化到 `~/.config/crossh/`。
- 常驻友好：日志裁剪（`/tmp/crossh/run.log`）、panic 现场保留、空闲内存 ~70MB。

## 构建与运行

```bash
# 需要 Xcode Command Line Tools + Rust (edition 2024)
cargo run            # 开发模式（日志同时 tee 到 stderr）
cargo run --release  # 发布模式
```

打包为 `.app`（含 ad-hoc 签名，无需开发者账号）：

```bash
scripts/package.sh          # 本机架构，输出 dist/crossh.app 与 dist/crossh-<version>-<arch>-macos.zip
scripts/package.sh x86_64-apple-darwin   # 指定架构（交叉编译）
open dist/crossh.app
```

三平台发布产物由 [.github/workflows/release.yml](.github/workflows/release.yml) 构建：macOS `.app` zip（aarch64/x86_64）、Linux `tar.gz` + AppImage（x86_64/aarch64）、Windows zip（x86_64）。

## 快捷键

| 快捷键 | 功能 |
| --- | --- |
| `Cmd/Ctrl+T` | 新终端标签（复制当前标签的目标） |
| `Cmd/Ctrl+W` | 关闭当前标签 |
| `Cmd/Ctrl+Tab` / `Cmd/Ctrl+Shift+Tab` | 切换标签 |
| `Cmd/Ctrl+1..9` | 跳到第 N 个标签 |
| 侧栏搜索框回车 | 打开主机 / 快速连接 `user@host` |

侧栏搜索支持关键词：`local` / `project`（或中文 `本地` / `项目`）直达目录视图与目录选择器。

## 架构

```
src/
  main.rs                    入口编排：窗口、快捷键、启动顺序
  infrastructure/
    config/                  ~/.ssh/config 解析（只读）
    logging.rs                日志、panic hook、日志裁剪
    ssh/
      connection.rs          无 UI 的连接引擎与 channel 多路复用
      pool.rs                连接键推导
      session.rs             认证候选与会话协议
      sftp.rs                SFTP worker
      forward.rs             -L / -R / -D 转发
      proxyjump.rs           单层 ProxyJump
  features/
    connections/             GPUI 连接实体、连接管理与认证提示
    terminal/                终端状态、输入编码、图片协议与渲染
    sftp/                    SFTP 面板、远程编辑器与路径逻辑
    forwarding/              端口转发面板
    workspace/               外壳、侧栏、标签和 Pane 抽象
    settings/                设置窗口与持久化编排
    commands/                本地/远程命令历史与后台任务
  shared/
    terminal/                UI 无关的终端事件、协议和 shell 逻辑
    ui/                      GPUI 主题、图标、菜单和通用控件
```

依赖方向保持单向：`infrastructure` 和 `shared/terminal` 不依赖 GPUI；feature 的 GPUI 适配层依赖基础设施；`workspace` 通过 `WorkspacePane` trait 消费终端、SFTP 和转发面板。可重复执行的分层检查位于 `scripts/check-architecture.sh`。

技术栈：`gpui`（UI，钉 zed git rev）、`russh`（SSH）、`alacritty_terminal` + `vte`（终端模拟）、`tokio`（2 worker 常驻）。

## 路线图

见 [ROADMAP.md](ROADMAP.md) 和 [.kilo/plans/](.kilo/plans/)。已实现的规划外补强：设置面板、i18n、时间戳 gutter、连接池生命周期。
未落地的 stretch 项：

- SFTP 拖拽上传 / 批量 / 断点续传
- 标签拖拽排序
- config 编辑 UI、自动重连
- 多层 ProxyJump、`Match exec` / `ProxyCommand` / GSSAPI
- 多窗口、浅色主题

## 许可证与致谢

MIT。UI 图标为 [Lucide](https://lucide.dev/) 1.27.0 官方 SVG（见 [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md)）；应用图标为自绘。gpui 来自 Zed 开源项目。
