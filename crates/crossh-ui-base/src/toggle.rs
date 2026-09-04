// Copyright (c) 2026 Crossh contributors.
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! 开关状态机：点击翻转的纯函数。
//!
//! 组件只渲染算出的目标态；新值由拥有方在回调里持久化，地基层永不触碰应用状态。

/// 点击后的目标态：开 → 关，关 → 开。
pub fn next_state(on: bool) -> bool {
    !on
}

#[cfg(test)]
mod tests {
    use super::next_state;

    #[test]
    fn click_moves_to_the_other_state() {
        assert!(next_state(false));
        assert!(!next_state(true));
    }

    #[test]
    fn double_click_returns_to_start() {
        assert_eq!(next_state(next_state(true)), true);
        assert_eq!(next_state(next_state(false)), false);
    }
}
