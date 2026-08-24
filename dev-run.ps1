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

# 后台 UI 服务器的 PID 记录：脚本退出后服务器仍常驻，重复启动时据此提示停止方式
$pidFile = Join-Path $env:TEMP "dshbox-dev-ui-server.pid"

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
    $server = Start-Process -FilePath "node" -ArgumentList $serverArg -WindowStyle Hidden -PassThru
    Set-Content -LiteralPath $pidFile -Value $server.Id -Encoding ascii
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

# 提示如何停止常驻 UI 服务器：PID 以实时端口查询为准并刷新记录，
# 覆盖脚本启动与手动启动两种来源（重复运行时同样可见）
$serverPid = Get-NetTCPConnection -LocalAddress 127.0.0.1 -LocalPort 4321 -State Listen -ErrorAction SilentlyContinue |
    Select-Object -First 1 -ExpandProperty OwningProcess
if ($serverPid) {
    Set-Content -LiteralPath $pidFile -Value $serverPid -Encoding ascii
    Write-Host "UI 静态服务器常驻中（PID $serverPid，已记录到 $pidFile）；如需停止：Stop-Process -Id $serverPid" -ForegroundColor DarkGray
}

Write-Host "启动开发版：$exe" -ForegroundColor Cyan
Start-Process -FilePath $exe
