# 发布前显式运行：递增补丁号（最后一位 +1），并同步所有使用版本号的位置。
# 同步：Cargo.toml、tauri.conf.json、package.json、package-lock.json、
# src-tauri/Cargo.lock（本包条目，--locked 构建依赖其与 Cargo.toml 一致）。
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
$cargoLock = Join-Path $root "..\src-tauri\Cargo.lock"

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

# 前置校验：三处版本号必须一致。bump 以 Cargo.toml 为准，若他处已被手动改过，
# Replace 只替换等于 oldVer 的行、不一致的行被静默跳过，会导致版本更不一致。
foreach ($p in @($confJson, $pkgJson, $lockJson)) {
  if (-not (Test-Path -LiteralPath $p)) { continue }
  $t = [System.IO.File]::ReadAllText($p)
  # 用 [^"]+ 而非 [\d.]+：后者会截断 0.1.20-beta.1 这类带后缀版本，恰好放过不一致。
  $vm = [regex]::Match($t, '"version"\s*:\s*"([^"]+)"')
  if ($vm.Success -and $vm.Groups[1].Value -ne $oldVer) {
    throw "版本号不一致：$p 为 $($vm.Groups[1].Value)，Cargo.toml 为 $oldVer；请先人工统一后再 bump。"
  }
  if ($p -eq $lockJson) {
    # package-lock 顶层 version 与 packages[""] 的 version 是前两个版本字段；
    # 两者都必须一致。只检查第一个会放过根包条目漂移，npm ci 仍可继续而把
    # 错误版本带入锁文件元数据。
    $lockVersions = [regex]::Matches($t, '(?m)^\s*"version"\s*:\s*"([^"]+)"')
    if ($lockVersions.Count -lt 2 -or
        $lockVersions[0].Groups[1].Value -ne $oldVer -or
        $lockVersions[1].Groups[1].Value -ne $oldVer) {
      throw "版本号不一致：package-lock.json 顶层与根包 version 必须都等于 $oldVer"
    }
  }
}

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

# 4) src-tauri/Cargo.lock：仅本包条目（name = "dsh-box" 后紧跟的 version 行），
#    避免误伤其他恰好同版本号的依赖
if (Test-Path -LiteralPath $cargoLock) {
  $t = [System.IO.File]::ReadAllText($cargoLock)
  $pattern = '(?m)(name = "dsh-box"\r?\nversion = ")' + [regex]::Escape($oldVer) + '(")'
  if (-not [regex]::IsMatch($t, $pattern)) {
    throw "Cargo.lock 中未找到 dsh-box 的 $oldVer 条目，请人工核对"
  }
  $t = [regex]::Replace($t, $pattern, "`${1}$newVer`${2}", 1)
  Write-Utf8NoBom $cargoLock $t
}

Write-Host "已同步到 Cargo.toml / tauri.conf.json / package.json / package-lock.json / Cargo.lock"
