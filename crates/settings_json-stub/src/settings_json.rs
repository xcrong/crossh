//! crossh 定向复制+删除：Zed settings_json 的轻量桩
//! 原版通过 `tree-sitter[wasm] -> wasmtime` 做 JSON 文本增量编辑，crossh 不使用 Zed 的 settings.json，此处 no-op/简化。

use anyhow::Result;
use serde::{Serialize, de::DeserializeOwned};
use serde_json::Value;
use std::ops::Range;

/// 推断 JSON 缩进，stub 返回 2
pub fn infer_json_indent_size(_text: &str) -> usize {
    2
}

pub fn update_value_in_json_text<'a>(
    text: &mut String,
    _key_path: &mut Vec<&'a str>,
    _tab_size: usize,
    _old_value: &'a Value,
    new_value: &'a Value,
    edits: &mut Vec<(Range<usize>, String)>,
) {
    let new_text = serde_json::to_string_pretty(new_value).unwrap_or_else(|_| text.clone());
    let range = 0..text.len();
    edits.push((range.clone(), new_text.clone()));
    *text = new_text;
}

pub fn replace_value_in_json_text<T: AsRef<str>>(
    text: &str,
    _key_path: &[T],
    _tab_size: usize,
    new_value: Option<&Value>,
    _replace_key: Option<&str>,
) -> (Range<usize>, String) {
    let replacement = new_value
        .map(|v| serde_json::to_string_pretty(v).unwrap_or_default())
        .unwrap_or_default();
    (0..text.len(), replacement)
}

pub fn replace_top_level_array_value_in_json_text(
    text: &str,
    _key_path: &[impl AsRef<str>],
    new_value: Option<&Value>,
    _replace_key: Option<&str>,
    _array_index: usize,
    _tab_size: usize,
) -> (Range<usize>, String) {
    let replacement = new_value
        .map(|v| serde_json::to_string_pretty(v).unwrap_or_default())
        .unwrap_or_default();
    (0..text.len(), replacement)
}

pub fn append_top_level_array_value_in_json_text(
    text: &str,
    new_value: &Value,
    _tab_size: usize,
) -> (Range<usize>, String) {
    let replacement = serde_json::to_string_pretty(new_value).unwrap_or_default();
    (text.len()..text.len(), replacement)
}

pub fn to_pretty_json(value: &impl Serialize, _indent_size: usize, _indent_prefix_len: usize) -> String {
    serde_json::to_string_pretty(value).unwrap_or_default()
}

pub fn parse_json_with_comments<T: DeserializeOwned>(content: &str) -> Result<T> {
    let mut deserializer = serde_json_lenient::Deserializer::from_str(content);
    let value = serde_path_to_error::deserialize(&mut deserializer)?;
    deserializer.end()?;
    Ok(value)
}
