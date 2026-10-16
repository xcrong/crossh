# 打包 Windows 产物：crossh.exe、crossh-git.exe、crossh-note.exe、crossh-updater.exe + README + LICENSE 的 zip，
# 以及同内容的 Inno Setup 安装程序 setup.exe（有 ISCC 时构建，缺失则只出 zip）。
#
# 用法:  powershell -File scripts/package-windows.ps1 [-Target <triple>] [-Version <ver>] [-SkipInstaller]
# 输出:  dist\crossh-<version>-windows-<arch>.zip
#         dist\crossh-<version>-windows-<arch>-setup.exe（需 Inno Setup 6）
param(
    [string]$Target = "x86_64-pc-windows-msvc",
    [string]$Version = "",
    [switch]$SkipInstaller
)

$ErrorActionPreference = "Stop"
Set-Location (Join-Path $PSScriptRoot "..")

if (-not $Version) {
    $Version = (Select-String -Path Cargo.toml -Pattern '^version\s*=\s*"(.*)"' |
        Select-Object -First 1).Matches.Groups[1].Value
}

$Arch = if ($Target -like "x86_64-*") { "x86_64" }
        elseif ($Target -like "aarch64-*") { "aarch64" }
        else { throw "unsupported target: $Target" }

Write-Host "==> rustup target add $Target"
rustup target add $Target | Out-Null

Write-Host "==> cargo build --release --target $Target"
cargo build --release --target $Target --bin crossh --bin crossh-git --bin crossh-note --bin crossh-updater
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

$BinDir = Join-Path "target" (Join-Path $Target "release")
$Stage = Join-Path "dist" "crossh-$Version-windows-$Arch"
New-Item -ItemType Directory -Force -Path $Stage | Out-Null
$AssetDestination = Join-Path $Stage "resources/crossh-assets"
New-Item -ItemType Directory -Force -Path (Join-Path $AssetDestination "brand"),
    (Join-Path $AssetDestination "fonts/ibm-plex-sans"),
    (Join-Path $AssetDestination "fonts/lilex"),
    (Join-Path $AssetDestination "icons") | Out-Null
# 字体已 vendor 进 crossh-assets（不再依赖 Zed assets crate），直接从仓库拷贝。
Copy-Item "crates/crossh-assets/assets/icons/*.svg" (Join-Path $AssetDestination "icons")
Copy-Item "assets/appicon/icon-master.svg" (Join-Path $AssetDestination "brand/crossh-logo.svg")
Copy-Item "crates/crossh-assets/assets/fonts/ibm-plex-sans/IBMPlexSans-Regular.ttf" (Join-Path $AssetDestination "fonts/ibm-plex-sans")
Copy-Item "crates/crossh-assets/assets/fonts/ibm-plex-sans/IBMPlexSans-SemiBold.ttf" (Join-Path $AssetDestination "fonts/ibm-plex-sans")
Copy-Item "crates/crossh-assets/assets/fonts/lilex/Lilex-Regular.ttf" (Join-Path $AssetDestination "fonts/lilex")
Copy-Item "crates/crossh-assets/assets/fonts/lilex/Lilex-Bold.ttf" (Join-Path $AssetDestination "fonts/lilex")
Copy-Item (Join-Path $BinDir "crossh.exe") $Stage
Copy-Item (Join-Path $BinDir "crossh-git.exe") $Stage
Copy-Item (Join-Path $BinDir "crossh-note.exe") $Stage
Copy-Item (Join-Path $BinDir "crossh-updater.exe") $Stage
Copy-Item "README.md" $Stage
Copy-Item "LICENSE" $Stage

