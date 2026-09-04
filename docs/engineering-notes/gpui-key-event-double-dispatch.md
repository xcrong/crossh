# GPUI 按键事件双重分发导致输入翻倍

## 症状

命令面板输入 `new` 显示 `nneeww`：每个字符被插入两次。

## 根因

同一 `KeyDownEvent` 被两处消费：`ModalField` 内层 `on_key_down` 与 shell 根节点
`handle_shell_key_down` 都调用 `handle_command_palette_key` 做文本插入。
GPUI 按键事件从焦点元素向上冒泡，内层处理完没有 `stop_propagation`，
冒到根节点又插入一遍。对照组：重命名/默认命令弹窗根节点直接 `return`、
只有内层处理，所以从不翻倍。

## 规则

- **消费即截断**：`on_key_down` 里凡改状态/消费按键，必须在同一函数内
  `stop_propagation`，不依赖调用点。`git/input.rs` 与 `compose.rs` 已是此模式；
  `handle_command_palette_key` 现也在内部截断（见其文档注释）。
- **一输入一归属**：同一字段的编辑分发只注册一处；模态类根节点只屏蔽
  （`stop` + `return`）不编辑。`handle_shell_key_down` 的面板分支只作失焦兜底。
- **反例**：终端 `key_down` 故意不截断，为放行 `⌘T` 等全局快捷键。
  是否截断看"是否消费"，不要一刀切全加 `stop`。
- 新增带 `entity` 的输入框时，沿祖先链检查有无根级别同名分发。

## 同类排查结论（2026-09-04）

覆盖 compose / modal（重命名+默认命令）/ 侧栏搜索 / git 各输入框与终端：
仅面板存在双分发。modal 与搜索框未处理键会继续冒泡到全局 keymap
（如改名时 `⌘W` 仍关闭标签），现状保留、有意不动；改动将改变模态下
全局快捷键可用性，需单独决策。

## 验证

- `cargo test -p crossh command_palette`（含翻倍回归与关键词测试）。
- 手动：`⌘/ctrl+K` 开面板逐字输入，确认单字符；`rustfmt --check` 相关文件。

## 关键词

`nneeww`, `双倍输入`, `输入翻倍`, `stop_propagation`, `on_key_down`, `冒泡`, `double dispatch`, `二次分发`, `消费即截断`, `command palette`, `handle_command_palette_key`
