# 更新加速源（gh-proxy.com 优先 + GitHub 回退）

## 元数据

- 状态：`done`（v0.16.4 发布成功；真实网络观察为发布后用户使用确认项）
- 创建：2026-08-18
- 相关 ADR：`docs/adr/0014-update-manifest-signature.md`
- 相关 issue / 路线图项：无
- CI 平台影响：`全部`（纯逻辑，跨平台；无 UI 变更）

## 背景

更新链路的 manifest 与 artifact 全部托管在 GitHub Releases，国内用户直连
github.com / objects.githubusercontent.com 经常超时或极慢，更新体验差。
Ed25519 签名（ADR 0014）落地后，manifest 的信任锚是内置公钥而非传输通道：
manifest 经任意 HTTPS 通道获取只要验签通过即可信，代理方无法注入恶意内容
（篡改即验签失败）。因此可以引入 gh-proxy.com 这类 URL 前缀代理作为**首选
传输通道**（代理不可达时回退 GitHub 原站），不降低安全边界。

用户裁决（2026-08-18）：这是**默认行为，不做任何设置项与 UI 变更**——
客户端内置加速前缀常量，每次请求固定按「加速源 → GitHub 原站」候选序列
尝试，无需用户配置。

## 目标

1. 更新检查与 artifact 下载**默认走加速前缀**（`https://gh-proxy.com/`），
   加速通道不可达时自动回退 GitHub 原站——对国内用户开箱即用，直连畅通的
   用户通过回退同样正常。
2. 安全边界不变：manifest 验签、artifact SHA-256/size 校验在两条通道下
   行为完全一致（复用现有校验，代理只是传输层）。
3. 零配置：不新增设置字段、不新增 UI 控件、不新增 i18n 文案。

## 非目标

- 不做可配置前缀、自建镜像地址设置、多加速节点选择、节点健康检测或速度测速。
- 不改 manifest / artifact 的 HTTPS 强制校验；不做 CDN 或自建镜像托管。
- 不调整请求超时策略（加速通道失败依赖 connect 快速失败 + 现有总超时）。
- 不做下载断点续传、并发分片。

## 行为契约

测试命名前缀：`spec_20260818_update_accel_`。

1. 当输入 GitHub release 资产 URL（`https://github.com/<owner>/<repo>/releases/download/...`
   或 `.../releases/latest/download/...`）时，URL 重写应输出
   `默认加速前缀 + 原始完整 URL`（即 `https://gh-proxy.com/https://github.com/...`）。
2. 当输入非 github.com 域名的 URL 时，URL 重写应原样返回（不重写）。
3. 候选请求序列应为 `[重写 URL, 原始 URL]`（加速优先）；当重写不生效
   （非 github.com 域名）时序列应去重为 `[原始 URL]`。
4. 网络层应按候选序列依次尝试，成功即止；全部失败时返回最后一次尝试的错误。
5. 传输类错误（连接失败、请求超时、HTTP 非成功状态、网络错误）触发回退尝试；
   校验类错误（验签失败、manifest 结构非法、checksum/size 不匹配、内容过大）
   **不**触发回退——校验失败意味着内容不可信，换通道无意义，直接返回。
6. 安全回归：manifest 经重写 URL 获取后仍执行完整验签（缺失/篡改签名拒绝）；
   artifact 经重写 URL 下载后仍执行 size + SHA-256 校验——两条通道共享同一
   校验路径，不因代理绕过任何校验。

## 边界与错误

- 每次检查/下载重新计算候选序列，不缓存通道状态。
- 加速前缀常量内置（`DEFAULT_ACCELERATE_PREFIX`），无运行时配置来源。
- `validate_https_url` 对每个候选 URL（含重写后）生效；重写后 URL 的
  `response.url()` 重定向校验同样要求 HTTPS（gh-proxy 直接转发内容，通常无
  重定向；即使有也必须是 HTTPS）。
- 错误信息分类与 UI 状态机（`Failed` 文案）沿用现有路径，不因通道变化改变。

## 接口与状态变更

- `crossh-update`：
  - `lib.rs` 新增导出常量 `DEFAULT_ACCELERATE_PREFIX`（`https://gh-proxy.com/`）；
  - `client.rs` 的 `fetch_manifest` 与 `download_artifact` **签名不变**，
    内部按候选序列尝试（先加速源后原站）；新增私有纯函数 `rewrite_url`、
    `candidate_urls` 与 `UpdateError::is_transport` 分类（可单测）。
