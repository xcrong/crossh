# 打包 Windows 产物：crossh.exe、crossh-git.exe、crossh-note.exe、crossh-updater.exe + README + LICENSE 的 zip。
#
# 用法:  powershell -File scripts/package-windows.ps1 [-Target <triple>] [-Version <ver>]
# 输出:  dist\crossh-<version>-windows-<arch>.zip
param(
    [string]$Target = "x86_64-pc-windows-msvc",
    [string]$Version = ""
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
$ZedRevision = (Select-String -Path Cargo.toml -Pattern '^assets = .*rev = "([^"]+)"' |
    Select-Object -First 1).Matches.Groups[1].Value
# cargo 的 git 依赖 checkout 目录名是 short ID（7 位起，歧义时更长），
# 而不是完整 rev（见 copy-shared-assets.sh 同款前缀匹配），所以这里
# 用前 7 位前缀匹配。
$ZedPrefix = $ZedRevision.Substring(0, [Math]::Min(7, $ZedRevision.Length))
# 兼容多 Cargo Home：优先 $CARGO_HOME，其次 $USERPROFILE/.cargo，兼容 scoop/localappdata 等
$CargoHomes = @()
if ($env:CARGO_HOME -and $env:CARGO_HOME.Trim() -ne "") { $CargoHomes += $env:CARGO_HOME }
$CargoHomes += Join-Path $env:USERPROFILE ".cargo"
if ($env:LOCALAPPDATA) { $CargoHomes += Join-Path $env:LOCALAPPDATA "cargo" }
if ($env:APPDATA) { $CargoHomes += Join-Path $env:APPDATA "cargo" }
$CargoHomes = $CargoHomes | Select-Object -Unique

$ZedRoot = $null
foreach ($CargoHome in $CargoHomes) {
    $CheckoutsDir = Join-Path $CargoHome "git/checkouts"
    if (-not (Test-Path $CheckoutsDir)) { continue }
    $candidate = Get-ChildItem $CheckoutsDir -Directory -Filter "zed-*" -ErrorAction SilentlyContinue |
        ForEach-Object { Get-ChildItem $_.FullName -Directory -ErrorAction SilentlyContinue } |
        Where-Object { $_.Name.StartsWith($ZedPrefix) } |
        Where-Object { Test-Path (Join-Path $_.FullName "assets") } |
        Select-Object -First 1
    if ($candidate) { $ZedRoot = $candidate; break }
}
# 回退：用 cargo metadata 直接定位 zed 的 manifest 路径（最可靠，不依赖目录约定）
if (-not $ZedRoot) {
    try {
        $metaFull = cargo metadata --format-version 1 2>$null | ConvertFrom-Json
        $zedPkg = $metaFull.packages | Where-Object { $_.name -eq "assets" -and $_.source -like "git+*zed*" } | Select-Object -First 1
        if ($zedPkg -and $zedPkg.manifest_path) {
            $maybeRoot = Split-Path (Split-Path $zedPkg.manifest_path)
            if (Test-Path (Join-Path $maybeRoot "assets")) {
                $ZedRoot = Get-Item $maybeRoot
            }
        }
    } catch { }
}
if (-not $ZedRoot) {
    Write-Host "cached Zed checkouts (searched CargoHomes: $($CargoHomes -join ', ')):"
    foreach ($CargoHome in $CargoHomes) {
        $CheckoutsDir = Join-Path $CargoHome "git/checkouts"
        if (Test-Path $CheckoutsDir) {
            Get-ChildItem $CheckoutsDir -Directory -Filter "zed-*" -ErrorAction SilentlyContinue |
                ForEach-Object { Get-ChildItem $_.FullName -Directory -ErrorAction SilentlyContinue } |
                ForEach-Object { Write-Host "  $($_.FullName)" }
        } else {
            Write-Host "  (missing) $CheckoutsDir"
        }
    }
    throw "unable to locate cached Zed assets for revision $ZedRevision (prefix $ZedPrefix). Tried CargoHomes: $($CargoHomes -join ', ')"
}
$ZedRoot = $ZedRoot.FullName
Copy-Item "crates/crossh-assets/assets/icons/*.svg" (Join-Path $AssetDestination "icons")
Copy-Item "assets/appicon/icon-master.svg" (Join-Path $AssetDestination "brand/crossh-logo.svg")
Copy-Item (Join-Path $ZedRoot "assets/fonts/ibm-plex-sans/IBMPlexSans-Regular.ttf") (Join-Path $AssetDestination "fonts/ibm-plex-sans")
Copy-Item (Join-Path $ZedRoot "assets/fonts/ibm-plex-sans/IBMPlexSans-SemiBold.ttf") (Join-Path $AssetDestination "fonts/ibm-plex-sans")
Copy-Item (Join-Path $ZedRoot "assets/fonts/lilex/Lilex-Regular.ttf") (Join-Path $AssetDestination "fonts/lilex")
Copy-Item (Join-Path $ZedRoot "assets/fonts/lilex/Lilex-Bold.ttf") (Join-Path $AssetDestination "fonts/lilex")
Set-Content -Path (Join-Path $AssetDestination "manifest.json") -Value (ConvertTo-Json @{ schema = 1; zed_revision = $ZedRevision } -Compress)
Copy-Item (Join-Path $BinDir "crossh.exe") $Stage
Copy-Item (Join-Path $BinDir "crossh-git.exe") $Stage
Copy-Item (Join-Path $BinDir "crossh-note.exe") $Stage
Copy-Item (Join-Path $BinDir "crossh-updater.exe") $Stage
Copy-Item "README.md" $Stage
Copy-Item "LICENSE" $Stage

$Zip = Join-Path "dist" "crossh-$Version-windows-$Arch.zip"
if (Test-Path $Zip) { Remove-Item $Zip }
Compress-Archive -Path (Join-Path $Stage "*") -DestinationPath $Zip

Write-Host "==> done:"
Write-Host "    $Zip"
