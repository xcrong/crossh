# 终端歧义宽度字符占两格渲染（修复 ②③ 叠印）

> 复制自 `docs/specs/template.md`。只描述行为与验收，不写实现方案。

## 元数据
- 状态：`in-progress`
- 创建：2026-08-26
- 批准：2026-08-27
- 相关 ADR：无
- 相关 issue / 路线图项：无
- CI 平台影响：`全部`（共享渲染路径；视觉验证仅 macOS 本地可做，见平台影响）

## 背景

终端里 ②③①⑴★☆ 等 East Asian Ambiguous 字符与后续字符叠印（用户截图：`锚点②③`
两个圈完全重叠）。根因是三层不一致：

1. **网格层**：`alacritty_terminal` 用 `unicode-width` 窄宽度语义，歧义宽度字符按
   **1 格**计宽（`term/mod.rs` 的 `c.width()`），无 wide-spacer。
2. **字体层**：默认字体 Lilex 无 U+2461 等字形（已验证 cmap；Menlo 亦无），CoreText
   级联回退到 PingFangSC，字形步进 1.0 em（14px @ 字号 14），而 cell_width = Lilex
   `'m'` 步进 0.6 em（8.4px）→ 字形实际约 **1.67 格宽**。
3. **渲染层**：gpui `apply_force_width_to_layout`（`line_layout.rs:849`）把第 N 个基
   字形强制钉在第 N 个格子边界，无视自然步进 → 下一字符被钉进溢出字形的一半上，叠印；
   再后续字符各自被钉回连续格子，故整行只有相邻一两个字叠印。

上游 Zed 存在同样问题（fork 继承）。首版按“缩字入格”（按 `cell/shaped` 缩小字号到恰好
1 格）实现后，视觉上字形小了约 40%（14px→8.4px），用户反馈“小了几号”。因此改为
**渲染层占两格、保持原字号**：超宽字符在渲染层按 2 格排版、后续列整体右移一格，
字形保持原大小不叠印，行宽每字符增加一格（与中文全角一致，接受该代价）。
## 目标

1. 歧义宽度字符在终端中不再与相邻字符叠印，字形保持原字号完整渲染。
2. 普通字符（ASCII、Lilex 已覆盖符号、真全角 CJK/emoji）渲染结果与现状一致。
3. 渲染层为超宽字符分配 2 格宽度、后续列右移，接受行宽每字符增加一格的代价；
   网格层（PTY 光标、选择坐标）仍按 1 格计宽的语义保持不变（已知背景/选区会有 1 格
   偏移，默认背景下不可见）。

## 非目标

- 不改网格层宽度语义（PTY 侧歧义字符仍按 1 格计宽），不做 East-Asian 宽度用户开关。
- 不 fork / patch `alacritty_terminal` 或 gpui。
- 不新增设置项、不改持久化格式。
- 不处理网格已按 2 格计宽的字符（其渲染已正常）。

## 行为契约

1. 当某字符按网格占 1 格且其实际字形步进超过一个格宽（如回退到 CJK 字体的
   ②③①⑴★☆），渲染时该字形保持原字号、按 2 格宽度排版，不与相邻格字形叠印；
   同行后续字符整体右移一格绘制（渲染列 = 网格列 + 已累计超宽字符数）。
2. 当字符字形步进不超过格宽加 1px 容差（与 gpui `force_width` 的容差一致）时，
   排版结果与现状一致：不拆分 batch、不占两格、不额外测量引入可见差异。
3. 当字符按网格占 2 格（CJK、emoji，含 spacer 跳过逻辑）时，排版行为不变。
4. 渲染层占两格仅改变文本绘制列，不改变网格坐标语义：PTY 光标、IME 定位、复制
   仍按原网格列；背景/选区高亮仍按原网格列（默认背景下不可见的 1 格偏移为已知取舍）。
5. 超宽字符自身的单元格样式（前景色、背景、粗体/斜体、下划线、超链接 hover 色）
   保持正确应用。
