# 每次 release 构建前自动递增补丁号（最后一位 +1），并同步所有使用版本号的位置。
# 同步：Cargo.toml、tauri.conf.json、package.json、package-lock.json。
# Windows 版本资源的字符串版本（FileVersion/ProductVersion）由 tauri-build
# 强制使用 tauri.conf.json 的 3 段 semver，此处不另设。
[CmdletBinding()]
param()

$ErrorActionPreference = "Stop"

$root = $PSScriptRoot
$cargoToml = Join-Path $root "..\src-tauri\Cargo.toml"
$confJson  = Join-Path $root "..\src-tauri\tauri.conf.json"
$pkgJson   = Join-Path $root "..\package.json"
$lockJson  = Join-Path $root "..\package-lock.json"

function Write-Utf8NoBom([string]$path, [string]$text) {
  [System.IO.File]::WriteAllText($path, $text, [System.Text.UTF8Encoding]::new($false))
}

# 1) 以 Cargo.toml 为准读取当前版本
$cargoText = [System.IO.File]::ReadAllText($cargoToml)
$m = [regex]::Match($cargoText, '(?m)^version\s*=\s*"(\d+)\.(\d+)\.(\d+)"')
if (-not $m.Success) { throw "未在 Cargo.toml 找到 version = `"x.y.z`"" }
$oldVer = "{0}.{1}.{2}" -f $m.Groups[1].Value, $m.Groups[2].Value, $m.Groups[3].Value
$newVer = "{0}.{1}.{2}" -f $m.Groups[1].Value, $m.Groups[2].Value, ([int]$m.Groups[3].Value + 1)
Write-Host "版本号：$oldVer -> $newVer"

# 2) Cargo.toml：package.version（仅此一处，行首锚定避免误伤依赖）
$cargoText = [regex]::Replace($cargoText, '(?m)^(version\s*=\s*")[\d.]+(")', "`${1}$newVer`${2}", 1)
Write-Utf8NoBom $cargoToml $cargoText

# 3) tauri.conf.json / package.json / package-lock.json（仅替换等于当前版本的行，
#    且每个文件最多 2 处——lock 根条目固定在最前，避免误伤同版本号的第三方依赖）
foreach ($p in @($confJson, $pkgJson, $lockJson)) {
  if (-not (Test-Path -LiteralPath $p)) { continue }
  $t = [System.IO.File]::ReadAllText($p)
  $pattern = '(?m)^(\s*"version"\s*:\s*")' + [regex]::Escape($oldVer) + '(")'
  $t = [regex]::Replace($t, $pattern, "`${1}$newVer`${2}", 2)
  Write-Utf8NoBom $p $t
}

Write-Host "已同步到 Cargo.toml / tauri.conf.json / package.json / package-lock.json"
