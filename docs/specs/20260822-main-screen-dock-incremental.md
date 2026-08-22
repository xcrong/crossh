# regular 模式 dock 高度变化改增量渲染（对齐 pi TuiMainScreen）

## 元数据

- 状态：`done`
- 创建：2026-08-22
- 批准：2026-08-22（用户在对话中批准）
- 完成：2026-08-22（`/model m` capture 实测 repaint_total=1）
- 相关 ADR：无（不改变边界/属主；若实现暴露新渲染决策规则，收尾时提炼进 ADR 或工程笔记）
- 相关 issue / 路线图项：本对话「为什么会出现全屏重复渲染 / 参考 pi 的实现」
- CI 平台影响：`仅 macOS`（纯 TUI 交互，Linux/Windows 由既有 `cargo test --workspace` 覆盖，无新增行为分支）

## 背景

`crossh-agent` regular 模式（transcript 追加进 scrollback、不进入备选缓冲）中，`/` 候选浮层并入 dock（`dock = popup + editor + footer`）。浮层出现/消失使 dock 高度变化，`MainScreenRenderer` 的 `dock_resized` 门控强制整屏 `\x1b[2J` 重绘。

整屏重绘在 iTerm2/Terminal 中会把被清旧帧留在 scrollback——每次浮层开/关都在 scrollback 留下一份整屏副本，翻页时看到同一屏内容重复多份（「全屏重复渲染」，用户实测截图）。已固化「浮层弹出/关闭各一次 2J、候选变化不 2J」的防御（`docs/engineering-notes/tui-render-frame-2j.md`）。

成熟参考 pi-tui（`earendil-works/pi`，本项目 1:1 移植对象）的 `TuiMainScreen` 没有 dock 概念：内容行数变化一律走差分渲染（`firstChanged..lastChanged` 区间），只有首帧、宽高变化、`firstChanged < viewportTop`（变化越过可视区）、`clearOnShrink` 才整屏重绘。编辑器补全列表（slash autocomplete）行数伸缩是纯 diff 输出，**浮层开/关不触发任何整屏重绘**。

本次变更把 regular 模式的 dock 高度变化从「整屏重绘事件」改为「增量渲染事件」，与 pi 对齐。

## 目标

1. `/` 候选浮层出现、消失、候选数量变化均不触发整屏重绘（帧内无 `\x1b[2J`）。
2. 浮层关闭后其占据的屏幕行被清除，不残留浮层字符；transcript 已提交行不受影响。
3. 浮层打开时 transcript 视口收缩、关闭时恢复；scrollback 中 transcript 保持完整（追加语义不变）。
4. 首帧、终端尺寸变化、transcript 变化越过可视区仍保留整屏重绘（现有行为不回退）。
5. 字节级可验证：同一命令下整屏重绘次数下降（`/model m`：2 次 → 1 次）。

## 非目标

- 不改 fullscreen（AltScreen）路径：`render_fullscreen` 已将浮层作为行数组内容、经 `ScreenRenderer` 行 diff 渲染，浮层开/关已零 `2J`，不属于本 spec 范围。
- 不移植 pi 的通用 Overlay 栈（`showOverlay`/`compositeOverlays`）：当前无对话框/模态需求；pi 中 slash 补全本就是编辑器内联行而非 overlay，与本次「行数组增量」语义一致，将来有模态需求时另行 spec。
- 不改浮层视觉样式、固定候选视口（8 行）设计、`build_slash_popup` 的 gating（`/`、`、` 前缀判定）。
- 不改 transcript 追加/尾部原位改写/越过可视区整屏重绘的既有语义。
- 不新增设置项、不改变持久化与 wire 格式。

## 行为契约

命名前缀：`spec_20260822_main_screen_dock_incremental__`

1. 当浮层出现（dock 高度增加）且 transcript 无新增行时，本帧不应包含 `\x1b[2J`；浮层行应以 `\x1b[2K` 清行后写入屏幕底部区域，覆盖原 transcript 区底部行（等效终端滚动），观察到浮层正常显示且页面不闪烁。
2. 当浮层消失（dock 高度减少）且 transcript 无新增行时，本帧不应包含 `\x1b[2J`；原浮层占据的屏幕行应被清行（帧内出现对应数量 `\x1b[2K`），观察到屏幕不残留浮层字符、footer 回到屏幕底部。
3. 当浮层出现/消失的同时 transcript 有新增或改写行时，本帧不应包含 `\x1b[2J`；新行写入与残行清除相互独立、互不覆盖，观察到最终屏幕与「整屏重绘后的等效画面」一致。
4. 当候选数量变化（浮层视口高度恒定 2+8 行）时，本帧不应包含 `\x1b[2J`（保持 `slash_popup_changes_do_not_trigger_full_screen_repaint` 既有契约）。
5. 当渲染器首帧渲染时，本帧应包含 `\x1b[2J`（保持现有契约 `spec_main_screen__first_frame_prints_transcript_and_dock` 语义）。
6. 当终端宽度或高度变化时，本帧应包含 `\x1b[2J`（保持现有契约）。
7. 当 transcript 变化越过可视区（旧行不可原位改写）时，本帧应包含 `\x1b[2J`（保持现有契约）。
8. 任意浮层开/关帧之后，硬件光标应被定位在编辑器行内（`\x1b[{row};{col}H`，行号不越界），观察到光标留在输入框内（保持既有行为）。

