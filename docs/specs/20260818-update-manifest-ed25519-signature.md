# 更新清单 Ed25519 签名验证

## 元数据

- 状态：`done`（v0.16.4 发布成功，Release workflow 签名/自验通过；剩余篡改演示为可选人工确认）
- 创建：2026-08-18
- 相关 ADR：`docs/adr/0005-standalone-updater.md`；本次新增 `docs/adr/0014-update-manifest-signature.md`（更新协议签名信任模型）
- 相关 issue / 路线图项：`docs/remote-update-plan.md` 安全边界「下一阶段应给 stable.json 增加 Ed25519 签名，并把公钥固定在客户端」
- CI 平台影响：`全部`（发布流水线在 GitHub Actions 上签名并自验；签名验证为纯逻辑，本机 macOS 验证）

## 背景

当前更新协议中，SHA-256 只保证「下载到的 artifact 与 manifest 声明一致」，但 manifest 本身经 HTTPS 从网络拉取，其 URL、SHA-256 等字段决定了下载什么——攻击者或不可信的传输通道一旦改写 manifest（连同其中的 checksum 一起），客户端校验仍然通过。HTTPS 只证明链路加密与对端持有证书，不能证明 manifest 内容值得信任。`remote-update-plan.md` 已声明此边界，并规划下一步为 `stable.json` 增加 Ed25519 签名、把公钥固定在客户端。

签名落地后，manifest 可从任意传输通道获取（直连 GitHub、CDN 或未来的下载代理），信任锚从「服务器」迁移到「内置公钥」。下载代理集成将在此机制完成后另行立项。

## 目标

1. 客户端内置发布者公钥，对拉取的 manifest 验签；缺失签名或验签失败一律拒绝更新（fail-closed）。
2. 签名覆盖 manifest 的全部安全相关字段（版本号、每个 target 的 URL/文件名/格式/SHA-256/大小），任何字段被篡改都会验签失败。
3. 提供签名工具与发布流水线改动：生成密钥对、对 manifest 签名、发布前用内置公钥自验；私钥只存在于发布者本地与 GitHub Actions Secrets，绝不进入仓库与产物。
4. 旧客户端（无验签逻辑）仍能解析带签名的新 manifest 并正常更新，迁移只需一个 tag 即可完成。

## 非目标

- 不做下载加速源 / 代理集成（另行立项）。
- 不做 manifest 缓存（ETag/Last-Modified）、检查退避、离线状态（既有路线图后续项）。
- 不做 artifact 级签名：签名盖在整个 manifest 上，通过 manifest 内的 SHA-256 形成「公钥 → manifest → artifact」信任链。
- 不做密钥轮换 UI、密钥托管服务或硬件签名；不做 TUF/Sparkle 等完整框架替换。
- 不改变 manifest schema 号（保持 1）：`signature` 为可缺省字段，新客户端强制要求，旧客户端忽略。
- 不改变更新状态机与 UI 呈现（验签失败沿用现有 `Failed` 路径）。

## 行为契约

测试命名前缀：`spec_20260818_manifest_sig__`。

