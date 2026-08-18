# 0014-update-manifest-signature

## 状态

已接受

## 背景

更新 manifest（stable.json）经 HTTPS 从网络拉取，决定「从哪下载、下载的东西对不对」。SHA-256 只保证下载内容与 manifest 声明一致，不能防止 manifest 本身被改写：攻击者或不可信的传输通道一旦篡改 manifest（连同其中的 checksum），客户端校验依然通过。HTTPS 只证明链路加密与对端持有证书，不证明内容值得信任。`docs/remote-update-plan.md` 的安全边界已声明此缺口，规划下一阶段为 manifest 增加 Ed25519 签名并把公钥固定在客户端。

## 决策

- **签名对象**：manifest 去掉 `signature` 字段后的 canonical 序列化（字段按结构体声明顺序的紧凑 JSON，`targets` 为 BTreeMap 按键排序）。生成端与验证端共用 `crossh-update` 的同一序列化实现，消除双实现漂移；语义相同（空白、键顺序不同）的字节表示验证通过，任何字段值改动验证失败。
- **fail-closed**：新客户端强制要求签名——缺失签名（`MissingSignature`）、格式非法或验签失败（`InvalidSignature`）一律拒绝更新，即使结构校验通过。
- **信任锚固定**：公钥编译期内置于 `crossh-update`（`DEFAULT_PUBLIC_KEY`），`CROSSH_UPDATE_PUBLIC_KEY` 编译期覆盖仅用于测试/开发。公钥非运行时设置，防攻击者运行时改写。
- **兼容策略**：`signature` 为可选字段（`#[serde(default)]`），schema 保持 1；旧客户端（无验签逻辑）忽略该字段照常更新，新客户端要求签名——一个 tag 即完成迁移，无需两阶段发布。
- **密钥职责**：私钥（32 字节 seed 的 base64）只存在于发布者本地（加密备份）与 GitHub Actions Secrets（`CROSSH_UPDATE_SIGNING_KEY`），经环境变量注入 CI，不写入日志、仓库或产物。
- **发布防护**：release job 生成 manifest 后必须签名并用客户端同一固定公钥自验，签名或自验失败即中止发布，绝不产出未签名 manifest。
- **签名工具**：`crossh-sign-manifest`（generate / sign / verify）随 `crossh-update` 发布；`verify` 无公钥参数时走与客户端完全相同的解析路径（结构校验 + 固定公钥验签）。
- **降级保护不变**：签名覆盖版本号，重放旧版本合法 manifest 仍被 `candidate()` 的版本比较拒绝。

## 结果/代价

信任锚从「传输通道」迁移到「内置公钥」，manifest 未来可经任何 HTTPS 通道分发（CDN、下载代理）而不降低安全性；代价是发布链依赖私钥的正确配置（Secrets 缺失则发布中止，用户停留在旧版——接受的 fail-closed 取舍），且密钥泄露需要通过发布内置新公钥的新版本完成轮换。

## 关联规则

- `docs/remote-update-plan.md` 安全边界（Ed25519 签名已实现）
- `AGENTS.md` 的 Logic must not depend on UI（签名逻辑全部在纯逻辑 crate）
- `docs/architecture.md` 的 crate ownership（`crossh-update` 无 gpui）
- `crates/crossh-update/src/signature.rs`
- `crates/crossh-update/src/bin/crossh-sign-manifest.rs`
- `.github/workflows/release.yml`（签名 + 自验步骤）
