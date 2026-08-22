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
        .ok_or_else(|| "Release asset has no download URL".to_string())?;
    let parsed = url::Url::parse(asset_url).map_err(|_| "Invalid release asset URL".to_string())?;
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
            download_app_exe(app, &target, &expected)?;
        }
        app.state::<AppState>()
            .set_app_update_ready(Some((expected.version.clone(), expected.sha256.clone())));
        apply_downloaded_exe(app, &target, &expected.sha256)
    }
}

/// 预下载文件是否可应用：存在 + 体积下限 + MZ 头 + GitHub 资产 SHA-256。
#[cfg(windows)]
fn verify_downloaded_exe(target: &std::path::Path, expected_sha256: &str) -> Result<(), String> {
    use sha2::{Digest, Sha256};
    use std::fmt::Write as _;
    use std::io::Seek as _;

    let Ok(meta) = target.metadata() else {
        return Err("Downloaded executable is missing".into());
    };
    if meta.len() < 1024 * 1024 {
        return Err("Downloaded executable is too small".into());
    }
    let mut file = std::fs::File::open(target).map_err(|e| e.to_string())?;
    let mut mz = [0u8; 2];
    file.read_exact(&mut mz).map_err(|e| e.to_string())?;
    if &mz != b"MZ" {
        return Err("Downloaded executable has no MZ header".into());
    }
    file.rewind().map_err(|e| e.to_string())?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let read = file.read(&mut buf).map_err(|e| e.to_string())?;
        if read == 0 {
            break;
        }
        hasher.update(&buf[..read]);
    }
    let mut actual = String::with_capacity(64);
    for byte in hasher.finalize() {
        let _ = write!(&mut actual, "{byte:02x}");
    }
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
#[cfg(windows)]
fn download_app_exe(
    app: &AppHandle,
    target: &std::path::Path,
    release: &AppReleaseAsset,
) -> Result<(), String> {
    static DOWNLOAD_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _guard = DOWNLOAD_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    if verify_downloaded_exe(target, &release.sha256).is_ok() {
        return Ok(());
    }
    let result = download_app_exe_inner(app, target, release);
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
) -> Result<(), String> {
    use std::io::Write as _;

    emit_progress(
        app,
        crate::locale::text("正在下载应用更新…", "Downloading the app update…"),
    );
    let resp = runtime::download_client()
        .get(&release.url)
        .header("User-Agent", "DSHBox")
        .call()
        .map_err(|e| crate::locale::error("下载失败", "Download failed", e))?;
    // 单文件 exe 上限 512MB：防止异常响应/恶意源写满磁盘
    const MAX_APP_EXE_BYTES: u64 = 512 * 1024 * 1024;
    let mut reader = resp.into_body().into_reader().take(MAX_APP_EXE_BYTES + 1);
    let part = target.with_extension("exe.part");
    let _ = std::fs::remove_file(&part);
    let mut file = std::fs::File::create(&part)
        .map_err(|e| crate::locale::error("写入失败", "Failed to write the update", e))?;
    let copied = std::io::copy(&mut reader, &mut file)
        .map_err(|e| crate::locale::error("下载中断", "Download interrupted", e))?;
    if copied > MAX_APP_EXE_BYTES {
        return Err(crate::locale::text(
            "下载内容超出预期大小，已取消更新。",
            "The downloaded content exceeds the expected size. Update cancelled.",
        )
        .into());
    }
    file.flush().map_err(|e| {
        crate::locale::error("写入更新包失败", "Failed to write the update package", e)
    })?;
    file.sync_all().map_err(|e| {
        crate::locale::error("写入更新包失败", "Failed to write the update package", e)
    })?;
    drop(file);
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
           exit 1\n\
         }} finally {{\n\
           if (Test-Path -LiteralPath $new) {{ Remove-Item -LiteralPath $new -Force }}\n\
         }}\n",
        ps_quote(source),
        ps_quote(destination),
        expected_sha256.to_ascii_lowercase(),
    )
}

/// 应用已下载的更新包：确认 → 写替换脚本 → 退出（脚本替换并重启新版本）。
#[cfg(windows)]
fn apply_downloaded_exe(
    app: &AppHandle,
    target: &std::path::Path,
    expected_sha256: &str,
) -> Result<(), String> {
    // 确认前再次校验，覆盖“预下载完成后文件被替换”的窗口。
    verify_downloaded_exe(target, expected_sha256)?;
    // 3) 确认：更新需要退出并自动重启
    use tauri_plugin_dialog::MessageDialogKind;
    if !crate::native_dialog::ask(
        app,
        crate::locale::text(
            "应用将退出并自动重启以完成更新。是否继续？",
            "The app will exit and restart automatically to finish the update. Continue?",
        )
        .to_string(),
        crate::locale::text("更新应用", "Update app"),
        MessageDialogKind::Info,
        crate::locale::text("更新并重启", "Update and restart"),
        crate::locale::text("取消", "Cancel"),
    ) {
        return Ok(());
    }

    // 4) 写替换脚本。新版先复制到当前 exe 同目录并复验摘要，再通过
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
    let script_text = windows_replace_script(target, &exe, expected_sha256);
    std::fs::write(&script, script_text).map_err(|e| {
        crate::locale::error(
            "写入替换脚本失败",
            "Failed to write the replacement script",
            e,
        )
    })?;

    // 5) 启动替换脚本（隐藏、独立于本进程），保存窗口状态后退出
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
    crate::logging::log(&format!("updater: 应用更新已就绪，退出并重启（{exe:?}）"));
    // 保存窗口状态 + 清理子进程树，然后退出（替换脚本接管重启）
    crate::window::save_window_state_now(app);
    crate::dsh::shutdown(app);
    app.exit(0);
    Ok(())
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
            if let Err(e) = download_app_exe(&handle, &target, &release) {
                crate::logging::log(&format!("updater: 应用更新预下载失败：{e}"));
                return;
            }
            handle
                .state::<AppState>()
                .set_app_update_ready(Some((release.version.clone(), release.sha256.clone())));
            crate::logging::log("updater: 应用更新已预下载，提示用户重启应用");
            prompt_apply_prefetched(&handle, &target, &release.version, &release.sha256);
        });
    }
}

/// 提示用户应用已下载的更新（"重启应用以完成更新"）。
#[cfg(windows)]
fn prompt_apply_prefetched(app: &AppHandle, target: &std::path::Path, version: &str, sha256: &str) {
    use tauri_plugin_dialog::MessageDialogKind;
    let msg = crate::locale::owned(
        format!("新版本 {version} 已下载完成。\n是否立即重启应用以完成更新？"),
        format!("Version {version} has been downloaded.\nRestart DSHBox now to finish the update?"),
    );
    if crate::native_dialog::ask(
        app,
        msg,
        crate::locale::text("应用更新已就绪", "App update ready"),
        MessageDialogKind::Info,
        crate::locale::text("重启并更新", "Restart and update"),
        crate::locale::text("稍后", "Later"),
    ) {
        if let Err(e) = apply_downloaded_exe(app, target, sha256) {
            crate::logging::log(&format!("updater: 应用更新应用失败：{e}"));
        }
    }
}
