//! 选择模型，对齐 pi-tui 的 selectionAnchor/Focus/ granularity

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SelectionPoint {
    pub row: usize,
    pub col: usize,
    pub scroll_view_id: Option<usize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Granularity {
    Character,
    Word,
    Line,
}

#[derive(Debug, Clone)]
pub struct SelectionState {
    pub anchor: Option<SelectionPoint>,
    pub focus: Option<SelectionPoint>,
    pub granularity: Granularity,
    pub press_active: bool,
    pub dragged: bool,
}

impl Default for SelectionState {
    fn default() -> Self {
        Self {
            anchor: None,
            focus: None,
            granularity: Granularity::Character,
            press_active: false,
            dragged: false,
        }
    }
}

impl SelectionState {
    pub fn start(&mut self, point: SelectionPoint, granularity: Granularity) {
        self.anchor = Some(point);
        self.focus = Some(point);
        self.granularity = granularity;
        self.press_active = true;
        self.dragged = false;
    }

    pub fn update_focus(&mut self, point: SelectionPoint) {
        if self.anchor.is_some() {
            self.focus = Some(point);
            self.dragged = true;
        }
    }

    pub fn bounds(&self) -> Option<(SelectionPoint, SelectionPoint)> {
        let a = self.anchor?;
        let f = self.focus?;
        if a.scroll_view_id != f.scroll_view_id {
            return None;
        }
        if a.row == f.row && a.col == f.col {
            return None;
        }
        let anchor_before_focus = a.row < f.row || (a.row == f.row && a.col < f.col);
        if anchor_before_focus {
            Some((a, f))
        } else {
            Some((f, a))
        }
    }

    pub fn clear(&mut self) {
        self.anchor = None;
        self.focus = None;
        self.press_active = false;
        self.dragged = false;
        self.granularity = Granularity::Character;
    }

    pub fn is_empty(&self) -> bool {
        self.bounds().is_none()
    }
}

pub fn word_segment_at(line: &str, col: usize) -> Option<(usize, usize)> {
    // pi 用 Intl.Segmenter(granularity: "word")；这里用 unicode-segmentation 的
    // split_word_bounds 等价实现，按可见列（grapheme）定位词边界
    use unicode_segmentation::UnicodeSegmentation;
    let mut char_off = 0usize;
    for word in line.split_word_bounds() {
        let chars = word.chars().count();
        // 跳过纯空白段（与 pi 的 word segmenter 一致：空白不是词）
        if word.chars().all(|c| c.is_whitespace()) {
            char_off += chars;
            continue;
        }
        let start = char_off;
        let end = char_off + chars;
        if col >= start && col < end {
            return Some((start, end));
        }
        char_off = end;
    }
    None
}

#[cfg(test)]
mod tests {
    #![allow(non_snake_case)]

    use super::*;

    #[test]
    fn spec_20260822_agent_tui_pi_parity__selection_bounds_cross_scrollview_none() {
        let s = SelectionState {
            anchor: Some(SelectionPoint {
                row: 0,
                col: 0,
                scroll_view_id: Some(1),
            }),
            focus: Some(SelectionPoint {
                row: 0,
                col: 5,
                scroll_view_id: Some(2),
            }),
            ..SelectionState::default()
        };
        assert!(s.bounds().is_none());
    }

    #[test]
    fn spec_20260822_agent_tui_pi_parity__selection_empty_when_same_point() {
        let p = SelectionPoint {
            row: 1,
            col: 2,
            scroll_view_id: None,
        };
        let s = SelectionState {
            anchor: Some(p),
            focus: Some(p),
            ..SelectionState::default()
        };
        assert!(s.is_empty());
    }

    #[test]
    fn spec_20260822_agent_tui_pi_parity__word_segment() {
        assert_eq!(word_segment_at("hello world", 1), Some((0, 5)));
        assert_eq!(word_segment_at("hello world", 6), Some((6, 11)));
        assert_eq!(word_segment_at("hello world", 5), None);
    }
}
