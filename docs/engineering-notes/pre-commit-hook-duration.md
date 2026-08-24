# pre-commit 钩子耗时与提交超时设置

## 症状

`git commit` 命令在 300 秒超时被切断：钩子输出停在 `cargo clippy --workspace --all-targets -- -D warnings`，改动留在暂存区，提交未落库。

## 根因

本仓 pre-commit 钩子依次跑三项门禁：`scripts/check-architecture.sh`（秒级）、`cargo fmt --check`（秒级）、`clippy --workspace --all-targets -- -D warnings`（分钟级）。clippy 耗时取决于编译缓存温度：

- 缓存热（近期跑过同配置 clippy/test）：约 **5 分钟**（2026-08-24 实测 279s）。
- 缓存冷（新 clone、依赖变更、跨 crate 大改）：**10 分钟以上**。

## 规则

- 用非交互方式执行 `git commit` 时，超时**最低设 600 秒，推荐 1200 秒以上**；宁可后台跑再轮询结果，也不要用默认短超时。
- 钩子被切断不会丢改动——文件仍在暂存区，加大超时重跑 `git commit` 即可。
- 判断是否落库以 `git log --oneline -1` 为准，不要以命令退出状态推断（超时 ≠ 失败）。

## 例外：纯文档变更可跳过

三项门禁防的都是代码回归（架构分层 / 格式 / lint），对 `.md` 等纯文档变更没有输入。此类提交可用 `git commit --no-verify` 跳过钩子。判据：暂存区里没有任何 `.rs`/`Cargo.toml`/脚本文件。含代码的提交**不要**跳过。

## 关键词

`git commit`, `pre-commit`, `clippy -D warnings`, `timeout`, `超时`, `钩子`, `提交慢`
