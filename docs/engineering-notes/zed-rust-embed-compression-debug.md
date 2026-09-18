# Zed 升级后 rust-embed `compression` 打断 debug 构建

## 症状

升级 Zed rev 后，debug 构建（`cargo check` / `cargo test`）报两类与 `compressed` 有关的错误；release 构建（`debug_assertions` 关闭）不触发：

```
error[E0046]: not all trait items implemented, missing: `compressed`
  --> <zed-checkout>/crates/assets/src/assets.rs:10:1   // util::fs_embed! 展开处
error[E0407]: method `compressed` is not a member of trait `rust_embed::RustEmbed`
  --> crates/crossh-assets/src/lib.rs:7:10              // #[derive(RustEmbed)] 展开处
```

## 根因

- 新 Zed rev 在 workspace 依赖中给 `rust-embed` 打开了 `compression`：
  `rust-embed = { version = "8.11", features = ["include-exclude", "compression"] }`。
- `rust-embed` 8.12 在该 feature 下给 `RustEmbed` trait 增加 `fn compressed(file_path: &str) -> Option<EmbeddedCompressedFile>`；
  而 Zed `util::fs_embed!` 的 debug（非 `debug-embed`）分支是手写 impl，只实现 `get` / `iter`，因此 `E0046`。
- `crates/crossh-assets` 原先声明 `rust-embed = "8.12.0"`，把整张依赖图钉在 8.12，于是 debug 必挂。Zed 自带 `Cargo.lock` 用的是 8.11.0，所以 Zed 自身 CI 不暴露该问题。
- 若只把主 crate 降到 8.11 而 `rust-embed-impl` / `rust-embed-utils` 仍留在 8.12，derive 会生成 trait 中不存在的 `compressed`，表现为 `E0407`。

## 规则

- `rust-embed` 三件套（`rust-embed` / `rust-embed-impl` / `rust-embed-utils`）必须同版本，并与 Zed 自带 `Cargo.lock` 对齐；当前 `crates/crossh-assets/Cargo.toml` 钉在 `=8.11.0`。
- 升级 Zed 后出现 `compressed` 相关错误时：先确认 Zed 是否新增 `compression` feature，再检查三件套是否同步降级。
- 解除条件：Zed 在 `util::fs_embed!` 的 debug 分支实现 `compressed` 后，可放开 `=8.11.0` 钉死。

## 验证

```bash
cargo check --workspace --all-targets
```

## 关键词

`rust-embed`, `compression`, `compressed`, `RustEmbed`, `fs_embed`, `debug-embed`, `E0046`, `E0407`, `assets`, `crossh-assets`, `Zed 升级`, `debug 构建`