# Tasks

## 1. 排序与证据（纯逻辑，不碰 SQL）

- [x] 1.1 在 `crates/crossh-note/src/lib.rs` 新增命中证据结构与计算函数：`MatchEvidence { hits, first_index }` 与 `evidence(content: &str, terms: &[String]) -> MatchEvidence`（用 `to_ascii_lowercase()` 折叠大小写——必须与 SQLite `LIKE` 的 ASCII-only 不敏感范围一致，不能用全 Unicode 折叠；跳过空词；`first_index` 取所有词中的最小位置）。验证：新增单元测试覆盖多词命中、零命中、位置取最小值三种情况。
- [x] 1.2 新增统一排序函数，把证据接进 `(pinned, hits, first_index, updated_at, id)` 的字典序比较。验证：单元测试覆盖置顶优先、命中次数优先、同分时按 `updated_at`/`id` 排定、同一输入重复调用顺序一致。

## 2. 并集与容错

- [x] 2.1 `search_like` 改为接收词列表并拼接多词 `OR`，每个词独立做 `%` / `_` 转义并统一 `ESCAPE '\'`。验证：`search("a%b")` 只命中内容 `a%b`、不命中 `axb`；`search("hello 天气")` 能命中中文笔记。
- [x] 2.2 `search` 把 FTS 路径的 `Err` 降级为「只用子串路径」并 `log::warn!`，同时保留现有的引号转义。验证：新增用例在测试内部先 `DROP TABLE notes_fts` 制造 FTS 失败（同模块测试可直接拿到私有连接），再断言 `search("hello")` 仍返回命中且结果为 `Ok`。
- [x] 2.3 `search` 改为并集合并：FTS 候选 id 与子串候选 id 合并、按 id 去重、再统一排序；查询只含空白时仍直接返回 `list()`。验证：`search("hello 天气")` 同时返回两条笔记，且结果中无重复 `id`。

## 3. 规格回归与用例

- [x] 3.1 改写 `spec_20260903_note__fts_hit_preferred`（旧断言要求 `search == search_fts`，与并集语义冲突）与 `spec_20260903_note__chinese_search_fallback_like`（不再把「回退 LIKE」当契约）。验证：`cargo test -p crossh-note` 通过，两条用例名与新语义一致。
- [x] 3.2 按 delta spec 补齐场景用例：空白查询等于 `list()`、中文单字查询、未命中返回空列表、`search("HELLO")` 命中 `hello world`、软删除笔记不出现在结果中、置顶笔记排在命中次数更多的普通笔记之前。验证：用例名统一以 `spec_20261107_note_search__` 开头，`cargo test -p crossh-note` 全绿。
- [x] 3.3 复核 `crates/crossh-note-view/src/window/tests.rs` 是否依赖检索结果顺序：已确认 `note_search_reload_is_debounced` 用的是无匹配查询、其余用例直接赋值 `notes`，无需改断言。验证：`cargo test -p crossh-note-view` 无回归；仅在实际出现依赖旧顺序的断言时才更新。

## 4. 上限注释与文档

- [x] 4.1 在检索实现处补 `ponytail:` 注释：子串路径是随笔记数与正文大小线性增长的扫描、ASCII 查询同样会跑这条路径、升级路径是影子 bigram 索引。验证：`rg -n "ponytail:" crates/crossh-note/src/lib.rs` 能定位到该注释且内容覆盖三点。
- [x] 4.2 更新 `README.md` 的 Note 特性行（删除已不存在的「标签」，写明并集检索与排序优先级）与 `docs/architecture.md` 中 `crossh-note` 的 "FTS5 with a LIKE fallback for CJK text" 描述。验证：`rg -n "LIKE fallback|标签" README.md docs/architecture.md` 无相关残留。

## 5. 变更级验证

- [x] 5.1 跑通 `cargo test -p crossh-note -p crossh-note-view`、`cargo clippy -p crossh-note --all-targets -- -D warnings`、`scripts/check-architecture.sh`，三者均无告警失败。验证：命令输出与退出码全绿，且提交前 `scripts/pre-commit` 同样通过。

## Workflow follow-up

- 提案评审结论记录在 `openspec/changes/note-search-relevance/review.md`，状态必须为「通过」或「带条件通过」才能进入 apply。
- apply 完成后运行 `openspec validate note-search-relevance --strict`，再 `openspec archive note-search-relevance`。
