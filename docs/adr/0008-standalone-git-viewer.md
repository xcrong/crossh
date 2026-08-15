# 0008-Standalone Git Viewer binary

## 状态

已接受

## 背景

命令行 `crossh git` 和工作区状态栏的 Git 分支入口都只需要 Git Viewer，但此前状态栏入口会在完整 `crossh` GUI 进程中创建 Git 窗口。这样会让 Git Viewer 的 UI 代码和生命周期继续耦合到主工作区；命令行入口则需要避免初始化 SSH、终端、agent、workspace 和设置功能。

## 决策

Cargo package 同时产出 `crossh` 和 `crossh-git` 两个二进制。`crossh` 只保留轻量 Git 状态扫描、同步操作和启动器；`crossh git [DIRECTORY]` 与状态栏 Git 入口都负责传入目录并启动同目录的 `crossh-git`，找不到时回退到系统 `PATH`。`crossh-git` 只初始化 GPUI、主题、资源和 Git Viewer；Git Viewer 的窗口、渲染、输入和模型源码只在独立入口中装配。

启动是单向 fire-and-forget，不建立应用层 IPC。主工作区通过已有的周期性 Git 状态扫描自然看到子进程对仓库产生的变化。

各平台发布脚本将二进制放在同一安装目录或 bundle 内，并将共享字体、图标和必要资源放入同级 `crossh-assets` 目录，使发布产物不依赖外部 `PATH` 配置。debug 构建保留嵌入回退，release 构建从外置资源目录读取。本地只负责 macOS arm64 的构建和验证；Linux、Windows 以及 macOS x86_64 由 GitHub Actions 构建和校验，不在本地安装对应工具链或执行交叉编译。

## 结果/代价

Git Viewer 的启动不再加载完整 Crossh 功能，主程序也不再携带完整 Git UI，两个二进制可以独立测量和优化。代价是每个平台发布包需要携带第二个可执行文件，状态栏重复点击可能启动多个 Viewer 实例，且主程序与 `crossh-git` 必须保持兼容版本。
