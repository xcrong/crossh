# Windows 本机 bash 指向 WSL，release.sh 必须用 Git-bash

## 症状

- `bash scripts/release.sh X.Y.Z --allow-dirty` 报 `unexpected worktree change: .trae/documents/...`，但该文件明明被全局 gitignore（`~/.gitignore_global`）忽略；`git status --porcelain` 直接跑又看不到它。
- 同一脚本接着报 `line 123: cargo: command not found`，而 PowerShell 里 `cargo --version` 正常。
- 版本 manifest 已被改写但 Cargo.lock 未同步，工作区停在半途状态。

## 根因

本机（Windows）`bash` 命令解析到 **WSL bash**（`HOME=/home/xcrong`），不是 Git-bash：

1. WSL 里的 git 读不到 Windows 侧的全局 gitignore（`C:\Users\Xcrong\.gitignore_global`），`.trae/` 下 IDE 生成文件以 untracked 身份出现，触发 `release.sh` 的白名单检查 `check_worktree_paths`（它用 `--untracked-files=all`，比裸 `git status` 更严）。
2. WSL PATH 不含 Windows cargo（scoop rustup 装在 `C:\Users\Xcrong\scoop\apps\rustup-gnu\current\.cargo\bin`），`release.sh` 同步 Cargo.lock 时找不到 cargo。

## 稳定规则

1. 在本机跑 repo 内 bash 脚本（`release.sh`、`check-architecture.sh` 等）一律用 Git-bash 全路径：`& 'C:\Program Files\Git\bin\bash.exe' scripts/release.sh ...`；裸 `bash` 是 WSL。
2. 判别方法：`bash -c 'echo $HOME'` —— `/home/<user>` 是 WSL，`/c/Users/<user>` 是 Git-bash。
3. repo 级 `.gitignore` 才是跨环境可靠的忽略层（全局 gitignore 只对 Windows 侧 git 生效）；IDE 产物目录（如 `/.trae`）应进 repo `.gitignore`。
4. release.sh 中途失败后重跑是安全的：manifest 改写幂等，重跑会补齐 Cargo.lock 并完成 commit + tag。

## 验证方法

- `& 'C:\Program Files\Git\bin\bash.exe' -c 'command -v cargo'` 应解析到 scoop rustup 路径。
- `bash -c 'git status --porcelain=v1 --untracked-files=all'`（WSL）与 PowerShell 直跑 `git status` 对比，可复现忽略差异。

## 当前实现

- `/.gitignore` 增加 `/.trae`（`dd130d0`）。
- `scripts/release.sh` 的白名单检查：`check_worktree_paths`（`scripts/release.sh:91`）。

## 搜索关键词

`WSL`, `Git-bash`, `release.sh`, `cargo command not found`, `全局 gitignore`, `.gitignore_global`, `unexpected worktree change`, `.trae`, `check_worktree_paths`, `白名单误报`, `bash 解析`
