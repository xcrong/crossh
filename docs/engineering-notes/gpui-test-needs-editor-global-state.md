# Note 窗口：查找面板的测试需要手动初始化 `GlobalState`

## 症状

给 Note 编辑器加上查找功能后，新增的测试 panic：

```
thread 'note_find_closes_when_entering_preview' panicked at
  gpui/src/app.rs:2271:32:
no state of type crossh_editor::global_state::GlobalState exists
```

同一个文件里既有的 12 个测试全部通过，只有触发查找面板的新测试失败；
单独跑也稳定失败，不是并发干扰。

## 根因

`GlobalState` 由 `crossh_editor::init(cx)` 注册（`crates/crossh-editor/src/lib.rs`）。
生产路径上 `crossh-note` 的 `init()` 会调它，所以真实运行时没问题。

但测试用的是 `cx.add_window(NoteWindow::new)`，直接构造窗口，绕过了 `init()`。
既有的 Note 测试之所以不炸，是因为它们从不触发查找面板；
而查找会走到编辑器的命中/高亮渲染路径（`element.rs` 的 `layout_search_matches`），
那条路径读 `GlobalState`。

## 规则

- Note 视图的测试里，凡是会触发编辑器**命中测试 / 高亮 / 文本选择**的路径，
  先 `cx.update(crossh_editor::init);` 再建窗口。`NoteWindow::new` 本身不做这件事。
- 这类"构造窗口绕过 `init`"的测试写法是 `crossh-note-view` 独有的；
  `crossh-editor` 自己的测试里 `GlobalState::init` 由内部 helper 兜底
  （见 `text_selection.rs` / `popover.rs` 的测试）。
- 判断标准很简单：测试是否触发了**跨实体**的交互。纯状态机断言不需要，
  涉及鼠标命中、选区、渲染的准备阶段就需要。

## 验证

```bash
cargo test -p crossh-note-view
```

连跑多次确认不 flaky（`NoteWindow::new` 会打开真实笔记库 `~/Library/Application
Support/crossh/note.db`，测试间存在真实的 IO 竞争，但不应影响断言结果）。

## 关键词

`GlobalState`, `no state of type`, `crossh_editor::init`, `TestAppContext`,
`add_window`, `gpui::test`, `查找面板`, `search_session`, `highlight`,
`crossh-note-view`, `测试初始化`
