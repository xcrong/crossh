// Copyright (c) 2026 Crossh contributors.
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! 列表状态：空态语义 + 受控选择光标。
//!
//! [`ListStatus`] 描述列表处于加载 / 失败 / 空 / 就绪中的哪一种；
//! [`ListCursor`] 是值语义的选择光标：应用持有，基础库只计算 next，
//! 永不触碰应用状态。

use gpui::SharedString;

/// 列表空态：`Ready` 表示有数据，其余状态携带待显示文案。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ListStatus {
    /// 正在加载。
    Loading(SharedString),
    /// 加载失败。
    Error(SharedString),
    /// 无数据。
    Empty(SharedString),
    /// 有数据。
    Ready,
}

impl ListStatus {
    /// 是否有数据。
    pub fn is_ready(&self) -> bool {
        matches!(self, Self::Ready)
    }

    /// 非就绪状态的待显示文案；`Ready` 返回 `None`。
    pub fn message(&self) -> Option<&SharedString> {
        match self {
            Self::Loading(message) | Self::Error(message) | Self::Empty(message) => Some(message),
            Self::Ready => None,
        }
    }
}

/// 选择步进方向。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StepDirection {
    /// 下一项。
    Next,
    /// 上一项。
    Prev,
}

/// 受控选择光标：值语义，只计算 next，不触碰应用状态。
///
/// 所有移动方法在选择发生变化时返回 `true`，无变化（空列表、
//  越界、单元素循环回自身）时返回 `false`，调用方据此决定是否通知更新。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ListCursor {
    count: usize,
    selected: Option<usize>,
}

impl ListCursor {
    /// 以列表长度创建，未选中。
    pub fn new(count: usize) -> Self {
        Self {
            count,
            selected: None,
        }
    }

    /// 设置选中；越界（或空列表的任何值）钳制为 `None`。
    pub fn with_selected(mut self, selected: Option<usize>) -> Self {
        self.set_selected(selected);
        self
    }

    /// 更新列表长度；原选择越界则回落为 `None`。
    pub fn set_count(&mut self, count: usize) {
        self.count = count;
        if self.selected.is_some_and(|index| index >= count) {
            self.selected = None;
        }
    }

    /// 列表长度。
    pub fn count(&self) -> usize {
        self.count
    }

    /// 当前选中。
    pub fn selected(&self) -> Option<usize> {
        self.selected
    }

    /// 选中指定行；越界返回 `false` 且保持不变。
    pub fn select(&mut self, index: usize) -> bool {
        if index >= self.count {
            return false;
        }
        self.set_selected(Some(index));
        true
    }

    /// 清除选中。
    pub fn clear(&mut self) -> bool {
        if self.selected.is_none() {
            return false;
        }
        self.selected = None;
        true
    }

    /// 下一项（循环）。
    pub fn move_next(&mut self) -> bool {
        self.move_by(StepDirection::Next)
    }

    /// 上一项（循环）。
    pub fn move_prev(&mut self) -> bool {
        self.move_by(StepDirection::Prev)
    }

    /// 按方向步进（循环）。
    pub fn move_by(&mut self, direction: StepDirection) -> bool {
        let next = next_index(self.count, self.selected, direction);
        if next == self.selected {
            return false;
        }
        self.selected = next;
        true
    }

    fn set_selected(&mut self, selected: Option<usize>) {
        self.selected = selected.filter(|index| *index < self.count);
    }
}

/// 下一个选中索引：空列表返回 `None`；`current` 为 `None` 时 `Next` 取首项、
/// `Prev` 取末项；越界的 `current` 返回 `None`；首尾循环回绕。
pub fn next_index(len: usize, current: Option<usize>, direction: StepDirection) -> Option<usize> {
    if len == 0 {
        return None;
    }
    match current {
        None => match direction {
            StepDirection::Next => Some(0),
            StepDirection::Prev => Some(len - 1),
        },
        Some(current) if current >= len => None,
        Some(current) => {
            let delta = match direction {
                StepDirection::Next => 1,
                StepDirection::Prev => -1,
            };
            Some(((current as i32 + delta).rem_euclid(len as i32)) as usize)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{ListCursor, ListStatus, StepDirection, next_index};

    #[test]
    fn empty_list_rejects_everything() {
        let mut cursor = ListCursor::new(0);
        assert_eq!(cursor.selected(), None);
        assert!(!cursor.move_next());
        assert!(!cursor.move_prev());
        assert!(!cursor.select(0));
        assert!(!cursor.clear());
        assert_eq!(next_index(0, None, StepDirection::Next), None);
        assert_eq!(next_index(0, Some(0), StepDirection::Prev), None);
    }

    #[test]
    fn unset_cursor_starts_at_head_or_tail() {
        assert_eq!(next_index(3, None, StepDirection::Next), Some(0));
        assert_eq!(next_index(3, None, StepDirection::Prev), Some(2));
        let mut cursor = ListCursor::new(3);
        assert!(cursor.move_next());
        assert_eq!(cursor.selected(), Some(0));
        let mut cursor = ListCursor::new(3);
        assert!(cursor.move_prev());
        assert_eq!(cursor.selected(), Some(2));
    }

    #[test]
    fn wraps_around_both_ends() {
        assert_eq!(next_index(3, Some(2), StepDirection::Next), Some(0));
        assert_eq!(next_index(3, Some(0), StepDirection::Prev), Some(2));
        let mut cursor = ListCursor::new(3).with_selected(Some(2));
        assert!(cursor.move_next());
        assert_eq!(cursor.selected(), Some(0));
    }

    #[test]
    fn out_of_range_current_yields_none() {
        assert_eq!(next_index(3, Some(3), StepDirection::Next), None);
        assert_eq!(next_index(3, Some(99), StepDirection::Prev), None);
    }

    #[test]
    fn select_clamps_and_reports_change() {
        let mut cursor = ListCursor::new(2);
        assert!(cursor.select(1));
        assert_eq!(cursor.selected(), Some(1));
        assert!(!cursor.select(2));
        assert_eq!(cursor.selected(), Some(1));
        assert!(cursor.clear());
        assert!(!cursor.clear());
        assert_eq!(ListCursor::new(2).with_selected(Some(7)).selected(), None);
    }

    #[test]
    fn shrinking_count_drops_invalid_selection() {
        let mut cursor = ListCursor::new(3).with_selected(Some(2));
        cursor.set_count(2);
        assert_eq!(cursor.selected(), None);
        assert_eq!(cursor.count(), 2);
        cursor.set_count(0);
        assert!(!cursor.move_next());
    }

    #[test]
    fn status_readers() {
        assert!(ListStatus::Ready.is_ready());
        assert_eq!(ListStatus::Ready.message(), None);
        let loading = ListStatus::Loading("loading".into());
        assert!(!loading.is_ready());
        assert!(
            loading
                .message()
                .is_some_and(|message| message.to_string() == "loading")
        );
    }
}
