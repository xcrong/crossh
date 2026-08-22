# Agent TUI 渲染机制与 Pi 完全对齐（AltScreen 1:1 移植）

> 复制本文件到 `docs/specs/YYYYMMDD-<slug>.md`，填写后进入评审。
> 只描述行为与验收，不写实现方案。语言与项目文档保持一致。

## 元数据

- 状态：`approved` → `in-progress`
- 创建：2026-08-22
- 批准：2026-08-22（用户在对话中批准）
- 相关 ADR：`docs/adr/0002-logic-ui-layering.md`、`docs/adr/0003-agent-logic-and-view-split.md`、`0009-standalone-agent-binary.md`，新增 ADR 待定（TUI 渲染与 ScrollView 边界）
- 相关 issue / 路线图项：本对话“把渲染机制从根本上全部替换成和 Pie 完全相同的机制”
- CI 平台影响：`仅 macOS`（TUI 交互，Linux/Windows 由 Actions 产物验证，不新增平台行为）

## 背景

`crossh-agent` 的 TUI 当前为简易 `ratatui/crossterm` 实现：`agent_cli_render.rs` 的 `Paragraph.scroll()` + `app.scroll: u16::MAX` + 分散在 `run_app/wait_for_model/confirm_tool/wait_for_background` 的 `ScrollUp/Down → ±3`，在 `a8e4f52` 为支持原生选区而 `DisableMouseCapture`，导致滚轮在 `alternate screen` 下完全失效；后续 `4e26236` 为修滚动又重新 `EnableMouseCapture`，导致普通拖拽需 `Option` 才能选中。`pi` 的 `pi-tui/dist/tui-alt-screen.js` 是成熟的 `AltScreen{ ScrollView(primary,follow:end), wheelScrollLines, scrollbar, selection(字符/词/行)、自动滚动、OSC52 拷贝、同步输出、搜索高亮、multiplexer 降级 }` 闭环，`crossh` 与之行为不一致，顶部状态栏也只是表象。

## 目标

1. 将 `crossh-agent` 的渲染与输入管线**完全替换**为 `pi-tui` 的 `AltScreen` 机制的 Rust 等价实现，不保留现有 `ratatui` 简易滚动/鼠标分散处理。
2. 滚轮、键盘分页、选区、滚动条、搜索、同步输出、`mouseEnabled` 降级等行为与 `pi 0.84.2` 的 `tui-alt-screen.js` 1:1 对齐。
3. 顶部状态栏、思考/工具折叠等 `crossh-agent` 现有视觉信息在新管线中保留，但滚动/选区不再由上层散落逻辑驱动。
4. 普通拖拽即可选中并 `Copied!`（`OSC52` + 注入式 `copySelection`），无需 `Option`，同时滚轮可用。

## 非目标

- 不改变 `crossh-agent` 的会话、工具、上下文、思考档位等纯逻辑（`crates/crossh-agent` 零 `ratatui` 依赖保持）。
- 不改变 `crossh` 的 `gpui` 主窗口与 `crossh-terminal`，仅 `src/agent_cli*` / 新 `crates/crossh-tui` 受影响。
- 不引入 `pi` 的图片协议（kitty/iterm2 `prepareKittyScreen`）与 `photon-node` 等重型依赖，首版仅覆盖文本、`OSC133`、`scrollbar`、`selection`、`search`。
- 不改变模型目录/预设（`opencode-go` 已对齐），不新增 `settings.toml` 字段。

## 行为契约

命名前缀：`spec_20260822_agent_tui_pi_parity__`