$Zip = Join-Path (Resolve-Path "dist").Path "crossh-$Version-windows-$Arch.zip"
if (Test-Path $Zip) { Remove-Item $Zip }
# zip 规范要求条目分隔符是 '/'，但 Windows PowerShell 5.1 的 Compress-Archive
# 会写 '\'（.NET ZipFile::CreateFromDirectory 同样如此），而客户端解压会把
# 反斜杠条目当不安全路径拒绝（见 crates/crossh-update/src/installer.rs）。
# 这里手工写条目并统一分隔符，PS 5.1 与 PS 7 产出保持一致。
# 注意：.NET 的 ZipFile 按进程工作目录解析相对路径（PowerShell cmdlet 才按
# location），所以 $Zip / $Stage 都先解析成绝对路径再交给它。
Add-Type -AssemblyName System.IO.Compression -ErrorAction SilentlyContinue
Add-Type -AssemblyName System.IO.Compression.FileSystem -ErrorAction SilentlyContinue
$StageFull = (Resolve-Path $Stage).Path
$Archive = [System.IO.Compression.ZipFile]::Open($Zip, [System.IO.Compression.ZipArchiveMode]::Create)
try {
    Get-ChildItem -Path $StageFull -Recurse -File | ForEach-Object {
        $EntryName = $_.FullName.Substring($StageFull.Length + 1).Replace('\', '/')
        [System.IO.Compression.ZipFileExtensions]::CreateEntryFromFile(
            $Archive, $_.FullName, $EntryName,
            [System.IO.Compression.CompressionLevel]::Optimal) | Out-Null
    }
} finally {
    $Archive.Dispose()
}

Write-Host "==> zip done:"
Write-Host "    $Zip"

# --- Windows 安装程序（Inno Setup 6）-------------------------------------------
# 与上面的 zip 共用 $Stage 暂存目录，保证 setup.exe 与便携 zip 内容一致。
# 默认 per-user 安装到 %LOCALAPPDATA%\Programs\crossh（见 scripts/crossh.iss），
# 免 UAC，自更新继续原地替换 exe，无需提权。未签名：SmartScreen 仍会提示。
if (-not $SkipInstaller) {
    # ISCC 查找顺序：PATH → where.exe → 默认安装目录（含 per-user）→ 卸载注册表。
    # winget 可能装到当前用户作用域（HKCU / %LOCALAPPDATA%），且 InstallLocation
    # 值不一定存在，因此同时从 UninstallString 所在目录反推。
    $Iscc = $null
    $IsccSearched = @()
    try { $Iscc = (Get-Command iscc.exe -ErrorAction Stop).Source } catch { }
    if (-not $Iscc) {
        try { $w = where.exe ISCC.exe 2>$null | Select-Object -First 1 } catch { $w = $null }
        if ($w -and (Test-Path $w)) { $Iscc = $w }
    }
    if (-not $Iscc) {
        $IsccCandidates = @()
        $Pf86 = ${env:ProgramFiles(x86)}
        if ($Pf86) { $IsccCandidates += Join-Path $Pf86 "Inno Setup 6\ISCC.exe" }
        if ($env:ProgramFiles) { $IsccCandidates += Join-Path $env:ProgramFiles "Inno Setup 6\ISCC.exe" }
        if ($env:LOCALAPPDATA) { $IsccCandidates += Join-Path $env:LOCALAPPDATA "Programs\Inno Setup 6\ISCC.exe" }
        foreach ($RegKey in @(
            "HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\Inno Setup 6_is1",
            "HKLM:\SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall\Inno Setup 6_is1",
            "HKCU:\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\Inno Setup 6_is1"
        )) {
            try {
                $Props = Get-ItemProperty -Path $RegKey -ErrorAction Stop
                if ($Props.InstallLocation) { $IsccCandidates += Join-Path $Props.InstallLocation "ISCC.exe" }
                if ($Props.UninstallString) {
                    $Unins = $Props.UninstallString.Trim()
                    if ($Unins.StartsWith('"')) {
                        $Unins = $Unins.Substring(1)
                        $q = $Unins.IndexOf('"')
                        if ($q -ge 0) { $Unins = $Unins.Substring(0, $q) }
                    } else {
                        $Unins = ($Unins -split '\s+')[0]
                    }
                    $UninsDir = Split-Path $Unins -Parent
                    if ($UninsDir) { $IsccCandidates += Join-Path $UninsDir "ISCC.exe" }
                }
            } catch { }
        }
        $IsccSearched = $IsccCandidates | Select-Object -Unique
        foreach ($Candidate in $IsccCandidates) {
            if ($Candidate -and (Test-Path $Candidate)) { $Iscc = $Candidate; break }
        }
    }
    if (-not $Iscc) {
        Write-Warning "ISCC.exe not found; skipping Windows installer (zip only). Install Inno Setup 6 to build setup.exe."
        if ($IsccSearched.Count -gt 0) {
            Write-Warning ("searched: " + ($IsccSearched -join "; "))
        }
    } else {
        Write-Host "==> building Windows installer with $Iscc"
        $StageFull = (Resolve-Path $Stage).Path
        $DistFull = (Resolve-Path "dist").Path
        $SetupBase = "crossh-$Version-windows-$Arch-setup"
        & $Iscc "/DMyAppVersion=$Version" "/DMyArch=$Arch" "/DMySrcDir=$StageFull" "/DMyOutputDir=$DistFull" "/DMySetupBase=$SetupBase" (Join-Path $PSScriptRoot "crossh.iss")
        if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
        Write-Host "    dist\$SetupBase.exe"
    }
}
