# Design

## Context

现状见 `proposal.md` 与 `docs/engineering-notes/session-daemon-protocol.md`：`crossh-session` 持 PTY master、做 512 KiB 单块字节环（`feed.rs/session.rs`），`serve.rs:attach_and_stream` 发 `Attached -> Output* -> ReplayEnd`，前端是全新 VT 从零解析。前端 `view.rs:ReplayDone` 无条件发 `Resize(cols+1)+Resize(cols)` 原本只为 `p10k` 首帧（见 `view.rs:363-365` 注释）。对照实现是 `../tty7/crates/tty7-core/src/daemon/pane.rs`（`redraw_nudge / ReplayRing / replay_prelude / detached_queries`，`#1122/#1136/#711/#857`）。

## Goals / Non-Goals

**Goals:**
- 全屏程序首画已丢时发起条件重画请求；处理 `SIGWINCH` 的程序自愈，不处理者仍需按键（诚实边界）。
- 首画仍在、主屏 shell 时零打扰，不抖、不闪。
- 回放永不从转义序列中间开始。

**Non-Goals:**
- 不做服务端 VT 快照 / 截图 diff（daemon 仍不持网格，保持现有分层；`TerminalModes` 折叠与 `HeadCut` 是最小解析器，非网格）。
- 不改输入权、workspace 布局、认证与版本协商。
- Windows 首版无自愈：无 `tcgetpgrp`，恒不发起、只回放，由 CI 断言。当前无 NativeSSH 后端，条件恒按 Pty 路径；若新增后端，照抄 `runs_foreground_program` 分裂逻辑。

## Decisions

1. **分段环替代单块环。**
   `RingSegment{ size: TermSize, bytes } + VecDeque`，上限 `MAX_SEGMENTS=64`（同 `tty7`），超限合并最旧两段；`resize` 时封段（空尾直接改尺寸，非空尾推新段）。`replay()` 按段发 `Size + Snapshot`。总预算仍是 512 KiB + 段开销。备选：整屏 VT 快照——否，违背“daemon 不持 VT 网格”边界。

2. **驱逐对齐到解析边界。**
   引入最小 `HeadCut` 状态机（`Ground/Esc/EscIntermediate/Csi/Str`）+ `at_boundary`（非续接字节），`evict(n)` 后继续推进到边界；被驱逐字节同步折叠进 `head_modes` 并 `take_queries()` 清掉已攒查询，防止脱离期查询堆积终身。备选：按原样截断——否，会复现 `tty7#857` 的 `33;C` 碎片。

3. **记住终端模式，决定要不要 nudge。**
   Daemon 侧折叠 `TerminalModes`（至少 `ALT_SCREEN`，复用 `feed` 同批字节推进）。`attach` 时算 `ring.modes()`：当前在 `alt-screen` 但环内已无 `?1049h` 即判定首画已丢。备选：前端判断——否，前端只有重放字节，看不到被丢掉的前史。

4. **条件 nudge，只动 PTY。**
   条件：`alive + 前台有程序 + 在 `alt-screen` + 首画已丢 + 距上次 nudge 超冷却（2s）`。前台判定：Unix 问 `tcgetpgrp` vs shell pid（见 `tty7:has_foreground_command`），失败按无前台处理；Windows 恒按无前台处理。动作：`rows>1 ? rows-1 : rows+1` 立刻发，`sleep 50ms` 后读最新真实尺寸恢复（`run_redraw_nudge` 独立线程，不挡 `attach`）。50ms 是关键：背靠背两次改尺寸会被内核合并为一次 `SIGWINCH`（`tty7` 注释原话）。恢复时读最新值，中间真实 `resize` 不被覆盖；睡眠窗内 daemon 退出/会话关闭（`settled()->None`）则放弃；连续 attach 以后到者尺寸为准（先到线程读到最新值自然收敛）。nudge 期间不发客户端 `Size`，不改环分段。observer 重连同样可触发 PTY 侧请求（只动 PTY，不影响布局，与 `observer` 静默规则正交）。备选：保留前端双 `Resize`——否，无延迟、无条件，既可能无效又打扰。

