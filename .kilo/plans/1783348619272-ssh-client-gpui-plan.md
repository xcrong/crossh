# crossh —— 基于 gpui 的轻量 SSH 客户端（实现方案）

## 目标
用 gpui + Rust 构建一个常驻型、低内存的桌面 SSH 客户端，定位为"开发连接管家"。

## 约束（硬性）
- 桌面专用（macOS 优先，基于现有 `crossh` 工程，gpui rev `1d217ee`）。已移除 web/wasm 支持，不恢复。
- `~/.ssh/config` 是**唯一真源**且**只读**：主机、端口转发规则都来自它，添加/修改必须用户手动编辑该文件，App 不写 config、不存自有主机库。
- 常驻低内存：断开的主机不占资源；限制后台线程数；滚动历史有上限；传输流式不整缓存。
- 自研组件，不引入第三方 UI 组件库。

## 已敲定的决策（逐项确认）
1. **交互式终端**：进第一期（live shell）。
2. **终端状态模型**：`alacritty_terminal`（借用其 `Term`/网格/cell/ANSI/alt-screen/鼠标模式），仅自研 gpui 渲染器。
3. **SSH 传输**：`russh`（纯 Rust/async/tokio）+ `russh-sftp`；另起独立 tokio 运行时，channel 桥接到 gpui 主线程。
4. **连接模型**：每主机单一已认证会话，终端/SFTP/转发多路复用 channel；只要有任意 channel 存活（如转发）会话即保活，全部关闭才断开。
5. **ssh config 解析**：常用子集（`Host` 通配匹配 + `Include` + `ProxyJump`；`HostName/User/Port/IdentityFile/LocalForward/RemoteForward/DynamicForward`）。优先试 `ssh-config` crate，不可用则手写行解析。**不支持** `Match exec` / `ProxyCommand`。
6. **端口转发来源**：纯 config 驱动（读 `LocalForward/RemoteForward/DynamicForward`），UI 仅启用/停用开关。
7. **SFTP 范围**：浏览 + 上传 + 下载（含目录递归、拖拽、进度、覆盖确认）。
8. **认证方式**：ssh-agent(`SSH_AUTH_SOCK`) + IdentityFile（含口令输入框）+ password/keyboard-interactive（含密码输入框）。跳过 GSSAPI/智能卡。
9. **状态持久化**：无状态。仅持久化窗口几何；启动空会话，连接/转发都手动点；不自动重连。

## 架构与模块划分
```
Cargo.toml  新增: russh, russh-sftp, russh-keys, alacritty_terminal(固定版本),
              tokio(rt-multi-thread,macros), ssh-config(可选), anyhow, serde
src/
  main.rs              App 入口；窗口；cx.init_colors()
  app_view.rs          根视图：左主机列表 + 右标签工作区
  config/
    ssh_config.rs      解析 ~/.ssh/config（子集）+ host 匹配解析（含 Include/ProxyJump）
  ssh/
    runtime.rs         单例 tokio Runtime(限定 worker 线程) + 与 gpui 的 channel 桥接
    session.rs         Connection(gpui Entity)：russh Handle + channel 注册表 + 生命周期
    auth.rs            认证方式选择与回调（agent/密钥口令/密码）
    host_key.rs        读 ~/.ssh/known_hosts 校验；未知/变更时弹接受(once/always)确认
    terminal.rs        开 PTY channel：russh 字节 ↔ alacritty Term
    sftp.rs            russh-sftp channel：浏览/上传/下载（流式）
    forward.rs         Local(-L)/Remote(-R)/Dynamic(-D SOCKS5) 转发
  ui/
    sidebar.rs         主机列表（搜索/分组/连接状态徽标）
    workspace.rs       标签页容器（终端/SFTP/转发面板）
    terminal_pane.rs   gpui 终端渲染器（自研 Element）
    sftp_pane.rs       远程目录浏览 + 传输列表/进度/拖拽
    forward_pane.rs    转发规则开关列表
    button.rs          （现有，复用）
```

## 数据流
- **终端（出方向）**：gpui 键盘事件 → 命令 channel → tokio 任务 → `russh channel.data()`。
- **终端（入方向）**：russh `channel.data()` 字节 → 无界小缓冲 channel → gpui Entity 主线程 `Term.feed()` → `cx.notify()` 触发重绘。**`Term` 只在 gpui 主线程持有与触碰**，避免跨线程。
- **尺寸同步**：面板尺寸变化 → 计算行列(用等宽字体 metrics) → 发 `window_change(rows,cols,pixels)` 到远端 PTY。
- **SFTP**：动作(列表/上传/下载)经 channel → tokio 任务执行 russh-sftp；进度按 chunk 回报；Entity 更新进度 UI。
- **端口转发**：开关经 channel → tokio 任务启停 listener；-L: 本地 listener→`channel_open_direct_tcpip`；-R: `tcpip_forward` 请求 + 监听 `ForwardedTcpIp`；-D: 本地 SOCKS5 server。

