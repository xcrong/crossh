# 工程经验索引

这里记录已经确认、可复用的调试经验，目的是让相似问题能够通过症状或关键词快速定位。架构约束和长期决策仍然记录在 `docs/adr/`；这里不重复 ADR，也不记录未经验证的猜测。

使用方式：先搜索下表中的症状和关键词，只读取匹配的主题文档。解决新的非显然问题后，优先更新已有主题；只有当问题属于新的技术边界时才新增文件。

| 主题 | 典型症状 | 关键词 | 文档 |
| --- | --- | --- | --- |
| GPUI 首窗口与 CLI 生命周期 | Dock 有图标但没有窗口；GUI 命令阻塞终端；关闭隐藏窗口后命令才退出 | `GPUI`, `open_window`, `defer`, `Dock`, `CLI`, `detached process`, `cold start` | [GPUI 窗口启动生命周期](gpui-window-startup.md) |
| GPUI Flex 滚动容器 | 设置 `max_h` 和 `overflow_y_scroll` 后滚轮仍无效；长列表被压缩 | `GPUI`, `flex_shrink_0`, `overflow_y_scroll`, `max_h`, 滚轮无效 | [GPUI Flex 滚动容器](gpui-flex-scroll.md) |
| GPUI 轮询与大列表性能 | Git 视图静止时周期性卡顿；多终端持续启动 Git；长 diff 滚动变慢 | `GPUI`, `Git`, `polling`, `notify`, `UniformList`, `diff`, 卡顿 | [GPUI 轮询与大列表性能](gpui-polling-performance.md) |
| Git NUL 协议路径解析 | 含连续空格或 Tab 的文件无法暂存；重命名文件的变更计数为零 | `Git`, `porcelain v2`, `numstat`, `-z`, `NUL`, 文件名空格 | [Git NUL 协议路径](git-nul-paths.md) |
| SSH 连接生命周期与路径逃逸 | 释放连接后 CPU 100%；关标签后服务器会话残留；agent write 写到工作区外；主机密钥变更后接受按钮无效 | `SSH`, `busy loop`, `select!`, `WeakEntity`, `symlink`, `allow_missing`, `host key changed`, `known_hosts` | [SSH 连接生命周期与路径逃逸](ssh-lifecycle-and-path-escape.md) |
| Cargo 发布后 workspace 版本与锁文件不一致；tag 发布漏提交 `Cargo.lock` | `Cargo.lock`, `release.sh`, `cargo metadata`, 版本发布 | [Cargo 锁文件发布同步](cargo-lock-release-sync.md) |
| 命令历史测试只在 windows-latest 上断言 2 vs 3 | `commands.rs`, `aggregates_commands_and_returns_top_thirty`, `last_used`, 秒级时间戳, 时间竞态 | [命令历史测试的时间精度竞态](command-history-test-timing.md) |
| 跨终端 TUI splash 颜色对不上;白色刺眼、阴影过渡"细线"消失,但字节流一致 | `opencode`, `VT Code`, `OSC 11`, `38;2 真彩`, `canvas`, 自适应主题, `splashShadow`, `ColorRequest`, 256 色, 细线消失 | [终端自适应颜色与主题底色偏差](terminal-adaptive-colors.md) |
| CI/local 全绿但某 crate 测试从不执行；`--workspace --lib` 静默跳过 bin 测试 | `cargo test`, `--workspace`, `--lib`, 测试没跑, bin target, 静默跳过 | [Cargo 测试选择陷阱](cargo-test-workspace-selection.md) |
| serde_json Value 字段顺序随构建图变化；`-p` 通过但 `--workspace` 失败 | `serde_json`, `preserve_order`, `IndexMap`, 单包测试通过但 workspace 失败, canonical JSON | [serde_json preserve_order 与构建图](serde-json-preserve-order-workspace.md) |
| 分屏(左 X/右 Y)跑 Codex,切到另一 tab 再点完成通知跳回后右终端独占全屏、左终端被盖住 | `分屏丢失`, `split`, `active_view`, `右窗格全屏`, `Codex`, `完成通知`, `SkyComputerUseClient`, `windowDidResignKey`, `on_active_status_change`, `focus_out`, `system notifications disabled`, `not running from an app bundle`, `jump_back_to_split_pane`, `PTY reader`, `test_scheduler` | [终端分屏与系统通知:窗口激活链路](terminal-split-notification-window-activation.md) |
| agent TUI 打字时页面整屏闪烁；每敲一个字符候选浮层一变就 2J 全屏重绘；滚轮/鼠标正常但视觉跳动 | `2J`, `dock_resized`, `render_frame_regular`, popup, 候选浮层, 整屏重绘, 闪烁, flicker, repaint, 每键重绘, capture_frames | [TUI 整屏重绘风暴](tui-render-frame-2j.md) |
