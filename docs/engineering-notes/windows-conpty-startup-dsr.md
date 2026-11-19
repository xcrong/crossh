# Windows ConPTY 启动光标查询导致无头命令不退出

## 症状

- `crossh run -- <命令>` 在 Windows daemon 下永远不返回，输出里只有 `ESC[6n`（DSR 6，光标位置查询）。
- `cargo test -p crossh-session` 的 `cli_sessions::cli_full_cycle` 在 Windows CI 上 20 秒超时；macOS / Linux 全绿。
- 隔离测试 `spawn::windows_tests::one_shot_cmd_exits_without_closing_pty` 在 10 秒内 `try_wait()` 始终返回 `None`，会话输出只有 `"\x1b[6n"`。

## 根因

`portable-pty 0.9.0` 的 Windows 后端用 `PSEUDOCONSOLE_INHERIT_CURSOR` 创建 ConPTY。ConPTY 启动时先向“终端”发 DSR 6（`ESC[6n`）询问光标位置，收到 `ESC[<row>;<col>R` 回报前不启动被托管的子进程。

- GUI 会话没事：alacritty 的 `device_status(6)` 会回 `ESC[row;colR`。
- 无头 `crossh run` 没人应答：`cmd /C ...` 根本没开始执行，进程不退出，客户端等 `Exited` 永远等不到。

daemon 是 PTY 的宿主，ConPTY 视角里的“终端”也是 daemon；把这次握手留给 GUI 客户端会让无头路径永远缺一个应答者。

## 稳定规则

1. daemon 的读线程代答 **仅限输出流开头** 的那一条 ConPTY 启动查询，并把查询本身从回放/广播中吞掉；客户端看到的是干净输出，GUI 接入后不会重复应答。
2. 一旦已出现其他输出，后续 DSR 属于应用自己的终端交互，必须原样透传，由 GUI 终端回答。
3. 非 Windows 平台没有这条握手，`StartupCursorProbe` 保持透传实现，不做任何改写。
4. 读线程需要回写 PTY 时共享 `Arc<Mutex<Writer>>`；`write_all` 与自动应答共用同一把锁。

## 验证方法

- Windows lib 测试 `spawn::windows_tests::one_shot_cmd_exits_without_closing_pty`：ConPTY 下 `cmd /C echo` 必须在 10 秒内报告退出码 0。
- `cli_sessions::cli_full_cycle`：Windows 上一次性命令的输出与退出码（0 与 7）端到端透出，不挂起。
- 三平台 CI：run `44437641063` 的 Linux / Windows / macOS job 全绿。
- 本机（macOS arm64）跑 `cargo test -p crossh-session`、`cargo clippy -p crossh-session --all-targets -- -D warnings`；Windows 行为以 CI 为准。

## 当前实现

- 握手过滤与回写：`crates/crossh-session/src/spawn.rs` 的 `StartupCursorProbe` 与 `ChildSession::spawn` 读线程
- 写入端共享：`ChildSession.writer`（`Arc<Mutex<Box<dyn Write + Send>>>`）
- Windows 隔离测试：`crates/crossh-session/src/spawn.rs::windows_tests`

## 搜索关键词

`ConPTY`, `INHERIT_CURSOR`, `PSEUDOCONSOLE_INHERIT_CURSOR`, `ESC[6n`, `DSR 6`, `device_status`, `portable-pty`, `Windows 不退出`, `crossh run 挂起`, `one-shot hang`
