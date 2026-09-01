//! crossh 定向复制+删除：Zed migrator 的轻量桩
//! 原版 `crates/migrator` 通过 `tree-sitter[wasm] -> wasmtime -> cranelift` 做 JSON 迁移，
//! crossh 的 `settings.toml` 无需历史迁移，此处全部 no-op。

use anyhow::Result;

/// 迁移 keymap，crossh 不使用 Zed keymap，直接返回 None
pub fn migrate_keymap(_text: &str) -> Result<Option<String>> {
    Ok(None)
}

/// 迁移 settings.json，crossh 使用 TOML 的 `crossh/settings`，无需迁移
pub fn migrate_settings(_text: &str) -> Result<Option<String>> {
    Ok(None)
}

/// 迁移 edit prediction 配置，同上
pub fn migrate_edit_prediction_provider_settings(_text: &str) -> Result<Option<String>> {
    Ok(None)
}
