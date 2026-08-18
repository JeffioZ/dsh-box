# 开发模式一键构建：不嵌入 UI 资源，运行时从 ui/ 目录直接读取。
# 之后只改 ui/ 下的文件（HTML/CSS/JS/图标），重启 dist-dev\DSHBox-dev.exe 即可看到效果，
# 无需重新编译。改 Rust 代码仍需重新执行本脚本。
[CmdletBinding()]
param()

$ErrorActionPreference = "Stop"

$root = $PSScriptRoot
& (Join-Path $root "scripts\cargo.ps1") dev
if ($LASTEXITCODE -ne 0) {
    throw "开发模式构建失败（退出码 $LASTEXITCODE）"
}
$distDev = Join-Path $root "dist-dev"
New-Item -ItemType Directory -Path $distDev -Force | Out-Null
Copy-Item -LiteralPath (Join-Path $root "src-tauri\target\release\DSHBox.exe") -Destination (Join-Path $distDev "DSHBox-dev.exe") -Force
Write-Host "开发版已就绪：dist-dev\DSHBox-dev.exe（UI 资源运行时从 ui\ 目录读取）" -ForegroundColor Green
