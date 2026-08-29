//! Application-local shared code: pure logic reused across features.
//!
//! 本层是纯逻辑层，零 GPUI、零 UI crate 依赖；GPUI 装配
//! （全局态、按键转换、选区注入）由 features 视图层负责。

pub(crate) mod i18n;
pub(crate) mod input_handler;
pub(crate) mod text_editing;
