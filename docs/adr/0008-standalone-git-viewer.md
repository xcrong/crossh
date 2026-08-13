# 0008-Standalone Git Viewer binary

## 状态

已接受

## 背景

`crossh git` 只需要 Git Viewer，但此前会启动完整 `crossh` GUI 进程，再初始化 SSH、终端、agent、workspace 和设置功能。这样增加了冷启动开销，也让 Git CLI 与完整应用生命周期耦合。

## 决策

Cargo package 同时产出 `crossh` 和 `crossh-git` 两个二进制。`crossh git [DIRECTORY]` 负责解析目录并启动同目录的 `crossh-git`，找不到时回退到系统 `PATH`。`crossh-git` 只初始化 GPUI、主题、资源和 Git Viewer；Git Viewer 的窗口、渲染、输入和模型源码保持单一实现。

各平台发布脚本将二进制放在同一安装目录或 bundle 内，并将共享字体、图标和必要资源放入同级 `crossh-assets` 目录，使发布产物不依赖外部 `PATH` 配置。debug 构建保留嵌入回退，release 构建从外置资源目录读取。本地只负责 macOS arm64 的构建和验证；Linux、Windows 以及 macOS x86_64 由 GitHub Actions 构建和校验，不在本地安装对应工具链或执行交叉编译。

## 结果/代价

Git Viewer 的启动不再加载完整 Crossh 功能，二进制也可以独立测量和优化。代价是每个平台发布包需要携带第二个可执行文件，且主程序与 `crossh-git` 必须保持兼容版本。
