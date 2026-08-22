# main_screen 增量路径空追加下溢崩溃

## 症状

- `crossh-agent` regular 模式运行中突然崩溃，`attempt to subtract
  with overflow`，panic 位于 `crates/crossh-tui/src/main_screen.rs:63`
  （`plan_rows` 的 `cap2 = viewport - start_row + 1`）。
- 崩溃时界面通常正在转 `Working -` spinner（或候选浮层内容在变），
  对话已填满视口。
- debug 构建必现（release 构建在 debug_assertions 关闭时静默绕回，
  产生错误几何而不是崩溃）。

## 根因

对话行数 `old_len >= viewport`（视口已满）后，任何 transcript 不变、
但 dock 内容变化的增量帧都会走「纯追加」分支，此时 `n = 0`、
`first_row = visible_old + 1 = viewport + 1 > viewport` → `cap = 0` →
`scroll_n = 0` → `start_row = viewport + 1` → `cap2 = viewport - start_row
+ 1` 无符号下溢。

这类"空追加"帧并不少见：status spinner 每 ~80ms 一帧、候选浮层
内容随键入变化但高度固定（高度固定正是避免 2J 闪烁的既有规则）、
光标位移动。一旦对话超过一屏，第一个这类帧即崩溃。

## 规则

- `plan_rows` 内所有视口几何行数一律用饱和运算：
  `cap2 = viewport.saturating_sub(start_row - 1)`（`start_row` 受
  `.max(1)` 约束，`start_row - 1` 安全）；`start_row` 越过视口底时
  容量为 0，`skip = m - 0 = m` 整段跳过（调用方对 `m=0` 不写任何行）。
- 该函数是纯几何计算，按「调用方保证 m 为新行数」写：追加 0 行是
  合法输入，不得假定 `m >= 1`。

## 验证方法

```sh
cargo test -p crossh-tui --lib \
  spec_main_screen__empty_append_when_viewport_full_does_not_panic
```

回归测试构造：第一帧 transcript 11 行（视口 9 行满），第二帧
transcript 不变、dock 内容变化（高度不变）→ 修复前 63:20 下溢 panic，
修复后输出含新 dock 行且无 `\x1b[2J`。

PTY 复核：两次 `/help` 填满视口后输入 `/model zw/gpt`（候选内容变化
帧），无 panic、进程存活、`Esc` 正常退出 0。

## 搜索关键词

`attempt to subtract with overflow`, plan_rows, cap2, start_row,
视口已满, empty append, n=0, 空追加, main_screen, Working spinner,
运行中崩溃, saturation arithmetic