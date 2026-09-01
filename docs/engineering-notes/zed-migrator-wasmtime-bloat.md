# Zed migrator 引入 wasmtime 导致 target 膨胀

## 症状
- `cargo build` 后 `target 15G -> 3-4天 100G`，`target/debug/deps` 中 `wasmtime 72M + cranelift 138M + wasmparser 30M + wasmtime_environ 70M` 合计 `633M/变体`，`cargo tree --invert wasmtime` 显示两条链
- `Cargo.lock 972包`，`cargo metadata` 显示 `wasmtime/cranelift` 7 变体，`targetdebug/incremental 146个 4.3G`

## 根因
`crossh` 通过 `settings = {git=zed}` 间接依赖 `migrator`，`migrator/Cargo.toml:22` 和 `settings_json/Cargo.toml:19` 均写 `tree-sitter = {workspace=true, features=["wasm"]}`。`tree-sitter 0.27.0` 的 `wasm = ["wasmtime-c-api"]` 将 `wasmtime 48.0.1 + cranelift-codegen 0.135.1 + wasmparser 0.254.0 + pulley` 全量引入。`crossh` 实际只用 `tree_sitter::Parser::set_language(&tree_sitter_json::LANGUAGE.into())` 做 `settings.json` 文本迁移，且自身 `settings` 已改为 `TOML ~/.config/crossh/settings.toml`，`migrator` 的 30+ 条历史迁移对 `crossh` 为死代码。

链路：
```
crossh -> settings -> migrator -> tree-sitter[wasm] -> wasmtime
crossh -> settings -> settings_json -> tree-sitter[wasm] -> wasmtime
```

## 规则
**Zed git 依赖若仅为历史兼容而启用 `wasm`，在 crossh 中必须通过本地 patch 桩化。** 保留 `Zed` 的 `settings/terminal/theme` 以复用 `GPUI` 渲染，仅将 `migrator/settings_json` 替换为本地 no-op，不修改 `gpui` 本体。`profile.dev debug=1 + split-debuginfo=unpacked` 与此配合，可将 `targetdebug` 基线 `15G -> ~9G`。

## 定向复制+删除做法
1. 复制：新建 `crates/migrator-stub`（`name=migrator v0.1.0`，仅 `anyhow`）和 `crates/settings_json-stub`（`name=settings_json v0.1.0`，仅 `anyhow/serde/serde_json`），`src/migrator.rs` 桩化 `migrate_settings/migrate_keymap/migrate_edit_prediction_provider_settings` 为 `Ok(None)`，`src/settings_json.rs` 桩化 `infer/update/replace/append/to_pretty/parse` 为简单 `serde_json` 实现
2. 删除：`Cargo.toml` 增加
   ```toml
   [patch."https://github.com/zed-industries/zed"]
   migrator = { path = "crates/migrator-stub" }
   settings_json = { path = "crates/settings_json-stub" }
   ```
   去除 `tree-sitter/wasmtime/cranelift` 依赖
3. 校验：`cargo tree | grep wasmtime` 计数 `0`，`Cargo.lock 972 -> 925包`，`cargo check --workspace --all-targets --quiet` 通过

## 验证
```sh
cargo tree --invert wasmtime # error: no package
cargo tree | grep -c wasmtime # 0
cargo metadata --format-version=1 | python3 -c "print(len(json.load(sys.stdin)['packages']))" # 925
cargo check --workspace --all-targets --quiet # exit 0
du -sh target # 清理后重建，wasmtime 633M/变体 消失，增量日增 40M/次 消失
```

## 关键词
`wasmtime, cranelift, wasm, migrator, settings_json, tree-sitter, target 膨胀, patch, 定向复制, Cargo.lock 972, incremental`
