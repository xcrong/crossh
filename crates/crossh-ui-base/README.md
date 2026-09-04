# crossh-ui-base

无样式行为 / 几何地基：行为归地基，表现归应用。

## 职责

- **语义元素**（`button`）：稳定 `ElementId`、keyed 焦点、disabled 门控、点击转发（含 GPUI 原生 Enter / Space → click 合成）。零默认颜色 / 内外边距 / 圆角。
- **布局骨架**（`layout`）：`h_flex` 横向居中容器、`scroll_y` 纵向滚动容器（占位 id 由调用方覆盖）。只定结构，不管颜色间距。
- **开关状态机**（`toggle`）：`next_state` 点击翻转纯函数。
- **定位几何**（`positioner`）：`place_popup` / `clamp_origin` 纯函数，无 Window 依赖，可单元测试。
- **列表状态**（`list_state`）：`ListStatus` 空态语义 + `ListCursor` 受控选择光标（值语义）+ `next_index` 纯函数。
- **文本选区护栏**（`text_selection`）：`normalize_selection` / `clamp_to_char_boundary` / `is_valid_selection` / `should_highlight_selection` / `use_cursor_split` / `resolve_selection` / `selection_or_cursor` 纯函数，零 `gpui` 依赖。
- **单行文本快照**（`text_state`）：`SharedTextState` 值 / 光标 / 锚点 / IME 状态（`new` + `with_*` + 原名 readers + `selection` / `has_selection` / `selection_or_cursor`）。

## 边界

- 只依赖 `gpui`（与工作区同 pin）。**禁止**依赖 `crossh-ui`、`crossh-ui-component`、`crossh-core` 及任何业务 crate；依赖只能向下。
- 跨 seam 的结构体**无 `pub` 字段**：`new()` + builder + readers；bool 用 `is_` / `has_`，混合字段用 `with_` / 原名 readers。
- 绝不把 context 缩写成 `ctx`；统一用 `window` / `cx`。
- 受控值：接受当前值、报告变化，永不静默改应用状态。
- `hover` / `active` 用 GPUI 原生，地基层不自造。

## 入口

`crossh_ui_base::{BaseButton, ButtonPress}`、`{h_flex, scroll_y}`、`{next_state}`、`{PopupRequest, PopupPlacement, place_popup, clamp_origin}`、`{ListCursor, ListStatus, StepDirection, next_index}`、`{SplitAxis, SplitHandleSide, drag_width, drag_height, clamp_size}`、`{PanelSide, RAIL_AVATAR_SIZE, RAIL_AVATAR_GAP, clamp_panel_width, available_main_width, handle_side_for}`、`{normalize_selection, clamp_to_char_boundary, is_valid_selection, should_highlight_selection, use_cursor_split, resolve_selection, selection_or_cursor}`、`{SharedTextState}`。上层只走 crate-root，不许深挖 `crossh_ui_base::<模块>::` 私有路径。

主题适配在 `crossh-ui-component`：读地基状态 → 查 `crossh-ui` token → 生成 GPUI 样式。地基签名中不出现任何颜色 / 字体 / 圆角类型。

## 验证命令

```sh
cargo check -p crossh-ui-base
cargo test -p crossh-ui-base
cargo fmt --check
./scripts/check-architecture.sh
```

不跑 workspace 全量测试；不改 `CARGO_TARGET_DIR`；许可文本见 `LICENSE-APACHE` / `LICENSE-MIT`。
