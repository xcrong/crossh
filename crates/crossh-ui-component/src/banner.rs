//! DangerBanner / ConfirmBar — 统一的横幅确认条。
//!
//! 背景：`render_discard_confirmation`、`render_stash_drop_confirmation`、
//! `render_conflict_actions` 三处曾同为 `bg(diff_del_bg / surface) + border(danger) + text(danger)`，
//! 视觉层级完全塌缩。本组件以 `BannerTone` 显式区分语义：
//! - `Danger`  — 破坏性确认（丢弃工作区、删除 stash），红系 `diff_del_bg / danger`
//! - `Warning` — 需决策的阻塞状态（冲突解决），黄系 `warning` / `surface`
//!
//! 布局：
//! - `Stacked` — 标题 + 描述 纵向堆叠，操作区右对齐（丢弃确认）
//! - `Inline`  — 图标 + 标题 单行自适应，操作区跟随换行（stash 删除、冲突操作）；`compact` 时切为纵向。

use gpui::{
    AnyElement, App, ElementId, FontWeight, InteractiveElement, IntoElement, ParentElement,
    RenderOnce, SharedString, Styled, Window, div, px,
};

use crate::theme;

/// 横幅语义色：决定背景、边框、图标与标题色。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum BannerTone {
    #[default]
    Danger,
    Warning,
}

impl BannerTone {
    fn bg(self) -> gpui::Rgba {
        match self {
            Self::Danger => theme::diff_del_bg(),
            Self::Warning => theme::surface(),
        }
    }

    fn border(self) -> gpui::Rgba {
        match self {
            Self::Danger => theme::danger(),
            Self::Warning => theme::warning(),
        }
    }

    fn accent(self) -> gpui::Rgba {
        self.border()
    }

    fn title_color(self) -> gpui::Rgba {
        self.border()
    }
}

/// 横幅布局：`Stacked` 用于带描述的确认，`Inline` 用于单行操作条。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum BannerLayout {
    #[default]
    Inline,
    Stacked,
}

/// 无状态横幅组件：调用方持有状态与回调，组件仅负责视觉与布局。
///
/// 渲染为 `w_full flex_shrink_0 border_b_1` 容器，左侧 2px 语义色 rail，
/// 语义由 `tone` 统一：背景 / 边框 / 图标 / 标题 同色，描述保持 `muted_text`。
#[derive(IntoElement)]
pub struct Banner {
    id: ElementId,
    tone: BannerTone,
    layout: BannerLayout,
    icon: Option<AnyElement>,
    title: Option<SharedString>,
    description: Option<SharedString>,
    compact: bool,
    actions: Vec<AnyElement>,
}

impl Banner {
    pub fn new(id: impl Into<ElementId>) -> Self {
        Self {
            id: id.into(),
            tone: BannerTone::default(),
            layout: BannerLayout::default(),
            icon: None,
            title: None,
            description: None,
            compact: false,
            actions: Vec::new(),
        }
    }

    pub fn tone(mut self, tone: BannerTone) -> Self {
        self.tone = tone;
        self
    }

    pub fn layout(mut self, layout: BannerLayout) -> Self {
        self.layout = layout;
        self
    }

    pub fn icon(mut self, icon: impl IntoElement) -> Self {
        self.icon = Some(icon.into_any_element());
        self
    }

    pub fn title(mut self, title: impl Into<SharedString>) -> Self {
        self.title = Some(title.into());
        self
    }

    pub fn description(mut self, description: impl Into<SharedString>) -> Self {
        self.description = Some(description.into());
        self
    }

    pub fn compact(mut self, compact: bool) -> Self {
        self.compact = compact;
        self
    }

    pub fn action(mut self, action: impl IntoElement) -> Self {
        self.actions.push(action.into_any_element());
        self
    }
}

impl RenderOnce for Banner {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        let bg = self.tone.bg();
        let border = self.tone.border();
        let accent = self.tone.accent();
        let title_color = self.tone.title_color();

        match self.layout {
            BannerLayout::Stacked => {
                // 丢弃确认：纵向堆叠，标题（带图标）+ 描述 + 右对齐操作区
                let mut outer = div()
                    .id(self.id)
                    .w_full()
                    .flex_shrink_0()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .p_2()
                    .bg(bg)
                    .border_b_1()
                    .border_color(border)
                    .relative();

                // 左侧 2px 语义 rail，增强与普通 toolbar 的区分
                outer = outer.child(
                    div()
                        .absolute()
                        .left_0()
                        .top_0()
                        .bottom_0()
                        .w(px(2.))
                        .bg(accent),
                );

                // 标题行：图标 + 标题 同色强调
                if let Some(title) = self.title {
                    let mut title_row = div().flex().items_center().gap_2().pl(px(6.));
                    if let Some(icon) = self.icon {
                        title_row = title_row.child(icon);
                    }
                    title_row = title_row.child(
                        div()
                            .min_w_0()
                            .flex_1()
                            .text_xs()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(title_color)
                            .child(title),
                    );
                    outer = outer.child(title_row);
                } else if let Some(icon) = self.icon {
                    outer = outer.child(div().pl(px(6.)).child(icon));
                }

                if let Some(description) = self.description {
                    outer = outer.child(
                        div()
                            .pl(px(6.))
                            .text_xs()
                            .text_color(theme::muted_text())
                            .child(description),
                    );
                }

                if !self.actions.is_empty() {
                    let mut row = div().w_full().flex().justify_end().gap_1().pl(px(6.));
                    for action in self.actions {
                        row = row.child(action);
                    }
                    outer = outer.child(row);
                }

                outer
            }
            BannerLayout::Inline => {
                // stash 删除 / 冲突操作：单行自适应，compact 时纵向
                let mut outer = div()
                    .id(self.id)
                    .w_full()
                    .flex_shrink_0()
                    .px_3()
                    .py_2()
                    .flex()
                    .items_center()
                    .gap_2()
                    .flex_wrap()
                    .bg(bg)
                    .border_b_1()
                    .border_color(border)
                    .relative();

                outer = outer.child(
                    div()
                        .absolute()
                        .left_0()
                        .top_0()
                        .bottom_0()
                        .w(px(2.))
                        .bg(accent),
                );

                if self.compact {
                    outer = outer.flex_col().items_start();
                }

                if let Some(icon) = self.icon {
                    outer = outer.child(icon);
                }

                if let Some(title) = self.title {
                    outer = outer.child(
                        div()
                            .min_w_0()
                            .flex_1()
                            .text_xs()
                            .text_color(title_color)
                            .child(title),
                    );
                }

                if let Some(description) = self.description {
                    outer = outer.child(
                        div()
                            .min_w_0()
                            .text_xs()
                            .text_color(theme::muted_text())
                            .child(description),
                    );
                }

                for action in self.actions {
                    outer = outer.child(action);
                }

                outer
            }
        }
    }
}

