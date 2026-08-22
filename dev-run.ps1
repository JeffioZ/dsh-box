# 开发模式运行：确保 UI 静态服务器在跑（node 进程），然后启动开发版 exe。
# 之后只改 ui/ 下的文件：重启本脚本（或刷新页面）即生效，无需重新编译。
[CmdletBinding()]
param()

$ErrorActionPreference = "Stop"

$root = $PSScriptRoot
$exe = Join-Path $root "dist-dev\DSHBox-dev.exe"
if (-not (Test-Path -LiteralPath $exe)) {
    throw "未找到开发版 exe：$exe（请先运行 dev-build.ps1 构建一次）"
}

function Test-DshdUiServer {
    try {
        $response = Invoke-WebRequest -Uri "http://127.0.0.1:4321/" -TimeoutSec 2 -UseBasicParsing
        return $response.StatusCode -eq 200 -and
            $response.Content.Contains("<title>DSHBox</title>")
    } catch {
        return $false
    }
}

# 确保静态服务器在跑（探测 4321 端口；未监听则后台启动，最多重试 10 次）
$listening = Get-NetTCPConnection -LocalAddress 127.0.0.1 -LocalPort 4321 -State Listen -ErrorAction SilentlyContinue
if ($listening -and -not (Test-DshdUiServer)) {
    throw "端口 4321 已被其他服务占用，请关闭该服务后重试"
}
if (-not $listening) {
    Write-Host "启动 UI 静态服务器（node scripts\serve-ui.mjs）..." -ForegroundColor Cyan
    # ArgumentList 对含空格的路径需手动加引号，否则参数被拆断
    $serverArg = '"' + (Join-Path $root "scripts\serve-ui.mjs") + '"'
    Start-Process -FilePath "node" -ArgumentList $serverArg -WindowStyle Hidden
    $ok = $false
    for ($i = 0; $i -lt 10; $i++) {
        Start-Sleep -Milliseconds 500
        if (Test-DshdUiServer) {
            $ok = $true
            break
        }
    }
    if (-not $ok) {
        throw "UI 静态服务器启动失败（端口 4321 未监听）；可手动运行 node scripts\serve-ui.mjs 查看报错"
    }
}

Write-Host "启动开发版：$exe" -ForegroundColor Cyan
Start-Process -FilePath $exe
