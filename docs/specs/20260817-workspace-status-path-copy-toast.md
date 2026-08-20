# 应用内 Toaster 通知系统与路径复制反馈

## 元数据

- 状态：`done`（2026-08-21 文档漂移审计：Toaster 基础设施已完成，`docs/testing.md` Workspace/Toaster 已收录）
- 创建：2026-08-17
- 相关 ADR：`docs/adr/0002-logic-ui-layering.md`、`docs/adr/0010-git-workbench-layering.md`、`docs/adr/0012-spec-driven-development-loop.md`
- 相关 spec：`docs/specs/20260817-workspace-status-path-copy.md`
- 相关 issue / 路线图项：无
- CI 平台影响：`全部`

## 背景

路径复制已经接入底部状态栏，但当前通过 tooltip 说明“点击复制”。tooltip 是悬停时的被动说明，用户完成复制后没有明确的操作结果反馈；在路径较长、鼠标移动或窗口较窄时，这种提示也不适合作为复制成功的确认。

本次变更借此建立主应用窗口级、可复用的 Toaster 通知系统。Toaster 的通知状态和生命周期由主窗口 `AppShell` 作为应用组合根持有，跨 feature 的 UI 只提交通知，不重复实现计时和覆盖层布局；共享 UI crate 只负责无状态的 Toast 外观。路径复制是第一个接入方，未来设置、Git 同步和其他工作区功能可以复用同一个入口。原路径复制 spec 中关于完整复制、会话同步和响应式布局的契约继续有效；其中“路径复制 tooltip”契约由本 spec 取代。

## 目标

1. 主应用窗口提供一个跨 feature 可复用的 Toaster 提交入口；调用方只提供通知文案和语气，不直接管理 Toast 的渲染、计时或清理。
2. Toaster 至少支持 `Info`、`Success`、`Warning`、`Error` 四种通知语气，并使用现有主题色保持稳定区分；本次路径复制使用 `Success`。
3. 用户点击本地状态栏路径并发起剪贴板写入后，在该点击处理触发的下一次工作区重绘中，通过 Toaster 看到当前 locale 的“路径已复制”反馈。
4. Toaster 使用当前 locale 提供的简短文案，不改变状态栏和终端区域的布局尺寸。
5. Toast 为单实例短时反馈：重复提交时更新同一个 Toast 并重新计时，不堆叠、不排队。
6. 路径区域继续通过指针光标和 hover 背景表达可点击性，但不再依赖路径专用 tooltip 解释复制动作。

## 非目标

- 不新增系统级通知、通知中心、通知历史或用户可配置的通知设置。
- 不为复制失败增加应用级重试流程；当前 GPUI 剪贴板写入接口没有返回失败结果，仍沿用现有平台边界。
- 不改变完整 `cwd` 复制、路径截断显示、远程视图不显示本地路径等原路径复制契约。
- 不在本次变更中接入 Git 状态、终端 BEL、更新结果或其他业务事件；它们只获得未来复用 Toaster 的能力，不改变现有通知路径。
- 不增加独立复制按钮、键盘快捷键或持久化字段。
- 不实现 Toast 队列、同时堆叠、多窗口广播、跨进程共享或带操作按钮的通知。
- 不把现有用于退出流程的 `AppShell.status` 状态标签迁移为 Toast；它仍表示工作区关闭中的持久状态。

## 行为契约

1. 当主应用内任一受支持的 feature 提交一条 Toast 通知时，当前主窗口应在下一次重绘中显示该通知；提交方不需要拥有或操作 Toaster 的内部状态。
2. 当通知语气分别为 `Info`、`Success`、`Warning`、`Error` 时，Toast 应使用对应的稳定视觉语气；未知语气不能绕过 Toaster 直接渲染自定义通知。
3. 当活动视图是本地会话且用户点击状态栏路径时，系统剪贴板得到该会话当前 `cwd` 的完整文本值，并通过 Toaster 显示一条 `Success` 语气的“路径已复制”反馈。
4. 当活动视图不是本地会话或状态栏没有路径区域时，不应显示该路径复制 Toast，也不应因该视图触发路径复制。
5. 路径复制 Toast 应使用当前 locale 的本地化短文案：英文表达为 `Path copied`，简体中文表达为 `路径已复制`；文案不包含完整路径，避免在小提示中重复占用空间。
6. Toast 应位于工作区底部、状态栏上方的应用内覆盖层中；它不参与正常布局流，不改变终端、状态栏或其他控件的尺寸和位置，也不应遮挡状态栏交互区域。Toast 本身不是交互控件，不获得键盘焦点，不拦截工作区的鼠标和键盘操作。
7. 每条 Toast 默认显示 2 秒后自动消失；消失后不保留空白占位，也不影响后续点击和键盘操作。
8. 当 Toast 已显示时再次提交通知，应更新为同一个最新 Toast 并重新开始 2 秒的显示周期；界面上最多同时存在一条 Toast。
9. 当旧 Toast 的到期任务晚于一次新的通知提交时，旧任务不得提前清除新 Toast；只有当前 Toast 的到期事件可以清除当前反馈。
10. 路径区域 hover 时仍应显示指针光标和现有 hover 背景；hover 本身不显示复制说明 tooltip，其他状态栏控件的既有 tooltip 不受影响。

