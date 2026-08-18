# serde_json Value 字段顺序随构建图变化（preserve_order）

## 症状

`cargo test -p crossh-update` 全部通过，`cargo test --workspace` 中同一测试失败：
断言「serde_json::Value roundtrip 后字节 ≠ 原始字节」在 workspace 构建下不成立，
Value roundtrip 输出与原 JSON 字节完全一致（字段顺序被保留而非按字母排序）。

## 根因

Zed git 依赖（`fs`、`cloud_llm_client`、`tree-sitter` 等）启用了
`serde_json/preserve_order` feature。Cargo 的 feature 解析按「本次构建图」
统一：单包构建（`-p`）不包含 Zed 包，serde_json 无 preserve_order
（Map = BTreeMap，字段按字母序）；workspace 全量构建包含 Zed 包，
serde_json 全局启用 preserve_order（Map = IndexMap，字段保持插入顺序）。
因此 `serde_json::Value` 的序列化行为在两种构建方式下不同。

## 持久规则

- **测试不得依赖 `serde_json::Value` 的字段排序**：preserve_order 是否生效
  取决于构建图，单包与 workspace 测试结果可能不一致。
- 需要「语义等价但字节不同」的 JSON 输入时，显式构造：pretty 打印（空白差异）、
  手写打乱字段顺序的字符串，并断言字节确实不同（`assert_ne!` 作为前提检查）。
- 生产代码不受影响：canonical 序列化一律走结构体 `serde_json::to_vec`
  （字段顺序 = 结构体声明顺序），与 Value 的 Map 实现无关。

## 验证

`cargo test -p crossh-update && cargo test --workspace` 均通过后，
`spec_20260818_manifest_sig_semantically_equivalent_bytes_verify` 的
pretty / 乱序两个变体都执行。

## 关键词

serde_json, preserve_order, IndexMap, workspace feature 统一, 单包测试通过但 workspace 失败,
Value 序列化顺序, canonical JSON
