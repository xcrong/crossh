//! 模态对话框骨架：scrim 遮罩 + 居中卡片 + 标题行 / 可选正文 / 尾部内容。

use std::rc::Rc;

use gpui::{
    AnyElement, App, ClickEvent, FontWeight, InteractiveElement, IntoElement, ParentElement,
    RenderOnce, SharedString, StatefulInteractiveElement, Styled, Window, div, px,
};

use crate::theme;

type ClickHandler = Rc<dyn Fn(&ClickEvent, &mut Window, &mut App)>;

/// 模态对话框的通用骨架。
///
/// 渲染为绝对定位的 scrim 遮罩(可带 id 与点击取消)内嵌一张居中卡片
/// (`w*p_5` + surface 背景 + `border_strong` 描边 + `RADIUS_MD` 圆角 + 阴影)。
/// 卡片内容依次为标题行(icon 由调用方传入的完整元素)、可选正文、追加内容。
/// 交互(输入框/按钮等)由调用方以 `.child()` 传入,组件本身无业务逻辑。
#[derive(IntoElement)]
pub struct ModalDialog {
    title: SharedString,
    title_icon: AnyElement,
    width: gpui::Pixels,
    body: Option<SharedString>,
    scrim_id: Option<SharedString>,
    card_id: Option<SharedString>,
    on_backdrop_click: Option<ClickHandler>,
    blocks_card_clicks: bool,
    children: Vec<AnyElement>,
    actions: Option<AnyElement>,
}

impl ModalDialog {
    pub fn new(title: impl Into<SharedString>, title_icon: impl IntoElement) -> Self {
        Self {
            title: title.into(),
            title_icon: title_icon.into_any_element(),
            width: px(440.),
            body: None,
            scrim_id: None,
            card_id: None,
            on_backdrop_click: None,
            blocks_card_clicks: false,
            children: Vec::new(),
            actions: None,
        }
    }

    pub fn width(mut self, width: impl Into<gpui::Pixels>) -> Self {
        self.width = width.into();
        self
    }

    /// 可选正文：`mt_2` + `text_xs` + `muted_text` 的说明段落。
    pub fn body(mut self, body: impl Into<SharedString>) -> Self {
        self.body = Some(body.into());
        self
    }

    pub fn scrim_id(mut self, scrim_id: impl Into<SharedString>) -> Self {
        self.scrim_id = Some(scrim_id.into());
        self
    }

    pub fn card_id(mut self, card_id: impl Into<SharedString>) -> Self {
        self.card_id = Some(card_id.into());
        self
    }

    /// scrim 遮罩的点击回调(通常用于点击背景取消模态)。
    ///
    /// 注意:该回调仅在设置了 [`ModalDialog::scrim_id`] 时才会被分发——
    /// 无 id 的元素在 GPUI 中不会收到 click listener 事件。
    pub fn on_backdrop_click(
        mut self,
        on_backdrop_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_backdrop_click = Some(Rc::new(on_backdrop_click));
        self
    }

    /// 卡片自身阻止事件冒泡,防止卡片内点击穿透触发遮罩回调。
    ///
    /// 注意:同 [`ModalDialog::on_backdrop_click`],需要 [`ModalDialog::card_id`] 才会生效。
    pub fn blocks_card_clicks(mut self) -> Self {
        self.blocks_card_clicks = true;
        self
    }

    /// 追加尾部内容;按调用顺序渲染在标题行 / 正文之后。
    pub fn child(mut self, child: impl IntoElement) -> Self {
        self.children.push(child.into_any_element());
        self
    }

    /// 尾部按钮行;渲染在 children 之后的统一 `flex_row + items_center + gap_2 + mt_4` 骨架上。
    pub fn actions(mut self, actions: impl IntoElement) -> Self {
        self.actions = Some(actions.into_any_element());
        self
    }
}

