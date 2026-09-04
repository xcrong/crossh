# Zed `fs_embed!` dev 资源误定位到 crossh 仓库根

## 症状

- dev 构建（`target/debug/crossh`）的终端里所有字符间距被拉宽，中英文皆然（用户截图：`x c ro ng@min is er`、`已 创建`）；release 正常。
- `UiAssetSource::default().list("fonts")` 在 dev 下返回 `[]`，`load_fonts` 静默加载 0 个字体（`add_fonts([])` 为 Ok），终端回退系统字体、格宽错乱。`load_fonts().expect()` 拦不住——错的是空列表，不是 Err。

## 根因

Zed `assets` crate 用 `util::fs_embed!`：release 真内嵌，dev 按可执行文件向上找 `.git` 在运行时读 checkout（`dev_repo_root`）。编进 crossh 后找到的是 **crossh 自己的仓库根**，其 `assets/` 下没有 Zed 的 `fonts/`（只有 `appicon/`），于是：

1. `list("fonts")` → `[]`；
2. `load()` 更隐蔽：dev 的 `get` 对缺失文件返回 None，而 `Assets::load` 用 `Option::with_context` 把 None 转成了 **Err**（不是 `Ok(None)`），`load_fonts` 里一旦列表非空就会 `?` 直接炸。

## 持久规则

1. **不要信任 dev 下的 `ZedAssets` 读写**：凡经 `UiAssetSource` 取 Zed 侧资源，必须处理"列表为空 / 加载为 Err"并回退到 cargo git checkout（按 Cargo.toml 锁定的 `assets` rev 定位 `git/checkouts/zed-<hash>/<rev>/assets`，仅 `debug_assertions`）。
2. **dev 回退只认完整 rev，短 rev 前缀仅作降级**；`CROSSH_ZED_CHECKOUT` 环境变量可直指定 checkout 根。
3. `load` 的回退要同时覆盖 `Ok(None)` 和 `Err`（dev 缺文件是 Err）。
4. 路径防穿越：只接受相对路径且拒绝 `..`（与 `AssetStore` 同规则）。

## 验证方法

- `cargo test -p crossh-ui assets`：`lists_bundled_lilex_without_external_store` 在 dev 下跑兜底链，断言能列出 `fonts/lilex/Lilex-Regular.ttf`（修前返回 `[]`，失败）。
- `cargo check -p crossh-ui --release`：`#[cfg(not(debug_assertions))]` 的 `ZedAssets.load(path)?` 分支平时编译不到，改动此处必须显式过一遍。
- 人工视觉：dev 构建重启后终端执行 `echo "已创建：https://example.com/docs（备注）。"`，确认等宽紧凑、无拉宽。

## 涉及代码

- `crates/crossh-ui/src/assets.rs`：`load`/`list` 的 `zed_dev_*` 兜底（dev 专属）、`locate_zed_dev_assets_dir`、`zed_assets_rev`（`CROSSH_ZED_CHECKOUT` 优先）。
- `scripts/check-architecture.sh` 无需改动；注意 `crates/terminal` 系 Zed fork 文件走 size 白名单。

## 关键词

`fs_embed`, `dev_repo_root`, `list("fonts")`, `load_fonts`, `Lilex`, `字符间距`, `拉宽`, `回退系统字体`, `with_context`, `CROSSH_ZED_CHECKOUT`, `git/checkouts`, `debug_assertions`