- 调用方零改动：`src/features/updates/mod.rs`、updater、CLI 均无感知。
- 无设置字段、无 UI、无 i18n 变更。

## 平台影响

- URL 重写、候选序列、错误分类为纯逻辑，本机 macOS 全量测试覆盖。
- 真实网络行为（直连失败触发回退、gh-proxy 连通性）无法本地确定性验证，
  由真实用户环境与下一次发版观察确认。

## 涉及纪律

- [x] Logic must not depend on UI：重写/候选/分类逻辑全部在 `crossh-update`
  （纯逻辑 crate），无任何 UI 层改动。
- [x] Feature-owned settings：无新设置（用户裁决零配置）。
- [x] 图标纪律：不涉及新图标/组件/文案。
- [x] 文件规模 < 2000 行：改动集中在 `client.rs` 的小模块。
- [x] 工程笔记 / ADR 同步义务：无新结构性边界（传输层策略，不新建 ADR）；
  更新 `docs/remote-update-plan.md` 后续顺序与 `docs/testing.md` 行为矩阵。

## 影响模块

- `crates/crossh-update/src/client.rs`：候选序列、重写、回退尝试、错误分类。
- `crates/crossh-update/src/lib.rs`：`DEFAULT_ACCELERATE_PREFIX` 常量。
- `crates/crossh-update/src/model.rs`：无（`validate_https_url` 复用）。
- `docs/remote-update-plan.md`、`docs/testing.md`：文档同步。

## 验收清单

- [x] spec 评审通过（AI 评审 + 人批准）
- [x] 行为契约全部固化为失败测试并确认失败原因正确（Red），测试名
      `spec_20260818_update_accel_*`
- [x] 最小实现通过聚焦测试（Green）
- [x] `cargo fmt --check`
- [x] `scripts/check-architecture.sh`
- [x] `cargo clippy --all-targets -- -D warnings`
- [x] `cargo test --workspace`
- [x] 声明的平台 CI job 通过（本次 CI 三平台全绿，含 6 个 `spec_20260818_update_accel_*` 契约测试）
- [x] 结构性决策提炼进 ADR（如有）：无新边界
- [x] 调试根因合并进 `docs/engineering-notes/`（如有）：无
- [x] 新增行为合并进 `docs/testing.md` 关键行为矩阵（Update 行）
- [x] 用户可观察效果人工确认（待发布后实机观察）：更新检查与下载默认经加速通道完成；加速通道不可达时回退 GitHub 原站并正常完成（v0.16.4 已发布，下次检查更新时即可观察）

## AI 评审意见

逐项核查（2026-08-18，评审后按用户裁决更新）：

- 配置化删除：用户明确「这是默认行为，不要输入控件」——前缀常量内置、
  候选序列固定、无设置项/UI/i18n 变更，调用方零改动。目标 3 与接口小节
  已按此简化。
- 可测性：契约 1–3、5 全部可固化为纯函数测试（`rewrite_url` /
  `candidate_urls` / `UpdateError::is_transport`），契约 4 的网络循环是
  这些纯函数的确定性组合，契约 6 由既有验签/校验回归覆盖（两条通道共用
  `parse_manifest` 与 `DownloadVerifier`，无新校验分支可测）。
- 错误路径：明确「传输类错误触发回退、校验类错误直接返回」（内容不可信换
  通道无意义——安全关键语义）；「回退通道错误直接返回不循环」。
- 平台影响：真实网络行为（gh-proxy 连通性、回退触发）无法在本地或 Actions
  确定性验证（Actions 在境外直连畅通），声明为真实用户环境验证——已在验收
  清单标注。
- 纪律冲突：加速逻辑在 `crossh-update`（无 gpui）；无新结构性边界（传输
  策略，不新建 ADR）；`validate_https_url` 强制 HTTPS 不因通道变化放宽。
- 契约冲突：`CROSSH_UPDATE_MANIFEST_URL`（编译期覆盖点）与内置加速行为
  独立并存；manifest 验签协议（ADR 0014）不因通道变化而改变。若编译期
  manifest URL 指向非 github.com 域名，候选序列自动去重为直连（契约 3）。
