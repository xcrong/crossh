# 打包脚本与 Cargo.toml 的隐式耦合失效

## 症状

- Release workflow 的两个 Windows job 编译成功后挂在 Build & package：`Cannot index into a null array`（`package-windows.ps1` 的 `Select-String ... .Matches.Groups[1]`），产物缺失导致 GitHub Release job 跳过。
- macOS/Linux 照常变绿，掩盖了问题。

## 根因

- `package-windows.ps1` / `copy-shared-assets.sh` 用正则从 Cargo.toml 的 `assets = { git = ..., rev = ... }` 行提取 Zed rev，再据此定位 `~/.cargo/git/checkouts/zed-*/` 里的字体。依赖重构（`f4d935f` 移除该行、字体 vendor 进 `crossh-assets`）后：
  - PowerShell 侧：`Select-String` 无匹配 → 返回 null → 索引 `.Matches` 直接崩（`$ErrorActionPreference = "Stop"`）。
  - bash 侧：`awk` 无匹配 → `ZED_REV` 为空 → `ZED_PREFIX` 为空 → 前缀匹配退化为"匹配一切"，撞上 runner 缓存里 gpui 的 zed checkout（仓库根恰有 `assets/`）侥幸通过。换环境即坏，且拷贝来源不受控。

## 稳定规则

1. 打包脚本不要用正则从 Cargo.toml 提取依赖信息（rev、路径等）；这类隐式耦合在依赖重构时必然失联。需要依赖元数据时用 `cargo metadata --format-version 1` 结构化读取。
2. 资源 vendor 进仓库后，打包脚本直接从仓库路径拷贝，删除对 git checkout 缓存目录的查找。
3. 警惕"匹配不到"的静默降级：空字符串前缀匹配等于匹配一切；PowerShell 管道空结果后接 `.Matches` 崩溃。脚本应对"未找到"显式报错，而不是带着空值继续。
4. 脚本与 Cargo.toml 的耦合点在依赖行增删时必须同步审查：`rg 'Select-String|awk.*Cargo.toml' scripts/`。

## 验证方法

- `bash -n scripts/copy-shared-assets.sh`；PowerShell 解析检查见 [Windows 打包 zip 的反斜杠条目](windows-zip-backslash-separators.md)。
- 打新 tag 观察 Release workflow build 矩阵全绿（Windows 行为由 GitHub Actions 验证）。

## 当前实现

- `scripts/package-windows.ps1`、`scripts/copy-shared-assets.sh`：字体改从 `crates/crossh-assets/assets/fonts/` 拷贝，不再读 Cargo.toml。

## 搜索关键词

`package-windows.ps1`, `copy-shared-assets.sh`, `Select-String`, `Cannot index into a null array`, `Zed rev`, `assets =`, `awk 空匹配`, `空前缀匹配`, `vendor 字体`, `Cargo.toml 正则`, `release 打包失败`
