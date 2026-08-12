# GPUI Flex 滚动容器

## 症状

纵向 flex 容器同时设置了 `max_h` 和 `overflow_y_scroll()`，内容很多时却无法用滚轮或触控板滚动。

## 根因

flex 子项默认允许收缩。布局会先把所有行压缩进容器的最大高度，因此没有形成可滚动的 overflow。

## 持久规则

固定高度的列表行、分组标题和分隔线必须设置 `flex_shrink_0()`。滚动容器仍需稳定 `id`、确定的高度或最大高度，以及 `overflow_y_scroll()`。

## 验证

用足够多的条目让内容理论高度超过容器上限，确认条目保持原高度，并用滚轮或触控板到达最后一项。

关键词：`GPUI`、`flex_shrink_0`、`overflow_y_scroll`、`max_h`、滚轮无效、列表压缩