## 边界与错误

- 路径包含非 UTF-8 字节时，剪贴板和 toast 流程沿用现有 `to_string_lossy` 文本化行为，不因无法无损表示而 panic。
- 剪贴板写入请求由 GPUI 平台实现承接；由于调用没有失败返回值，toast 表示“已发起复制请求”，不声称完成了平台级验证。
- 快速连续点击、切换本地会话和路径更新交错时，每次点击仍复制点击发生时渲染的完整 `cwd`；toast 只反馈最近一次复制动作，不携带可能过期的路径文本。
- 主窗口关闭或 Toaster 承载状态销毁时，相关短时任务应随承载状态释放，不得在窗口销毁后更新 UI。
- 窗口处于声明的最小尺寸时，toast 文案仍应完整显示；必要时 toast 宽度应受可用工作区宽度约束，不得让文字溢出或挤压状态栏控件。
- 设置窗口等辅助窗口提交的通知应路由到主窗口 Toaster；辅助窗口关闭后，已经提交的 Toast 仍由主窗口按自身生命周期处理。
- 独立 `crossh-git` 进程不读取主窗口 Toaster 状态；若未来需要通知，由该进程独立装配同一套无状态 UI 原语和本地 Toaster owner。

## 接口与状态变更

- 无对外公开 API、设置项、持久化格式或 wire 格式变更；增加应用 crate 内供 feature 使用的内部 Toaster 提交契约。
- 增加主窗口级 Toaster 状态和到期生命周期；状态只在内存中存在，由 `AppShell` 这个应用组合根持有，不注册为 GPUI 全局状态。
- 移除路径区域专用 tooltip 文案；状态栏其他 tooltip 和上下文菜单中的复制路径文案保持不变。
- `crossh-ui-component` 增加无状态 Toast/Toaster 视觉原语；它只消费通知快照和主题，不持有业务消息、计时任务或 feature 回调。
- 路径复制、设置窗口和未来 feature 通过主窗口通知入口提交消息；通知系统不反向依赖任何具体业务 feature。

## 平台影响

- macOS、Linux、Windows 的主应用窗口均显示相同的应用内 Toast，并继续使用各平台的 GPUI 剪贴板实现；独立二进制不与主进程共享通知状态。
- 本地只验证 macOS arm64 和平台无关状态逻辑；Linux/Windows 的构建与平台行为由 CI 的 `terminal-compat` job 验证，不在本机交叉编译或运行目标平台检查。

## 涉及纪律

- [x] Logic must not depend on UI（层级）：剪贴板调用和 toast 渲染留在工作区 GPUI 边界；若抽取状态模型，保持其不依赖 GPUI。
- [ ] Feature-owned settings：无设置变更。
- [x] 图标纪律：不新增图标资产；toast 如需状态图标只复用现有 Lucide 映射，不修改 `assets/icons/`。
- [x] 文件规模 < 2000 行：保持工作区视图、壳层和 UI 组件文件在架构脚本限制内。
- [x] 工程笔记 / ADR 同步义务：本次明确跨 feature 的通知 owner 和 UI component 边界；实施收尾时评估是否需要新增 ADR，并同步 `docs/architecture.md`。
- [x] 响应式 UI（最小窗口尺寸可用性）：toast 是覆盖层，不改变布局流，并声明最小宽度下的文字约束与状态栏避让。

## 影响模块

- `src/features/workspace/shell.rs`：作为主窗口组合根持有 Toaster、通知提交入口、重复触发和到期清理状态。
- `src/features/workspace/view.rs`：移除路径 tooltip，触发路径复制通知，并挂载主窗口 Toast 覆盖层。
- `src/features/settings/window.rs`：保留辅助窗口通过弱引用向主窗口提交通知的接入边界（如本次需要覆盖该路径）。
- `crates/crossh-ui-component/`：增加可复用、无状态的 Toast/Toaster 视觉原语；不承载业务消息和计时。
- `locales/en.yml`、`locales/zh-CN.yml`：增加复制成功 Toast 文案并移除路径专用 tooltip 文案。
- `docs/architecture.md`、必要时新增 `docs/adr/` 记录：同步 Toaster owner 与 UI component 边界。
- `docs/testing.md`：补充通用通知提交、语气映射、重复触发和过期任务乱序行为矩阵。

## 验收清单

- [x] spec 评审通过（AI 评审 + 人批准）
- [x] 行为契约全部固化为失败测试并确认失败原因正确（Red）
- [x] 最小实现通过聚焦测试（Green）
- [x] `cargo fmt --check`
- [x] `scripts/check-architecture.sh`
- [x] `cargo clippy --all-targets -- -D warnings`
- [x] `cargo test --workspace`
- [x] 声明的平台 CI job 通过（本地 `cargo test --workspace` 绿，三平台由 Actions 覆盖）
- [x] 结构性决策提炼进 ADR（如有）并登记 `docs/architecture.md`
- [x] 调试根因合并进 `docs/engineering-notes/`（本次无新增工程根因）
- [x] 新增行为合并进 `docs/testing.md` 关键行为矩阵（如有）
- [x] 用户可观察效果人工确认（本地 macOS 已确认：路径复制显示 `Success` Toast）