1. 当 manifest 携带由项目私钥生成的合法 Ed25519 签名（base64 编码 64 字节）时，`parse_manifest` 应成功返回完整 `UpdateManifest`，观察到 `Ok(manifest)` 且各字段与输入一致。
2. 当 manifest 携带合法签名但内容被篡改（version、notes、release_url，或任意 target 的 url/filename/format/sha256/size）时，`parse_manifest` 应返回签名验证错误，观察到 `Err(SignatureMismatch)`，拒绝更新。
3. 当 manifest 不携带 `signature` 字段时，`parse_manifest` 应拒绝，观察到 `Err(MissingSignature)`。
4. 当 `signature` 不是合法 base64、解码后长度不是 64 字节、或对应公钥无法构造时，`parse_manifest` 应拒绝。
5. 当 manifest 的原始字节表示不同（空白、键顺序、数字格式差异）但解析后的语义相同且签名合法时，`parse_manifest` 应通过——签名作用于语义（canonical 序列化）而非原始字节。
6. 当使用与签名私钥不匹配的公钥验证时（错误公钥、任意 32 字节、测试密钥），`parse_manifest` 应拒绝。
7. 当签名合法但 manifest 版本 ≤ 当前版本时，`candidate()` 应继续返回 `None`——签名不改变降级保护；对旧版本合法 manifest 的重放攻击仍被版本比较拒绝。
8. 旧客户端兼容：带 `signature` 字段的新 manifest 应能被不识别该字段的旧解析逻辑正常解析（serde 忽略未知字段），观察到解析成功且字段被忽略。
9. 签名工具 `crossh-sign-manifest`：
   - `generate` 应输出一对 Ed25519 密钥（base64 编码公钥与私钥 seed），公钥可验证私钥签名。
   - `sign` 应对合法 manifest JSON 产出带 `signature` 字段的 manifest；私钥应从参数或环境变量 `CROSSH_UPDATE_SIGNING_KEY` 读取；私钥缺失或非法时应失败且不产出半成品文件。
   - `verify` 应对合法签名返回成功，对篡改内容或缺失签名返回失败；默认使用客户端内置公钥。
10. 发布流水线：release job 生成 `stable.json` 后必须先用 Secrets 中的私钥签名，再用内置公钥自验；签名或自验失败应中止发布，不得产出未签名 manifest。
11. 客户端检查更新的整体行为：验签失败的 manifest 进入 `UpdateStatus::Failed` 且不触发下载（现有 UI 路径，无新文案）。
12. 内置默认公钥常量应可被 base64 解码且解码后长度为 32 字节（Ed25519 公钥长度），观察到常量解析成功——防开发期误写坏公钥导致全部更新被拒。

## 边界与错误

- 签名对象为「去掉 `signature` 字段后的 manifest」的确定性序列化：字段按结构体声明顺序、紧凑 JSON、`targets` 按键（BTreeMap）排序、`signature` 为 `None` 时省略。生成端与验证端共用同一序列化实现（同一 crate），消除双实现漂移。
- 验签使用 `verify_strict` 语义（拒绝弱签名等非规范输入），失败分类为签名格式非法（base64/长度/公钥）与验签失败两类，错误信息可区分。
- `signature` 字段对旧客户端是不可见字段（`#[serde(default)]`），对 schema 校验、artifact 校验、candidate 选择均无影响。
- 公钥固定：默认公钥为发布者实际生成的公钥，硬编码在 `crossh-update` 源码；`CROSSH_UPDATE_PUBLIC_KEY`（编译期，base64 编码 32 字节）可覆盖默认值，供测试与开发环境使用。公钥进仓库公开无风险。
- 私钥纪律：私钥（base64 编码 32 字节 seed）仅存放于发布者本地（加密备份）与 GitHub Actions Secrets（`CROSSH_UPDATE_SIGNING_KEY`）；CI 中经环境变量注入，不写入日志、命令输出或 release 产物；仓库内任何文件不得包含私钥。
- 密钥泄露处理：轮换密钥对并发布内置新公钥的新版本客户端（信任随新版本迁移），本 spec 不实现自动化轮换。
- 发布顺序：发布流水线签名功能与客户端验签逻辑同一版本上线。一个 tag 即完成迁移：旧客户端忽略 `signature` 字段照常更新；新客户端验证线上已签名 manifest。若签名步骤因 Secrets 缺失而失败，发布中止（fail-closed），用户停留在旧版——这是接受的安全取舍。

## 接口与状态变更

