# Cargo 锁文件发布同步

## 症状

版本发布提交中的各个 workspace `Cargo.toml` 已更新，但 `Cargo.lock` 仍保留旧版本；发布产物可以继续构建，导致问题直到下次依赖操作才暴露。

## 根因

`cargo metadata --no-deps` 只读取元数据，不负责重写锁文件。发布脚本曾把它当作同步步骤，因此手动或脚本发布都可能留下旧的 workspace package 版本。

## 持久规则

版本 bump 后使用 `cargo check --workspace` 生成 workspace package 的锁文件版本，然后将 `Cargo.lock` 与所有 package manifest 一起提交。CI 和 tag 发布校验使用 `cargo metadata --no-deps --locked` 加 `git diff --exit-code -- Cargo.lock`，只验证一致性，不在干净 runner 上改写依赖图。

## 验证

```sh
cargo check --workspace
cargo metadata --format-version 1 --no-deps --locked >/dev/null
git diff --exit-code -- Cargo.lock
```

关键词：`Cargo.lock`, `release.sh`, `cargo metadata`, `cargo check`, 版本发布。
