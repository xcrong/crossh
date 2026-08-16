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

## 延伸：内容区提前结束，footer 上方出现大块空白

### 症状

窗口使用纵向 flex shell 固定底部状态栏时，中间 Git 内容区只延伸到最后一行内容，
内容区边界提前结束，footer 上方剩余区域显示为空白。

### 根因

外层中间节点虽然设置了 `flex_1().min_h_0()`，但它本身是普通 block 容器；内部
Git pane 的 `flex_1()` 没有 flex parent 可以分配剩余高度，因此只按固有内容高度布局。

### 持久规则

纵向 shell 中承载可伸缩 pane 的中间 wrapper 必须同时设置
`flex().flex_col()`，再把 pane 设置为 `flex_1().min_h_0()`。不要只给 block wrapper
设置 `flex_1()`，否则 footer 会固定在底部，但内容边界会提前结束。

### 验证

用包含少量变更的 Git fixture 捕获标准窗口，确认左侧 pane 的底边延伸到状态栏上方，
并确认紧凑窗口的列表仍保持滚动区域。关键词：`GPUI`、`flex_1`、`min_h_0`、
`flex_col`、`StatusBar`、footer 空白、内容区提前结束。
