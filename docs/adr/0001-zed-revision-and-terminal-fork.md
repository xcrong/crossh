# 0001-zed-revision-and-terminal-fork

## 状态

已接受

## 背景

Crossh 依赖 Zed 的 `gpui`、`terminal` 和相关基础设施，但完整的 `terminal_view` 还携带编辑器、LSP 和工作区应用层依赖。直接依赖它会扩大依赖图，也会让终端渲染随上游未锁定变化。

## 决策

在 `Cargo.toml` 中将 Zed 依赖固定到单个 git revision，并把所需的 `TerminalElement` 与 APCA 对比度代码按该 revision 裁剪到 `src/features/terminal/zed_view/`。本地代码只保留终端渲染所需部分，并由 Crossh 的薄宿主接入工作区。

## 结果/代价

构建可复现，终端层不需要引入编辑器应用层；代价是需要在 Zed revision 更新时人工同步本地 fork。第三方维护的 `terminal_element.rs` 由架构检查白名单豁免拆分。

## 关联规则

- `AGENTS.md` 的 Zed / GPUI Dependency Source
- `AGENTS.md` 的 Icon Assets 与文件大小纪律
- `scripts/check-architecture.sh`
