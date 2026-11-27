# Tasks

## 1. 回放环分段与驱逐对齐

- [ ] 1.1 实现 `RingSegment{size,bytes}` 分段环（`MAX_SEGMENTS=64`，超限合并最旧两段）与 `resize` 封段，并以 `cargo test -p crossh-session` 验证分段快照按段携带几何、段数上限生效
- [ ] 1.2 实现 `HeadCut` 驱逐对齐（`CSI/OSC/UTF-8` 不半截）+ 驱逐同步 `head_modes.feed + take_queries`，并以单测验证切点在序列中间时推进到边界、`33;C` 不碎片、旧查询不堆积

## 2. 终端模式与重画判定

- [ ] 2.1 在 `feed` 同批字节中折叠 `TerminalModes`（至少 `ALT_SCREEN`），被驱逐字节同步折叠进 `head_modes`，并以单测验证环内/环外模式判定正确
- [ ] 2.2 实现 `redraw_nudge` 判定真值表（alive×前台×alt×首画在环×冷却×`rows==1`边界×observer），Unix 问 `tcgetpgrp`、Windows 恒 false，并以单测覆盖五种不触发（含 `rows==1` 走 `rows+1`、observer 不影响布局）与一种触发；`rg -n "tcgetpgrp|foreground" crates/crossh-session/src` 可定位探针

## 3. Daemon 侧 Nudge 与协议

- [ ] 3.1 实现 `run_redraw_nudge`（临时尺寸立刻发，50ms 后读最新尺寸恢复，`settled()->None` 则放弃，独立线程不挡 `attach`），只动 PTY 不发客户端 `Size`，并以单测验证合并窗口、最新尺寸恢复、连续 attach 以后到者为准
- [ ] 3.2 先修 `decode_frame/read_frame` 跳过未知帧并补双向兼容单测（新 daemon→旧 GUI 跳过 `Size` 仍可显示；新 GUI→旧 daemon 无 `Size` 单流退化），再接 `attach/observe` 的分段 `Size(=22,不占seq)+Snapshot` 与 `Prelude(复用Output)`，`Attached.end_seq` 仍是总流偏移，以 `attach` 单测验证回放后跟 `ReplayEnd` 且冷却内只 nudge 一次
- [ ] 3.3 无订阅时代答 `DA/OSC11`（固定深色：`DA→ESC[?6c`，主题 `ESC[?997;1n`）+ 新增 `replayed_at_prompt(stored && !foreground)` 前景判定，并以单测验证脱离期不 hang、全屏不被误判、浅色偏差后续可纠正

## 4. 前端收敛与集成验证

- [ ] 4.1 删除 `view.rs:ReplayDone` 无条件双 `Resize`，改为看能力 `reattach-nudge-v1`（新+新走 nudge；新 GUI+旧 daemon 单流解析且不抖；旧 GUI 靠容错跳过），`note_viewport` 去重与 observer 静默不变，并以 `cargo test -p crossh-terminal-view` 验证；`rg -n "gpui" crates/crossh-session/src` 零命中 + `scripts/check-architecture.sh` 通过
- [ ] 4.2 集成单测：`attach` 分段回放 + nudge 时序（首画丢/仍在/主屏三态），Windows 恒不 nudge 只回放，并以 `cargo test -p crossh-session --test '*'` 验证
- [ ] 4.3 手动验收清单（非自动化）：全屏程序跑久（首画出环）→关 GUI→同尺寸重开自愈整屏；`SIGWINCH` 不敏感程序仍需按键属符合预期；`p10k` 首帧不错位；`cargo fmt --all --check` 通过
