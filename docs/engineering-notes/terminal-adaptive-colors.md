# 终端自适应颜色与主题底色偏差(TUI splash 颜色对不上)

## 症状

同一 TUI(如 opencode / VT Code)在 Crossh 里运行的进入/退出 splash(ASCII 艺术横幅)与 Zed、macOS 原生终端相比,"颜色对不上":白色区域刺眼、阴影过渡层("细线")消失、整体层次塌平。同时用户确认终端收到的**字节流与对比终端完全一致**。

## 根因

不是渲染 bug,而是"发射端自适应设计 + 接收端底色离群"的组合:

1. opencode 的 splash 是**动态自适应**的:启动时发 `OSC 10;?`/`11;?` 查询终端前景/背景色,把查询到的背景色作为阴影混合基点(`splashShadow` = 主色向背景混 14%),再以真彩 `38;2` 直出(源码:`packages/opencode/src/cli/cmd/run/theme.ts` 的 `splashTheme`/`splashShadow`/`nearestIndexed`)。
2. Crossh 的 `canvas`/`terminal_background` 旧值 `#0d1014` 比其他终端(Zed `#23272e`、mac `#1e1e1e`)暗一个量级 → 14% 混合的所有过渡层次全部塌进黑色,阴影"细线"消失、白色对比拉满。
3. 对比终端之间观感一致,是因为它们底色互相接近;Crossh 恰好是离群值,并非 Crossh 解析或渲染错误。

## 已排除项(排查时逐项验证过)

| 嫌疑 | 结论 | 依据 |
| --- | --- | --- |
| 256 色索引表不一致 | 排除 | 真彩能力下 opencode 直出 `38;2`,不量化成索引 |
| APCA 最小对比度修正改色 | 排除 | `is_app_chosen_exact_color` 对 `Color::Spec` 和 `Indexed(16..=255)` 返回 true,真彩跳过修正(Zed `crates/terminal/src/terminal.rs`) |
| `rgba_color` 色彩转换偏差 | 排除 | `/255` 直转 Hsla,无 gamma 抖动 |
| OSC 4/10/11 查询无应答 | 排除 | alacritty_terminal 把查询转 `Event::ColorRequest`,Zed terminal crate 已自动应答:`colors()[index]` 或 `get_color_at_index(index, theme)`(应答值 = `terminal_background`) |

## 持久规则

1. **跨终端颜色对比,先比"终端应答的背景色"再比字节流**。`printf '\x1b]11;?\x07'` 得到三端各自的背景值;opencode 一类自适应 TUI 的 splash 会随该值改变,这是设计行为,不是 bug。
2. **主题暗面 token 必须整体平移,不能只改 canvas**。`canvas` 是全主题最暗锚点,单独提亮会让 `sidebar`/`surface` 层级倒挂;整族同步平移保持相对距离。
3. 字节流一致时,把排查焦点从"协议解析"移到"渲染端 + 发射端自适应输入"(此处即背景色应答值)。
4. Crossh 的 `terminal_background` 与 `canvas` 同值,且无透明度;若未来引入透明/合成底色,必须保证 OSC 11 应答值与实际渲染底色一致,否则自适应 TUI 的填充块会出现拼缝。

## 验证方法

1. 跨终端跑 `printf '\x1b]11;?\x07'; printf '\r\n'`,对比应答 RGB。
2. `script -q /tmp/oc.ts opencode`,正常退出后 `xxd` 尾部,确认 splash 为 `38;2` 真彩直出、阴影色 = `mix(主色, 应答背景, 0.14)`。
3. 修改后重新编译,跑 opencode 退出 splash,与 Zed 逐像素对比阴影过渡层与白色区域。
4. 单测:`crates/crossh-theme` 的 `palette_tokens_expose_expected_channels` 覆盖 canvas 通道值(对齐时更新断言)。

## 涉及代码

- `crates/crossh-theme/src/lib.rs`:`canvas()` 等暗面 token(当前 `canvas = 0x23272e`,对齐 Zed)
- `src/infrastructure/theme.rs`:`terminal_background = canvas`、`terminal_ansi_background = canvas`(OSC 11 应答来源)
- `src/features/terminal/zed_view/terminal_element.rs`:真彩 `Color::Spec` → `rgba_color`;dim 用 `fg.a *= 0.7`(Crossh 特色,参考终端用亮度或调色板 dim 色)

## 关键词

opencode, VT Code, OSC 10, OSC 11, OSC 4, splash, 自适应主题, 38;2 真彩, 256 色, canvas, splashShadow, nearestIndexed, ColorRequest, get_color_at_index, is_app_chosen_exact_color, 阴影混合, 细线消失, 白色刺眼