## 线程与异步桥接（关键、影响内存）
- 启动时建一个 `tokio::runtime::Runtime`（`max_blocking_threads` 限到 2–4），存为全局，全程复用；不为每个连接新开 runtime。
- gpui Entity 持有 russh `client::Handle`（`Send`）。UI→后台用 `tokio::sync::mpsc`；后台→UI 用 channel + Entity 在 gpui 主线程 drain + `cx.notify()`。
- 密码/口令输入：UI 弹模态框，结果一次性送回 tokio 任务的 oneshot channel；不在日志中打印，用后清零。

## 终端渲染（自研 gpui Element，最高风险点）
- 自定义 `Element`：遍历 `Term` 可见视口，按行聚合同属性文本为少量 run，用 `window.paint_*` 批量绘制；选区/光标单绘。**不要每 cell 一个 div**（性能/内存不可接受）。
- 等宽字体：内嵌一个轻量 monospace ttf（如 DejaVu Sans Mono 子集或更轻的），避免平台字体依赖与排版的非等宽抖动。
- 滚动历史默认 1000 行（可配，alacritty_terminal 配置）。
- 先用最简 Element 跑通交互，再用大输出（`yes`/`cat 大文件`/`htop`）profile，按需做脏矩形/run 缓存。

## 失败模式与处理
- 认证失败：明确报错（区分密钥缺失/口令错/被拒），不静默重试刷屏。
- 网络断开：会话标错，释放 channel，UI 提示；转发 listener 关闭。
- 转发端口占用：报"端口被占"，不自动换端口。
- SFTP 覆盖：弹确认；传输错误保留已完成进度，可重试续传（第一期可做整文件重传，续传作为后续）。
- 主机密钥未知/变更：阻塞连接，弹指纹确认（接受一次/总是/拒绝）；"总是"写入 `~/.ssh/known_hosts`。

## 风险 / 待验证（需实现期先做最小验证）
- **alacritty_terminal API**：跨版本 `Term` 构造与 feed API 有变动 → 先固定一个版本并在 spike 里跑通 `feed(bytes)` + 读 grid 渲染。
- **russh ProxyJump**：跳板机需"channel 上再跑 russh client"，russh 是否便于嵌套待验证 → 第一期可先只支持直连 + 单层 ProxyJump；多层降级提示。
- **gpui 终端渲染性能**：大量 cell/帧 → spike 验证后再优化。
- **ssh-config crate**：匹配保真度/维护性 → 不可用即手写（行格式简单）。

## 实施任务清单（建议顺序）
1. 依赖与脚手架：加依赖；建模块骨架；tokio 单例 runtime + 与 gpui 的 channel 桥接 spike。
2. config 解析器（含单元测试：通配/Include/ProxyJump/转发指令）。
3. 认证 + host_key 校验 + 单会话连接打通（连真实主机跑通 PTY shell，先输出到日志）。
4. 终端 spike：`alacritty_terminal::Term.feed()` + 自研 gpui Element 渲染可见视口；接通 stdin；resize 同步。
5. 多路复用：单会话上多 channel 并存；保活/断开生命周期与状态徽标。
6. SFTP：浏览 + 上传/下载（含目录、进度、覆盖确认、拖拽）。
7. 端口转发：-L / -R / -D，config 驱动开关。
8. UI 整合：sidebar 主机列表 + 标签工作区 + 转发面板 + 密码/口令/主机密钥弹窗。
9. 资源调优：限制 worker 线程、滚动上限、传输流式、空闲无会话；测量空闲内存基线。

## 验证计划
- config 解析：样例配置单元测试（含 `Host *` 通配、`Include`、`ProxyJump`、多条 `LocalForward`）。
- 连真实测试主机（或本地 docker openssh-server）：终端回显、`vim`/`htop` 渲染、resize、断开/重连。
- 认证三路径：agent、带口令密钥、密码。
- SFTP：上传/下载单文件、目录、大文件进度、覆盖确认。
- 转发：`-L` 转发到远端服务、`-R`、`-D` 用 curl 走 SOCKS5。
- 主机密钥：未知主机弹确认；known_hosts 写入后再连无提示。
- 内存：空闲（无连接）与单连接单终端的常驻内存基线记录。

## 明确不在第一期范围
- `Match exec` / `ProxyCommand` / 多层 ProxyJump / GSSAPI / 智能卡。
- config 编辑 UI、App 自有主机库、连接/转发状态持久化与自动重连。
- 本地终端（local shell）、本地 PTY（`portable-pty` 仅在后续支持本地 shell 时引入）。
- 终端图像协议（Sixel/iTerm）、连字。
- 多窗口（单窗口多标签即可）。
