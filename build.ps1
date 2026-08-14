# 一键构建 DSHDesktop release，并复制到 dist/DSHDesktop.exe（复制逻辑复用 copy-exe.mjs）。
[CmdletBinding()]
param()

$ErrorActionPreference = "Stop"

$root = $PSScriptRoot
$cargoScript = Join-Path $root "scripts\cargo.ps1"
$copyScript = Join-Path $root "scripts\copy-exe.mjs"

Write-Host "正在构建 DSHDesktop release..." -ForegroundColor Cyan
& $cargoScript build
if ($LASTEXITCODE -ne 0) {
    throw "Rust release 构建失败（退出码 $LASTEXITCODE）"
}

Write-Host "正在复制产物到 dist/..." -ForegroundColor Cyan
node $copyScript
if ($LASTEXITCODE -ne 0) {
    throw "复制产物失败（退出码 $LASTEXITCODE）"
}
Write-Host "完成。"
