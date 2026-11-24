# Tasks（占位版）

> 只有 §1 可执行。§2 为补全后的草稿提示（不定为跟踪任务），触发补全时重写。

## 1 等待与观察（现在可执行）

- [ ] 1.1 冻结：2026-12-15 前不对本变更做任何代码与依赖改动。验证：本目录只有 planning artifacts。
- [ ] 1.2 12 月手动检查官方状态：`curl https://crates.io/api/v1/crates/gpui`（看 `max_version` 与发布时间）+ `curl https://crates.io/api/v1/crates/gpui_platform`（是否仍 404）+ gpui.rs 公告。验证：勾选时同步写下结论与依据链接（追加在本任务描述后）。
- [ ] 1.3 若已发布可切：补全 proposal/design/tasks（含源码对照的版本、启动 API、文件清单与验证），然后走 `review.md`。若未发布：继续等，下次复核时间由用户定。验证：两种出口有且仅有一个被执行。

## 2 发布后补全区（草稿，触发 1.3 前不跟踪）

- 试切：scratch branch 换源 → 启动代码 → `cargo check` → 错误清单定级
- 正式切换：`review.md` 关闭后合入主线 → 定向测试 → 视觉回归 → 文档
- 回滚：未合入主线前丢弃 scratch；合入后 `git revert` 单一提交

## Workflow follow-up

- 补全并实施完成后归档本变更。
- 若长期未发布，由用户决定关闭本变更或改门禁日期。