1. 当 `AltScreen` 进入（`beforeTerminalStart`）时，应写入 `ENTER_ALT_SCREEN + DISABLE_AUTOWRAP + mouseSequence + \x1b[2J\x1b[H\x1b[?25l`，其中 `mouseSequence` 按 `TMUX/ZELLIJ/STY` 或 `TERM` 前缀降级为 `ENABLE_BUTTON_MOTION_MOUSE`，否则 `ENABLE_ALL_MOTION_MOUSE`，且 `mouseEnabled=false` 时不写入任何鼠标序列，观察到终端进入备用缓冲并隐藏光标。
2. 当 `AltScreen` 离开（`beforeTerminalStop/afterTerminalStop`）时，应写入 `BEGIN_SYNCHRONIZED_OUTPUT + deleteKittyImages + (mouseEnabled?DISABLE_MOUSE:\"\") + ENABLE_AUTOWRAP + END_SYNCHRONIZED_OUTPUT`，并在 `preserveScreen=false` 时以 `EXIT_ALT_SCREEN + \x1b[?25h` 恢复并重放 `lastDocument`，观察到备用缓冲退出、光标恢复、可滚动内容落盘。
3. 当收到滚轮事件（SGR `\x1b[<64;...` 或 legacy `\x1b[M`，`button&64 !=0`）时，应按 `wheelScrollLines`（默认 1）调用 `routeWheel`，优先命中 `event.x/y` 下的 `ScrollView`（`getScrollViewsAt`），`overscroll==contain` 时不冒泡，否则冒泡至 `primaryScrollView`，观察到内容按方向滚动对应行数并 `requestRender`。
4. 当键盘触发 `tui.altScreen.pageUp/pageDown` 时，应按 `viewportHeight - PAGE_SCROLL_OVERLAP` 滚动 `primaryScrollView`，观察到分页滚动不丢失 `OVERLAP` 行。
5. 当触发 `halfPageUp/halfPageDown` 时，应按 `viewportHeight/2` 滚动，观察到半页滚动。
6. 当触发 `lineUp/lineDown` 时，应按 1 行滚动，观察到单行滚动。
7. 当触发 `top/bottom` 时，应 `scrollToStart/scrollToEnd`，观察到回到顶部/底部且 `follow:end` 状态更新。
8. 当触发 `previousPrompt/nextPrompt` 时，应基于 `OSC133_PROMPT_START` 的 `scrollContentLines` 向 `direction` 查找下一 prompt 行并 `scrollTo(row)`，观察到跳转到上一/下一 prompt。
9. 当鼠标在 `ScrollView` 视口内按下左键（`button&3==0`，非 `release`）时，应 `selectionPressActive=true`，记录 `selectionAnchor`（含 `scrollView`、`row/col`），按点击次数（`getClickCount` + `DOUBLE_CLICK_INTERVAL_MS`）决定 `granularity` 为 `character/word/line` 并设置 `selectionInitialRange`，观察到选区锚点建立。
10. 当鼠标拖动（`button&32 !=0`）且 `selectionPressActive` 时，应 `selectionDragged=true`，`updateSelectionFocus` 按 `granularity` 扩展，`updateSelectionAutoScroll` 在指针越界时以 50ms 间隔 `autoScrollSelection`（±1 行/50ms），观察到选区随拖动与自动滚动更新并高亮（`\x1b[7m`）。
11. 当鼠标释放左键（`release==true, button&3==0`）且 `selectionPressActive` 时，应 `selectionPressActive=false`，若未拖动且命中 OSC8 链接则 `openUrl`，否则 `copySelectionToClipboard`：若 `copySelection` 注入返回 `true` 则 `flash Copied!` 否则 `Copy failed`；无注入则写入 `OSC52` `\x1b]52;c;{base64}\x07` 并 `flash Copied!`，观察到剪贴板写入与提示。
12. 当鼠标在滚动条拇指（`getScrollbarTargetAt` 命中 `geometry.column/thumTop/height`）按下时，应进入 `scrollbarDrag{grabOffset}`，拖动时按 `event.y - trackTop - grabOffset` 比例映射 `scrollTop = thumbOffset / maxThumbOffset * maxScrollTop`，释放时 `stopScrollbarDrag`，观察到拖拽滚动条与内容同步。
13. 当 `activeSearch` 打开且有 `query` 时，`refreshSearch` 应基于 `scrollViewBox` 的 `scrollContentLines` 用 `findAltScreenSearchMatches` 匹配，按 `query/previous/next/retain` 决定 `selectedIndex`，并在 `shouldRevealSelection` 时将 `firstSegment.row` 居中到 `viewportHeight/3`，观察到搜索匹配计数与滚动到命中。
14. 当 `ScrollView` 内容长度超过 `viewportHeight` 时，`getScrollbarGeometry` 应按 `thumbHeight = viewportHeight * viewportHeight / contentHeight` 计算并在 `applySelection/applySearchHighlights` 时避开滚动条列，观察到滚动条正确显示且不覆盖文本。

## 边界与错误

