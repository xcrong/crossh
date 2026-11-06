# `Keystroke::parse` 只吃静态串，`secondary-*` 绑定会在运行期 panic

## 症状

升级 Zed rev 到 `1478fb5`（2026-11-06）后：

- 编译期：`Keystroke::parse(key)` 报 `E0521 borrowed data escapes outside of closure`
  （`key: &str`），终端按键转发、测试辅助函数等动态输入全部报错。
- 运行期：`crossh-editor` 单测一次性 88 个失败，panic 信息只有

```
called `Result::unwrap()` on an `Err` value: InvalidKeystrokeError { keystroke: "secondary-enter" }
   at gpui/src/keymap/binding.rs:47
```

真实的 `init` 里 `KeyBinding::new("secondary-enter", ...)` 同样会 panic，即启动即崩。

## 根因

GPUI 把 `secondary`（macOS 的 cmd / 其它平台的 ctrl）从「解析期固定别名」改成
「按宿主系统解析」：

- `Keystroke::parse(&'static str)` 现在只接受静态串，且**不解析 `secondary-`**，
  因为解析需要 `OperatingSystem`。
- 需要解析别名或处理动态输入时用 `Keystroke::parse_with_os(source, cx.platform_info().os)`。
- `Modifiers::secondary()` / `Modifiers::secondary_key()` 也从常量变成了
  `secondary(cx: &App)`：要先知道宿主系统才知道 secondary 是 cmd 还是 ctrl。
- `KeyBinding::new` 内部仍走 `Keystroke::parse`（静态、无 OS），所以带 `secondary-`
  的静态绑定不再合法；`KeyBinding::load(..., cx)` 才会用 `cx.platform_info().os`。
  测试平台上 `platform_info().os` 是 `Unknown`，于是 `secondary-` 直接判为非法。

## 规则

- 升级 Zed rev 后，凡 `Keystroke::parse(<非 &'static str>)` 一律改
  `Keystroke::parse_with_os(s, cx.platform_info().os)`；纯字面量可以保留 `parse`。
- 静态 `KeyBinding::new` 不要写 `secondary-`。本项目沿用按平台 `#[cfg]` 拆成
  `cmd-*` / `ctrl-*` 两行的写法（`crates/crossh-editor/src/input/base/state.rs`），
  与文件里既有的 `cmd-]` / `ctrl-]` 风格一致，也避免依赖宿主 OS 探测。
- `Modifiers::secondary()` / `secondary_key()` 的调用点补 `cx`（`&App` 或
  `&Context<T>` 皆可，`Context<T>: Deref<Target = App>`）。
- 编译期过不代表安全：`Keystroke::parse` 的静态化只会拦住变量，常量字符串里的
  `secondary-` 要等到 `KeyBinding::new` 运行期才炸。升级后请跑一遍依赖键盘绑定的
  测试（`cargo test -p crossh-editor --lib` 是暴露它的最短路径）。

## 验证

```bash
cargo check --workspace --all-targets
cargo clippy --workspace --all-targets -- -D warnings
cargo test -p crossh-editor --lib     # 88 个失败在此清零
cargo test -p terminal --lib
```

## 关键词

`Keystroke::parse`, `parse_with_os`, `InvalidKeystrokeError`, `secondary-enter`,
`secondary-`, `KeyBinding::new`, `KeyBinding::load`, `Modifiers::secondary`,
`secondary_key`, `E0521`, `platform_info`, `OperatingSystem`, `Zed 升级`
