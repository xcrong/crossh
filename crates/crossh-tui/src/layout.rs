//! layout — pi-tui 的 layout.js + components/stack.js 移植
//!
//! VStack 分配：basis/grow/shrink/minSize/maxSize（pi 的 allocateStackSizes）
//! LayoutBox 树：rect/clip/scrollContentLines + scrollbar 几何 + hit-test

use crate::scroll_view::ScrollView;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

impl Rect {
    pub fn new(x: i32, y: i32, width: i32, height: i32) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }
    pub fn bottom(&self) -> i32 {
        self.y + self.height
    }
    pub fn right(&self) -> i32 {
        self.x + self.width
    }
    pub fn contains(&self, x: i32, y: i32) -> bool {
        x >= self.x && x < self.right() && y >= self.y && y < self.bottom()
    }
    pub fn intersect(&self, other: &Rect) -> Rect {
        let x = self.x.max(other.x);
        let y = self.y.max(other.y);
        let right = self.right().min(other.right());
        let bottom = self.bottom().min(other.bottom());
        Self::new(x, y, (right - x).max(0), (bottom - y).max(0))
    }
}

/// Stack 条目（pi 的 StackEntry）
#[derive(Debug, Clone, Default)]
pub struct StackEntry {
    /// 固定基础高度；None = auto（用固有高度）
    pub basis: Option<usize>,
    pub grow: usize,
    pub shrink: usize,
    pub min_size: usize,
    pub max_size: usize,
}

impl StackEntry {
    pub fn fixed(basis: usize) -> Self {
        Self {
            basis: Some(basis),
            grow: 0,
            shrink: 1,
            min_size: 0,
            max_size: usize::MAX,
        }
    }
    /// pi 的 { basis: 0, grow: 1 } — 填满剩余
    pub fn fill() -> Self {
        Self {
            basis: Some(0),
            grow: 1,
            shrink: 1,
            min_size: 1,
            max_size: usize::MAX,
        }
    }
    pub fn auto() -> Self {
        Self {
            basis: None,
            grow: 0,
            shrink: 1,
            min_size: 1,
            max_size: usize::MAX,
        }
    }
}

/// 按需分配尺寸（pi 的 allocateStackSizes + distribute 1:1）：
/// 先取 basis（或固有高度）clamp 到 [minSize, maxSize]，
/// 总量 < 可用则按 grow 权重逐轮分配，> 可用则按 shrink*max(1,size) 权重收缩，
/// 每轮至少分配 1 且尊重容量上限，直到无剩余或无可分配项
pub fn allocate_stack_sizes(
    entries: &[StackEntry],
    intrinsic_sizes: &[usize],
    available: Option<usize>,
) -> Vec<usize> {
    let mut sizes: Vec<usize> = entries
        .iter()
        .zip(intrinsic_sizes)
        .map(|(entry, intrinsic)| {
            let base = entry.basis.unwrap_or(*intrinsic);
            base.clamp(entry.min_size, entry.max_size.max(entry.min_size))
        })
        .collect();
    let Some(available) = available else {
        return sizes;
    };
    let total: usize = sizes.iter().sum();
    if total < available {
        // grow
        let mut remaining = available - total;
        loop {
            let candidates: Vec<usize> = (0..entries.len())
                .filter(|&i| entries[i].grow > 0 && sizes[i] < entries[i].max_size)
                .collect();
            if candidates.is_empty() || remaining == 0 {
                break;
            }
            let total_weight: usize = candidates.iter().map(|&i| entries[i].grow).sum();
            let mut distributed = 0usize;
            for &i in &candidates {
                if remaining == 0 {
                    break;
                }
                let proposed = ((remaining * entries[i].grow) / total_weight).max(1);
                let capacity = entries[i].max_size.saturating_sub(sizes[i]);
                let delta = remaining.min(proposed).min(capacity);
                if delta == 0 {
                    continue;
                }
                sizes[i] += delta;
                remaining -= delta;
                distributed += delta;
            }
            if distributed == 0 {
                // 把剩余给最后一个 grow 项
                if let Some(&last) = candidates.last()
                    && sizes[last] < entries[last].max_size
                {
                    sizes[last] += remaining;
                }
                break;
            }
        }
    } else if total > available {
        // shrink：从大项开始收缩，尊重 minSize
        let mut remaining = total - available;
        loop {
            let candidates: Vec<usize> = (0..entries.len())
                .filter(|&i| entries[i].shrink > 0 && sizes[i] > entries[i].min_size)
                .collect();
            if candidates.is_empty() || remaining == 0 {
                break;
            }
            let total_weight: usize = candidates
                .iter()
                .map(|&i| entries[i].shrink * sizes[i].max(1))
                .sum();
            let mut distributed = 0usize;
            for &i in &candidates {
                if remaining == 0 {
                    break;
                }
                let weight = entries[i].shrink * sizes[i].max(1);
                let proposed = ((remaining * weight) / total_weight).max(1);
                let capacity = sizes[i].saturating_sub(entries[i].min_size);
                let delta = remaining.min(proposed).min(capacity);
                if delta == 0 {
                    continue;
                }
                sizes[i] -= delta;
                remaining -= delta;
                distributed += delta;
            }
            if distributed == 0 {
                break;
            }
        }
    }
    sizes
}

