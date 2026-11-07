# Review — note-search-relevance

- 评审日期：2026-11-07
- 评审对象：`proposal.md`、`specs/note-search/spec.md`、`design.md`、`tasks.md`
- 结论：**带条件通过**（下列条件已全部关闭，可进入 apply）
- 校验：`openspec validate note-search-relevance --strict` → valid

## 一、行为主张核对（对照代码）

| 主张 | 位置 | 结论 |
| --- | --- | --- |
| `search` 是「FTS 非空即返回、否则回退 LIKE」的二选一分叉 | `crates/crossh-note/src/lib.rs:478-489` | 属实 |
| `search_like` 把整串查询当子串，多词查询在子串路径必然零命中 | `crates/crossh-note/src/lib.rs:522-528` | 属实 |
| `search_fts` 未取 `bm25`，只按 `pinned`/`updated_at` 排序 | `crates/crossh-note/src/lib.rs:496` | 属实 |
| 两条路径都过滤软删除 | `crates/crossh-note/src/lib.rs:496`、`:528` | 属实 |
| 视图层按返回顺序渲染，空查询直接走 `list()` | `crates/crossh-note-view/src/window.rs:324-331` | 属实 |
| 现有测试固化了旧分叉 | `crates/crossh-note/src/lib.rs:885-899`、`:900-910` | 属实 |
| `docs/architecture.md` 写作 "FTS5 with a LIKE fallback for CJK text" | `docs/architecture.md:36` | 属实 |
| `README.md` 仍把「标签」列为 Note 特性，而 schema 已无 `tags` | `README.md:10`，对照 `spec_20260903_note__schema_has_no_tags` | 属实 |

## 二、发现

### R1 规格缺软删除约束（必改 · 已关闭）

`specs/note-search/spec.md` 原「并集」需求只谈两条路径的合并，未写「结果必须排除软删除笔记」。合并逻辑一旦绕过 SQL 的两处 `deleted_at IS NULL`（例如先取全部候选再排序），已删笔记就会回到列表。

处理：requirement 补 `MUST 排除已软删除（deleted_at 非空）的笔记`，并新增 Scenario「已删除笔记不出现在结果中」。

### R2 「FTS 路径失败」场景不可触发（必改 · 已关闭）

规格原场景写「FTS 查询因语法或索引异常返回错误」，但 `search_fts` 是私有函数，且查询词总被引号包裹（`lib.rs:493-503`），语法错误几乎不可能出现，该场景无法写成测试。

处理：场景改为「FTS 索引表缺失（或 FTS 查询返回错误）」；`tasks.md` 2.2 明确用同模块测试的私有连接 `DROP TABLE notes_fts` 制造失败。

### R3 design 的成本结论不成立（必改 · 已关闭）

`design.md` D5 原称「CJK 今天本来就走这条路径，本次没有让最坏情况变差」。并集语义要求每次都跑子串路径，纯 ASCII 查询因此从「FTS 命中即返回」变成「再扫一遍正文」，成本确实变大。

处理：D5 改为显式承认该取舍，Risks 首条同步改写为「先量化、再考虑影子 bigram 索引，不加缓存掩盖」。

### R4 证据函数的大小写折叠必须与 SQL 对齐（必改 · 已关闭）

若证据函数用 `to_lowercase()`（全 Unicode 折叠），会出现「Rust 侧算作命中、SQL 侧没命中」的不可见分歧，排序结果取决于哪条路径先进候选集。SQLite `LIKE` 与 FTS 的 tokenizer 都只折叠 ASCII。

处理：`tasks.md` 1.1 明确使用 `to_ascii_lowercase()`，并写明理由。

### R5 proposal 与 tasks 范围不一致（建议 · 已关闭）

`tasks.md` 4.2 要修 `README.md` 的「标签」陈旧描述，`proposal.md` 的 What Changes 未包含。

处理：proposal 补一条「顺带修正检索相关文档」。

### R6 tasks 3.3 属过度假设（建议 · 已关闭）

复核 `crates/crossh-note-view/src/window/tests.rs`，未发现依赖检索顺序的断言（`note_search_reload_is_debounced` 用的是无匹配查询，其余用例直接赋值 `notes`）。要求「按新排序更新断言」是凭空的返工预期。

处理：3.3 改为「跑 `cargo test -p crossh-note-view` 确认无回归，仅在实际出现依赖旧顺序的断言时才更新」。补充：该条修订在 apply 阶段才真正落到 `tasks.md`（评审时只写了结论，未同步改文件），现已改正。

### R7 排序键跨路径混用同一把尺子（记录 · 接受，无需动作）

「命中次数」对 FTS（整词匹配）与子串（可匹配词内片段，如 `wor` → `world`）两种命中混算，后者的次数可能偏高。`design.md` D2 已声明这是有意为之：只使用两条路径都能算出的证据，避免量纲不可比。保留。

## 三、复审提示

本评审与提案由同一会话完成，独立性有限。若该变更影响面扩大（例如真的引入 CJK 索引），应另找评审者复核 R7 的取舍与 `design.md` D1 的替代方案评估。

## 四、必改条件关闭状态

| 条件 | 状态 |
| --- | --- |
| R1 规格补软删除约束 | 已关闭（`specs/note-search/spec.md`） |
| R2 失败场景改为可验证条件 | 已关闭（spec 场景 + `tasks.md` 2.2） |
| R3 成本结论按实际改写 | 已关闭（`design.md` D5 与 Risks） |
| R4 大小写折叠对齐 ASCII | 已关闭（`tasks.md` 1.1） |
| R5 proposal 补文档范围 | 已关闭（`proposal.md` What Changes） |
| R6 task 3.3 收敛为回归验证 | 已关闭（`tasks.md` 3.3，apply 阶段补齐） |
