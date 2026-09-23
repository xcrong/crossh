# rust-i18n 多 crate：翻译表各归各，locale 全局共享

## 症状

- 把视图拆成新 crate 后，`rust_i18n::t!` 编译失败（找不到翻译表），或运行时回退英文，尽管 `locales/` 文件齐全、`set_locale` 已调用。
- 主二进制与独立二进制（`crossh-git` / `crossh-note`）用 `#[path]` 复用同一份视图源码时，各自手写 `mod shared` 垫片才能编译。

## 根因

rust-i18n 4.x（本仓库 4.2.1，`src/lib.rs` 实测）：

- 每个 `rust_i18n::i18n!` 调用点在**所在 crate**生成独立的翻译表；`t!` 永远解析到**调用者所在 crate** 的表，与 `locales/` 目录是否相同无关。
- `rust_i18n::set_locale` 写的是 **rust-i18n crate 内唯一的进程全局** `CURRENT_LOCALE`，一次调用对进程内所有表的无参 `t!` 同时生效。

因此新 crate 只要满足两条即可：自己的 `i18n!("../../locales", fallback = "en")`（路径相对各自 `CARGO_MANIFEST_DIR`），以及某处调用一次全局 `set_locale`。`src/shared/i18n.rs` 迁入 `crossh-core` 时已验证：`i18n::tests::resource_lookup_supports_both_locales` 在 core 内直接通过。

## 规则

- 每个调用 `t!` 的 crate 必须有自己的 `i18n!`（放 crate root 的 `lib.rs`，不要藏进子模块）。
- `Locale` / `LanguagePreference` / `text()` / `set_locale` 只在 `crossh-core::i18n` 定义一份；各 bin 只调全局 `rust_i18n::set_locale`，不再各自 `i18n!`。
- 根包没有 lib target，bin 之间不能 `use` 共享代码——共享逻辑必须下沉到 crate，禁止用 `#[path]` 回抄（`scripts/check-architecture.sh` 已对 `src/bin` 全目录拦截）。

## 验证方法

```sh
cargo test -p crossh-core --offline i18n
rg -n '#\[path' src/bin || echo "no path hacks"
```

## 搜索关键词

`rust-i18n`, `i18n!`, `set_locale`, `CURRENT_LOCALE`, `locales`, 多 crate, 翻译表, 回退英文, `#[path]`, bin 共享代码
