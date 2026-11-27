# Review: daemon-reattach-redraw-nudge

- 评审日期：2026-11-27
- 评审对象：`proposal.md` / `specs/session-host/spec.md` / `design.md` / `tasks.md`
- 结论：**带条件通过**
- 说明：按 `AGENTS.md — OpenSpec Change Review Gate` 执行。本次为双路子代理评审（A：行为↔代码，B：设计↔协议）汇总。`apply` 仅对下列 `C1-C8` 放行，归档前必须逐条关闭并在本文件标记。

## 核对方式

不采信提案自述。行为主张用 `rg` / 读源码核对（`crates/crossh-session/src/`、`crates/crossh-terminal-view/src/`、`../tty7/.../daemon/pane.rs`）。

| 主张 | 证据 | 结论 |
| --- | --- | --- |
| 单块 512 KiB 环、无分段 | `crates/crossh-session/src/session.rs:13` `REPLAY_LIMIT_BYTES`；`:17-72` 单`Vec<u8>`拼接；`feed.rs:82-84` 单字节流快照 | 成立 |
| `attach_and_stream` 流程 | `serve.rs:741-795` 订阅→快照→`Attached`→`Output*`→`ReplayEnd`→forwarder；`protocol.rs:40-45` kind 定义 | 成立 |
| `view.rs` 无条件双 Resize | `crates/crossh-terminal-view/src/view.rs:358-371` 无条件发 `Resize(cols+1)+Resize(cols)` | 成立，但动机被挪用，见 F2 |
| `note_viewport` 去重+observer忽略 | `view.rs:464-475` 去重；`serve.rs:592-596` observer静默忽略 | 成立 |
| `spawn.rs resize` 经 portable-pty | `spawn.rs:291-302` `master.resize(PtySize)` | 成立 |
| `feed.rs` 无 TerminalModes、Query丢弃 | `feed.rs:1-98` 仅`Osc7501/Provider+cwd`；`:63-64` `Query=>{}` | 成立（缺失判断正确） |
| tty7 对照物存在 | `../tty7/.../pane.rs:3110-3165 HeadCut`、`:3532-3612 nudge`、`:3396-3500 detached/prompt` | 符号成立，编号未核 issue |
| 协议“新增只能追加、旧端忽略” | `protocol.rs:14` 追加原则；`handshake.rs:81-106` 未知标志忽略 | 帧级忽略**不成立**，见 F4 |

## 发现

### F1（待改，P0）协议兼容阻塞：新 kind 会打断旧 GUI

`design.md:33,52` 称“kind 数值追加、旧端忽略”。实际 `protocol.rs:180-181` `decode_frame` 对未知 kind 直接 `Err(InvalidData,"unknown kind")`，`client.rs:37-55 read_frame` 直接 `?` 向上传播，`Attached` 回放中途第一帧新 kind 即整条 attach 失败。`session-daemon-protocol.md:13` 的“旧端忽略”仅指 JSON 字段级，不是帧级。二选一未决（新增 vs 复用 `Resize/Output`）不可留给实现。`Attached.end_seq` 单流偏移与多段 `Size+Snapshot` 的 seq 归属（Size 是否占 seq）亦未定义。

### F2（待改，P0）`at_prompt` 前提虚假 + 双 Resize 动机挪用

`proposal.md:14`、`design.md:38`、`specs/...:80-87` 写“修复 `replayed_at_prompt` 误判”。`rg at_prompt|replayed_at_prompt crates/crossh-session/src/` 零命中；存量只有 `program/cwd`（`feed.rs:30-37`）。这是把 `tty7 pane.rs:3528` 概念当 crossh 存量 bug。同段 `view.rs:363-365` 注释只说 p10k 逼 shell 重画，artifact 引申为“企图逼全屏 TUI 重画”无代码佐证，应标为 tty7 移植假设。

### F3（待改，P0）Spec 结果承诺与 Design 免责自相矛盾

