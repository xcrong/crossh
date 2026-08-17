# 终端分屏与系统通知:窗口激活链路

## 症状

一个 Tab 内分屏(左 X / 右 Y),在右终端 Y 运行 Codex 编码代理,切到其他 tab/应用;Codex 完成弹出通知,点击通知"跳回"后右终端独占全屏、左终端被盖住、看起来只剩一个 tab;关闭右终端后左终端才露出。

## 已确认的事实

0. **运行环境**:用户日常运行 `/Applications/crossh.app` bundle(0.16.1),启动日志无
   "notifications disabled" → **系统通知启用**;日志中 disabled 记录均来自 cargo run 调试实例。
   git 0.16.1 之后的 HEAD(628b643 toast)不含分屏/通知逻辑改动,分屏归属重构 dbb71ff 已在 0.16.1。
1. **crossh 的 bell 系统通知只在 bundle 版启用**:`/tmp/crossh/run.log` 中非 bundle 启动均记录
   `gpui_macos::system_notifications] system notifications disabled`。
   因此用户点击的"完成通知"就是 crossh 的 bell 通知,`AppShell::handle_system_notification_response`
   是该场景主线。
2. **Codex 完成通知来源**:`~/.codex/config.toml` 配置了 notify hook
   `[".../SkyComputerUseClient", "turn-ended"]`("Codex Computer Use" 客户端,arm64 二进制,
   strings 含 `com.apple.Terminal`/`terminal_status`,带辅助功能权限)。
3. **渲染契约**(`workspace/view.rs:118-131`):分屏只在 `active_view == split.left` 时渲染;
   `active_view` 是右窗格时,该窗格全屏渲染,分屏状态保留,另一侧被盖住——与症状"关闭右终端后左终端露出"吻合。
4. **窗口失活确实派发 focus_out**(修正早前错误结论):
   macOS `windowDidResignKey` → `gpui_macos/src/window.rs` `on_active_status_change` 回调
   (`gpui/src/window.rs:1713-1733`,`active.set(active)` + `refresh()`)→ 下一帧绘制时
   (`window.rs:2882-2919`)`window_active` 翻转,`previous_focus_path` 非空而 `current` 为空,
   `is_focus_out` 为 true → `TerminalView` 的 `on_focus_out` 触发、`focused=false`。
   之前"窗口失活不派发 focus 事件"的结论是错的,依据是只看了 `window_did_change_key_status`(a11y 路径),
   漏了 `on_active_status_change` 这条真正更新 `window.active` 的路径。
5. **根因(已确认)**:用户稳定复现路径是 —— 本地会话建分屏 X 左/Y 右 → Y 跑 Codex
   发任务 → **立即切到主机栏的另一个会话 tab**(active_view 离开 split owner)→ 切到浏览器
   → 点击通知。通知点击瞬间 `active_view` 是**另一个 tab 的会话 Z**,不是 split owner X;
   `focus_split_view(Y)`(`registry.rs:155-164`)的唯一失败条件 `active_view != split.left`
   恰好成立 → fallback `active_view = Some(Y)` → 渲染契约下 Y 全屏、X 存活被盖住、
   标签栏因 `is_split_secondary` 隐藏 Y → 与现象完全吻合。
6. **修复** `jump_back_to_split_pane`(`workspace/notifications.rs`):fallback 时
   先查 `split_containing(view)`;有则 `active_view = split.left` + `focus_terminal_split(view 所在侧)`
   + `refocus_active_terminal`,保留分屏;无分屏才退回 `active_view = Some(view)`。

## 测试基础设施教训

- `cx.on_system_notification_response` 注册在 `open_main_window`(shell.rs,**不在** `AppShell::new`);
  用 `AppShell::new` 的旧测试从未注册回调,"全绿"是假象。通知测试必须走 `open_main_window`
  (需 `release_channel::init` + `settings::init` + `theme_settings::init(JustBase)` + `install_crossh_theme`)。
- PTY 线程确定性坑:背景 executor 会真实完成 PTY builder 并 spawn alacritty "PTY reader" 线程;
  线程读到输出时 `unbounded_send` 唤醒 foreground executor → 触发 test_scheduler 的
  "not deterministic" 守卫(窗口竞争,时稳时不稳)。local 终端(zsh 静默)安全;
  remote 终端 spawn `ssh test-host` 解析失败立即输出 → 必触发。
  **remote 测试不得用 `open_terminal_target`**,改为直接构造 remote 语义
  (`is_remote_terminal = true` + `task::Shell::WithArguments { program: "sleep", ... }`)的静默终端
  (见 `shell_notification_tests.rs::push_silent_remote_terminal`)。

## 关键词

分屏丢失, split, active_view, 右窗格全屏, Codex, 完成通知, SkyComputerUseClient,
windowDidResignKey, on_active_status_change, focus_out, system notifications disabled,
not running from an app bundle, jump_back_to_split_pane, PTY reader, test_scheduler
