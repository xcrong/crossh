# TUI 整屏重绘风暴：dock 高度变化触发每键 2J

## 症状

- `crossh-agent` regular 模式下，输入 `/` 弹出候选浮层后，**每敲一个字符页面整屏清空重绘**（iTerm2/Terminal 里明显闪烁），scrollback 里累积整屏重复副本。
- PTY 逐字符注入实测（30x80）：键入 `/model m` 共 9 个字符，输出流中出现 **4 次 `\x1b[2J`**（`/` 弹出、候选 8→1、空格进入参数模式、候选过滤各一次）。
- 候选浮层固定视口后：同输入降为 **2 次**（启动首帧 + `/` 弹出帧）。
- 参考 pi `TuiMainScreen` 对齐后：同输入仅 **1 次**（启动首帧），浮层弹出帧也无 `2J`（该对齐随 crossh-tui 移除落地，对应 spec 已清除）。

## 根因

`crossh-tui` 的 `MainScreenRenderer::render_frame_regular` 三条路径（纯追加 / 尾部原位改写 / 整屏重绘）由 `incremental = !first_frame && !size_changed && !dock_resized` 门控：**dock 高度一变就强制整屏重绘**。而 `src/agent_cli_render.rs` 的候选浮层用 `for (idx, cand) in cands.iter().enumerate()` 画行——候选数 8→1→5 变化时 popup 高度随之伸缩，dock = popup + editor + footer 的总高度持续变化，于是每次输入都命中 `dock_resized`。

pi 的 `TuiMainScreen` 没有 dock 概念：内容行数变化一律走差分渲染（`firstChanged..lastChanged`），只有首帧、宽高变化、`firstChanged < viewportTop`（变化越过可视区）、`clearOnShrink` 才整屏重绘。

## 规则

- **候选浮层保持固定候选视口行数**（本项目 8 行），候选不足补空行，候选数量变化只改写行内容——这是防抖优化，也是 dock 高度稳定的基础（`POPUP_CANDIDATE_ROWS`）。
- **dock 高度变化（浮层开/关）走增量路径**（该对齐随 crossh-tui 移除落地，原 spec 已清除）：
  - dock 变高：dock 从新起始行覆写，原 transcript 底部行等效滚动出视口；
  - dock 变矮：先清除残行（旧 dock 多出的行逐行 `\x1b[2K`），再 transcript 增量写入、重写 dock——清除必须先于增量写入，否则新写入行会被随后的清除擦掉。
- **`\x1b[2J` 整屏重绘只允许发生在**：首帧、终端尺寸变化、transcript 变化越过可视区（scrollback 不可原位改写）。
- **已知行为（审查 P1 确认）**：dock 变矮（浮层关闭）且 transcript 末尾无新行时，原 dock 区域显示为空白——transcript 采用"追加不可改写 + 只重印尾部视口"模型，被 skip 的头部从未输出到终端，无法增量补印（反向滚动会拉回无关 scrollback 内容）。空白随后续 transcript 增长逐帧滚动上移直至滚出屏幕，scrollback 中内容始终完整；这与 pi 内容变短后留空、由后续 diff 填充的行为等价（pi 行数组整全重写空行，crossh 靠滚动替换）。
- 排查这类问题不要看屏幕，要看字节：`capture_frames.py` 逐字符注入 + 按 `\x1b[?2026h` 切帧 + 统计 `\x1b[2J`，修复前后同命令对比。

## 验证方法

```sh
python3 .agents/skills/terminal-pty-capture/scripts/capture_frames.py \
  --rows 30 --cols 80 --input "/model m" \
  --count '\x1b[2J' --name repaint -- target/debug/crossh-agent
```

预期（当前实现）：`repaint_total=1`（仅启动首帧），浮层弹出帧、候选过滤帧均 `repaint=0`——
即除 frame[1] 外每一帧的 repaint 计数都应为 0，不要误以为"浮层弹出帧也应为 1"。
回归测试：`slash_popup_changes_do_not_trigger_full_screen_repaint`（候选变化帧无 `2J`）、`slash_popup_height_is_stable_across_candidate_count_changes`（popup 行数恒定 2+8）、`spec_20260822_main_screen_dock_incremental__*`（dock 高度变化无 `2J` + 残行清除 + 流式增长共存 + 光标在界内）、`spec_main_screen__dock_resize_is_incremental_and_clears_residue`。

## 搜索关键词

`2J`, dock_resized, render_frame_regular, popup, 候选浮层, 整屏重绘,
闪烁, flicker, repaint, 每键重绘, MainScreenRenderer, capture_frames,
clear_residual, 残行清除, firstChanged, viewportTop, TuiMainScreen