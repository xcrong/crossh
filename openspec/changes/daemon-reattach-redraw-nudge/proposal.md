# Proposal

## Why

重连后全屏 TUI 常常乱屏：首屏早已被 512 KiB 回放环挤掉，重放只是没见过的屏幕的差量往空屏上画。当前 `view.rs:358-371` 的无条件双 `Resize` 原本只为 `p10k` 右提示错位逼 shell 重画（见该处注释），对全屏 TUI 无效，只能靠用户按键自救。按 `tty7` 经验（`#1122/#1136`），背靠背两次改尺寸会被内核合并为一次 `SIGWINCH`，程序读到尺寸未变就跳过重画——此处为移植假设，待实现用单测固定。

## What Changes

- Daemon 新增 `TerminalModes` 折叠（至少 `ALT_SCREEN`），记住每个会话是否处于 `alt-screen`，以及回放环是否还留着进入 `alt-screen` 的那次切换与首画（新增状态，非修复存量——crossh 现无 `at_prompt` 存量，见 `feed.rs`）。
- 回放环按 `WinSize` 分段（上限 `MAX_SEGMENTS=64`，超限合并最旧两段），`resize` 时封段；驱逐时不在 `CSI/OSC/UTF-8` 中间截断，驱逐同步清理已攒下的查询。
- 重放时按段发送 `Size + Snapshot`，并先发送进入当前模式所需的前导序列（复用 `Output` 承载），让客户端先进入正确的 `alt-screen` 再画差量。
- 仅当“活着 + 前台有程序 + 在 `alt-screen` + 环已丢首画 + 距上次请求超冷却”时，daemon 做一次条件重画请求：`rows>1` 时切到 `rows-1`（否则 `rows+1`），50ms 后恢复最新真实尺寸。只动 PTY，不通知客户端改尺寸，中间若有真实 `resize` 以最新值为准。短时间重复接入只请求一次。Unix 问内核前台进程组；Windows 恒不发起（只回放）；当前无 NativeSSH 后端，条件恒按 Pty 路径；observer 重连同样可触发 PTY 侧请求但不影响布局。
- 无客户端期间 daemon 按固定深色主题代答 `DA / OSC 11` 等查询，避免 `Claude Code` 类程序 hang 死；浅色偏差由后续真实查询纠正。
- 新增前景判定防护：重放是否声称“停在提示符”以内核前台为准，避免把刚重建的 `alt-screen` 当垃圾清掉。
- 移除视图侧无条件双 `Resize` 抖动，改为依赖 daemon 侧条件 nudge + 能力门控。

## Capabilities

### New Capabilities

- 无新增 capability 路径（本变更只改 `session-host`，新增行为以 ADDED 形式归属现有 capability，不新增 spec 文件）。

### Modified Capabilities

- `session-host`: 接入时回放最近输出的行为从“纯字节回放 + 前端抖尺寸”升级为“分段回放 + 模式前导 + daemon 侧条件重画请求”，新增全屏程序重连自愈要求。

## Impact

- 受影响代码：`crates/crossh-session/src/feed.rs`、`session.rs`、`spawn.rs`、`serve.rs`、`protocol.rs`、`client.rs`、`handshake.rs`，`crates/crossh-terminal-view/src/view.rs`、`daemon_link.rs`。
- 协议：新增 `Size=22` 帧（JSON `{cols,rows}`，不占 `seq`），`Prelude` 复用 `Output` 原始字节；前提是先让 `decode_frame/read_frame` 学会跳过未知帧并补双向兼容单测，否则旧 GUI 在回放中途会被打断（现状 `protocol.rs:180-181` 未知 kind 直接 `Err`）。
- 依赖：无新增依赖；`portable-pty` 的 `resize` 行为不变。
- 对照实现：`../tty7/crates/tty7-core/src/daemon/pane.rs` 的 `redraw_nudge / ReplayRing / replay_prelude / detached_queries`（`#1122/#1136/#711/#857`）。
