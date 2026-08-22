# TUI 整屏重绘风暴：dock 高度变化触发每键 2J

## 症状

- `crossh-agent` regular 模式下，输入 `/` 弹出候选浮层后，**每敲一个字符页面整屏清空重绘**（iTerm2/Terminal 里明显闪烁）。
- PTY 逐字符注入实测（30x80）：键入 `/model m` 共 9 个字符，输出流中出现 **4 次 `\x1b[2J`**（`/` 弹出、候选 8→1、空格进入参数模式、候选过滤各一次）。
- 修复后同输入仅 1 次（`/` 弹出帧），其余字符全走增量路径。

## 根因

`crossh-tui` 的 `MainScreenRenderer::render_frame_regular` 三条路径（纯追加 / 尾部原位改写 / 整屏重绘）由 `incremental = !first_frame && !size_changed && !dock_resized` 门控：**dock 高度一变就强制整屏重绘**。而 `src/agent_cli_render.rs` 的候选浮层用 `for (idx, cand) in cands.iter().enumerate()` 画行——候选数 8→1→5 变化时 popup 高度随之伸缩，dock = popup + editor + footer 的总高度持续变化，于是每次输入都命中 `dock_resized`。

## 规则

- regular 模式下 **dock 高度必须恒定**：候选浮层用固定候选视口行数（本项目 8 行），候选不足补空行，候选数量变化只改写行内容。
- dock 高度变化只允许发生在：首帧、终端尺寸变化、popup 出现/消失、编辑器跨行——这些是布局跨度的真实变化，整屏重绘是必要重排。
- 排查这类问题不要看屏幕，要看字节：`capture_frames.py` 逐字符注入 + 按 `\x1b[?2026h` 切帧 + 统计 `\x1b[2J`，修复前后同命令对比。

## 验证方法

```sh
python3 .agents/skills/terminal-pty-capture/scripts/capture_frames.py \
  --rows 30 --cols 80 --input "/model m" \
  --count '\x1b[2J' --name repaint -- target/debug/crossh-agent
```

预期：`repaint_total=2`（启动首帧 + `/` 弹出帧），其余帧 `repaint=0`。
回归测试：`slash_popup_changes_do_not_trigger_full_screen_repaint`（候选变化帧无 `2J`）、`slash_popup_height_is_stable_across_candidate_count_changes`（popup 行数恒定 2+8）。

## 搜索关键词

`2J`, dock_resized, render_frame_regular, popup, 候选浮层, 整屏重绘,
闪烁, flicker, repaint, 每键重绘, MainScreenRenderer, capture_frames