6. 相同样式相邻字符的 batch 合并规则保持现状：仅超宽字符从 batch 中独立，其余
   字符合并行为不变。

### 测试分层

- 契约 1 的"阈值判定"与"batch 拆分（占两格、独立）"固化为纯测试：注入测得的字形步进与
  格宽，断言判定结果与 batch 结构（超宽字符独立且 `cell_count=2`、其余合并不变）。字形步进的
  真实测量属平台行为，不在纯测试范围。
- 契约 2、3、5、6 由同一纯测试层覆盖（不超宽不拆不占两格、2 格字符不动、样式随
  batch 保持、合并规则不变）。
- 契约 1 的端到端"不叠印"与契约 4 的网格语义不变，由验收清单最后一项的
  macOS 人工视觉确认覆盖；自动化测试不模拟平台字体回退。

## 边界与错误

- 字形步进测量不可用或返回异常值（headless 测试字体、测量失败）时，回退为不占两格
  （即现状行为，不额外分配渲染列），不得 panic 或阻塞渲染。
- 测量必须复用 gpui 的行排版缓存，不得造成每帧全量重排级别的性能退化；仅对可见区
  内"网格 1 格"的候选字符测量。
- 超宽字符在渲染层按 2 格排版；字形本身不超宽时不额外占格。
- batch 拆分不得破坏背景色区域合并（`merge_background_regions`）与现有
  `can_append` 判定。

## 接口与状态变更

无。渲染内部行为修正，无公开 API、设置项、持久化、wire 格式变更。

## 平台影响

- 修改位于共享渲染路径（`src/features/terminal/zed_view/terminal_element.rs`），
  全平台生效；各平台"触发字符集"由各自字体回退决定（macOS 为 PingFang 回退的
  ②③①⑴★☆ 等）。
- 本地仅能视觉验证 macOS；Linux/Windows 由 GitHub Actions 完成编译、clippy、测试
  级验证（无视觉 CI），视觉差异不在本 spec 验收范围，予以声明。

## 涉及纪律

- [x] Logic must not depend on UI：改动限于 terminal view 渲染层；缩放判定逻辑写成
  可纯测的函数（不依赖 gpui 状态），测量入口薄封装。
- [ ] Feature-owned settings：无新设置。
- [ ] 图标纪律：不涉及。
- [x] 文件规模 < 2000 行：`terminal_element.rs` 在 `scripts/check-architecture.sh`
  白名单内（上游 fork 维护策略），新增代码继续留在该文件。
- [x] 工程笔记 / ADR 同步义务：根因（歧义宽度 × 字体回退 × force_width 量化）收尾时
  写入 `docs/engineering-notes/`。
- [x] 响应式 UI：字形缩放不改变网格与布局尺寸，不涉及。

## 影响模块

- `src/features/terminal/zed_view/terminal_element.rs`：`layout_grid`、
  `BatchedTextRun`、paint 路径。
- `src/features/terminal/zed_view/terminal_element_tests.rs`：新增纯测试。
- `docs/engineering-notes/`：收尾时新增根因笔记。

## 验收清单

- [x] spec 评审通过（AI 评审 + 人批准）
- [x] 行为契约全部固化为失败测试并确认失败原因正确（Red）
- [x] 最小实现通过聚焦测试（Green）
- [x] `cargo fmt --check`
- [x] `scripts/check-architecture.sh`
- [x] `cargo clippy --all-targets -- -D warnings`
- [x] `cargo test --workspace`
- [ ] 声明的平台 CI job 通过（非本机平台：提交后由 Actions 验证，spec 状态
      保持 in-progress 直到通过）
- [x] 结构性决策提炼进 ADR（如有）并登记 docs/architecture.md（预期：无）
- [x] 调试根因合并进 docs/engineering-notes/（预期：是）
- [x] 新增行为合并进 docs/testing.md 关键行为矩阵（如有）
- [ ] 用户可观察效果人工确认：在终端输出含 `锚点②③` 的中文行，②③ 不再叠印，
      且同行其余字符位置不变（代码层已验证；待下次构建后人工视觉确认）
