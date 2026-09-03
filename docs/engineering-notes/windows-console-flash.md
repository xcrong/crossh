# Windows 控制台黑窗口

## 症状

- 从资源管理器/开始菜单/安装包启动 `crossh.exe`，主界面旁边多一个常驻黑色控制台窗口，关掉它主程序跟着退出。
- 主界面工作期间不定时闪过黑窗口：打开 Git/Note 视图、Git 状态轮询、自更新等待时尤其明显。

## 根因

两类问题，同一机制：Windows 给每个控制台子系统进程分配 conhost 窗口。

1. 常驻黑窗口：`crossh`、`crossh-git`、`crossh-note` 是控制台子系统（`Cargo.toml` 无 `windows_subsystem`，`build.rs` 只嵌了图标），GUI 启动必然附带一个控制台。
2. 闪黑窗口：`git.exe`、`tasklist.exe` 也是控制台子系统。`Command::new("git").output()` 这类输出捕获调用只要没带 `CREATE_NO_WINDOW`，每次执行建一个可见控制台、用完即毁——Git 视图轮询一次闪一次；`tasklist` 在更新等待循环里每 250ms 调一次，闪成一片。

## 稳定规则

1. 三个 GUI 入口（`src/main.rs`、`src/bin/crossh-git.rs`、`src/bin/crossh-note.rs`）设 `#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]`，`main` 第一行调 `crossh_core::process::attach_parent_console()`，保证终端里 `--help` 等 CLI 输出仍然可见。
2. 所有输出捕获类调用走 `crossh_core::process::no_window()`（`CREATE_NO_WINDOW`，其他平台无操作）；git 调用统一走 `git::command::git_command()`，它是全仓唯一的 `git -C` 构造点。
3. 后台常驻的同伴进程（git/note/editor 拉起）继续用 `detach()`（独立进程组），不要换成 `no_window`。
4. `crossh-updater` 保持控制台子系统（手动运行能看到报错），但自更新链路的三处拉起（updater 本体、`tasklist` 轮询、新版拉起）必须经 `installer.rs` 内的 `no_window()`。
5. 新增 `Command::new` 先回答走哪个 helper：`sibling_command`（同伴 GUI）、`no_window`（捕获输出）、`detach`（后台常驻）、裸调用（仅限测试与 unix-only 代码）。

## 验证方法

- `scripts/check-architecture.sh` 通过；`crossh-core` 单测覆盖 `no_window` 不 panic 且不改 program。
- 本机不装交叉工具链、不跑 Windows target 构建（仓库规则）；Windows 行为由 CI 的 `release.yml` Windows x86_64 任务拥有，合并前确认该任务绿色。
- 真机确认：新构建双击启动无附带控制台；Git 视图轮询期间无闪窗；跑一次自更新全程无闪窗。

## 当前实现

- 无窗口 helper 与父控制台挂回：`crates/crossh-core/src/process.rs`
- git 唯一构造点：`crates/crossh-core/src/git/command.rs::git_command`（`ops.rs` 已收敛）
- updater 三处拉起：`crates/crossh-update/src/installer.rs`（`spawn_updater`、`process_is_running`、`launch`）
- GUI 入口子系统声明：`src/main.rs`、`src/bin/crossh-git.rs`、`src/bin/crossh-note.rs`

## 搜索关键词

`黑窗口`, `闪`, `conhost`, `windows_subsystem`, `CREATE_NO_WINDOW`, `DETACHED_PROCESS`, `AttachConsole`, `no_window`, `控制台子系统`, `console flash`
