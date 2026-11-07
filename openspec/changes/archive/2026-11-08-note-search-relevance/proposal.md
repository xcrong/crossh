# Proposal

## Why

`Note` 的检索现在是「先跑 FTS，结果为空才回退 LIKE」的二选一分叉（`crates/crossh-note/src/lib.rs:478` 的 `search`），这带来两个用户可见的缺陷：

1. **混合查询会静默漏结果**：搜 `hello 天气` 时 FTS 命中 `hello` 就立刻返回，正文里含 `天气` 的笔记永远不会出现——而当前 `search_like` 又只把整串查询当子串，即使走回退路径也匹配不到 `天气`。中文笔记是这个库的主要使用场景，等于「中英混排笔记搜不全」。
2. **没有任何相关度排序**：两条路径都只按 `pinned DESC, updated_at DESC` 出结果，`search_fts` 连 `bm25` 都没取。命中 1 次和命中 5 次的笔记无法区分。

现状还被测试固化了：`spec_20260903_note__chinese_search_fallback_like`（中文只能走 LIKE）与 `spec_20260903_note__fts_hit_preferred`（FTS 命中时 `search` 必须等于 `search_fts`）断言的就是上面这个分叉本身。规格化描述检索行为、把这两条断言改成「并集 + 可解释排序」，是这次变更的动机。

## What Changes

- **检索改为多路径并集**：FTS 命中与正文子串命中都参与，按笔记 id 去重合并，任一单条路径命中即必须出现在结果里，不再「FTS 非空就丢掉另一条路径」。
- **子串路径改为按词匹配**：查询按空白切词，任一词作为正文子串命中即入选（与现有 FTS 把词用 `OR` 连接的语义对齐），修掉 `hello 天气` 这类查询。
- **排序改为可解释的三段式**：置顶优先 → 命中次数多者优先 → 首次命中位置更靠前者优先 → `updated_at` 降序 → `id` 降序。相关度只用「正文里能算出来的证据」，不引入 `bm25`，保证两条路径同尺度可比。
- **查询容错**：只含空白等价于 `list()`；`%` / `_` 按字面匹配；引号、`*`、`NEAR(` 等 FTS 语法字符不产生错误；FTS 路径出错时降级为纯子串检索并记 `warn`，不再把错误抛给视图层的「查询失败」提示。
- **不改 schema、不加依赖、不做迁移**：CJK 仍靠正文子串扫描，成本上限写进 `ponytail:` 注释，升级路径（影子 bigram 索引）只记录不实现。
- **顺带修正检索相关文档**：`README.md` 的 Note 特性行（仍写着已删除的「标签」）与 `docs/architecture.md` 里 `crossh-note` 的 "FTS5 with a LIKE fallback for CJK text"，随本次语义变更一起改正。
- **BREAKING**：无。`NoteStore::search` 签名不变；`search_fts` / `search_like` 仍是私有路径（测试与合并逻辑使用）。

## Capabilities

### New Capabilities

- `note-search`: 笔记本地检索的行为契约——结果集合的完整性、排序优先级、查询容错与空查询语义，以及「CJK 不建专用索引」这一显式取舍。

### Modified Capabilities

无（`openspec/specs/` 目前为空，本变更首次建立该能力）。

## Impact

- `crates/crossh-note/src/lib.rs`：`search` 的合并与排序、`search_fts` 容错与取词、`search_like` 改多词 OR、新增纯排序函数与证据结构；既有两条 `spec_20260903_note__*` 测试按新语义更新。
- `crates/crossh-note-view`：无代码改动（`window.rs` 只消费 `Store::search` 的集合与顺序），需确认列表测试没有依赖旧的「LIKE 整串匹配」顺序。
- 文档：`README.md` 第 10 行的 Note 特性描述（仍写着已删除的「标签」，且未提检索语义）、`docs/architecture.md` 里 `crossh-note` 的 "FTS5 with a LIKE fallback for CJK text" 需要改成并集语义。
- 无新依赖、无 schema 变更、无平台差异：纯逻辑 crate，本机 `cargo test -p crossh-note` 可完整验证。