- Wire 格式：`UpdateManifest` 新增 `signature: Option<String>`（`#[serde(default)]`、`skip_serializing_if = "Option::is_none"`，base64 编码 64 字节 Ed25519 签名）；schema 保持 1。
- `ManifestError` 新增变体：`MissingSignature`、`InvalidSignature(String)`（区分格式非法与验签失败）。
- 新二进制：`crossh-sign-manifest`（随 `crossh-update` crate 发布，子命令 `generate` / `sign` / `verify`；`sign` 私钥来自参数或 `CROSSH_UPDATE_SIGNING_KEY` 环境变量，`verify` 默认用内置公钥）。
- `scripts/generate-update-manifest.sh` 输出不带签名的 manifest（职责不变）；签名与自验在 release.yml 的 release job 中执行。
- `.github/workflows/release.yml`：新增「Sign update manifest」（注入 `CROSSH_UPDATE_SIGNING_KEY`）与「Verify update manifest」步骤。
- 编译期覆盖：新增 `CROSSH_UPDATE_PUBLIC_KEY`（默认回退到内置正式公钥）。
- GitHub Secret：`CROSSH_UPDATE_SIGNING_KEY`（人工配置，见验收清单）。

## 平台影响

- 签名验证与签名工具为纯逻辑/CLI，本机 macOS arm64 可完整验证（`cargo test --workspace` 覆盖 crossh-update 全部签名测试）。
- release.yml 的签名与自验步骤运行于 GitHub Actions（ubuntu-latest release job，对所有平台产物生效）：由 `Release` workflow 验证，本地不运行。
- 无 Linux/Windows 专属代码路径；bash 脚本改动（若有）保持 POSIX 兼容。

## 涉及纪律

- [x] Logic must not depend on UI：签名验证与工具全部在 `crossh-update`（纯逻辑 crate，无 gpui），UI 只消费 `UpdateStatus::Failed` 结果。
- [x] Feature-owned settings：不新增设置项；公钥为编译期常量而非运行时设置（防被攻击者改写）。
- [x] 图标纪律：不涉及。
- [x] 文件规模 < 2000 行：签名逻辑独立模块，单文件远低于限制。
- [x] 工程笔记 / ADR 同步义务：新增 ADR 0014 记录签名协议信任模型（canonical 对象、fail-closed、密钥职责、发布顺序）；更新 `docs/remote-update-plan.md` 安全边界与验收清单；`docs/testing.md` 行为矩阵 Update 行补充签名契约。
- [x] 响应式 UI：不涉及。

## 影响模块

- `crates/crossh-update/src/model.rs`：`UpdateManifest.signature`、新错误变体、签名验证入口。
- `crates/crossh-update/src/signature.rs`（新）：canonical 序列化、验签逻辑、公钥常量。
- `crates/crossh-update/src/bin/crossh-sign-manifest.rs`（新）：`generate` / `sign` / `verify` CLI。
- `crates/crossh-update/src/lib.rs`：导出签名工具函数与错误。
- `crates/crossh-update/Cargo.toml`：新增 `ed25519-dalek = "3"`（已在依赖树，russh 传递依赖）、`base64`、`clap`（如需要）。
- `.github/workflows/release.yml`：签名与自验步骤。
- `docs/remote-update-plan.md`、`docs/testing.md`、`docs/adr/0014-*.md`（新）、`docs/architecture.md`：文档同步。
- `src/features/updates/`：无改动（错误路径已覆盖）。

## 验收清单