#[derive(Debug, Clone)]
pub struct ScrollbarGeometry {
    pub column: i32,
    pub track_top: i32,
    pub track_height: i32,
    pub thumb_top: i32,
    pub thumb_height: i32,
    pub max_scroll_top: i32,
}

/// scrollbar 几何（pi 的 getScrollbarGeometry）
pub fn get_scrollbar_geometry(
    box_rect: Rect,
    content_height: usize,
    scroll_view: &ScrollView,
) -> Option<ScrollbarGeometry> {
    if box_rect.width <= 0 || box_rect.height <= 0 || !scroll_view.scrollbar_visible() {
        return None;
    }
    let track_height = box_rect.height;
    let min_thumb = track_height.min(2);
    let ratio = (track_height * track_height) as f64 / (content_height.max(1) as f64);
    let thumb_height = (ratio.round() as i32).clamp(min_thumb, track_height);
    let max_scroll_top = (content_height as i32 - track_height).max(0);
    let max_thumb_top = track_height - thumb_height;
    let thumb_offset = if max_scroll_top == 0 {
        0.0
    } else {
        scroll_view.scroll_top as f64 / max_scroll_top as f64 * max_thumb_top as f64
    };
    let thumb_offset = thumb_offset.round() as i32;
    Some(ScrollbarGeometry {
        column: box_rect.right() - 1,
        track_top: box_rect.y,
        track_height,
        thumb_top: box_rect.y + thumb_offset,
        thumb_height,
        max_scroll_top,
    })
}

#[cfg(test)]
mod tests {
    #![allow(non_snake_case)]

    use super::*;

    #[test]
    fn spec_20260822_agent_tui_pi_parity__vstack_fill_takes_remaining_space() {
        // header fixed 3, transcript fill(grow), dock auto → fill 拿剩余
        let entries = vec![StackEntry::fill(), StackEntry::auto(), StackEntry::fixed(2)];
        let intrinsic = [0, 5, 2];
        let sizes = allocate_stack_sizes(&entries, &intrinsic, Some(20));
        assert_eq!(sizes[0], 13); // 20 - 5 - 2
        assert_eq!(sizes[1], 5);
        assert_eq!(sizes[2], 2);
    }

    #[test]
    fn spec_20260822_agent_tui_pi_parity__vstack_shrinks_respecting_min_size() {
        let mut dock = StackEntry::auto();
        dock.min_size = 3;
        let entries = vec![StackEntry::fixed(15), dock];
        let intrinsic = [15, 10];
        let sizes = allocate_stack_sizes(&entries, &intrinsic, Some(12));
        // 总高恰为可用高度，且每项不低于 minSize
        assert_eq!(sizes.iter().sum::<usize>(), 12);
        assert!(sizes[1] >= 3);
    }

    #[test]
    fn spec_20260822_agent_tui_pi_parity__scrollbar_geometry_matches_pi_formula() {
        let mut sv = ScrollView::with_viewport(10);
        sv.set_content_height(50);
        sv.set_scrollbar_visible(true);
        let geo =
            get_scrollbar_geometry(Rect::new(0, 0, 80, 10), 50, &sv).expect("scrollbar geometry");
        assert_eq!(geo.track_height, 10);
        assert_eq!(geo.max_scroll_top, 40);
        // thumb = round(track²/content) = round(100/50)=2
        assert_eq!(geo.thumb_height, 2);
    }
}
