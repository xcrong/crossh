# 0002-logic-ui-layering

## 状态

已接受

## 背景

终端协议、SSH、更新、AI wire adapter 等逻辑需要在后台任务和命令行环境中复用，不应被 GPUI 生命周期或视图状态绑定。单靠目录约定不能阻止错误依赖进入逻辑 crate。

## 决策

逻辑 crate 禁止导入 `gpui`、`gpui_platform`、`crossh-ui` 或应用 crate；GPUI view 只能向逻辑层调用 crate-root API。用 Cargo crate 依赖图提供编译期边界，再用 `scripts/check-architecture.sh` 检查容易回归的源码模式。

## 结果/代价

跨层调用会在依赖解析或编译阶段失败，后台逻辑更容易测试和复用；代价是需要通过 channel 和公共数据类型传递状态，不能直接传递 GPUI context 或 entity。

## 关联规则

- `AGENTS.md` 的 Logic must not depend on UI
- `docs/architecture.md` 的 Boundary Rules
- `scripts/check-architecture.sh`
