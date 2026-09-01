# 工程经验索引

这里记录已经确认、可复用的调试经验，目的是让相似问题能够通过症状或关键词快速定位。这里不记录未经验证的猜测。

使用方式：先搜索下表中的症状和关键词，只读取匹配的主题文档。解决新的非显然问题后，优先更新已有主题；只有当问题属于新的技术边界时才新增文件。

| 主题 | 典型症状 | 关键词 | 文档 |
| --- | --- | --- | --- |
| GPUI 首窗口与 CLI 生命周期 | Dock 有图标但没有窗口；GUI 命令阻塞终端；关闭隐藏窗口后命令才退出 | `GPUI`, `open_window`, `defer`, `Dock`, `CLI`, `detached process`, `cold start` | [GPUI 窗口启动生命周期](gpui-window-startup.md) |
| GPUI Flex 滚动容器 | 设置 `max_h` 和 `overflow_y_scroll` 后滚轮仍无效；长列表被压缩 | `GPUI`, `flex_shrink_0`, `overflow_y_scroll`, `max_h`, 滚轮无效 | [GPUI Flex 滚动容器](gpui-flex-scroll.md) |
| GPUI 轮询与大列表性能 | Git 视图静止时周期性卡顿；多终端持续启动 Git；长 diff 滚动变慢 | `GPUI`, `Git`, `polling`, `notify`, `UniformList`, `diff`, 卡顿 | [GPUI 轮询与大列表性能](gpui-polling-performance.md) |
| Cargo 发布后 workspace 版本与锁文件不一致；tag 发布漏提交 `Cargo.lock` | `Cargo.lock`, `release.sh`, `cargo metadata`, 版本发布 | [Cargo 锁文件发布同步](cargo-lock-release-sync.md) |
| 命令历史测试只在 windows-latest 上断言 2 vs 3 | `commands.rs`, `aggregates_commands_and_returns_top_thirty`, `last_used`, 秒级时间戳, 时间竞态 | [命令历史测试的时间精度竞态](command-history-test-timing.md) |
| 跨终端 TUI splash 颜色对不上;白色刺眼、阴影过渡"细线"消失,但字节流一致 | `opencode`, `VT Code`, `OSC 11`, `38;2 真彩`, `canvas`, 自适应主题, `splashShadow`, `ColorRequest`, 256 色, 细线消失 | [终端自适应颜色与主题底色偏差](terminal-adaptive-colors.md) |
| CI/local 全绿但某 crate 测试从不执行；`--workspace --lib` 静默跳过 bin 测试 | `cargo test`, `--workspace`, `--lib`, 测试没跑, bin target, 静默跳过 | [Cargo 测试选择陷阱](cargo-test-workspace-selection.md) |
| serde_json Value 字段顺序随构建图变化；`-p` 通过但 `--workspace` 失败 | `serde_json`, `preserve_order`, `IndexMap`, 单包测试通过但 workspace 失败, canonical JSON | [serde_json preserve_order 与构建图](serde-json-preserve-order-workspace.md) |
| 分屏(左 X/右 Y)跑 Codex,切到另一 tab 再点完成通知跳回后右终端独占全屏、左终端被盖住 | `分屏丢失`, `split`, `active_view`, `右窗格全屏`, `Codex`, `完成通知`, `SkyComputerUseClient`, `windowDidResignKey`, `on_active_status_change`, `focus_out`, `system notifications disabled`, `not running from an app bundle`, `jump_back_to_split_pane`, `PTY reader`, `test_scheduler` | [终端分屏与系统通知:窗口激活链路](terminal-split-notification-window-activation.md) |
| agent TUI 打字时页面整屏闪烁；每敲一个字符候选浮层一变就 2J 全屏重绘；浮层开/关在 scrollback 留整屏重复副本；滚轮/鼠标正常但视觉跳动 | `2J`, `dock_resized`, `render_frame_regular`, popup, 候选浮层, 整屏重绘, 闪烁, flicker, repaint, 每键重绘, capture_frames, 残行清除, 浮层开/关, scrollback 副本 | [TUI 整屏重绘风暴](tui-render-frame-2j.md) |
| 编译时 rustc SIGBUS 崩溃；大编译期间整机卡死/自动重启；测试长时间无输出后失败 | `SIGBUS`, `rustc`, `RTL9210`, `NVMe`, `USB`, `I/O error`, `DART panic`, 卡死, 强制重启, 外置盘 | [rustc SIGBUS 与外置盘 I/O 故障](rustc-sigbus-usb-nvme-failure.md) |
| agent TUI 运行中突然崩溃；`attempt to subtract with overflow` at main_screen.rs:63；界面在转 Working spinner 或候选在变 | `attempt to subtract with overflow`, `plan_rows`, `cap2`, `start_row`, 视口已满, empty append, n=0, 空追加, main_screen, Working spinner, 运行中崩溃 | [main_screen 增量路径空追加下溢](main-screen-incremental-panic.md) |
| `git commit` 卡在 clippy 后被超时切断；提交未落库但改动还在暂存区 | `git commit`, `pre-commit`, `clippy -D warnings`, `timeout`, 超时, 钩子, 提交慢 | [pre-commit 钩子耗时与提交超时设置](pre-commit-hook-duration.md) |
| 终端里 ②③ 等带圈数字与后续字符叠印、粘在一起 | `歧义宽度`, `②③`, `粘在一起`, `叠印`, `force_width`, `cell_width`, `Lilex`, `PingFang`, `Alacritty`, `unicode_width` | [终端歧义宽度字符叠印](terminal-ambiguous-width-overlap.md) |
| target 快速膨胀到 100G；`wasmtime/cranelift` 占 633M/变体；`cargo tree --invert wasmtime` 经 `migrator/settings_json -> tree-sitter[wasm]` | `wasmtime, cranelift, wasm, migrator, settings_json, tree-sitter, target 膨胀, patch, 定向复制` | [Zed migrator 引入 wasmtime 导致 target 膨胀](zed-migrator-wasmtime-bloat.md) |
