# 0010-Git workbench layering

## 状态

已接受

## 背景

`crossh-git` 已经作为独立进程承载 Git Viewer，但窗口实体此前同时保存 Git
扫描结果、选择状态、异步请求 generation、Git 操作状态和 GPUI 布局状态。继续在
这个实体中加入分支、历史、Stash 和冲突处理会让 Git 语义与 GPUI 生命周期重新耦合。

工作区状态栏还需要轻量的 Git 状态扫描和 push/pull 快捷操作。它与 Git Viewer
位于不同进程，不应通过进程通信共享内存；两者应共享纯数据类型和解析规则。

## 决策

Git 相关代码按三层演进：

1. `crossh-core::git` 和 `crossh-core::git_status` 负责 Git 命令、协议解析和
   UI 无关的数据结构，不依赖 GPUI。
2. `src/features/git/session.rs` 负责 Git Viewer 的纯会话状态、请求上下文和
   状态转换，不保存 GPUI entity、window、focus 或 scroll handle。
3. `src/features/git/window.rs` 负责 GPUI 生命周期与后台任务适配，
   `render.rs` 和 `input.rs` 负责视图与输入。
4. `crossh-ui-component` 负责跨 feature 复用的无状态 GPUI 外壳，包括
   `TabStrip`、`TabItem`、`StatusBar`、`StatusMetric` 和 `Badge`；workspace 与 Git Viewer 只提供
   各自的状态、内容和回调，不把 Git 语义放进组件层。

UI 事件应先转成 Git 会话动作，由 GPUI 适配层调度 `crossh-core` 操作，再把结果
应用回会话状态。工作区可以继续独立刷新轻量状态和执行快捷同步，不建立应用层 IPC。

## 结果/代价

Git 状态转换可以在无窗口的测试中验证，Git Viewer 的渲染重构不会改变 Git 协议
实现，文件级、Hunk 级暂存/取消暂存、History 提交列表/详情、Branch 列表与
checkout、Stash 列表生命周期以及冲突解决动作也沿用同一边界。代价是每
种异步 Git 操作都需要显式定义请求、generation 和结果应用路径；跨进程状态最终
一致仍依赖各自的刷新机制。共享 Tab 与状态栏外壳可以让独立进程保持统一的
Crossh 视觉语言，同时不引入进程通信或 feature 间状态依赖。

## 关联规则

- `docs/adr/0002-logic-ui-layering.md`
- `docs/adr/0008-standalone-git-viewer.md`
- `docs/engineering-notes/gpui-polling-performance.md`
