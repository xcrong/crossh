# 0007-workspace-panel-composition

## 状态

已接受

## 背景

主机侧栏、主区和快捷命令栏共同占用 workspace 的横向空间。快捷命令栏此前由
`render_main` 作为主区内容的子元素渲染，导致其尺寸和边界在概念上依附于终端/SFTP
内容区，而不是与主机侧栏同级的工作区面板。

快捷命令的 scope 和 cwd 同时依赖当前活动的本地会话或远端标签。无活动上下文时，
该面板不应出现；展开与 rail 收起态必须继续使用同一上下文和现有宽度拖拽状态。

## 决策

`AppShell::render` 是 workspace 的面板组合 owner，负责在横向布局中按
`sidebar | main | quick-commands` 的顺序渲染三个同级区域。它在渲染快捷命令面板时
解析当前活动视图的 scope/cwd；没有可用命令上下文时不添加第三个面板。

`features/workspace/view.rs` 继续拥有快捷命令展开态和 rail 的具体渲染函数，以及
命令历史、编辑和后台任务交互。`render_main` 仅组合标签条、活动内容区和状态提示，
不再决定快捷命令面板的存在或位置。

## 结果/代价

快捷命令栏与主机侧栏具有一致的 workspace 级布局语义，终端/SFTP 内容区不再因面板
归属而承担额外横向组合责任。面板选择逻辑位于 shell，需要保持其与快捷命令执行时
使用的活动上下文语义一致；具体交互仍由 view 层维护，避免 shell 演变为面板实现层。

## 关联规则

- `AGENTS.md` 的 Split vertical features, then split logic and view inside each
- `docs/architecture.md` 的 Crate Ownership
- `src/features/workspace/shell.rs`
- `src/features/workspace/view.rs`
