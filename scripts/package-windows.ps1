# 打包 Windows 产物：crossh.exe + README + LICENSE 的 zip。
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
cargo build --release --target $Target
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

$BinDir = Join-Path "target" (Join-Path $Target "release")
$Stage = Join-Path "dist" "crossh-$Version-windows-$Arch"
New-Item -ItemType Directory -Force -Path $Stage | Out-Null
Copy-Item (Join-Path $BinDir "crossh.exe") $Stage
Copy-Item "README.md" $Stage
Copy-Item "LICENSE" $Stage

$Zip = Join-Path "dist" "crossh-$Version-windows-$Arch.zip"
if (Test-Path $Zip) { Remove-Item $Zip }
Compress-Archive -Path (Join-Path $Stage "*") -DestinationPath $Zip

Write-Host "==> done:"
Write-Host "    $Zip"
