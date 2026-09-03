---
name: release
description: Use when releasing a new crossh version (0.x.0), writing hand-written release notes, or working with scripts/release.sh and .github/workflows/release.yml. Covers version bump, docs/release-notes maintenance, tag annotation with --cleanup=verbatim, and the GitHub Release publish flow.
---

# 发布新版本（Release）

> 适用：`0.x.0` 功能版本与 `0.x.y` 修复版本。发布日志**必须手写**，禁止直接使用 `git log` 自动生成（见 `docs/release-notes/v0.25.0.md` ）。

## 前置检查

- 分支：`main`，`git remote get-url origin` 存在。
- 工作区：`git status --porcelain` 干净；若已准备好手写日志与脚本改动，则用 `--allow-dirty`（见下）。
- 版本一致性：`bash scripts/package-version.sh Cargo.toml` 与 `crates/*/Cargo.toml` 全部一致（`scripts/release.sh:67` 的 `ensure_versions_match` 会校验）。
- 远程 tag 不存在：`git ls-remote --tags origin refs/tags/vX.Y.Z` 为空。

## 标准流程（6 步）

### 1. 确定版本号与范围

```bash
git tag --list | sort -V | tail -5          # 上一版本
git log v0.24.1..HEAD --oneline --no-merges # 本次待发布提交
bash scripts/package-version.sh             # 当前 Cargo 版本
```

`0.x.0` 递增 minor，`0.x.y` 仅修 bug。确认 `HEAD` 已包含所有待发布提交（通常已领先 `origin/main` 若干提交）。

### 2. 手写发布日志（必须）

**位置**：`docs/release-notes/vX.Y.Z.md`（仓库内归档，随 tag 一起提交）。

**模板**（精简，面向用户，中文为主）：

```markdown
# vX.Y.Z — 一句话标题

> YYYY-MM-DD · 基于 vPrev 的 N 个提交

## ✨ Highlights

### 亮点功能名

- 入口/窗口/能力/搜索/标签/持久化 等用户可见描述

## 💅 Improvements

- 布局/交互/打包 等体验改进

## 🛠 Fixes

- 存储/渲染/性能/编辑器 等修复

## 🔧 Chores

- Zed 同步至 <rev>（日期，领先 N 提交）等内部维护

## 📦 升级说明

无需手动迁移 / 回退方式 / 平台说明
```

**写作要求**：

- 按 Highlights / Improvements / Fixes / Chores 分组，不罗列 `git log --oneline` 原样。
- 每个 bullet 说明**用户可见行为**，必要时标注文件路径与 （如 ``）。
- `SKILL.md` 与 `docs/release-notes/` 是唯一真相来源，`git log` 仅作素材。
- 控制长度：GitHub Release 页约 40–60 行，避免超长技术细节堆砌（详见 `v0.25.0` 范例）。

**反例**：直接把 `git log --oneline` 粘贴为 Release Notes（已被 `release.yml:176` 的 hand-written 优先逻辑取代）。

### 3. 本地打版（commit + tag）

```bash
# 已准备好 docs/release-notes/vX.Y.Z.md 时（推荐）：
bash scripts/release.sh 0.25.0 --allow-dirty

# 完全干净的工作区（无预先改动）：
bash scripts/release.sh 0.25.0
```

`scripts/release.sh` 的关键行为（`scripts/release.sh:80`）：

- `is_allowed_path` 白名单：`Cargo.lock` / `Cargo.toml` / `README.md` / `scripts/*` / `crates/*/Cargo.toml` / `docs/release-notes/v*.md` / `.github/workflows/release.yml`。白名单外的未提交文件会触发 `die "unexpected worktree change"`。
- 批量改写 `Cargo.toml`/`crates/*/Cargo.toml` 的 `version`，`cargo check --workspace` 同步 `Cargo.lock`。
- `git add` 阶段：`Cargo.lock` + `README.md` + `scripts/*` + `crates/*/Cargo.toml` + `docs/release-notes/vX.Y.Z.md` + `.github/workflows/release.yml`（后两者无改动时为 no-op）。
- `git commit --no-verify -m "chore: release vX.Y.Z"`。
- `git tag -a vX.Y.Z -F <tmp> --cleanup=verbatim`：`tmp` 为 `Release vX.Y.Z\n\n` + 手写日志全文。**必须 `--cleanup=verbatim`**，否则以 `#` 开头的 Markdown 标题会被 `strip` 丢弃（`v0.25.0` 已踩坑并修复）。

验证：

