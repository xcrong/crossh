# 0013-Application Toaster ownership

## 状态

已接受

## 背景

状态栏路径复制需要在操作完成后提供短时的应用内反馈。tooltip 只能表达悬停说明，不能稳定表示操作结果；未来设置、Git 同步和其他工作区功能也会需要同类反馈。

如果每个 feature 各自保存提示文本、计时任务和覆盖层布局，通知行为会分散且容易产生过期任务清理新提示的问题。另一方面，`crossh-ui-component` 的职责是提供无状态、可复用的 GPUI 外观，不应持有工作区业务状态。

## 决策

1. 主应用窗口的 `AppShell` 作为通知组合根，持有工作区级 `ToasterState` 和短时任务生命周期。feature 通过主窗口内部提交入口发送通知，不直接管理 Toast 的渲染或计时；设置窗口通过已有的 `WeakEntity<AppShell>` 路由通知。
2. `crossh-ui-component` 提供无状态的 `Toast` 和 `Toaster` 视觉原语，只消费通知快照和主题语气，不持有业务消息、任务或回调。
3. Toaster 采用单槽策略：最新通知替换当前通知，默认显示 2 秒，不建立队列或通知历史。每条通知获得单调递增标识，过期任务必须校验标识后才能清理，避免旧任务清除新通知。
4. Toaster 支持 `Info`、`Success`、`Warning`、`Error` 四种语气，并映射到现有主题色。路径复制使用 `Success`；通知文案由调用方按当前 locale 提供。
5. Toaster 不注册为 GPUI 全局状态，也不跨进程共享。独立的 `crossh-git` 进程如果未来需要反馈，应独立装配同一套无状态视觉原语和自己的 owner。

## 结果与代价

通知触发逻辑集中，未来 feature 可以复用同一入口并获得一致的布局、生命周期和竞态处理；共享组件仍保持无业务状态，符合 logic/UI 分层。主窗口是当前应用内通知的边界，辅助窗口需要持有或路由到主窗口；队列、操作按钮、跨窗口广播和跨进程通知需要另行设计。

## 关联

- `docs/specs/20260817-workspace-status-path-copy-toast.md`
- `docs/adr/0002-logic-ui-layering.md`
- `docs/adr/0010-git-workbench-layering.md`
