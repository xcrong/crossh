# Cargo 测试选择陷阱：非 virtual workspace 下 `cargo test` 只选根 crate

## 症状

- CI 或本地 `cargo test` 看似全绿，但 workspace members（如 crossh-core、
  crossh-agent）的测试从未执行；某 crate 故意引入失败测试后 CI 仍通过。
- `cargo test --workspace --lib` 在根 crate 无 `src/lib.rs` 时，其 bin
  target 上的全部单元测试被静默跳过，无任何失败信号。

## 根因

根 crate 是 `[package]`（非 virtual workspace）且未设置
`default-members` 时，不带 `--workspace` 的 `cargo test` 只选择根 crate
（Cargo 官方文档明确）。`--lib` 只选中 lib target；测试挂在 bin target
（src/main.rs、src/bin/*.rs）时与 `--lib` 零交集，且 Cargo 不报错。

本仓库实测（2026-08-17）：`cargo test -- --list` 选中 200 个，
`cargo test --workspace -- --list` 选中 401 个，约 200 个测试被静默排除。

## 规则

- 全量门禁一律 `cargo test --release --workspace`（或显式含 `--bins`）。
- 在非 virtual workspace 的根 crate，永远不要相信不带 `--workspace` 的
  `cargo test` 输出代表全部测试。
- 验证选择范围：`cargo test --workspace -- --list | grep -c ': test'`，
  数字与既有基线对比（本仓库基线 401 注册 / 398 可运行，3 个 `#[ignore]`
  真实主机测试除外）。

## 验证方法

```sh
cargo test -- --list 2>/dev/null | grep -c ': test'
cargo test --workspace -- --list 2>/dev/null | grep -c ': test'
```

两行数字不相等即存在被排除的测试。

## 搜索关键词

`--workspace`, `--lib`, `--list`, default-members, virtual workspace,
bin target, 测试没跑, tests not selected, 静默跳过