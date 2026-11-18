# GPUI 单行输入鼠标选区

## 症状

共享 `TextInput` 点击只把焦点放进去，光标不跟随点击位置；拖拽、双击、三击、Shift+点击都没有选区。地址栏、侧栏搜索、Git 筛选与 Note 搜索/查找同病。键盘编辑、IME 与选区高亮本身正常。

## 根因

- `TextInput` 根节点只绑了 `.on_click` 聚焦，没有“点击坐标 → 文本字节偏移”的命中测试；`EntityInputHandler::character_index_for_point` 返回 `None` 也不是鼠标路径，GPUI 不会替普通 div 做选区。
- 组件无状态，`cursor` / `anchor` 归 owner 的 `TextEditingState`；鼠标事件必须把字节偏移回传给 owner。
- 绝对定位 canvas 的 bounds 是父元素 border 内的 padding box（Taffy `absolute_position_area = container_size - border`），文本从 padding 之后开始：内容区起点 = `canvas.origin + padding_x`。
- 按下用元素级 `.on_mouse_down`（受 hitbox 约束）；指针拖出输入框后只能用 canvas paint 里注册的 `window.on_mouse_event`。监听器每帧清空，capture 和 bubble 都会调用，需只处理 bubble。
- 应用失活可能收不到 `MouseUp`，`selecting` 会一直挂着；用 `.on_mouse_down_out` 在别处按下时补一个 `Up`。

## 规则

- `TextInput` 只负责 `x → 字节偏移`（`byte_offset_for_x` / `text_offset_for_point`），不在组件里存光标；owner 用 `TextEditingState::pointer_down/drag/up` 落点。
- 偏移必须在 UTF-8 字符边界上二分，等距取右，越界 clamp；不要按字符数或 UTF-16 截断。
- 掩码输入（`display` 非空）不发射指针事件，避免用掩码长度反推原文偏移。
- 双击选词（点在文本末尾时回退到最后一个字符），三击全选（单行即整段），Shift+点击保留锚点。
- 组件测试要 `debug_bounds` 时给元素加 `.debug_selector(...)`；`#[gpui::test]` 可用 `simulate_event(MouseDownEvent/MouseMoveEvent/MouseUpEvent)` 驱动真实事件链。

## 验证

- `cargo test -p crossh-core text_editing`
- `cargo test -p crossh-ui-component`
- `cargo test -p crossh-note-view note_search_text_input_mouse_places_caret_and_selects`（窗口级：点击、拖拽、双击、三击、Shift+点击）
- macOS 手动烟测：地址栏、侧栏搜索、Git 筛选与 remote 输入、Note 搜索/查找。

## 关键词

`TextInput`, 鼠标选区, 光标定位, 拖选, 双击选词, 三击全选, Shift+点击, I-beam, byte offset, 字符边界, GPUI, canvas bounds, padding box, `on_mouse_down`, `on_mouse_event`, `selecting`, `debug_selector`