`specs/session-host/spec.md:32,39` “使程序全量重画、无需按键即恢复整屏”“收到两次 SIGWINCH 后全量重画”是确定性保证；`design.md:45` 承认部分 TUI 不处理 SIGWINCH 仍需按键。按门禁必须以 Design 为准降级 Spec，否则部分程序必 fail，测试写不出。

### F4（待改，P1）Spec 混入实现参数、无观测手段

`spec.md:37` `50ms/rows-1`、`spec.md:57` “最新真实尺寸”、任务“独立线程不挡 attach / 2s”均是实现细节。Spec 应只保留可观测项（发起条件、布局/环不受影响、真实 resize 优先）并补 WHEN/THEN 可执行断言。`Size+Snapshot/Prelude/rows-1/50ms/HeadCut` 等实现词应全部移出 spec。

### F5（待改，P1）`Size/Prelude` 与能力标志无规范

全 change 无帧定义：`Size` kind 数值、JSON 结构、`Prelude` 编码（独立 kind 还是复用 `Output`）、缺失时退化、`reattach-nudge-v1` 在 `handshake.rs:13-22` 的声明/协商/缺失语义、新旧 daemon×新旧 GUI 四格退化表全部缺失。`tasks.md:3.2,4.1` 因此不可验收。

### F6（待改，P1）前景判定平台作用域 + 三处边界遗漏

`spec.md:81` 把 Unix `tcgetpgrp` 写成无条件 MUST，与 `design.md:17,31` Non-Goal/降级矛盾；crossh 现无任何 `tcgetpgrp/foreground` 代码。另：`design.md:31` 缺 `rows==1` 护栏（tty7 为 `rows>1?rows-1:rows+1`，`pane.rs:3584-3590`）；NativeSSH（tty7 仍 nudge）vs Windows Pty（恒不 nudge，`pane.rs:2536-2590`）被混为“按能力降级”；`observe` 是否 nudge 未定（tty7 两处都调，`pane.rs:2507,2602`）。

### F7（待改，P1）文件笔误 + Capabilities 自述矛盾 + 四件套复述

`design.md:5` `feed.rs/sessions.rs` 实际仅 `session.rs`（单数）。`proposal.md:19-25` 写“新能力：无”却新增 4 条 ADDED，模板填错。proposal 7 条≈design 7 条≈spec 5 条≈tasks 7 格逐句复述同一机制，未分层（proposal 讲为什么 / spec 只讲可观测 / design 讲权衡 / tasks 讲步骤）。

### F8（待改，P2）Open Questions 虚假 + 实现缺口

`design.md:58` 写“无”却同段留“段数上限与 512 KiB 是否同调留给实现”。另缺：`MAX_SEGMENTS` 取值与合并规则（tty7 `MAX_RING_SEGMENTS=64`）；`evict` 漏 `take_queries` 清理（tty7 `pane.rs:3324-3325`）；脱离期代答 `dark` 主题来源（tty7 取自 `machine::appearance`，daemon 无窗口，固定值还是上报未定）；50ms 睡眠窗内 daemon 退出/会话关闭（`settled()->None` 则放弃）与连续 attach 串行语义；`tasks.md:4.2` 堆积（自愈+p10k+fmt+架构一框）且缺双向兼容单测、observer 真值表、`rg gpui` 架构显式项。

## 必须关闭的条件

`apply` 前逐条落实并回写标记（带条件通过只放行下列各项）：

