//! DSHBox Windows 单文件更新与预下载。

#[cfg(windows)]
use super::check::check_app_update;
use super::*;
#[cfg(windows)]
use std::io::Read;

// ---------- 应用自身更新（Windows：下载 → 替换 → 重启） ----------

#[cfg(windows)]
struct AppReleaseAsset {
    version: String,
    url: String,
    sha256: String,
}

/// 从精确 tag 的 GitHub Release 元数据中取 Windows 资产 URL 与平台生成的摘要。
/// 版本查询仍走 Atom；只有确有更新并准备下载时才调用 API，避免日常检查消耗限额。
#[cfg(windows)]
fn fetch_app_release_asset(version: &str) -> Result<AppReleaseAsset, String> {
    let tag = format!("v{version}");
    let url = format!("https://api.github.com/repos/{APP_REPO}/releases/tags/{tag}");
    let response = runtime::check_client()
        .get(&url)
        .header("Accept", "application/vnd.github+json")
        .header("X-GitHub-Api-Version", "2026-03-10")
        .header("User-Agent", "DSHBox")
        .call()
        .map_err(|e| {
            crate::locale::error(
                "读取 Release 元数据失败",
                "Failed to fetch release metadata",
                e,
            )
        })?;
    let mut text = String::new();
    response
        .into_body()
        .into_reader()
        .take(1024 * 1024)
        .read_to_string(&mut text)
        .map_err(|e| {
            crate::locale::error(
                "读取 Release 元数据失败",
                "Failed to read release metadata",
                e,
            )
        })?;
    let json: serde_json::Value = serde_json::from_str(&text).map_err(|e| {
        crate::locale::error(
            "解析 Release 元数据失败",
            "Failed to parse release metadata",
            e,
        )
    })?;
    let (asset_url, sha256) = parse_app_release_asset(&json, version)?;
    Ok(AppReleaseAsset {
        version: version.to_string(),
        url: asset_url,
        sha256,
    })
}