```bash
git log --oneline -3
git cat-file -p vX.Y.Z | head -50   # 确认标题与章节完整
bash scripts/package-version.sh     # 应为新版本
git status                         # 干净，领先 origin/main 1 commit + 1 tag
```

### 4. 推送触发发布

```bash
git push origin main
git push origin vX.Y.Z
# 或一次性：bash scripts/release.sh 0.25.0 --push --allow-dirty
```

推送后 `release.yml`（`on.push.tags: v*`）自动触发。

### 5. 观察 GitHub Actions

```bash
gh run list --workflow="Release" --limit 3
gh run view <run-id> --json jobs,conclusion,status
```

流水线（`.github/workflows/release.yml:17`）：

1. `validate`（`macos-latest`）：校验 tag 版本与全部 `Cargo.toml` 一致。
2. `build` 矩阵（`fail-fast: false`）：
   - macOS aarch64/x86_64 → `scripts/package.sh`
   - Linux x86_64/aarch64 → `scripts/package-linux.sh`
   - Windows x86_64 / aarch64(experimental, `continue_on_error`) → `scripts/package-windows.ps1`
3. `release`（`ubuntu-latest`）：
   - `Generate release notes`：优先 `cp docs/release-notes/v${VERSION}.md RELEASE_NOTES.md`，缺失则回退 `git log` 自动生成；若手写文件未含 `Full Changelog` 则自动追加 `compare/PREV...TAG` 链接（`release.yml:176`）。
   - `Generate checksums` + `Generate update manifest`（`scripts/generate-update-manifest.sh`）+ `Sign/Verify update manifest`（`CROSSH_UPDATE_SIGNING_KEY`，fail-closed，）。
   - `softprops/action-gh-release@v3` 发布 `dist/*.{zip,tar.gz,AppImage,deb,rpm}`（含 `*.AppImage.tar.gz` 一键安装包） + `sha256sums.txt` + `stable.json`，`body_path: RELEASE_NOTES.md`。

失败回溯：`validate` 失败多为版本不一致；`build` 失败看对应平台日志；`sign` 失败为 manifest 签名密钥缺失（需 `secrets.CROSSH_UPDATE_SIGNING_KEY`）。

### 6. 发布后

- 本地 `cargo run` 自检新版本：`crossh --version`。
- 若需回退：`git tag -d vX.Y.Z && git push origin :refs/tags/vX.Y.Z`（谨慎，需同时删除 GitHub Release），或直接发布 `vX.Y.(Z+1)` 修复。

## 常见坑与修复

| 现象 | 原因 | 修复 |
| --- | --- | --- |
| `worktree is not clean` | 已创建 `docs/release-notes/*.md` 但未加 `--allow-dirty` | `bash scripts/release.sh X.Y.Z --allow-dirty` |
| `unexpected worktree change: <path>` | 白名单外文件有改动 | 提交/丢弃该文件，或在 `scripts/release.sh:80` 追加白名单 |
| tag 正文丢失 `#` 标题 | `git tag -F` 默认 `strip` | 已修复为 `--cleanup=verbatim`（`scripts/release.sh:156`），旧 tag 需 `git tag -d && git tag -a -F --cleanup=verbatim` 重建 |
| `local tag already exists` / `remote tag already exists` | 重复打版 | 删除本地/远程 tag 后重打 |
| `Cargo.lock` diff 未提交 | `cargo check` 后未 `git add` | `release.sh` 已自动 `git add Cargo.lock`，手动则需自行 add |
| Release Notes 为空或仅 `git log` | 未创建手写文件 | 创建 `docs/release-notes/vX.Y.Z.md` 再打版；workflow 会回退但不符合手写要求 |

## 相关文件

- `scripts/release.sh` — 唯一打版入口（版本改写、lock 同步、提交、打 tag、推送）。
- `scripts/package-version.sh` — 单一 manifest 版本读取。
- `.github/workflows/release.yml` — tag 触发的三平台打包与发布。
- `docs/release-notes/v*.md` — 手写日志归档（随版本提交，workflow 优先读取）。
- `scripts/generate-update-manifest.sh` + `crates/crossh-update` — `stable.json` 生成与 Ed25519 签名。

## 最小验证清单

- [ ] `docs/release-notes/vX.Y.Z.md` 已手写并提交
- [ ] `git cat-file -p vX.Y.Z` 含完整 Markdown 标题
- [ ] `bash scripts/package-version.sh` 为新版本
- [ ] `git push origin main && git push origin vX.Y.Z` 成功
- [ ] `gh run list --workflow="Release"` 的 `validate` 通过，`build` 矩阵全绿
