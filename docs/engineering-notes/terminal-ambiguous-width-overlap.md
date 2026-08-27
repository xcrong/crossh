# 终端歧义宽度字符叠印（②③ 粘在一起）

## 症状

终端里 `②③①⑴★☆` 等带圈数字/符号与后续字符**叠印**，看上去"粘在一起"（用户截图：`锚点②③` 两个圈完全重叠）。与剪贴板、粘贴实现无关，任何输出途径（含 agent 日志、echo）只要出现该字符就会复现。

## 根因

三层不一致叠加：

1. **网格层**：`alacritty_terminal`（Zed fork `4c12966`）用 `unicode-width 0.2` 窄宽度语义，East Asian Ambiguous 字符按 **1 格**计宽（`term/mod.rs` 的 `c.width()`），`②` 占 1 格、`③` 紧邻下一格，无 `WIDE_CHAR` 标记。
2. **字体层**：终端默认字体 Lilex（`Zed Plex Mono` 映射）**没有** U+2461 等字形（已用 `CTFontGetGlyphsForCharacters` 验证；Menlo 亦无），CoreText 级联回退到 **PingFangSC-Regular**，字形步进 **1.0 em**（14px @ 字号 14），而 `cell_width = advance('m' in Lilex) = 0.6 em`（8.4px）→ 字形实际约 **1.67 格宽**。
3. **渲染层**：`gpui` 的 `apply_force_width_to_layout`（`line_layout.rs:849`）把第 N 个基字形强制钉在第 N 个格子边界（`glyph_pos * cell_width`，`1px` 容差），无视自然步进 → `②` 被钉在第 N 格（字形溢出半格多），`③` 被钉到第 N+1 格，正好压在 `②` 右半边上；后续字符各自被钉回连续格子，故整行只有相邻一两个字叠印。

上游 Zed 存在同样问题（fork 继承）。

## 已排除项

| 嫌疑 | 结论 | 依据 |
| --- | --- | --- |
| 粘贴/括号粘贴模式 | 排除 | 直接 `echo` 同样复现，字节流一致 |
| 256 色/真彩转换 | 排除 | 颜色与字符宽度无关 |
| APCA 对比度修正 | 排除 | 仅调前景色，不改字形步进 |
| 字体本身有 `②` | 排除 | `CTFontGetGlyphsForCharacters(Lilex, U+2461)` 返回 `glyph 0` |

## 持久规则

1. **网格与字形不一致时，渲染层占两格、保持原字号，不改网格**。首版“缩字入格”（`cell/shaped` 等比缩小）会让字形小 40%（14→8.4），用户反馈“小了几号”；改为超宽字符在渲染层按 2 格排版、后续列右移一格，字形保持原大小，行宽每字符 +1 格（与中文全角一致，接受该代价）。网格改“歧义=宽”会与 `zsh`/`readline` 窄语义脱节导致提示符错位，故不改 PTY 侧。
2. **不 fork `alacritty_terminal` / `gpui` 处理歧义宽度**。渲染端在 `layout_grid` 内按需分配 2 格即可。
3. **测量复用缓存，仅对 1 格非 ASCII 候选测量**。`shaped_width = text_system.layout_width(font_id, base_font_pixels, ch)` 走 `LineLayoutCache`，同一 `(char, FontId)` 一帧内只测一次；ASCII/`' '`/`width != 1` 跳过。
4. **`BatchedTextRun` 合并必须感知超宽**。超宽字符独占 `cell_count=2` 的 batch，`can_append` 需比较 `font_size` 且 `extra_offset` 保证后续列右移；否则超宽后的独立 batch 会被错误合并。
## 验证方法

1. 探针脚本（`swift` + CoreText）：
   ```swift
   let base = CTFontCreateWithName("Lilex" as CFString, 14, nil) // 从 `assets/fonts/lilex/Lilex-Regular.ttf` 加载验证缺字
   CTFontGetGlyphsForCharacters(base, [0x2461], …) // → glyph 0，确认缺字
   // 级联后 PingFang 的 advance = 14px (1.0 em)，cell_width = advance('m' in Lilex) = 8.4px (0.6 em)
   ```
2. 纯测试：`ambiguous_shrink_factor(shaped=14, cell=8.4) == Some(0.6)` 判定超宽，`can_append` 对不同 `font_size` 返回 `false`，超宽 `cell_count=2` 且 `extra_offset` 右移（见 `terminal_element_tests.rs` 的 `spec_20260826_*`）。
3. 人工视觉：终端执行 `echo '锚点②③ 输出与文档记录一致。'` 与 `echo "②③"`，确认 `②③` 保持原字号、占两格、与后续字符不叠印（首版缩字会小一号，此版已修复）。

## 涉及代码

- `src/features/terminal/zed_view/terminal_element.rs`：`layout_grid`（`cell_width`/`rem_size` 参数、`shaped_cache`、`ambiguous_shrink_factor` 判定、`extra_offset` + `cell_count=2` 占两格）、`BatchedTextRun::can_append`（新增 `font_size` 比较）、`::paint` 保持 `Some(cell_width)` 不变
- `src/features/terminal/zed_view/terminal_element_tests.rs`：`spec_20260826_*` 5 项
- `Cargo.toml`：`unicode-width = "0.2"`（网格宽度判定复用同一语义）

## 关键词

歧义宽度, East Asian Ambiguous, 粘在一起, 叠印, ②③, ①⑴, ★☆, force_width, cell_width, Lilex, PingFang, 字体回退, CoreText, Alacritty, unicode_width, BatchedTextRun, shrink factor
