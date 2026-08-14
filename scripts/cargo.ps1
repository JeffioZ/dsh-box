# 构建 DSHDesktop（自动加载 MSVC 环境，工具链从 PATH/环境变量解析）。
# 用法: powershell -File scripts/cargo.ps1 [check|test|build]
param(
  [string]$Mode = "build"
)
$ErrorActionPreference = "Stop"

# 1) 定位 cargo：优先 CARGO_HOME，其次标准 PATH
$cargo = $null
if ($env:CARGO_HOME -and (Test-Path (Join-Path $env:CARGO_HOME "bin\cargo.exe"))) {
  $cargo = Join-Path $env:CARGO_HOME "bin\cargo.exe"
}
if (-not $cargo) {
  $cargo = (Get-Command cargo -ErrorAction SilentlyContinue).Source
}
if (-not $cargo) {
  throw "未找到 cargo：请安装 Rust（https://rustup.rs）并确保 cargo 在 PATH 中。"
}

# 2) 定位 MSVC 环境：优先 vswhere 精确查找（任意 VS 版本/版本类型），
#    常见固定路径仅作回退；都找不到则依赖已配置的环境。
$vcvars = $null
$vswhere = Join-Path ${env:ProgramFiles(x86)} "Microsoft Visual Studio\Installer\vswhere.exe"
if (Test-Path $vswhere) {
  $vsPath = & $vswhere -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath 2>$null | Select-Object -First 1
  if ($vsPath) {
    $cand = Join-Path $vsPath.Trim() "VC\Auxiliary\Build\vcvars64.bat"
    if (Test-Path $cand) { $vcvars = $cand }
  }
}
if (-not $vcvars) {
  foreach ($cand in @(
    "C:\Program Files\Microsoft Visual Studio\2022\Community\VC\Auxiliary\Build\vcvars64.bat",
    "C:\Program Files\Microsoft Visual Studio\2022\Professional\VC\Auxiliary\Build\vcvars64.bat",
    "C:\Program Files\Microsoft Visual Studio\2022\Enterprise\VC\Auxiliary\Build\vcvars64.bat",
    "C:\Program Files (x86)\Microsoft Visual Studio\2022\BuildTools\VC\Auxiliary\Build\vcvars64.bat"
  )) {
    if (Test-Path $cand) { $vcvars = $cand; break }
  }
}
if ($vcvars) {
  # chcp 65001 让 set 输出按 UTF-8 解析，避免中文用户名/路径在 GBK 代码页下乱码
  $dump = cmd /c "chcp 65001 >nul && `"$vcvars`" >nul 2>&1 && set"
  foreach ($line in $dump) {
    $i = $line.IndexOf('=')
    if ($i -gt 0) { [Environment]::SetEnvironmentVariable($line.Substring(0, $i), $line.Substring($i + 1)) }
  }
} elseif (-not (Get-Command cl.exe -ErrorAction SilentlyContinue)) {
  Write-Warning "未找到 MSVC 环境（vcvars64.bat）；若链接失败请安装 VS C++ 工具链或先手动加载环境"
}

# 3) 执行构建
$manifest = Join-Path $PSScriptRoot "..\src-tauri\Cargo.toml"
if ($Mode -eq "check") {
  & $cargo check --manifest-path $manifest
} elseif ($Mode -eq "test") {
  & $cargo test --manifest-path $manifest --lib
} elseif ($Mode -eq "dev") {
  # 开发模式：注入 devUrl 使 tauri 不嵌入 UI 资源（运行时从 ui/ 目录直接读取），
  # 不递增版本号。之后只改 ui/ 文件，重启开发版 exe 即生效。
  $env:TAURI_CONFIG = '{"build":{"devUrl":"http://localhost:4321"}}'
  & $cargo build --release --manifest-path $manifest
} else {
  # release 构建前自动递增版本号（patch +1），同步 Cargo.toml/tauri.conf.json/package.json
  & (Join-Path $PSScriptRoot "bump-version.ps1")
  if ($LASTEXITCODE -ne 0) { throw "版本号递增失败" }
  & $cargo build --release --manifest-path $manifest
}
exit $LASTEXITCODE