5. **协议定死（C1）。**
   新增 `Size=22` 帧（JSON `{cols,rows}`，不占 `seq`；`Attached.end_seq` 仍是总流字节偏移，`Size` 只做分段标记）。`Prelude` 复用 `Output` 原始字节（进入当前模式的最简序列，由 `modes.replay_prelude(head,ring)` 算出，环自带者优先）。前提：先让 `decode_frame/read_frame` 学会跳过未知帧（`len+kind+payload` 格式支持跳过，参考 `handshake` 未知标志忽略原则），并补“新 daemon→旧 GUI 跳过未知帧仍可显示”单测；否则旧 GUI 在回放中途收到第一帧 `Size` 即整条失败（现状 `protocol.rs:180-181` 直接 `Err`）。

6. **脱离期代答 + 新增前景判定。**
   无订阅时 `take_queries()` 攒下 `DA/OSC11`，由 daemon 按固定深色回（`DA→ESC[?6c`，主题深色 `ESC[?997;1n`；daemon 无窗口可取主题，浅色偏差由后续真实查询纠正），有订阅时由客户端正常应答。`replayed_at_prompt = stored_at_prompt && !foreground_command` 为本次新增状态（存 `FeedState`），同样问内核，避免 `133;B` 残留导致客户端清掉刚重建的 `alt-screen`（`tty7#711`）。

7. **删前端抖动，能力门控四格。**
   `view.rs:ReplayDone` 双 `Resize` 删除，改为看能力 `reattach-nudge-v1`：新 daemon+新 GUI 走 daemon nudge，前端不抖；新 daemon+旧 GUI 靠旧端容错跳过 `Size` 后单流显示；新 GUI+旧 daemon 按旧单流解析且不抖（无自愈，需按键，文档诚实标注）；旧+旧保持现状双抖（回滚态）。`note_viewport` 去重与 `observer` 静默忽略保持不变。

## Risks / Trade-offs

- [Risk] 50ms 内用户真实 resize 被 nudge 覆盖 → Mitigation：恢复时重读最新尺寸，而非缓存值；`settled()->None` 则放弃。
- [Risk] 部分 TUI 不处理 `SIGWINCH` → Mitigation：spec 只承诺“发起重画请求”，不承诺自愈，并在文档注明需按键；这是协议极限。
- [Risk] `TerminalModes` 折叠与 `vte` 漂移 → Mitigation：只折叠 `alt-screen` 等少数模式，单元测试对拍 `tty7` 用例。
- [Risk] 协议新增帧打断旧 GUI → Mitigation：先修旧端跳过未知帧（C1 前提），再发新帧；`Size` 缺失退化单流。
- [Risk] `tcgetpgrp` 在 sandbox 无前台时失败 → Mitigation：按无前台处理，不 nudge，只回放。
- [Risk] Windows 无自愈被当 bug → Mitigation：spec 明确 Windows 只回放，CI 断言恒不 nudge，发布说明不写“Windows 已支持自愈”。

## Migration Plan

- daemon 先上（带旧端容错），再上 GUI。旧 GUI 忽略新增帧需先发版，否则不发 `Size`。
- 四格：新+新：分段+前导+nudge；新 daemon+旧 GUI：跳过 `Size` 单流显示；新 GUI+旧 daemon：单流解析、不抖、无自愈；旧+旧：现状。
- 回滚任一侧即退化（可能乱屏，需按键），不丢会话、不丢布局。
- 发布跑定向用例：`vim/htop` 跑久→关 GUI→同尺寸重开自愈；`p10k` 不错位。

## Open Questions

- 无。本轮已定死：`MAX_SEGMENTS=64`、`Prelude` 最小集（进 `alt-screen` 所需）、Windows 恒不 nudge、能力名 `reattach-nudge-v1`。
