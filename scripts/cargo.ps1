# 构建 DSHBox（自动加载 MSVC 环境，工具链从 PATH/环境变量解析）。
# 用法: pwsh -File scripts/cargo.ps1 [check|test|dev|build]
param(
  [ValidateSet("check", "test", "dev", "build")]
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

# 3) 配置前置校验：withGlobalTauri 必须为 true——它控制 window.__TAURI__
#    注入（tauri v2 AppConfig 字段，默认 false）。缺失/为 false 时所有页面
#    IPC 断链（标题栏/托盘菜单整页失效且无任何报错），曾误删引发回归。
#    以 tauri-utils 源码字段语义为准，在此构建期兜底拦截。
$confPath = Join-Path $PSScriptRoot "..\src-tauri\tauri.conf.json"
$conf = Get-Content -LiteralPath $confPath -Raw | ConvertFrom-Json
if (-not $conf.app.withGlobalTauri) {
  throw "tauri.conf.json 的 app.withGlobalTauri 必须为 true（window.__TAURI__ 注入开关），请勿删除或置 false。"
}

# 4) 执行构建
# 同一会话先跑过 dev 模式时，残留的 TAURI_CONFIG（devUrl）会让后续 check/test/build
# 静默按"不嵌入 UI 资源"的 dev 配置执行——正式构建被无声污染，先显式清除。
if ($Mode -ne "dev") {
  Remove-Item Env:\TAURI_CONFIG -ErrorAction SilentlyContinue
}
$manifest = Join-Path $PSScriptRoot "..\src-tauri\Cargo.toml"
if ($Mode -eq "check") {
  & $cargo check --locked --manifest-path $manifest
} elseif ($Mode -eq "test") {
  & $cargo test --locked --manifest-path $manifest --all-targets
} elseif ($Mode -eq "dev") {
  # 开发模式：注入 devUrl 使 tauri 不嵌入 UI 资源（运行时从 ui/ 目录直接读取），
  # 不递增版本号。使用独立 target/dev，避免与正式版互相触发重编译或争抢 exe。
  # 之后只改 ui/ 文件，重启开发版 exe 即生效。
  $env:TAURI_CONFIG = '{"build":{"devUrl":"http://localhost:4321"}}'
  $devTarget = Join-Path $PSScriptRoot "..\src-tauri\target\dev"
  & $cargo build --locked --release --manifest-path $manifest --target-dir $devTarget
} else {
  # 构建本身不修改版本号；发布前如需升版，请显式运行 bump-version.ps1。
  & $cargo build --locked --release --manifest-path $manifest
}
exit $LASTEXITCODE
