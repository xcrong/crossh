//! 本地化文案接缝。
//!
//! 本 crate 不直接依赖 `rust-i18n`：翻译职责留在宿主应用，由它在启动时注入一个
//! 查询函数。locale 可在运行时切换，因此这里保存的是函数指针而不是字符串快照。

use std::sync::OnceLock;

static RESOLVER: OnceLock<fn(&str) -> String> = OnceLock::new();

/// 由宿主应用在启动时注入本地化查询函数。
///
/// 未注入时 [`text`] 退化为返回 key 本身，便于本 crate 独立测试。
pub fn set_text_resolver(resolver: fn(&str) -> String) {
    let _ = RESOLVER.set(resolver);
}

/// 查询本地化文案；未注入解析器时返回 key。
pub(crate) fn text(key: &str) -> String {
    RESOLVER
        .get()
        .map(|resolver| resolver(key))
        .unwrap_or_else(|| key.to_string())
}