impl RenderOnce for ModalDialog {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        debug_assert!(
            self.on_backdrop_click.is_none() || self.scrim_id.is_some(),
            "on_backdrop_click requires scrim_id; click listeners on id-less elements are never dispatched"
        );
        debug_assert!(
            !self.blocks_card_clicks || self.card_id.is_some(),
            "blocks_card_clicks requires card_id for the same reason"
        );
        let ModalDialog {
            title,
            title_icon,
            width,
            body,
            scrim_id,
            card_id,
            on_backdrop_click,
            blocks_card_clicks,
            children,
            actions,
        } = self;

        let mut card = div()
            .w(width)
            .p_5()
            .bg(theme::surface())
            .border_1()
            .border_color(theme::border_strong())
            .rounded(px(theme::RADIUS_MD))
            .shadow_md()
            .flex()
            .flex_col()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .text_sm()
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(theme::text())
                    .child(title_icon)
                    .child(title),
            );
        if let Some(body) = body {
            card = card.child(
                div()
                    .mt_2()
                    .text_xs()
                    .text_color(theme::muted_text())
                    .child(body),
            );
        }
        for child in children {
            card = card.child(child);
        }
        if let Some(actions) = actions {
            card = card.child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .mt_4()
                    .child(actions),
            );
        }

        let card: AnyElement = if let Some(card_id) = card_id {
            let card = card.id(card_id);
            let card = if blocks_card_clicks {
                card.on_click(|_ev, _window, cx| cx.stop_propagation())
            } else {
                card
            };
            card.into_any_element()
        } else {
            let mut card = card;
            if blocks_card_clicks {
                card.interactivity()
                    .on_click(|_ev, _window, cx| cx.stop_propagation());
            }
            card.into_any_element()
        };

        let scrim = div()
            .absolute()
            .size_full()
            .top_0()
            .left_0()
            .flex()
            .items_center()
            .justify_center()
            .bg(theme::scrim());
        let scrim: AnyElement = if let Some(scrim_id) = scrim_id {
            let scrim = scrim.id(scrim_id);
            let scrim = if let Some(handler) = on_backdrop_click {
                scrim.on_click(move |event, window, cx| handler(event, window, cx))
            } else {
                scrim
            };
            scrim.child(card).into_any_element()
        } else {
            let mut scrim = scrim;
            if let Some(handler) = on_backdrop_click {
                scrim
                    .interactivity()
                    .on_click(move |event, window, cx| handler(event, window, cx));
            }
            scrim.child(card).into_any_element()
        };
        scrim
    }
}

#[cfg(test)]
mod tests {
    use gpui::{div, px};

    use super::ModalDialog;

    #[test]
    fn modal_dialog_has_expected_defaults() {
        let modal = ModalDialog::new("title", div());
        assert_eq!(modal.title.as_ref(), "title");
        assert_eq!(modal.width, px(440.));
        assert_eq!(modal.body, None);
        assert_eq!(modal.scrim_id, None);
        assert_eq!(modal.card_id, None);
        assert!(modal.on_backdrop_click.is_none());
        assert!(!modal.blocks_card_clicks);
        assert!(modal.children.is_empty());
        assert!(modal.actions.is_none());
    }

    #[test]
    fn modal_dialog_builder_sets_options() {
        let modal = ModalDialog::new("title", div())
            .width(px(500.))
            .body("Body text")
            .scrim_id("scrim")
            .card_id("card")
            .on_backdrop_click(|_, _, _| {})
            .blocks_card_clicks()
            .child(div())
            .child(div())
            .actions(div());
        assert_eq!(modal.width, px(500.));
        assert_eq!(modal.body.as_deref(), Some("Body text"));
        assert_eq!(modal.scrim_id.as_deref(), Some("scrim"));
        assert_eq!(modal.card_id.as_deref(), Some("card"));
        assert!(modal.on_backdrop_click.is_some());
        assert!(modal.blocks_card_clicks);
        assert_eq!(modal.children.len(), 2);
        assert!(modal.actions.is_some());
    }

    #[test]
    fn modal_dialog_title_is_preserved() {
        let modal = ModalDialog::new("delete entry", div());
        assert_eq!(modal.title.as_ref(), "delete entry");
    }
}