## 边界与错误

- decay：浮层行数超过终端高度时按下限截断（`dock_len = dock.len().min(height)`），不 panic、不越界写。
- 尺寸为 0（`cols/rows == 0`）时返回空帧（保持）。
- dock 为空时沿用「至少保留 1 行编辑器」的既有回退（保持）。
- 连续多帧浮层开→关→开：渲染器状态在每帧正确推进，不依赖「上一帧是否 2J」的记忆；任何一帧都不允许因状态错位而出界。
- 浮层出现时 transcript 超过新视口：允许使用既有滚动腾行（`\r\n`×N）路径，但不得回退到 `\x1b[2J`。
- `prev_dock_len` 状态在 reset（`MainScreenRenderer::reset`）后归零，重新进入首帧语义（保持）。

## 既有契约变更

- `crates/crossh-tui/src/main_screen.rs` 单测 `spec_main_screen__dock_resize_triggers_full_repaint`（L378-389）断言「dock 高度变化帧包含 `\x1b[2J`」，与本 spec 契约 1/2 直接冲突——**该测试语义被替换**：改为断言 dock 高度变化帧无 `2J`、新 dock 行正确写入、残行被清除。
- `docs/engineering-notes/tui-render-frame-2j.md` 规则「regular 模式下 dock 高度必须恒定；popup 出现/消失是布局跨度的真实变化，整屏重绘是必要重排」被本 spec 推翻——收尾时更新为「dock 高度变化走增量渲染（覆写 + 清残行），`2J` 仅限首帧/尺寸变化/transcript 越过可视区」。
- 其余既有 main_screen 单测（首帧、纯追加、尾部原位改写、越过可视区、显式滚动、光标定位、空追加）语义不变。

## 接口与状态变更

- `crates/crossh-tui` 的 `MainScreenRenderer::render_frame_regular`：`dock_resized` 不再作为整屏重绘的触发条件；返回帧的字节语义变化（dock 高度变化帧从 `\x1b[2J` 全量改为增量 + 残行清除）。
- `src/agent_cli_render.rs` 的 `render_regular` 调用方无需改动（`build_slash_popup`/dock 组装签名不变）。
- 无设置项、无持久化格式变更、无 wire 格式变更。

## 平台影响

- 纯 TUI 字节流行为：macOS 本地用 `capture_frames.py` 验证；Linux/Windows 上行为与平台无关（终端序列语义相同），由既有 `cargo test --workspace` + Actions 产物验证，无新增 CI job。

## 涉及纪律

- [x] Logic must not depend on UI（层级）：改动局限 `crates/crossh-tui`（UI crate），`crates/crossh-agent` 纯逻辑零依赖不变
- [x] 文件规模 < 2000 行（scripts/check-architecture.sh）：`main_screen.rs` 当前 426 行，新增残行清除与测试保持在限内
- [x] 工程笔记 / ADR 同步义务：`docs/engineering-notes/tui-render-frame-2j.md` 的规则「dock 高度必须恒定、popup 出现/消失可 2J」将被本次变更推翻，收尾时必须更新该笔记与 `README.md` 索引
- [x] 不新增设置项（Feature-owned settings 不涉及）

## 影响模块

- `crates/crossh-tui/src/main_screen.rs`（渲染决策 + 残行清除 + 单测）
- `crates/crossh-tui/src/terminal.rs`（如需新增清除序列常量）
- `docs/engineering-notes/tui-render-frame-2j.md`、`docs/engineering-notes/README.md`（规则更新）
- `docs/testing.md`（行为矩阵增量，如有新契约条目）

## 验收清单

- [x] spec 评审通过（AI 评审 + 人批准）
- [x] 行为契约全部固化为失败测试并确认失败原因正确（Red）：浮层开/关/候选变化帧无 `2J`、残行清除断言、边界用例
- [x] 最小实现通过聚焦测试（Green）
- [x] `cargo fmt --check`
- [x] `scripts/check-architecture.sh`
- [x] `cargo clippy --all-targets -- -D warnings`
- [x] `cargo test --workspace`
- [x] `capture_frames.py` 字节级对比：`/model m` 整屏重绘次数降为 1（仅启动首帧），浮层弹出帧无 `2J`（实测 repaint_total=1，旧基线 2）
- [x] 工程笔记 `tui-render-frame-2j.md` 与索引更新（规则变更说明）
- [x] 用户可观察效果人工确认：regular 模式下输入 `/` 弹出/关闭浮层无整屏闪烁，scrollback 不再累积整屏副本