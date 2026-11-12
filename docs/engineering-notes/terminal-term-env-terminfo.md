# 会话 shell 的 TERM 必须有 terminfo 条目

## 症状

- GUI 终端里逐键输入显示翻倍（敲 `ls` 显示 `llsls`），粘贴整行显示双份（`ls` 显示 `lsls`）。
- 服务端证据全干净：daemon 只收到一次输入（`input 1B` 日志单行）、回放环字节正确、本地/远端两条 VT 入向对拍一致。
- 回放环里 shell 自己打出的重绘行已是乱的：prompt 之后是 ` l` + 绿 `l` + `s` + 绿 `ls`，中间没有任何 `\r` 或光标移动；还夹着残缺 SGR（裸 `\x1b[38m`）。

## 根因

会话进程的 `TERM=xterm-256-color`（带横杠）在 macOS 没有 terminfo 条目（`infocmp` 直接失败；标准名是无横杠的 `xterm-256color`）。
zsh 启动时查不到 `cr`/`el` 等能力，ZLE 退化成行尾硬追加：每键回显一次、高亮重印一次，视觉即翻倍。
prompt 本身漂亮是因为 p10k/高亮插件的样式多为硬编码 ANSI，只有走 terminfo 的行编辑坏掉——所以"看着像渲染 bug，实则是 shell 环境 bug"。
`insert_zed_terminal_env`（`crates/crossh-core/src/terminal/shell_select.rs`）同时喂给本地终端与 daemon 会话，两边同病；daemon 侧 `SpawnOptions` 的 `xterm-256color` 默认值会被它覆盖，改一处即全修。

##  durable 规则

- 给 PTY 子进程设 `TERM` 时，只用 `infocmp <name>` 在本机验证过的名字；注释写明无横杠是故意的。
- 输入翻倍先分层：daemon `input` 日志（发了几次）→ 回放环（壳打出了什么）→ 网格日志（VT 算出什么）→ 屏幕。不要跳过中间层猜渲染。
- 相关症状不同根因见 [GPUI 按键事件双重分发](gpui-key-event-double-dispatch.md)（真双发按键，本案是单发双显）。

## 验证

- `cargo test -p crossh-core term_has_terminfo_entry_name` 锁死标准名。
- 真 daemon + 按键级 `Input` 帧复现：`echo $TERM` 回 `xterm-256color`；逐键 `l`/`s` 的重绘字节含 `\x08`/CUB 光标移动且字母不重复。
- 旧会话的环境在 spawn 时已固定，修后必须重启 daemon 并开新会话；`crossh capture` 旧环仍是乱的属正常。

## 关键词

`TERM`, `terminfo`, `infocmp`, `xterm-256color`, `ZLE`, `行编辑`, `输入翻倍`, `llsls`, `insert_zed_terminal_env`, `cr/el`, `dumb`, `硬追加`
