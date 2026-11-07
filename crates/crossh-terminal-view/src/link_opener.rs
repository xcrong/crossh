//! 链接打开策略的注入点。
//!
//! 终端只负责识别链接并发事件，真正"怎么打开"由应用层决定：应用可以安装
//! [`LinkOpener`]（例如把链接开进临时 WebView 预览），未安装或拒绝接管时
//! 回退到系统浏览器。

use std::rc::Rc;

use gpui::{App, Global};

/// 链接打开回调：返回 `false` 表示不接管这次打开。
type OpenFn = dyn Fn(&str, &mut App) -> bool;

/// 应用层安装的链接打开器。
#[derive(Clone)]
pub struct LinkOpener(pub Rc<OpenFn>);

impl Global for LinkOpener {}

/// 安装链接打开器（后装覆盖先装）。
pub fn set_link_opener(opener: LinkOpener, cx: &mut App) {
    cx.set_global(opener);
}

/// 统一的打开入口：先问应用层，未接管则走系统浏览器。
pub fn open_link(url: &str, cx: &mut App) {
    // 先取出 Rc：全局引用会与传给 opener 的 `&mut App` 冲突。
    let opener = cx.try_global::<LinkOpener>().map(|opener| opener.0.clone());
    if let Some(opener) = opener
        && opener(url, cx)
    {
        return;
    }
    cx.open_url(url);
}
