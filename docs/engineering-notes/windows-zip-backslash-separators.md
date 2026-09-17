# Windows 打包 zip 的反斜杠条目与自更新失败

## 症状

- Windows 设置页「更新」显示：`更新失败：update archive contains an unsafe path: resources\crossh-assets\fonts\`。
- zip 已下载完、manifest 验签与 SHA-256 校验都通过，失败发生在解压第一步；应用重启后仍是旧版本。

## 根因

两层叠加才致命：

1. zip 规范要求条目名用 `/` 分隔，但 Windows 上的打包工具会写 `\`：Windows PowerShell 5.1 的 `Compress-Archive` 和 .NET `ZipFile.CreateFromDirectory` 都产出反斜杠，只有 PowerShell 7 的 `Compress-Archive` 是 `/`。`.github/workflows/release.yml` 用 `powershell -NoProfile -File ...`（5.1）调 `scripts/package-windows.ps1`，于是发布出来的 zip 条目全是反斜杠。
2. `extract_zip` 当时把任何含 `\` 的条目名直接判为 `UnsafeArchivePath`，合法的 Windows 产物因此被当成路径穿越拒绝。

还有一个更隐蔽的坑：`zip::read::ZipFile::is_dir()` 判定的是 `name().ends_with('/')`。反斜杠目录条目（`resources\crossh-assets\`）会被判成普通文件并被 `File::create` 成一个同名文件，之后该目录下的条目写入必然失败。

## 稳定规则

1. 解压先归一化再校验：`entry.name().replace('\\', "/")` 之后才做 `is_safe_relative_path`（要求全 `Normal` 组件）与目录判定。`..\..\evil` 归一化后是 `../../evil`，照样被拒，防护不降级。
2. 打包侧不依赖宿主 PowerShell 版本：手工写 `ZipArchive` 条目并统一 `/`，不要换回 `Compress-Archive`。
3. PowerShell 脚本里用 .NET API（`ZipFile::Open`、`[System.IO.File]::` 等）时，相对路径按**进程工作目录**解析，而不是 PowerShell 的 location；脚本内一律先 `Resolve-Path` 成绝对路径。
4. 含中文注释的 `.ps1` 必须存为 UTF-8 with BOM：Windows PowerShell 5.1 按 ANSI 代码页读取无 BOM 文件，中文机器（GBK）上解码出的字节会破坏 token，`powershell -File` 直接解析失败。CI 的 windows-latest 是 CP1252，碰巧不炸，本地中文机器会炸。
5. Windows 允许 rename 正在运行的 exe（自更新可以借此替换自身），但不允许删除它；被替换下来的旧 exe 备份只能等该进程退出后再删。

## 验证方法

- `cargo test -p crossh-update`：`backslash_zip_entries_extract_into_staging`、`backslash_zip_traversal_is_still_rejected`、`windows_zip_payload_replaces_every_top_level_entry`、`windows_zip_payload_without_main_binary_is_rejected`。
- 打包产物：跑 `scripts/package-windows.ps1` 后用 `[System.IO.Compression.ZipFile]::OpenRead` 列条目名，确认没有 `\`。
- 解析检查：`[System.Management.Automation.Language.Parser]::ParseFile('<ps1>', [ref]$null, [ref]$errors)`，PS 5.1 与 PS 7 都应为 0 错误。
- 真机验证替换：装 zip 版本后跑一次自更新，确认 `resources/` 与各附属 exe 一起变成新版本。

## 当前实现

- 解压归一化与载荷替换：`crates/crossh-update/src/installer.rs`（`extract_zip`、`replace_payload_directory`、`staging_path`）
- 打包：`scripts/package-windows.ps1`
- 发布入口：`.github/workflows/release.yml` 的 Windows 任务

## 搜索关键词

`反斜杠`, `backslash`, `Compress-Archive`, `ZipArchiveMode`, `unsafe path`, `update archive contains an unsafe path`, `zip 分隔符`, `PS 5.1`, `UTF-8 BOM`, `语法错误`, `ParseFile`, `rename 运行中的 exe`, `replace_payload_directory`, `resources 未更新`