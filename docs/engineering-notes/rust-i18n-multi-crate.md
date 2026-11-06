# rust-i18n 多 crate：翻译表各归各，locale 全局共享

## 症状

- 把视图拆成新 crate 后，`rust_i18n::t!` 编译失败（找不到翻译表），或运行时回退英文，尽管 `locales/` 文件齐全、`set_locale` 已调用。
- 主二进制与独立二进制（`crossh-git` / `crossh-note`）用 `#[path]` 复用同一份视图源码时，各自手写 `mod shared` 垫片才能编译。

## 根因

rust-i18n 4.x（本仓库 4.2.1，`src/lib.rs` 实测）：

- 每个 `rust_i18n::i18n!` 调用点在**所在 crate**生成独立的翻译表；`t!` 永远解析到**调用者所在 crate** 的表，与 `locales/` 目录是否相同无关。
- `rust_i18n::set_locale` 写的是 **rust-i18n crate 内唯一的进程全局** `CURRENT_LOCALE`，一次调用对进程内所有表的无参 `t!` 同时生效。

因此新 crate 只要满足两条即可：自己的 `i18n!("../../locales", fallback = "en")`（路径相对各自 `CARGO_MANIFEST_DIR`），以及某处调用一次全局 `set_locale`。`src/shared/i18n.rs` 迁入 `crossh-core` 时已验证：`i18n::tests::resource_lookup_supports_both_locales` 在 core 内直接通过。

## 增量构建：改了 locales/ 却仍在用旧翻译表

### 症状

- 新加的键在运行/测试里**原样返回键名**（`note.status.no_store`），而 `locales/*.yml` 里键明明存在、`set_locale` 也已调用。
- 同一个 crate 里一半键能查到、一半查不到：`crossh-note-view` 自己刚重编过（新键可查），但它的 `text()` 走的是 `crossh-core` 的表，而那个 crate 没重编 → 旧表查不到新键。
- `cargo clean -p <crate>` 后立刻恢复正常；只 `touch src/*.rs` 无效。

### 根因

`i18n!` 是 proc macro：编译期把 YAML 读成 `&'static str` 表写进 crate。cargo 只按文件依赖判断是否重编，而 `locales/` 不在任何 crate 的依赖里，rust-i18n 4.2.1 也不会自动声明 → 改了 YAML，crate 不重编，沿用旧表。

### 规则

- 每个调用 `i18n!` 的包都必须有自己的 `build.rs`（根包、`crossh-core`、`crossh-git-view`、`crossh-note-view`），打印 `cargo:rerun-if-changed=<locale 目录>`（相对各自 `CARGO_MANIFEST_DIR`）。加新 crate 时同步补。
- 新增键后必须验证构建系统真的重编了：`touch locales/en.yml && cargo build -p crossh-core --offline` 应输出 `1 crates compiled`。
- 测试里断言文案不要写死 locale：期望值用与生产代码同形式的无参 `t!(key)`，两种语言下都成立；键名写错时 `t!` 回吐键名，测试照样失败。

## 规则

- 每个调用 `t!` 的 crate 必须有自己的 `i18n!`（放 crate root 的 `lib.rs`，不要藏进子模块）。
- `Locale` / `LanguagePreference` / `text()` / `set_locale` 只在 `crossh-core::i18n` 定义一份；独立子进程 bin（`crossh-git` / `crossh-note`）只调全局 `rust_i18n::set_locale`，不再各自 `i18n!`。根包自身要用 `t!`（`src/features/**` 大量调用），所以根包的 `src/main.rs` 必须保留 `i18n!("locales")`；不能因为它同时调 `set_locale` 就把这一行当成多余。
- 根包没有 lib target，bin 之间不能 `use` 共享代码——共享逻辑必须下沉到 crate，禁止用 `#[path]` 回抄（`scripts/check-architecture.sh` 已对 `src/bin` 全目录拦截）。

## 验证方法

```sh
cargo test -p crossh-core --offline i18n
rg -n '#\[path' src/bin || echo "no path hacks"
# 每个 i18n! 调用点都应有 build.rs 声明 locales 依赖
touch locales/en.yml && cargo build -p crossh-core --offline   # 必须重编 1 crate
```

## 搜索关键词

`rust-i18n`, `i18n!`, `set_locale`, `CURRENT_LOCALE`, `locales`, 多 crate, 翻译表, 回退英文, `#[path]`, bin 共享代码, 增量构建, 旧翻译表, 键名原样返回, `cargo:rerun-if-changed`, `build.rs`, `cargo clean -p`