- `AltScreen` 多次 `beforeTerminalStart` 不重复写入 `ENTER_ALT_SCREEN`；`TMUX` 等复用器下降级为 `ENABLE_BUTTON_MOTION_MOUSE` 避免指针风暴。
- 滚轮事件在有 `overlay` 覆盖时 `shouldDeferViewportInputToOverlay` 归 overlay 处理，不冒泡至 `primaryScrollView`。
- 选区跨不同 `ScrollView` 时 `getSelectionBounds` 返回 `undefined`，不拷贝；空选区（锚点==焦点）不触发拷贝与 `flash`。
- `FOCUS_OUT` 时清理 `selectionPressActive/selectionDragged/pressedUrl/scrollbarDrag` 并 `requestRender`，防止悬停残留。
- 搜索无命中时 `matches=[], selectedIndex=-1`，不滚动；`activeSearch` 关闭时 `stopScrollbarHover/Drag`。
- 同步输出（`BEGIN_SYNCHRONIZED_OUTPUT/END_SYNCHRONIZED_OUTPUT`）包裹所有 `beforeTerminalStop/afterTerminalStop/doRender` 的终端写入，避免撕裂。
- `mouseEnabled=false` 时所有鼠标解析与滚动条交互短路，不写入 `DISABLE_MOUSE`。

## 接口与状态变更

- 新增 `crates/crossh-tui`（或 `crates/crossh-agent/src/tui/`）：`AltScreen`、`ScrollView{scrollTop, viewportHeight, contentHeight, follow, overscroll}`、`LayoutNode`、`FlashContainer`、`ScrollbarGeometry`，对 `pi-tui` 的 `tui-alt-screen.js`/`layout.js`/`terminal.js` 做 Rust 等价移植（不引入 kitty 图片）。
- `src/agent_cli.rs`：`run_with_options` 委托 `crossh_tui::AltScreen::beforeTerminalStart`，`run_app` 的 `event::read` 分发收敛到 `AltScreen::handleViewportInput`，`agent_cli_render.rs` 的 `Paragraph.scroll` 改为 `ScrollView` 的 `scrollContentLines` + `applySelection/applySearchHighlights`。
- `src/agent_cli_input.rs`：`FOCUS_IN/OUT` 的选区清理复用 `pi` 逻辑。
- 无 `settings.toml` 新增字段，无持久化格式变更，无 wire 格式变更。

## 平台影响

- 纯 TUI，`macOS` 本地验证（`cargo run --bin crossh-agent` 的鼠标滚轮、拖拽选区、滚动条、分页键）；`Linux/Windows` 无新增平台分支，现有 `cargo test --workspace` 与 `check-architecture` 覆盖，CI 无新增 job。

## 涉及纪律

- [x] Logic must not depend on UI（层级）：`crates/crossh-agent` 仍零 `ratatui/crossterm`，新 `crossh-tui` 为独立 UI crate，`src/agent_cli` 仅消费其 `AltScreen` 公开 API
- [x] 文件规模 < 2000 行：`agent_cli.rs` 当前 1830 行，拆分 `tui/*.rs` 后单文件 < 800 行，`check-architecture.sh` 白名单无需新增
- [x] Keep the app entry point thin：`main.rs` 仍仅窗口/启动，`AltScreen` 生命周期收敛至 `crossh-tui`
- [ ] Feature-owned settings：本 spec 不新增设置项
- [x] 工程笔记 / ADR 同步义务：新增 ADR（AltScreen 与 ScrollView 边界），`docs/architecture.md` 登记

## 影响模块

- `crates/crossh-tui`（新增）或 `crates/crossh-agent/src/tui/{alt_screen.rs, scroll_view.rs, layout.rs, scrollbar.rs, selection.rs}`
- `src/agent_cli.rs`：`run_with_options/run_app/handle_key/wait_for_model/confirm_tool/wait_for_background`
- `src/agent_cli_render.rs`：`render` 的 `ScrollView` 管线与 `applySelection`
- `src/agent_cli_input.rs`：`FOCUS_*` 选区清理
- `docs/architecture.md`、`docs/adr/00XX-*.md`

## 验收清单

- [ ] spec 评审通过（AI 评审 + 人批准）
- [ ] 行为契约全部固化为失败测试并确认失败原因正确（Red）
- [ ] 最小实现通过聚焦测试（Green）
- [ ] `cargo fmt --check`
- [ ] `scripts/check-architecture.sh`
- [ ] `cargo clippy --all-targets -- -D warnings`
- [ ] `cargo test --workspace`
- [ ] 声明的平台 CI job 通过（非本机平台：提交后由 Actions 验证，spec 状态
      保持 in-progress 直到通过）
- [ ] 结构性决策提炼进 ADR（如有）并登记 docs/architecture.md
- [ ] 调试根因合并进 docs/engineering-notes/（如有）
- [ ] 新增行为合并进 docs/testing.md 关键行为矩阵（如有）
- [ ] 用户可观察效果人工确认（针对 UI/交互变更）
