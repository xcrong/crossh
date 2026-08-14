# 命令历史测试的时间精度竞态

## 症状

GitHub Actions 中 `commands::tests::aggregates_commands_and_returns_top_thirty` 仅在 windows-latest 上偶发失败：`commands.rs` 断言 `left: 2, right: 3`（`top()[0].count` 期望 3，实际 2）。macOS 与 Ubuntu CI 稳定通过。

## 根因

`unix_timestamp()` 以秒为单位记录 `last_used`。该测试先循环写入 320 条记录（每条 `record()` 都会 `persist()` 落盘），再对 `command-0` 补录 2 次。快速机器上整个循环在 1 秒内完成，所有记录 `last_used` 相同，排序按命令名 tiebreak，`command-0` 排最前，不会被 300 条上限淘汰，补录后 count 为 3；慢速 runner 上 320 次落盘跨秒，最早的 `command-0` 成为最旧记录被淘汰，2 次补录只累计到 2。测试结果因此取决于机器速度和墙钟秒边界，而不是被测逻辑。

## 持久规则

- 需要精确相对顺序的断言不要依赖秒级时间戳 + 大批量落盘的完成时机。
- 时间相关测试应让关键记录在时间上严格最新（最后写入且写后不再淘汰），或直接构造确定性的 `last_used`。
- `record()` 的 300 条上限与按 count 排序的聚合逻辑本身无平台差异，问题只出在测试对时间边界的耦合。

## 验证

循环改为 `1..MAX_HISTORY_ENTRIES + 21`（不预录 `command-0`），结尾显式补录 3 次：`command-0` 总是最新记录，count 恒为 3，其余断言（total 300、top 置顶、显示 30 条）不变。本地连跑 5 次 + 完整 `crossh-core` 库测试通过；CI windows-latest 复跑归属最终验证。

关键词：`commands.rs`、`aggregates_commands_and_returns_top_thirty`、`last_used`、`unix_timestamp`、秒级精度、windows-latest、断言 2 vs 3、命令历史