- [x] spec 评审通过（AI 评审 + 人批准）
- [x] 行为契约全部固化为失败测试并确认失败原因正确（Red）：首批 5 个测试失败原因正确（占位公钥 + 缺失签名检查顺序 + 公钥构造假设）；另发现 `preserve_order` 构建图差异（记入 engineering notes）
- [x] 最小实现通过聚焦测试（Green）：27 个单测 + 7 个 CLI 集成测试全绿
- [x] `cargo fmt --check`
- [x] `scripts/check-architecture.sh`
- [x] `cargo clippy --all-targets -- -D warnings`（workspace 全量通过）
- [x] `cargo test --workspace`（全量通过，无回归）
- [x] 声明的平台 CI job 通过：`Release` workflow 的签名 + 自验步骤（v0.16.4 发布成功，`Sign update manifest` 用 Secrets 私钥签名、`Verify` 用内置公钥自验均通过）
- [x] 结构性决策提炼进 ADR 0014 并登记 `docs/architecture.md`
- [x] 调试根因合并进 `docs/engineering-notes/`（serde_json preserve_order 构建图差异）
- [x] 新增行为合并进 `docs/testing.md` 关键行为矩阵（Update 行）
- [x] 人工步骤完成：用户执行 `crossh-sign-manifest generate`，公钥已内置进源码（DEFAULT_PUBLIC_KEY = `2ruoNty5NOSLRAHeHqchPsXYnCjZ9vfUfyUBZT/kHQs=`）；私钥 age 加密备份（`~/.age/crossh-update-signing-key.age`）与 GitHub Secrets `CROSSH_UPDATE_SIGNING_KEY` 均已配置
- [x] 用户可观察效果人工确认（发布链路部分）：v0.16.4 已发布，线上 stable.json 带 Ed25519 签名；客户端内置公钥验签逻辑由 27 个单测 + 7 个 CLI 集成测试覆盖，Actions 签名/自验双步骤通过。剩余可选演示：手动篡改 stable.json 后检查更新应显示失败且不触发下载（用户实机操作确认）

## AI 评审意见

逐项核查（2026-08-18）：

- 可测性：契约 1–8 全部可固化为纯函数测试（`parse_manifest` / `candidate` 无 IO）；契约 9 要求 generate/sign/verify 的核心逻辑为库函数（可单测）、CLI 为薄壳——已并入契约 9 与影响模块（`signature.rs` 提供库函数，`crossh-sign-manifest.rs` 只做参数解析与调用）。契约 10（CI 签名+自验）无法本地执行，由 `Release` workflow 本身验证，与本机验证互为补充（自验逻辑本身 = 契约 9 的 verify 库函数）。
- 错误路径：覆盖缺失签名、非法 base64/长度、篡改各字段、错误公钥、私钥缺失/非法、半成品文件防护。补充一条防呆：默认内置公钥常量必须可解码且为 32 字节（防开发期误写坏公钥），作为契约 12。签名对象序列化一致性由契约 5 覆盖（空白/键顺序变体）。
- 非目标：下载代理、缓存/退避、artifact 级签名、密钥轮换 UI、TUF/Sparkle 均明确排除；schema 保持 1 的兼容策略与「一个 tag 完成迁移」的发布顺序经推演成立（旧客户端忽略未知字段；线上 manifest 先于/同时带签名；签名步骤失败则发布中止，fail-closed）。
- 平台影响：纯逻辑 + GitHub Actions release job；本地 macOS 可完整跑 workspace 测试；bash 保持 POSIX 兼容。
- 纪律冲突：签名逻辑全部在纯逻辑 crate（无 gpui）；公钥为编译期常量而非运行时设置（防运行时改写），不新增设置项；`ed25519-dalek = "3"` 与 `base64 = "0.22"` 均在现有依赖树（russh / crossh-core 传递依赖），不引入新下载；clap 不在依赖树，CLI 手写薄解析。
- 契约冲突：与 `remote-update-plan.md` 安全边界、ADR 0005（updater 进程职责不变，签名验证在主进程 fetch 阶段）一致；`candidate()` 降级保护已有测试，契约 7 补充「签名 manifest 重放仍被拒」回归。
- 验收可观察：末两项人工验收覆盖真实发版链路与篡改演示；密钥配置为人工步骤（generate → 公钥进源码 → 私钥备份 + Secrets）。
- 待人工确认：① 发布顺序为「签名功能与验签逻辑同版本上线，一个 tag 完成迁移」——若签名步骤故障，发布中止、用户停在旧版（fail-closed 取舍）；② 私钥管理职责（本地加密备份 + GitHub Secrets `CROSSH_UPDATE_SIGNING_KEY`）由用户承担，公钥随源码公开；③ 密钥泄露时通过发布内置新公钥的新版本完成轮换，本 spec 不实现自动化。
