//! scroll_view — pi-tui 的 components/scroll-view.js 移植
//!
//! follow:end / scrollTop / updateLayout / scrollTo(scrollBy 返回剩余冒泡)

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FollowMode {
    End,
    None,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Overscroll {
    Contain,
    Chain,
}

#[derive(Debug, Clone)]
pub struct ScrollView {
    pub scroll_top: usize,
    pub viewport_height: usize,
    pub content_height: usize,
    pub follow_end: bool,
    /// 是否正在跟随末尾（pi 的 followingEnd）
    pub following_end: bool,
    pub overscroll: Overscroll,
    scrollbar_visible_flag: std::cell::Cell<bool>,
}

impl ScrollView {
    pub fn new(follow_end: bool) -> Self {
        Self {
            scroll_top: 0,
            viewport_height: 0,
            content_height: 0,
            follow_end,
            following_end: follow_end,
            overscroll: Overscroll::Chain,
            scrollbar_visible_flag: std::cell::Cell::new(false),
        }
    }

    /// 兼容旧签名：默认 follow end
    pub fn with_viewport(viewport_height: usize) -> Self {
        let mut sv = Self::new(true);
        sv.viewport_height = viewport_height.max(1);
        sv
    }

    pub fn is_following_end(&self) -> bool {
        self.following_end
    }

    /// pi 的 isScrollbarVisible（auto 模式下内容超出且处于活动窗口）
    pub fn scrollbar_visible(&self) -> bool {
        self.scrollbar_visible_flag.get()
    }

    pub fn set_scrollbar_visible(&self, visible: bool) {
        self.scrollbar_visible_flag.set(visible);
    }

    /// 测试/外部设置 content_height（保留兼容名）
    pub fn set_content_height(&mut self, height: usize) {
        self.update_layout(height, self.viewport_height);
    }

    pub fn set_viewport_height(&mut self, height: usize) {
        self.viewport_height = height.max(1);
        // 重新 clamp（pi 在 updateLayout 内做）
        let max = self.max_scroll_top();
        if self.following_end {
            self.scroll_top = max;
        } else {
            self.scroll_top = self.scroll_top.min(max);
        }
    }

    pub fn max_scroll_top(&self) -> usize {
        self.content_height.saturating_sub(self.viewport_height)
    }

    /// pi 的 updateLayout：布局后同步内容/视口高度并处理跟随
    pub fn update_layout(&mut self, content_height: usize, viewport_height: usize) {
        self.content_height = content_height;
        if viewport_height > 0 {
            self.viewport_height = viewport_height;
        }
        let max = self.max_scroll_top();
        if self.following_end {
            self.scroll_top = max;
        } else {
            self.scroll_top = self.scroll_top.min(max);
        }
        if self.content_height <= self.viewport_height {
            self.set_scrollbar_visible(false);
        }
    }

    /// pi 的 scrollTo：返回是否变化
    pub fn scroll_to(&mut self, top: usize) -> bool {
        let requested = top;
        let max = self.max_scroll_top();
        let next = requested.min(max);
        let next_following = self.follow_end && next == max;
        let changed = next != self.scroll_top || next_following != self.following_end;
        self.scroll_top = next;
        self.following_end = next_following;
        changed
    }

    /// pi 的 scrollBy：返回未消费的剩余量（用于冒泡到外层 ScrollView）
    pub fn scroll_by(&mut self, lines: i32) -> i32 {
        if lines == 0 {
            return 0;
        }
        let max = self.max_scroll_top() as i32;
        let start = if self.following_end {
            max
        } else {
            self.scroll_top as i32
        };
        let next = (start + lines).clamp(0, max);
        let moved = next - start;
        self.scroll_top = next as usize;
        self.following_end = self.follow_end && next == max && lines >= 0 || (lines < 0 && false);
        if lines < 0 {
            self.following_end = false;
        }
        (lines - moved).abs().min(lines.abs()) * lines.signum()
    }

    pub fn scroll_to_start(&mut self) {
        self.scroll_to(0);
    }

    pub fn scroll_to_end(&mut self) {
        let next = self.max_scroll_top();
        self.scroll_top = next;
        self.following_end = self.follow_end;
    }
}

pub const PAGE_SCROLL_OVERLAP: usize = 4;

#[cfg(test)]
mod tests {
    #![allow(non_snake_case)]

    use super::*;

    #[test]
    fn spec_20260822_agent_tui_pi_parity__follow_end_tracks_content_growth() {
        let mut sv = ScrollView::new(true);
        sv.update_layout(30, 10);
        assert_eq!(sv.scroll_top, 20); // 跟随到底
        assert!(sv.is_following_end());
        sv.update_layout(40, 10);
        assert_eq!(sv.scroll_top, 30); // 内容增长继续跟随
    }

    #[test]
    fn spec_20260822_agent_tui_pi_parity__scroll_up_breaks_follow() {
        let mut sv = ScrollView::new(true);
        sv.update_layout(30, 10);
        sv.scroll_by(-5);
        assert_eq!(sv.scroll_top, 15);
        assert!(!sv.is_following_end());
        // 内容增长不再跟随
        sv.update_layout(40, 10);
        assert_eq!(sv.scroll_top, 15);
        // 回到底部恢复跟随
        sv.scroll_to_end();
        assert!(sv.is_following_end());
    }

    #[test]
    fn spec_20260822_agent_tui_pi_parity__scroll_by_returns_bubble_remainder() {
        let mut sv = ScrollView::new(true);
        sv.update_layout(15, 10);
        sv.scroll_to_start();
        // 向上滚 5，只能到 0，剩余 5 冒泡
        let remaining = sv.scroll_by(-5);
        assert_eq!(remaining, -5);
        // 向下滚 100，只能到 5，剩余 95 冒泡
        let remaining = sv.scroll_by(100);
        assert_eq!(remaining, 95);
        assert_eq!(sv.scroll_top, 5);
    }

    #[test]
    fn spec_20260822_agent_tui_pi_parity__page_scroll_overlap_is_4() {
        let mut sv = ScrollView::new(true);
        sv.update_layout(100, 20);
        sv.scroll_to_start();
        sv.scroll_by((20 - PAGE_SCROLL_OVERLAP) as i32);
        assert_eq!(sv.scroll_top, 16);
    }
}
