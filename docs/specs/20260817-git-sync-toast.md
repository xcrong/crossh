# Git 同步结果 Toaster 提示(Push / Pull)

## 元数据

- 状态:`in-progress`(实现与本地验证完成;三平台 CI 通过后转 `done`)
- 创建:2026-08-17
- 相关 ADR:`docs/adr/0002-logic-ui-layering.md`、`docs/adr/0010-git-workbench-layering.md`、`docs/adr/0012-spec-driven-development-loop.md`
- 相关 spec:`docs/specs/20260817-workspace-status-path-copy-toast.md`(Toaster 基础设施,已完成)
- 相关 issue / 路线图项:无
- CI 平台影响:`全部`

## 背景

状态栏的 Git Push / Pull 按钮完成操作后目前没有任何应用内反馈:成功时按钮静默消失(ahead 徽章清零),失败时错误只缩在按钮 tooltip 里,用户不容易注意到结果。Toaster 系统(`20260817-workspace-status-path-copy-toast.md`)已落地且明确把"Git 同步"列为未来复用方,本次变更把 Push / Pull 的完成结果接入 Toaster,与既有的单实例、2 秒生命周期、覆盖层渲染契约保持一致。

## 目标

1. Push / Pull 结束后给出用户可见、易感知的结果反馈,成功与失败语气可区分。
2. 完全复用既有 Toaster 提交入口 `AppShell::show_toast`(`toaster_view.rs`,负责计时与替换),不新增通知基础设施。
3. 错误细节仍保留在按钮 tooltip(既有契约),Toast 只表达结果语气,不承担可点击或长文本职责。

## 非目标

- 不改变 `run_git_sync` 的按钮状态机(running 态、error tooltip、ahead/behind 徽章全部保留)。
- 不为失败增加重试、跳转、系统通知或通知中心条目。
- 不改 `crossh-core::git::{push, pull}` 及 `GitError`(纯 UI 侧接入)。
- 不做按会话/多会话聚合的 toast(多会话并发完成时沿用单实例契约,最新者胜)。
- 不新增设置项、文案配置或持久化字段。
- 不为 status 刷新失败(如推送成功后 `refresh_git_status` 报错)单独发 toast。

## 行为契约

1. 当用户点击状态栏 Push 按钮且 `git push` 成功退出时,主窗口应通过 Toaster 显示一条 `Success` 语气的 Toast,文案为当前 locale 的"推送成功"/`Push succeeded`。
2. 当用户点击状态栏 Pull 按钮且 `git pull` 成功退出时,主窗口应通过 Toaster 显示一条 `Success` 语气的 Toast,文案为当前 locale 的"拉取成功"/`Pull succeeded`。
3. 当 Push / Pull 以非零状态退出或抛错时,主窗口应通过 Toaster 显示一条 `Error` 语气的 Toast,文案为当前 locale 的"推送失败"/`Push failed` 或"拉取失败"/`Pull failed`;按钮保持既有错误 tooltip(完整错误文本),两者并存。
4. Toast 显示与既有 Toaster 契约一致:单实例覆盖层,提交时替换并重置 2 秒周期,不参与布局流、不获得键盘焦点、不拦截鼠标与键盘操作。
5. 当 Push / Pull 仍在 running(按钮 disabled)时再次点击、包括点击另一个 operation 按钮,不会产生新的 toast;最终 toast 只反映第一次操作的结果——该差异仅在"Push 后立即 Pull"这类跨 operation 触发下可观察,由 GPUI 层测试覆盖。
6. 错误 Toast 与后续任何成功 Toast 互不残留:成功提交会替换错误 Toast(既有替换契约),到期任务只清除当前 id 的 Toast(既有 stale 守卫)。

## 边界与错误

- 推送前未配置 upstream 时 `git push -u origin HEAD` 失败:按契约 3 显示错误 Toast,按钮 tooltip 保留 `GitError` 文本。
- 操作完成时对应 session 已被关闭(状态条目缺失):结果分支直接返回,不产生 toast。
- 快速连点按钮:第二次点击发生在 running 期间,被既有守卫忽略,不产生第二个 toast。
- 推送成功后状态刷新(`refresh_git_status`)自身失败:不额外发 toast(非目标),不影响已提交的成功 Toast。
- Toaster 到期任务乱序:复用既有 stale-dismiss 守卫,旧任务不得清除新 Toast。
- 窗口最小尺寸下 Toast 文字完整显示:继承既有 Toaster 渲染约束,不新增布局改动。

## 接口与状态变更

- 无公开 API、设置项、持久化或 wire 格式变更。
- `AppShell::run_git_sync` 在成功/失败结果分支各一处调用 `AppShell::show_toast`(复用 `toaster_view.rs` 既有入口,由它负责 2 秒计时与单实例替换;不得直接调 `WorkspaceState.toaster.show`,否则 Toast 不会自动消失)。
- `locales/en.yml`、`locales/zh-CN.yml` 增加 4 条文案(推/拉 × 成功/失败),键为 `git.push_success`、`git.push_failed`、`git.pull_success`、`git.pull_failed`,与既有 `git.push` / `git.pull` 键风格对齐。

## 平台影响

- macOS、Linux、Windows 主窗口行为一致,均为应用内 Toast;涉及逻辑与渲染平台无关。
- 本地只验证 macOS arm64 与平台无关逻辑;三平台构建与测试由 CI `check`(macOS)与 `terminal-compat`(ubuntu-22.04 / windows-latest)job 验证,GPUI 层 git sync 测试会在三平台 CI 上执行,spec 保持 `in-progress` 直到 Actions 通过。

## 涉及纪律

- [x] Logic must not depend on UI(层级):toast 提交发生在工作区 GPUI 边界,`crossh-core::git` 保持不变。
- [ ] Feature-owned settings:无设置变更。
- [ ] 图标纪律:不新增图标资产。
- [x] 文件规模 < 2000 行:`shell.rs` 当前约 1942 行,本次仅在两处结果分支插入 show 调用,过线风险低;如接近线则把 git sync 状态机拆出。
- [x] 工程笔记 / ADR 同步义务:无新结构性决策,实施收尾不新增 ADR;不产生新的调试根因。
- [x] 响应式 UI:复用既有覆盖层渲染,不改变布局流。

## 影响模块

- `src/features/workspace/shell.rs`:`run_git_sync` 成功/失败结果分支接入 `AppShell::show_toast`。
- `src/features/workspace/toaster_view.rs`:无改动(提交入口 `show_toast` 复用)。
- `locales/en.yml`、`locales/zh-CN.yml`:新增 4 条 git 同步结果文案。
- `docs/testing.md`:补充 git 同步 toast 契约到行为矩阵(如已有 Toaster 条目,并入同一条目)。

## 验收清单

- [x] spec 评审通过(AI 评审 + 人批准)
- [x] 行为契约全部固化为失败测试并确认失败原因正确(Red)
- [x] 最小实现通过聚焦测试(Green)
- [x] `cargo fmt --check`
- [x] `scripts/check-architecture.sh`
- [x] `cargo clippy --all-targets -- -D warnings`
- [x] `cargo test --workspace`
- [ ] 声明的平台 CI job 通过(spec 状态保持 in-progress 直到通过)
- [x] 结构性决策提炼进 ADR(如有)并登记 `docs/architecture.md`
- [x] 调试根因合并进 `docs/engineering-notes/`(如有)
- [x] 新增行为合并进 `docs/testing.md` 关键行为矩阵(如有)
- [ ] 用户可观察效果人工确认:实际点一次 Push / Pull 看到对应 Toast