#[cfg(any(windows, test))]
pub(super) fn parse_app_release_asset(
    json: &serde_json::Value,
    expected_version: &str,
) -> Result<(String, String), String> {
    let expected_tag = format!("v{expected_version}");
    if json.get("tag_name").and_then(|v| v.as_str()) != Some(expected_tag.as_str())
        || json.get("draft").and_then(|v| v.as_bool()).unwrap_or(true)
        || json
            .get("prerelease")
            .and_then(|v| v.as_bool())
            .unwrap_or(true)
    {
        return Err(crate::locale::text(
            "Release 版本或发布状态与检查结果不一致。",
            "The release version or publication state does not match the update check.",
        )
        .into());
    }
    let matching_assets = json
        .get("assets")
        .and_then(|v| v.as_array())
        .map(|assets| {
            assets
                .iter()
                .filter(|asset| {
                    asset.get("name").and_then(|v| v.as_str()) == Some(APP_WINDOWS_ASSET)
                        && asset.get("state").and_then(|v| v.as_str()) == Some("uploaded")
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    if matching_assets.len() != 1 {
        return Err(crate::locale::text(
            "Release 必须恰好包含一个可用的 Windows x64 程序。",
            "The release must contain exactly one uploaded Windows x64 executable.",
        )
        .into());
    }
    let asset = matching_assets.into_iter().next().ok_or_else(|| {
        crate::locale::text(
            "Release 中没有可用的 Windows x64 程序。",
            "The release does not contain an uploaded Windows x64 executable.",
        )
        .to_string()
    })?;
    let asset_url = asset
        .get("browser_download_url")
        .and_then(|v| v.as_str())
        .ok_or_else(|| {
            crate::locale::text(
                "Release 资产没有下载地址。",
                "The release asset has no download URL.",
            )
            .to_string()
        })?;
    let parsed = url::Url::parse(asset_url).map_err(|_| {
        crate::locale::text(
            "Release 资产下载地址无效。",
            "The release asset download URL is invalid.",
        )
        .to_string()
    })?;
    let expected_path = format!("/{APP_REPO}/releases/download/{expected_tag}/{APP_WINDOWS_ASSET}");
    if parsed.scheme() != "https"
        || parsed.host_str() != Some("github.com")
        || parsed.path() != expected_path
        || parsed.query().is_some()
        || parsed.fragment().is_some()
    {
        return Err(crate::locale::text(
            "Release 返回了非预期的下载地址。",
            "The release returned an unexpected download URL.",
        )
        .into());
    }
    let sha256 = asset
        .get("digest")
        .and_then(|v| v.as_str())
        .and_then(|v| v.strip_prefix("sha256:"))
        .filter(|v| v.len() == 64 && v.bytes().all(|b| b.is_ascii_hexdigit()))
        .ok_or_else(|| {
            crate::locale::text(
                "Release 资产没有有效的 SHA-256 摘要，请从发布页手动更新。",
                "The release asset has no valid SHA-256 digest. Update manually from the release page.",
            )
            .to_string()
        })?;
    Ok((asset_url.to_string(), sha256.to_ascii_lowercase()))
}

/// 更新应用本体 exe：已预下载则直接应用，否则先下载再应用。
/// 仅 Windows 支持（单文件分发场景）；macOS/Linux 提示从官网下载。
pub(super) fn update_app_exe(
    app: &AppHandle,
    config: &crate::app_state::Config,
) -> Result<(), String> {
    #[cfg(not(windows))]
    {
        let _ = (app, config);
        Err(crate::locale::text(
            "当前平台请从官网下载新版安装包。",
            "Please download the new version from the official website on this platform.",
        )
        .into())
    }
    #[cfg(windows)]
    {
        let dir = config.root.join("exe-update");
        std::fs::create_dir_all(&dir).map_err(|e| {
            crate::locale::error("创建更新目录失败", "Failed to create the update folder", e)
        })?;
        let target = dir.join("DSHBox.exe");
        let info = check_app_update()
            .filter(|info| info.update_available)
            .ok_or_else(|| {
                crate::locale::text(
                    "暂时无法确认可用的应用更新。",
                    "Could not confirm an app update right now.",
                )
                .to_string()
            })?;
        let ready = app.state::<AppState>().app_update_ready();
        let expected = if let Some((version, sha256)) = ready {
            if version == info.latest && verify_downloaded_exe(&target, &sha256).is_ok() {
                AppReleaseAsset {
                    version,
                    url: String::new(),
                    sha256,
                }
            } else {
                app.state::<AppState>().set_app_update_ready(None);
                fetch_app_release_asset(&info.latest)?
            }
        } else {
            fetch_app_release_asset(&info.latest)?
        };
        if verify_downloaded_exe(&target, &expected.sha256).is_err() {
            let _ = std::fs::remove_file(&target);
            download_app_exe(app, &target, &expected, true)?;
        }
        app.state::<AppState>()
            .set_app_update_ready(Some((expected.version.clone(), expected.sha256.clone())));
        apply_downloaded_exe(app, &target, &expected)
    }
}

/// 预下载文件是否可应用：存在 + 体积下限 + MZ 头 + GitHub 资产 SHA-256。
#[cfg(windows)]
fn verify_downloaded_exe(target: &std::path::Path, expected_sha256: &str) -> Result<(), String> {
    let Ok(meta) = target.metadata() else {
        return Err(crate::locale::text(
            "已下载的程序文件不存在。",
            "The downloaded executable is missing.",
        )
        .into());
    };
    if meta.len() < 1024 * 1024 {
        return Err(crate::locale::text(
            "已下载的程序文件过小，不是有效的更新包。",
            "The downloaded executable is too small to be a valid update.",
        )
        .into());
    }
    let mut file = std::fs::File::open(target).map_err(|e| e.to_string())?;
    let mut mz = [0u8; 2];
    file.read_exact(&mut mz).map_err(|e| e.to_string())?;
    if &mz != b"MZ" {
        return Err(crate::locale::text(
            "已下载的程序文件缺少 MZ 头。",
            "The downloaded executable has no MZ header.",
        )
        .into());
    }
    drop(file);
    let actual = runtime::sha256_file(target).map_err(|e| e.to_string())?;
    if actual != expected_sha256.to_ascii_lowercase() {
        return Err(crate::locale::text(
            "应用更新包 SHA-256 校验失败。",
            "The app update failed SHA-256 verification.",
        )
        .into());
    }
    Ok(())
}

/// 下载并完整校验应用更新包；失败时清理半截文件。
/// `report`：交互式更新为 true（进度上报检查更新弹窗）；后台静默预下载
/// 为 false——不写 check_progress，避免用户未点更新时弹窗出现/残留
/// 下载进度文案。
#[cfg(windows)]
fn download_app_exe(
    app: &AppHandle,
    target: &std::path::Path,
    release: &AppReleaseAsset,
    report: bool,
) -> Result<(), String> {
    static DOWNLOAD_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _guard = DOWNLOAD_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    if verify_downloaded_exe(target, &release.sha256).is_ok() {
        return Ok(());
    }
    let result = download_app_exe_inner(app, target, release, report);
    // 下载结束（含预下载成功待确认）：任务栏进度使命完成
    crate::progress::clear(app);
    if result.is_err() {
        let _ = std::fs::remove_file(target);
        let _ = std::fs::remove_file(target.with_extension("exe.part"));
    }
    result
}

/// 下载并校验应用更新包（精确 tag URL + .part 落盘 + 大小上限 + SHA-256）。
#[cfg(windows)]
fn download_app_exe_inner(
    app: &AppHandle,
    target: &std::path::Path,
    release: &AppReleaseAsset,
    report: bool,
) -> Result<(), String> {
    if report {
        emit_progress(
            app,
            crate::locale::text("正在下载应用更新…", "Downloading the app update…"),
        );
    }
    // 下载百分比与 MB 进度：检查更新弹窗的进度行实时承接（节流由
    // stream_to_file 统一实施，百分比变化 + 200ms）
    let version = release.version.as_str();
    let on_progress = |done: u64, total: u64| {
        // 任务栏进度不分交互/后台静默预下载：后台场景用户往往已切离
        // 应用，任务栏指示恰是唯一可见渠道
        crate::progress::set(app, done, total);
        if !report {
            return;
        }
        let pct = ((done as f64 / total as f64 * 100.0) as i64).min(100);
        emit_progress(
            app,
            &crate::locale::owned(
                format!(
                    "正在下载应用更新 {version}… {pct}%（{:.1}/{:.1} MB）",
                    done as f64 / 1048576.0,
                    total as f64 / 1048576.0
                ),
                format!(
                    "Downloading the app update {version}… {pct}% ({:.1}/{:.1} MB)",
                    done as f64 / 1048576.0,
                    total as f64 / 1048576.0
                ),
            ),
        );
    };
    // 单文件 exe 上限 512MB：防止异常响应/恶意源写满磁盘。
    // 下载本体（分块/上限/失败清理）复用 runtime::download::stream_to_file。
    const MAX_APP_EXE_BYTES: u64 = 512 * 1024 * 1024;
    let part = target.with_extension("exe.part");
    let _ = std::fs::remove_file(&part);
    runtime::stream_to_file(runtime::StreamRequest {
        url: &release.url,
        path: &part,
        max_bytes: MAX_APP_EXE_BYTES,
        user_agent: Some("DSHBox"),
        progress: Some(&on_progress),
        cancelled: None,
    })
    .map_err(|error| match error {
        runtime::DownloadError::Transport(e) => {
            crate::locale::error("下载失败", "Download failed", &e)
        }
        runtime::DownloadError::Body(e) => {
            crate::locale::error("下载中断", "Download interrupted", &e)
        }
        runtime::DownloadError::Local(e) => {
            crate::locale::error("写入失败", "Failed to write the update", &e)
        }
        runtime::DownloadError::Limit => crate::locale::text(
            "下载内容超出预期大小，已取消更新。",
            "The downloaded content exceeds the expected size. Update cancelled.",
        )
        .into(),
        // 应用更新未接取消回调，此分支不可达；保守映射为中断
        runtime::DownloadError::Cancelled => {
            crate::locale::text("下载中断", "Download interrupted").into()
        }
    })?;
    verify_downloaded_exe(&part, &release.sha256)?;
    if target.exists() {
        std::fs::remove_file(target).map_err(|e| {
            crate::locale::error(
                "清理旧更新包失败",
                "Failed to remove the previous update package",
                e,
            )
        })?;
    }
    std::fs::rename(&part, target).map_err(|e| {
        crate::locale::error("提交更新包失败", "Failed to finalize the update package", e)
    })?;
    Ok(())
}

#[cfg(any(windows, test))]
pub(super) fn windows_replace_script(
    source: &std::path::Path,
    destination: &std::path::Path,
    expected_sha256: &str,
) -> String {
    let ps_quote = |path: &std::path::Path| path.to_string_lossy().replace('\'', "''");
    format!(
        "$ErrorActionPreference = 'Stop'\n\
         Start-Sleep -Seconds 2\n\
         $src = '{}'\n\
         $dst = '{}'\n\
         $new = $dst + '.new'\n\
         $old = $dst + '.old'\n\
         $expected = '{}'\n\
         $replaced = $false\n\
         try {{\n\
           if ((-not (Test-Path -LiteralPath $dst)) -and (Test-Path -LiteralPath $old)) {{ Move-Item -LiteralPath $old -Destination $dst -Force }}\n\
           Copy-Item -LiteralPath $src -Destination $new -Force\n\
           $actual = (Get-FileHash -Algorithm SHA256 -LiteralPath $new).Hash.ToLowerInvariant()\n\
           if ($actual -ne $expected) {{ throw 'staged executable digest mismatch' }}\n\
           $i = 0\n\
           while ($i -lt 60 -and -not $replaced) {{\n\
             try {{\n\
               if (Test-Path -LiteralPath $old) {{ Remove-Item -LiteralPath $old -Force }}\n\
               if (Test-Path -LiteralPath $dst) {{ [System.IO.File]::Replace($new, $dst, $old, $true) }} else {{ Move-Item -LiteralPath $new -Destination $dst -Force }}\n\
               $replaced = $true\n\
             }} catch {{ Start-Sleep -Milliseconds 500; $i++ }}\n\
           }}\n\
           if (-not $replaced) {{ throw 'unable to replace executable' }}\n\
           $process = Start-Process -FilePath $dst -PassThru\n\
           Start-Sleep -Seconds 3\n\
           if (-not $process.HasExited -and (Test-Path -LiteralPath $old)) {{ Remove-Item -LiteralPath $old -Force }}\n\
         }} catch {{\n\
           if ((-not (Test-Path -LiteralPath $dst)) -and (Test-Path -LiteralPath $old)) {{ Copy-Item -LiteralPath $old -Destination $dst -Force }}\n\
           # 失败留痕：下次启动由 cleanup 读取并转发（此前静默 exit 1，只能靠版本不匹配间接推断）\n\
           $_ | Out-File -LiteralPath (Join-Path (Split-Path -Parent $src) 'replace-error.log') -Encoding utf8\n\
           exit 1\n\
         }} finally {{\n\
           if (Test-Path -LiteralPath $new) {{ Remove-Item -LiteralPath $new -Force }}\n\
         }}\n",
        ps_quote(source),
        ps_quote(destination),
        expected_sha256.to_ascii_lowercase(),
    )
}

/// 应用已下载的更新包：校验 → 写替换脚本 → 退出（脚本替换并重启新版本）。
/// 确认（退出并重启）由调用链上游的自绘弹窗完成。
#[cfg(windows)]
fn apply_downloaded_exe(
    app: &AppHandle,
    target: &std::path::Path,
    release: &AppReleaseAsset,
) -> Result<(), String> {
    // 1) 应用前再次校验，覆盖“预下载完成后文件被替换”的窗口。
    verify_downloaded_exe(target, &release.sha256)?;
    // 退出并重启的确认已前移到自绘弹窗（更新提示/检查更新页确认弹窗），
    // 此处不再弹原生 msgbox 二次打扰。

    // 2) 写替换脚本。新版先复制到当前 exe 同目录并复验摘要，再通过
    // File.Replace 原子替换；断电发生在提交前时旧 exe 始终保持可启动。
    let exe = std::env::current_exe().map_err(|e| {
        crate::locale::error(
            "无法定位当前程序路径",
            "Failed to locate the current executable",
            e,
        )
    })?;
    let dir = target.parent().unwrap_or_else(|| std::path::Path::new("."));
    let script = dir.join("replace.ps1");
    let script_text = windows_replace_script(target, &exe, &release.sha256);
    // 写入 BOM：Windows PowerShell 5.1 对无 BOM 的 .ps1 按 ANSI 解码，
    // 脚本内含中文路径时会解码错乱导致替换失败
    std::fs::write(&script, format!("\u{FEFF}{script_text}")).map_err(|e| {
        crate::locale::error(
            "写入替换脚本失败",
            "Failed to write the replacement script",
            e,
        )
    })?;

    // 3) 启动替换脚本（隐藏、独立于本进程），保存窗口状态后退出
    let mut replace_cmd = std::process::Command::new("powershell");
    replace_cmd
        .args([
            "-NoProfile",
            "-WindowStyle",
            "Hidden",
            "-ExecutionPolicy",
            "Bypass",
            "-File",
        ])
        .arg(&script);
    processes::hide_console(&mut replace_cmd);
    let spawn = replace_cmd.spawn();
    if spawn.is_err() {
        return Err(crate::locale::text(
            "无法启动更新脚本。",
            "Failed to start the update script.",
        )
        .into());
    }
    // 记录本轮替换目标版本：新版启动后据此确认上一轮更新成功，再回收
    // exe-update 暂存与 .old 备份（见 cleanup_applied_app_update）。
    // 写失败不阻断更新——残留只是留待下一轮成功更新后清理。
    if let Err(e) = std::fs::write(dir.join(PENDING_APPLY_MARKER), &release.version) {
        crate::logging::log(&format!("updater: 写入应用更新确认标记失败：{e}"));
    }
    crate::logging::log(&format!("updater: 应用更新已就绪，退出并重启（{exe:?}）"));
    // 保存窗口状态 + 清理子进程树，然后退出（替换脚本接管重启）。
    // 先置 quitting：ExitRequested 的守卫据此跳过后台停服重试，也让各
    // 周期任务在替换脚本接管前停止活动。
    app.state::<crate::app_state::AppState>().set_quitting(true);
    crate::window::save_window_state_now(app);
    // 与 quit_sequence 同口径：先取得生命周期锁再停服，避免退出竞态
    crate::dsh::shutdown_for_quit(app);
    app.exit(0);
    Ok(())
}

/// 替换流程目标版本标记（exe-update 目录下，纯文本版本号）：
/// apply_downloaded_exe 启动替换脚本后写入；下次启动由
/// cleanup_applied_app_update 读取并判断上一轮应用更新是否成功。
#[cfg(any(windows, test))]
const PENDING_APPLY_MARKER: &str = "pending-apply";

/// 启动时回收上一轮已成功的应用更新残留（仅 Windows 有该替换流程）：
/// exe-update/ 暂存目录（新 exe 副本与 replace.ps1）整体删除；安装目录的
/// .old 备份在确认当前运行版本==目标版本后删除。标记缺失或版本不匹配
/// （替换失败/等待重试）时一律不动，当轮回滚所需文件全部保留。
/// 失败只记日志，不影响启动恢复流程。
#[cfg(windows)]
pub(crate) fn cleanup_applied_app_update(config: &crate::app_state::Config) {
    let dir = config.root.join("exe-update");
    let exe_old = std::env::current_exe().ok().map(|exe| {
        let mut old = exe.into_os_string();
        old.push(".old");
        std::path::PathBuf::from(old)
    });
    if cleanup_applied_update_in(&dir, env!("CARGO_PKG_VERSION"), exe_old.as_deref()) {
        crate::logging::log("updater: 上一轮应用更新已确认成功，暂存目录与旧版备份已回收");
    }
}

/// 按标记与运行版本执行残留清理（纯逻辑，便于单测）：返回是否发生了清理。
#[cfg(any(windows, test))]
fn cleanup_applied_update_in(
    dir: &std::path::Path,
    running_version: &str,
    exe_old: Option<&std::path::Path>,
) -> bool {
    let marker = dir.join(PENDING_APPLY_MARKER);
    let Ok(target) = std::fs::read_to_string(&marker) else {
        return false; // 无标记：上一轮未进入替换流程，不碰任何文件
    };
    let target = target.trim();
    if target.is_empty() || target != running_version {
        // 替换未完成（脚本失败后旧版仍在运行）或版本已被其他途径覆盖：
        // 保留暂存与备份供回滚/同版本重试；标记一并保留——若替换脚本仍在
        // 重试窗口内并最终成功，下次启动仍能完成回收；下一轮更新会重写标记。
        return false;
    }
    if let Err(e) = std::fs::remove_dir_all(dir) {
        crate::logging::log(&format!("updater: 清理应用更新暂存目录失败：{e}"));
    }
    if let Some(old) = exe_old {
        if old.exists() {
            if let Err(e) = std::fs::remove_file(old) {
                crate::logging::log(&format!("updater: 清理应用旧版备份失败：{e}"));
            }
        }
    }
    true
}

/// 后台预下载应用更新（无需用户确认）：发现新版且未下载时自动下载，
/// 完成后弹提示"重启应用以更新"。仅 Windows（其他平台提示官网下载）；失败静默记日志。
pub fn prefetch_app_update(app: &AppHandle) {
    #[cfg(not(windows))]
    {
        let _ = app;
    }
    #[cfg(windows)]
    {
        use std::sync::atomic::{AtomicBool, Ordering};
        static PREFETCHING: AtomicBool = AtomicBool::new(false);
        if PREFETCHING.swap(true, Ordering::SeqCst) {
            return; // 已有预下载在进行
        }
        let handle = app.clone();
        std::thread::spawn(move || {
            // 作用域内复位标记（提前返回/正常结束都复位）
            struct Reset;
            impl Drop for Reset {
                fn drop(&mut self) {
                    PREFETCHING.store(false, Ordering::Release);
                }
            }
            let _reset = Reset;
            let Some(info) = check_app_update() else {
                return;
            };
            if !info.update_available {
                return;
            }
            if let Some((ready_version, sha256)) = handle.state::<AppState>().app_update_ready() {
                let target = handle
                    .state::<AppState>()
                    .config()
                    .root
                    .join("exe-update")
                    .join("DSHBox.exe");
                if ready_version == info.latest && verify_downloaded_exe(&target, &sha256).is_ok() {
                    return; // 已下载且摘要仍匹配
                }
                handle.state::<AppState>().set_app_update_ready(None);
                let _ = std::fs::remove_file(&target);
            }
            let release = match fetch_app_release_asset(&info.latest) {
                Ok(release) => release,
                Err(e) => {
                    crate::logging::log(&format!("updater: 应用更新元数据校验失败：{e}"));
                    return;
                }
            };
            let config = handle.state::<AppState>().config();
            let dir = config.root.join("exe-update");
            if std::fs::create_dir_all(&dir).is_err() {
                return;
            }
            let target = dir.join("DSHBox.exe");
            crate::logging::log(&format!(
                "updater: 后台预下载应用更新 {}（当前 {}）",
                info.latest, info.installed
            ));
            if let Err(e) = download_app_exe(&handle, &target, &release, false) {
                crate::logging::log(&format!("updater: 应用更新预下载失败：{e}"));
                return;
            }
            handle
                .state::<AppState>()
                .set_app_update_ready(Some((release.version.clone(), release.sha256.clone())));
            crate::logging::log("updater: 应用更新已预下载，提示用户重启应用");
            prompt_apply_prefetched(&handle, &release.version);
        });
    }
}

/// 提示用户应用已下载的更新（自绘弹窗：重启并更新 / 稍后 / 查看更新内容）。
/// 「重启并更新」由弹窗前端走 app_dialog_update("app")：update_app_exe 会复用
/// 已预下载且摘要吻合的安装包，不重复下载。
#[cfg(windows)]
fn prompt_apply_prefetched(app: &AppHandle, version: &str) {
    let release_url = format!("https://github.com/{APP_REPO}/releases/tag/v{version}");
    crate::control_center::open_update_prompt(
        app,
        crate::control_center::UpdatePrompt {
            kind: "app".into(),
            version: version.to_string(),
            current: None,
            release_url: Some(release_url),
            simulated: None,
        },
    );
}

#[cfg(test)]
mod tests {
    use super::{cleanup_applied_update_in, PENDING_APPLY_MARKER};

    fn temp_dir(tag: &str) -> std::path::PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "dshbox-appupdate-{tag}-{}-{nanos}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn cleanup_only_runs_with_a_matching_success_marker() {
        // 无标记：上一轮未进入替换流程，暂存文件原样保留
        let dir = temp_dir("nomarker");
        let staged = dir.join("DSHBox.exe");
        std::fs::write(&staged, b"new").unwrap();
        assert!(!cleanup_applied_update_in(&dir, "1.2.3", None));
        assert!(staged.exists());
        std::fs::remove_dir_all(&dir).ok();

        // 标记版本与运行版本不一致（替换失败/已回滚）：暂存、备份与标记全部保留
        let dir = temp_dir("mismatch");
        let staged = dir.join("DSHBox.exe");
        std::fs::write(&staged, b"new").unwrap();
        std::fs::write(dir.join(PENDING_APPLY_MARKER), "1.9.9").unwrap();
        let old = temp_dir("mismatch-old").join("DSHBox.exe.old");
        std::fs::write(&old, b"old").unwrap();
        assert!(!cleanup_applied_update_in(&dir, "1.2.3", Some(&old)));
        assert!(staged.exists());
        assert!(dir.join(PENDING_APPLY_MARKER).exists());
        assert!(old.exists());
        std::fs::remove_dir_all(&dir).ok();
        std::fs::remove_dir_all(old.parent().unwrap()).ok();

        // 标记匹配（当前运行版本==目标版本）：回收暂存目录与 .old 备份，且幂等
        let dir = temp_dir("match");
        std::fs::write(dir.join("DSHBox.exe"), b"new").unwrap();
        std::fs::write(dir.join("replace.ps1"), b"script").unwrap();
        std::fs::write(dir.join(PENDING_APPLY_MARKER), "1.2.3").unwrap();
        let old = temp_dir("match-old").join("DSHBox.exe.old");
        std::fs::write(&old, b"old").unwrap();
        assert!(cleanup_applied_update_in(&dir, "1.2.3", Some(&old)));
        assert!(!dir.exists());
        assert!(!old.exists());
        // 标记随目录删除，再次运行不再动作
        assert!(!cleanup_applied_update_in(&dir, "1.2.3", Some(&old)));
        std::fs::remove_dir_all(old.parent().unwrap()).ok();
    }
}
