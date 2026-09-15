# macOS GUI 最小 PATH 下 /usr/bin/git shim 因 Xcode license 失效

## 症状

- 工作区底部状态栏 Git 段整体消失（分支/短 hash/`↑↓+~?!` 徽章全无），路径段仍在；内置终端里 `git` 完全正常，`git rev-parse --show-toplevel` 正常。
- `/tmp/crossh/run.log` 只有 `local session opened` + `git status refresh loop alive`，无任何 Git 错误。

## 根因

- 状态栏与终端用的不是同一个 `git`：终端 shell 的 `PATH` 完整（本机 `command -v git` → `/opt/homebrew/bin/git`）；GUI 进程由 launchd 启动（实测 PPID 1），只继承 `/usr/bin:/bin:/usr/sbin:/sbin`，只能命中 `/usr/bin/git`。
- `/usr/bin/git` 是 Xcode CLT shim，license 未接受时直接拒绝干活：`You have not agreed to the Xcode license agreements...`，exit 69，无 stdout。Xcode/CLT 更新会重置 license 接受状态，所以“之前好好的，某次发版/更新后突然没了”。
- 调用链静默吞错：`git_command()` 裸 `Command::new("git")` 继承 GUI 环境 → `try_git_output` 失败映射为 `None`（crossh-core 无 log 依赖）→ `render_workspace_status_bar` 以 `if let Some(status)` 跳过整个 Git 段。整条状态栏一直在渲染，消失的只是 Git 段。
- 已排除：`safe.directory`（全局已为 `*` 且目录属主与 uid 一致；dubious ownership 应为 exit 128 + `detected dubious ownership`，本次终端侧实测 exit 0 且 stderr 为空）；v0.34/v0.35 的 diff 未动 git 二进制寻址（`767ce53` 只加短 hash 显示、`7b69cec` 的 fetch 轮询与 `GIT_TERMINAL_PROMPT=0` 只动 `run_git_output`、`c0f72a1` 纯 Zed rev bump）。

## 稳定规则

1. “终端里 git 可用”不能作为后台 git 可用的证据；排障必须用 GUI 最小 PATH 对照复现。
2. 后台探测类失败禁止静默 `None`：至少记一条带 `cwd` 的日志，否则 `run.log` 无痕，下次还得从头查。
3. GUI 进程直接 `Command::new` 的调用默认只能看到系统最小 PATH；若要复用登录 shell/Homebrew 的 PATH（如 `editor_launcher::effective_path` 的合并策略），必须在 core 内自包含实现（`scripts/check-architecture.sh` 禁止 core 引用 app/GPUI 层）。本次暂不改，只记录。
4. 用户侧恢复：`sudo xcodebuild -license` 后重启 App；Xcode/CLT 更新后复发先查这一条。

## 验证方法

- 终端侧：`command -v git` 应为 Homebrew 路径；`git -C <repo> status --porcelain=v2 --branch -z --untracked-files=normal` exit 0 且 stderr 为空（用独立文件接 stderr 确认，不要与 stdout 混在一起看）。
- GUI 侧复现：`env -i PATH=/usr/bin:/bin:/usr/sbin:/sbin git -C <repo> status --porcelain=v2 --branch -z --untracked-files=normal`；若报 license 文案且 exit 69 即定案。
- 恢复确认：`sudo xcodebuild -license` 后 `/usr/bin/git -C <repo> status ...` 不再报错，重启 App，下个 5s 轮询后状态栏 Git 段恢复。
- 排除信任问题：`git config --global --get-all safe.directory` 与目录属主/`id` 对照；dubious ownership 的特征是 exit 128，与本次 exit 69 不同。

## 涉及代码

- `crates/crossh-core/src/git/command.rs`：`git_command` / `try_git_output`
- `crates/crossh-core/src/git_status.rs`：`inspect`
- `src/features/workspace/view.rs`：`render_workspace_status_bar` / `render_git_status` 的 `Some` 门控
- `src/features/workspace/shell/mod.rs`：`refresh_git_status`（5s 轮询）
- `src/features/editor_launcher.rs`：`effective_path` 合并 PATH 先例（git 未用）

## 关键词

macOS, GUI, PATH, launchd, /usr/bin/git, Xcode license, xcodebuild -license, exit 69, CLT shim, Homebrew git, 状态栏, git_status, try_git_output, 静默 None, safe.directory, dubious ownership
