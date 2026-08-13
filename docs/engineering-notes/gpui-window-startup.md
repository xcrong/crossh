# GPUI 窗口启动生命周期

## 症状

- 从已有工作区点击入口时窗口可以打开，但通过 CLI 冷启动时只有 Dock 图标，没有可见窗口。
- GUI 入口直接运行事件循环，导致终端命令一直阻塞，直到窗口关闭。
- 仅确认进程存活或 Dock 图标存在，容易误判为窗口已经成功创建。

## 根因

`cx.defer` 把操作安排到当前 `App` 更新结束之后。它适合从已有窗口的输入或渲染事件中安全地修改窗口列表，但不能无条件用于应用冷启动的第一个窗口：此时应用没有现存窗口维持正常的窗口生命周期，延迟任务可能无法产生用户可见窗口。

CLI 的另一个生命周期独立于 GPUI：如果终端进程本身进入 `Application::run`，命令自然会被 GUI 事件循环阻塞。要实现普通桌面 CLI 的启动语义，终端进程应只负责生成独立 GUI 子进程，然后立即返回。Git Viewer 使用同目录的 `crossh-git` 独立二进制，不再通过环境变量让完整 `crossh` 递归进入 Git 窗口路径。

## 稳定规则

1. 在应用启动回调中创建首个窗口时，直接调用 `cx.open_window`。
2. 只有在已经存在其他窗口、且调用来自当前 UI 事件时，才使用 `cx.defer` 延后创建新窗口。
3. CLI 启动 GUI 时，父进程生成独立 GUI 子进程后返回。`crossh git` 优先查找自身旁边的 `crossh-git`，再回退到 `PATH`；子进程断开标准输入、输出和错误流。
4. Unix 子进程使用独立进程组；Windows 使用 `CREATE_NEW_PROCESS_GROUP | DETACHED_PROCESS`。平台行为只能在对应平台或 CI 中声明为已验证。
5. 不要把“进程仍在运行”或“Dock 出现图标”当成窗口可见性的证据。

## 验证方法

- GPUI 回归测试应在同一次 `TestAppContext::update` 中调用开窗入口，并断言窗口数立即从 `0` 变为 `1`。这能捕获首窗口被错误延迟的问题。
- CLI 命令构造测试应验证子进程使用 `crossh-git`、传入目标目录、继承目标工作目录，并保持独立进程组。
- 手工验证需要同时满足：CLI 父进程以成功状态快速返回；返回后 GUI 子进程仍存活；系统窗口元数据表明窗口位于屏幕上。macOS 可使用 `CGWindowListCopyWindowInfo` 检查 `kCGWindowIsOnscreen`，无需截取用户屏幕内容。

## 当前实现

- CLI 进程分离：`src/features/git/cli.rs`；独立入口：`src/bin/crossh-git.rs`
- 共享资源发现：`crates/crossh-assets/src/lib.rs`；打包资源：`scripts/copy-shared-assets.sh`
- 启动路径装配：`src/main.rs`
- 首窗口同步创建和已有窗口延迟创建：`src/features/git/window.rs`

## 搜索关键词

`GPUI`、`App::run`、`open_window`、`cx.defer`、`cold start`、`Dock icon`、`invisible window`、`CLI blocking`、`detached process`、`process_group`、`CGWindowIsOnscreen`