- **C1（F1）定死协议编码并修复旧端容错。** 删掉“或复用”二选一，明确 `Size/Prelude` 的 kind、方向、载荷、`end_seq` 语义；若用新 kind，先让 `decode_frame/read_frame` 学会跳过未知帧并补双向兼容单测（新 daemon→旧 GUI 不中断，新 GUI→旧 daemon 单流退化+不抖）。
- **C2（F5）定义能力标志。** `reattach-nudge-v1` 按 `handshake.rs` 风格逐条注明不支持时怎么办，覆盖四格并诚实写明旧 daemon 下无自愈。
- **C3（F2）澄清 `at_prompt` 前提。** 给出 crossh 内真实位置（文件:行）或改写为“新增前景判定”，定义状态存放层与观测方式；双 Resize 动机改称移植假设。
- **C4（F3/F4）Spec 去强承诺、剔实现参数。** THEN 改为“发起一次条件重画请求”，删“无需按键”“全量重画”保证；`50ms/rows-1/线程/2s` 移入 design/tasks，spec 只留可观测断言。
- **C5（F6）前景判定加平台作用域与护栏。** Unix 问内核，Windows/NativeSSH 明确降级（无后端则明写恒走 Pty 路径）；补 `rows==1` 分支；明确 observer 是否 nudge；失败即不 nudge 用例入 spec/tasks。
- **C6（F7）修正笔误与分层。** `sessions.rs→session.rs` 全仓清零；如实填写新增能力；spec 删实现词；proposal 只留影响面，行号归 design。
- **C7（F8前半）补段数与清理。** 明确 `MAX_SEGMENTS` 与合并最旧两段规则，`evict` 补 `take_queries`；`Open Questions` 如实列出并至少定死前两项。
- **C8（F8后半）补主题与任务。** 脱离期代答明确主题来源或固定值影响半径；拆 `tasks.md:4.2` 为自动化验收+手动清单；补 2.2 真值表（alive×前台×alt×首画×冷却×rows边界）、双向兼容测试、`rg gpui+check-architecture.sh` 显式项。

## 条件状态（2026-11-27 修订后复核：全部关闭）

- C1 待关闭 → **已关闭。** `design.md:Decisions-5` 定死 `Size=22（不占seq）+Prelude复用Output`，前提先修 `decode_frame/read_frame` 跳过未知帧；`tasks.md:3.2` 补双向兼容单测；`protocol.rs:180-181` 现状已在设计中引用。
- C2 待关闭 → **已关闭。** `design.md:Decisions-7` 与 `specs/...:新旧版本互操作` 补四格退化表，能力名 `reattach-nudge-v1`，旧 daemon 下诚实标注无自愈。
- C3 待关闭 → **已关闭。** `proposal.md:What Changes` 改“新增前景判定防护（非修复存量）”，`design.md:Decisions-6` 注明本次新增存 `FeedState`，`specs/...:引入前景判定` 去掉存量口径；双 Resize 动机改称移植假设（`proposal.md:Why`）。
- C4 待关闭 → **已关闭。** `specs/...:全屏程序重连后请求重画` 改为“发起一次条件重画请求”，删“无需按键”“全量重画”保证；`50ms/rows-1/线程/2s` 移入 `design.md:Decisions-4`，spec 只留可观测断言。
- C5 待关闭 → **已关闭。** `specs/...:引入前景判定` 加平台作用域（Unix 问内核，Windows 只回放）；`design.md:Decisions-4` 补 `rows>1?rows-1:rows+1`、Windows 恒 false、无 NativeSSH 说明、observer 可触发但不影响布局；`tasks.md:2.2` 真值表覆盖。
- C6 待关闭 → **已关闭。** `design.md:Context` `sessions.rs→session.rs`；`proposal.md:Capabilities` 注明无新增路径、ADDED 归属现有 capability；proposal 只留影响面，行号归 design。
- C7 待关闭 → **已关闭。** `design.md:Decisions-1/2` 明确 `MAX_SEGMENTS=64` 与合并规则，`evict` 补 `take_queries`；`Open Questions` 改为无（本轮已定死）。
- C8 待关闭 → **已关闭。** `design.md:Decisions-6` 固定深色代答并说明浅色纠正；`tasks.md:4.2/4.3` 拆分为集成单测+手动清单，补双向兼容、真值表、`rg gpui+check-architecture.sh` 显式项。

评审仍为只读评审之外的修订后复核（改产物文本，未改代码）。`openspec validate daemon-reattach-redraw-nudge --strict` 通过。
