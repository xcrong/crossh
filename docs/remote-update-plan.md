# Crossh 远程更新计划

## 目标

Crossh 客户端通过 GitHub Releases 检查新版本、下载对应平台产物，并在用户确认后由独立 updater 完成替换。主进程不覆盖正在运行的自身文件。

macOS 暂时没有 Developer ID，因此当前发布未签名 `.app`。更新流程不依赖 Developer ID 或 Sparkle；后续获得证书后，可以在不改变清单协议的情况下增加原生签名校验。

## 已实施的第一阶段

- 发布 workflow 生成 `dist/stable.json`，包含版本、平台、产物格式、文件名、下载 URL、大小和 SHA-256，并用发布者私钥做 Ed25519 签名后发布；客户端用内置公钥验签，缺失或无效签名一律拒绝。
- manifest 与 artifact 请求默认经加速前缀 `https://gh-proxy.com/` 转发（对国内用户更友好），加速通道不可达时自动回退 GitHub 原站；两条通道共享同一验签/校验路径，安全边界不降级（见 `docs/specs/20260818-update-gh-proxy-acceleration.md`）。
- 客户端只接受 HTTPS 清单和 HTTPS 产物 URL。
- 版本比较使用 semver，忽略相同版本和降级版本。
- 清单大小限制为 1 MiB，单个下载限制为 1 GiB。
- 下载采用临时文件，流式计算 SHA-256；校验失败不会留下可安装文件。
- 设置页新增 Updates：启动时检查、手动检查、下载、打开发布页、重启安装。
- `crossh-updater` 随 macOS `.app`、Linux tar/AppImage 和 Windows zip 发布。
- updater 等待主进程退出后再替换，并在替换失败时尝试恢复备份。
- zip 使用安全路径提取；tar.gz 只接受安全相对路径和普通文件/目录。

## 运行状态

```text
Idle
  -> Checking
  -> UpToDate
  -> Available
  -> Downloading
  -> Ready
  -> updater hand-off
```

任何网络、清单、下载、校验或安装错误都会进入 `Failed`，不会覆盖当前版本。

## 发布协议

清单的稳定地址是：

```text
https://github.com/xcrong/crossh/releases/latest/download/stable.json
```

当前支持的目标键：

| Target | Preferred artifact |
| --- | --- |
| `macos-aarch64` | `.zip` containing `crossh.app` |
| `macos-x86_64` | `.zip` containing `crossh.app` |
| `linux-aarch64` | `.tar.gz` + `.AppImage`（AppImage 优先） |
| `linux-x86_64` | `.tar.gz` + `.AppImage`（AppImage 优先） |
| `windows-x86_64` | `.zip` containing `crossh.exe` |
| `windows-aarch64` | optional experimental `.zip` |

> Linux 两产物由 `scripts/package-linux.sh` 同时产出（`dist/*.tar.gz` 与同前缀 `*.AppImage`），manifest 校验两者；表格的“优先”指更新时 AppImage 为首选。

`CROSSH_UPDATE_MANIFEST_URL` 是编译期覆盖点，便于测试环境或未来迁移到 CDN。

## 平台安装策略

### macOS

updater 找到当前 `.app` 根目录，复制新 bundle 到同一目录下的临时目录，替换旧 bundle，然后使用 `open` 重新启动。当前包未签名，用户首次下载仍可能需要在系统安全提示中手动允许。

### Linux

AppImage 通过 `APPIMAGE` 环境变量定位当前文件，直接替换 AppImage。tar.gz 安装方式替换当前目录中的 `crossh` 二进制。包内同时放置 `crossh-updater`。

### Windows

updater 在主进程退出后替换 `crossh.exe`。Windows zip 中包含 `crossh-updater.exe`。当前 updater 不会覆盖正在运行的 updater 自身，因此下一次更新继续使用旧 updater 也是允许的；后续可以增加独立固定版本 bootstrapper。

## 安全边界

当前实现提供：

- HTTPS 传输（加速前缀 `https://gh-proxy.com/` 优先，不可达时回退 GitHub 原站）；
- manifest 的 Ed25519 签名验证（公钥固定在客户端，`CROSSH_UPDATE_PUBLIC_KEY` 编译期覆盖仅用于测试）；
- artifact URL、文件名、格式、大小和 SHA-256 校验；
- semver 降级保护；
- archive 路径穿越防护；
- 临时文件和备份替换。

签名覆盖整个 manifest（版本号、URL、checksum 等全部字段），缺失或验签失败即拒绝更新；发布流水线签名后用内置公钥自验，失败则中止发布。签名属于更新协议安全，不等同于 macOS Developer ID 代码签名。

## 后续实施顺序

1. 增加 ETag/Last-Modified 缓存、检查退避和离线状态。
2. 增加下载进度、取消下载和缓存清理。
3. 在真实发布目录验证 macOS `.app` 替换、Linux tar.gz/AppImage 替换和 Windows exe 替换。
4. 为 updater 增加安装失败日志和明确的回滚标记。
5. 获得 Developer ID 后再启用 macOS notarization/Sparkle，不改变当前清单字段。

> 已完成（v0.16.4）：manifest Ed25519 签名与 gh-proxy 加速前缀（`https://gh-proxy.com/` 优先 + GitHub 回退，见 `docs/specs/20260818-update-manifest-ed25519-signature.md` / `20260818-update-gh-proxy-acceleration.md`）已在“已实施的第一阶段”落地；本节剩余项为后续路线图。

## 验收清单

- [x] 打 tag 后所有必需产物和 `stable.json`（已签名）出现在同一个 GitHub Release。（v0.16.4 已验证：release workflow 签名后自验，见 `docs/specs/20260818-update-manifest-ed25519-signature.md`）
- [x] 清单中每个必需 target 的文件名、大小和 SHA-256 与 Release 文件一致。
- [x] 当前版本检查到旧版本时不显示可更新状态。
- [x] 篡改清单 JSON、URL、文件名、格式、大小或 SHA-256 时拒绝更新；篡改签名或缺失签名同样拒绝。
- [x] 下载中断或 hash 不一致时不替换安装目录。
- [ ] 用户取消退出或安装器启动失败时保留当前版本。
- [ ] 更新后应用重新启动，设置不受影响。
- [x] macOS 没有 Developer ID 时仍可完成下载、校验和 bundle 替换。

> 已勾选项由 `cargo test -p crossh-update`（34 项，含签名/篡改/回退）与发布自验覆盖；未勾选项为手工发布验证项。
