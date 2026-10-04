# `wrap_line` 新增 `IndentAdjustment` 参数打断换行调用

## 症状

升级 Zed rev 后编译报 `E0061`：

```
error[E0061]: this method takes 3 arguments but 2 arguments were supplied
   --> crates/crossh-editor/src/text/inline_flow.rs:580:14
    |  .wrap_line(&wrap_fragments, wrap_width)
    |           argument #3 of type `IndentAdjustment` is missing
```

## 根因

Zed 给 `LineWrapper::wrap_line` 增加了第三个参数 `IndentAdjustment`，用于控制软换行后
续行的缩进：

- `NoIndent`：续行从第 0 列开始。
- `SameIndent`：续行对齐原行前导空白（enum 的 `#[default]`）。
- `ExtraColumns(u32)`：在原行缩进基础上再加 N 列，Zed 的列表项续行用它做悬挂缩进。

升级前的 `wrap_line` 没有这个参数，缩进行为是硬编码的「取原行前导空白」，
所以 `SameIndent` 与旧行为等价。新增该参数的动机是让调用方能显式表达意图，
而 Zed 内部为方便下游迁移把 `SameIndent` 设成了默认值。

## 规则

- 升级 Zed rev 时，`LineWrapper::wrap_line` 的调用点若报 `E0061`，按旧行为传
  `IndentAdjustment::SameIndent`；只有在确实希望续行顶格或悬挂缩进时才选其它取值。
- `IndentAdjustment` 走 `gpui::` 顶层导出，调用方需要在自己的 `use gpui::{...}` 中补上。
- 改这类换行逻辑要同时跑 `input::display_map::text_wrapper` 下的测试，它们直接覆盖
  缩进与续行边界。

## 验证

```bash
cargo check --workspace --all-targets
cargo test -p crossh-editor input::display_map::text_wrapper
```

## 关键词

`wrap_line`, `IndentAdjustment`, `SameIndent`, `NoIndent`, `ExtraColumns`, `LineWrapper`,
`E0061`, `line_wrapper`, `换行`, `续行缩进`, `悬挂缩进`, `crossh-editor`, `Zed 升级`