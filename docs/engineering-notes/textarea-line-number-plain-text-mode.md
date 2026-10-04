# 行号栏此前只对 `CodeEditor` 生效，`TextareaState` 画不出来

## 症状

想给 Note 编辑器加行号，`TextareaState::line_number(true)` 编译不过：

```
error[E0599]: no method named `line_number` found for
              struct `InputBaseState<TextareaMode>`
note: the method was found for `InputBaseState<EditorMode>`
```

## 根因

`InputBaseState` 的 builder 按"输入形态"分成几个 impl 块，方法只挂在它够得着的那个上：

- `impl<M: InputModeKind>` —— 所有模式共有。
- `impl<M: MultiLineMode>` —— 多行模式（`TextareaState` / `EditorState`）共有。
- `impl InputBaseState<EditorMode>` —— 只对代码编辑器。

`line_number` / `set_line_number` 原本在最后一个块里，只对 `EditorMode` 可见。
更底层的 `LayoutMode::line_number()` 也只读 `CodeEditor` 变体，其余一律返回 `false`。

数据侧同样受限：`LayoutMode::PlainText { tab, rows }` 根本没有存行号开关的字段。

值得注意的是**渲染侧本来就是通用的**。`element.rs` 只通过
`state.mode.line_number()` 这一个开关决定是否画行号，并且已经正确处理了软换行
（续行补空占位，只在首行显示数字）。所以缺的是开关，不是绘制。

## 规则

- 给某个输入形态加共有能力时，先确认它在**哪个 impl 块**里，再确认底层
  `LayoutMode` 的对应变体有没有存这个字段的字段。两处都要改，只改一处会出现
  "方法可见但开关不生效"或"开关能设但不画"。
- `LayoutMode` 的匹配尽量用穷尽 `match` 写 `line_number()` / `set_line_number()`，
  这样新增变体时编译器会指出所有需要处理的地方。
- 加完跑 `cargo test -p crossh-editor`，`mode.rs` 里有逐变体的断言
  （`test_plain` / `test_code_editor` / `test_auto_grow`）会覆盖到。

## 验证

```bash
cargo test -p crossh-editor input::base::mode
cargo test -p crossh-editor          # 全量，含行号渲染与命中测试的回归
```

## 关键词

`line_number`, `LineNumber`, `LayoutMode::PlainText`, `LayoutMode::CodeEditor`,
`TextareaState`, `EditorMode`, `MultiLineMode`, `E0599`, `行号`, `gutter`,
`crossh-editor`, `input/base/mode.rs`, `input/base/element.rs`
