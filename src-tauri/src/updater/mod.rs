//! 更新协调：检查并分派应用、dsh、Node 与 PowerShell 更新。

use std::time::Duration;

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};

use crate::app_state::{AppState, BootPhase};
#[cfg(windows)]
use crate::processes;
use crate::runtime;
mod app;
mod check;
#[path = "dsh.rs"]
mod dsh_update;
mod node;
mod powershell;
pub(crate) mod transaction;

use crate::versions;
use crate::{dsh, emit_status, navigate_to_splash};
pub use app::prefetch_app_update;
use app::update_app_exe;
#[cfg(test)]
use app::{parse_app_release_asset, windows_replace_script};
pub(crate) use check::apply_dsh_update;
pub use check::{check, check_and_report, silent_check, start_periodic_check, CheckResult};
use dsh_update::update_dsh;
use node::update_node;
#[cfg(test)]
use powershell::parse_pwsh_metadata;
#[cfg(test)]
use powershell::parse_releases_atom;
use powershell::update_pwsh;
#[cfg(windows)]
use powershell::{latest_pwsh_version, pwsh_version};
use transaction as update_txn;

#[cfg(windows)]
fn truncate(text: &str, max_chars: usize) -> String {
    let mut truncated: String = text.chars().take(max_chars).collect();
    if text.chars().count() > max_chars {
        truncated.push('…');
    }
    truncated
}

const APP_REPO: &str = "JeffioZ/dsh-box";
#[cfg(any(windows, test))]
const APP_WINDOWS_ASSET: &str = "DSHBox-windows-x64.exe";

/// 回滚失败后，报错文案承诺的恢复方式差异：
/// - `Restore`：下次启动自动还原旧版本（dsh，目录被整体替换）；
/// - `KeepMarker`：已保留备份与事务标记，下次启动再次恢复（Node）。
enum RollbackRecoveryNote {
    Restore,
    KeepMarker,
}

impl RollbackRecoveryNote {
    fn after_install_failure(
        self,
        name: &str,
        installed_error: &str,
        rollback_error: &str,
    ) -> String {
        match self {
            RollbackRecoveryNote::Restore => crate::locale::owned(
                format!(
                    "{installed_error}；旧版本自动恢复失败：{rollback_error}。\n\
                     下次启动将自动还原旧版本。"
                ),
                format!(
                    "{installed_error}; automatic rollback failed: {rollback_error}.\n\
                     The previous version will be restored on the next launch."
                ),
            ),
            RollbackRecoveryNote::KeepMarker => crate::locale::owned(
                format!(
                    "{installed_error}；旧 {name} 自动恢复失败：{rollback_error}。\n\
                     已保留备份和事务标记，下次启动将再次恢复。"
                ),
                format!(
                    "{installed_error}; automatic {name} rollback failed: {rollback_error}.\n\
                     The backup and transaction marker were kept for recovery on the next launch."
                ),
            ),
        }
    }
}

/// dsh / Node.js 目录更新的统一事务骨架：
/// 两阶段钩子——`prepare` 在停服/备份前执行（保证网络/安装器准备失败不产生
/// 停机窗口），`install` 在目录备份完成后执行。参数化两者的真实差异：
/// - `name`：错误文案里的展示名（dsh / Node.js）；
/// - `require_current`：current 目录是否必须存在（dsh true；Node false）；
/// - `restore_note`：回滚失败后的恢复承诺差异。
#[allow(clippy::too_many_arguments)]
fn with_directory_transaction<T>(
    app: &AppHandle,
    current: &std::path::Path,
    backup: &std::path::Path,
    marker: &std::path::Path,
    name: &str,
    require_current: bool,
    restore_note: RollbackRecoveryNote,
    prepare: impl FnOnce() -> Result<T, String>,
    install: impl FnOnce(T) -> Result<(), String>,
) -> Result<(), String> {
    // 1) 停服前准备：与目录/服务无关的前置条件全部先检查（含网络与安装器）。
    let prepared = prepare()?;
    // 2) 残留与存在性检查。
    if backup.exists() || marker.exists() {
        return Err(crate::locale::owned(
            format!(
                "检测到未完成的 {name} 更新，请重启应用后重试：{}",
                backup.display()
            ),
            format!(
                "An unfinished {name} update was found. Restart the app before trying again: {}",
                backup.display()
            ),
        ));
    }
    if require_current && !current.exists() {
        return Err(crate::locale::owned(
            format!("未找到当前 {name} 安装目录"),
            format!("The current {name} installation directory was not found"),
        ));
    }

    // 3) 进入事务：停止可用服务、备份当前版本。
    emit_progress(
        app,
        crate::locale::text("正在停止 dsh 服务…", "Stopping the dsh service…"),
    );
    update_txn::create_marker(marker)?;
    dsh::shutdown(app);
    navigate_to_splash(app);
    std::thread::sleep(Duration::from_millis(800));
    if current.exists() {
        if let Err(e) = std::fs::rename(current, backup) {
            update_txn::remove_marker(marker);
            let _ = restart_service_locked(app);
            return Err(crate::locale::owned(
                format!("备份当前 {name} 失败：{e}"),
                format!("Failed to back up the current {name} installation: {e}"),
            ));
        }
    }

    // 4) 安装；失败回滚并重启恢复。
    if let Err(e) = install(prepared) {
        if let Err(re) = update_txn::rollback_directory(current, backup, marker) {
            dsh::shutdown(app);
            return Err(restore_note.after_install_failure(name, &e, &re));
        }
        return match restart_service_locked(app) {
            Ok(()) => Err(crate::locale::owned(
                format!("{e}；已恢复旧版本"),
                format!("{e}; the previous version was restored"),
            )),
            Err(re) => Err(crate::locale::owned(
                format!("{e}；旧版本恢复后未能启动：{re}"),
                format!("{e}; the restored version did not start: {re}"),
            )),
        };
    }

    // 5) 重启成功提交：清除标记与备份。
    emit_progress(
        app,
        crate::locale::text(
            format!("{name} 更新完成，正在重启服务…").as_str(),
            format!("{name} update complete. Restarting the service…").as_str(),
        ),
    );
    if let Err(e) = restart_service_locked(app) {
        dsh::shutdown(app);
        if let Err(re) = update_txn::rollback_directory(current, backup, marker) {
            return Err(crate::locale::owned(
                format!(
                    "新 {name} 启动失败：{e}；旧版本自动恢复也失败：{re}。\n\
                     服务已停止，下次启动将自动还原旧版本。"
                ),
                format!(
                    "The new {name} version did not start: {e}; automatic rollback also failed: {re}.\n\
                     The service is stopped; the previous version will be restored on the next launch."
                ),
            ));
        }
        let restore_result = restart_service_locked(app);
        return match restore_result {
            Ok(()) => Err(crate::locale::owned(
                format!("新 {name} 启动失败，已恢复旧版本：{e}"),
                format!(
                    "The new {name} version did not start; the previous version was restored: {e}"
                ),
            )),
            Err(re) => Err(crate::locale::owned(
                format!("新 {name} 启动失败：{e}；旧版本恢复后也未能启动：{re}"),
                format!(
                    "The new {name} version did not start: {e}; the restored version also failed to start: {re}"
                ),
            )),
        };
    }
    // 提交成功：清除事务标记（幂等；删除失败仅日志——更新已成功，不应
    // 因清理标记失败而向用户报“更新失败”，下次启动 recover 自愈）。
    update_txn::remove_marker(marker);
    if backup.exists() {
        if let Err(e) = std::fs::remove_dir_all(backup) {
            crate::logging::log(&format!(
                "updater: 清理 {name} 备份失败（不影响当前版本）：{e}"
            ));
        }
    }
    Ok(())
}

/// 确保更新函数无论如何返回都会恢复更新标记。
struct UpdatingReset<'a>(&'a AppState);

impl Drop for UpdatingReset<'_> {
    fn drop(&mut self) {
        self.0.set_updating(false);
    }
}

fn emit_progress(app: &AppHandle, message: &str) {
    // 事件之外同步写入状态：检查更新弹窗关闭再打开后，进行中的更新进度
    // 仍能经轮询（app_dialog_check_get）拉取——事件通道对隐藏窗口不可靠。
    app.state::<AppState>()
        .set_check_progress(Some(message.to_string()));
    let _ = app.emit("update-progress", serde_json::json!({ "message": message }));
}

// ---------- 应用更新 ----------

/// 应用更新（which: "dsh" | "node" | "pwsh"）。
pub fn apply(app: &AppHandle, which: &str) -> Result<(), String> {
    let state = app.state::<AppState>();
    if state.service_ownership().is_external() && matches!(which, "dsh" | "node" | "npm") {
        return Err(crate::locale::text(
            "当前连接由外部 dsh 服务管理，请在原服务环境中执行这项更新。",
            "The current connection is managed by an external dsh service. Update it in that service's environment.",
        )
        .into());
    }
    if !state.try_begin_update() {
        let msg = crate::locale::text(
            "启动或更新流程正在进行，请稍后再试。",
            "Startup or another update is in progress. Please try again later.",
        )
        .to_string();
        emit_progress(
            app,
            &format!(
                "{}: {msg}",
                crate::locale::text("更新失败", "Update failed")
            ),
        );
        return Err(msg);
    }
    let _updating = UpdatingReset(state.inner());
    // 覆盖停止、安装、切换目录和重启的完整周期，避免与手动重启交叉执行。
    let _lifecycle = state.lifecycle_guard();
    let result = if which == "dsh" {
        update_dsh(app, &state.config())
    } else if which == "node" {
        update_node(app, &state.config())
    } else if which == "pwsh" {
        update_pwsh(app)
    } else if which == "app" {
        update_app_exe(app, &state.config())
    } else if which == "npm" {
        // strict：手动更新失败要报给用户（区别于启动时静默降级）
        runtime::upgrade_portable_npm(app, &state.config(), true)
    } else {
        Err(format!(
            "{}: {which}",
            crate::locale::text("未知更新目标", "Unknown update target")
        ))
    };
    if let Err(msg) = &result {
        // 让启动页/托盘能看到失败原因
        emit_progress(
            app,
            &format!(
                "{}: {msg}",
                crate::locale::text("更新失败", "Update failed")
            ),
        );
    }
    result
}

// ---------- 重启服务 ----------

const RESTART_VIEW_TIMEOUT: Duration = Duration::from_secs(1);

/// 从 dsh 页面重启时，先把主 WebView 交给永远可用的内置启动页。导航提交后
/// 才停止本地服务，避免服务端口短暂离线时露出 WebView2/浏览器原生错误页。
/// 内置页会从 get_status 读取 Starting 状态，继续显示同一套加载反馈。
fn enter_restart_view(app: &AppHandle, from_dsh_page: bool) {
    if !from_dsh_page {
        return;
    }
    navigate_to_splash(app);
    let deadline = std::time::Instant::now() + RESTART_VIEW_TIMEOUT;
    loop {
        let local_page_committed = crate::main_webview(app)
            .and_then(|webview| webview.url().ok())
            .is_some_and(|url| {
                let dev = crate::app_dev_origin(app);
                crate::is_local_app_url(&url, dev.as_ref())
            });
        if local_page_committed {
            return;
        }
        if std::time::Instant::now() >= deadline {
            crate::logging::log("updater: 重启过渡页 1s 内未完成导航，继续重启服务");
            return;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// 重启服务并进入界面（托盘“重启服务”/更新后复用）。
/// 持有生命周期锁，与 boot_once 互斥，杜绝双服务并发。
pub(crate) fn restart_service(app: &AppHandle) -> Result<(), String> {
    let state = app.state::<AppState>();
    if state.service_ownership().is_external() {
        return Err(crate::locale::text(
            "当前连接的是外部 dsh 服务，DSHBox 不会重启它。",
            "The current dsh service is external, so DSHBox will not restart it.",
        )
        .into());
    }
    if state.is_updating() {
        return Err(crate::locale::text(
            "更新流程正在进行，请稍后再重启。",
            "An update is in progress. Please restart the service later.",
        )
        .into());
    }
    let _guard = state.lifecycle_guard();
    // 覆盖“检查后、拿锁前”更新刚好开始的竞争窗口。
    if state.is_updating() {
        return Err(crate::locale::text(
            "更新流程正在进行，请稍后再重启。",
            "An update is in progress. Please restart the service later.",
        )
        .into());
    }
    restart_service_locked(app)
}

/// 调用方已持有生命周期锁时使用。
pub(crate) fn restart_service_locked(app: &AppHandle) -> Result<(), String> {
    let state = app.state::<AppState>();
    if state.service_ownership().is_external() {
        return Err(crate::locale::text(
            "当前连接的是外部 dsh 服务，DSHBox 不会重启它。",
            "The current dsh service is external, so DSHBox will not restart it.",
        )
        .into());
    }
    let mut config = state.config();
    let resume_url = crate::main_webview(app)
        .and_then(|webview| webview.url().ok())
        .filter(|url| crate::is_dsh_url(url, &config))
        .map(|url| url.to_string());
    let restarting = crate::locale::text("正在重启服务…", "Restarting the service…");
    state.set_phase(BootPhase::Starting, restarting, "");
    emit_status(app, BootPhase::Starting, restarting, "");
    enter_restart_view(app, resume_url.is_some());
    let result = (|| -> Result<(), String> {
        // 先停掉残留进程
        dsh::shutdown(app);
        std::thread::sleep(Duration::from_millis(800));
        let node = runtime::ensure_node(app, &config)?;
        state.set_node_version(Some(node.version.clone()));
        state.set_npm_version(runtime::npm_version(&config));
        let port = dsh::launch_managed(app, &mut config, &node.executable)?;
        config.port = port;
        Ok(())
    })();
    match &result {
        Ok(()) => {
            let ready = crate::locale::text("已就绪", "Ready");
            state.set_phase(BootPhase::Ready, ready, "");
            // 唤醒可能阻塞在错误页等待的 boot_loop，让其重入引导（复用本服务）进入看门狗
            state.signal_retry();
            let target = resume_url
                .and_then(|url| remap_service_url(&url, config.port))
                .unwrap_or_else(|| config.web_url());
            dsh::enter_web_app(app, &target);
        }
        Err(msg) => {
            state.set_phase(BootPhase::Error, msg, "");
            emit_status(app, BootPhase::Error, msg, "");
            // 用户此刻可能在 dsh 界面：导航回启动页让错误与重试按钮可见
            navigate_to_splash(app);
        }
    }
    result
}

fn remap_service_url(url: &str, port: u16) -> Option<String> {
    let mut parsed = url::Url::parse(url).ok()?;
    parsed.set_scheme("http").ok()?;
    parsed.set_host(Some("127.0.0.1")).ok()?;
    parsed.set_port(Some(port)).ok()?;
    Some(parsed.to_string())
}

#[cfg(test)]
mod tests {
    use super::{
        parse_app_release_asset, parse_pwsh_metadata, parse_releases_atom, remap_service_url,
        windows_replace_script, RollbackRecoveryNote,
    };

    #[test]
    fn rollback_recovery_notes_preserve_distinct_promises() {
        // 两种恢复承诺必须不同：dsh 说“还原”，Node 说“保留标记”。
        // 不依赖具体语言，只验证两分支确实产生不同文案、避免被抹平成同一句。
        let restore =
            RollbackRecoveryNote::Restore.after_install_failure("dsh", "安装失败", "回滚失败");
        let keep = RollbackRecoveryNote::KeepMarker.after_install_failure(
            "Node.js",
            "安装失败",
            "回滚失败",
        );
        assert_ne!(restore, keep);
        assert!(!restore.is_empty());
        assert!(!keep.is_empty());
    }

    #[test]
    fn parses_official_powershell_stable_tag() {
        let metadata = serde_json::json!({ "StableReleaseTag": "v7.6.4" });
        assert_eq!(parse_pwsh_metadata(&metadata).unwrap(), "7.6.4");
    }

    #[test]
    fn parses_releases_atom_tags_in_order() {
        let xml = r#"<?xml version="1.0"?>
<feed><entry>
  <link rel="alternate" type="text/html" href="https://github.com/o/r/releases/tag/v0.2.0"/>
  <title>v0.2.0</title>
</entry><entry>
  <link rel="alternate" type="text/html" href="https://github.com/o/r/releases/tag/v0.1.0"/>
</entry></feed>"#;
        assert_eq!(parse_releases_atom(xml), vec!["v0.2.0", "v0.1.0"]);
    }

    #[test]
    fn service_restart_preserves_route_when_port_changes() {
        assert_eq!(
            remap_service_url("http://127.0.0.1:18080/session/abc?view=chat#latest", 49152)
                .as_deref(),
            Some("http://127.0.0.1:49152/session/abc?view=chat#latest")
        );
    }

    #[test]
    fn releases_atom_skips_non_tag_links() {
        let xml = "<entry><link rel=\"alternate\" href=\"https://github.com/o/r/releases/tag/v1.0.0\"/><link rel=\"self\" href=\"https://x/atom\"/></entry>";
        assert_eq!(parse_releases_atom(xml), vec!["v1.0.0"]);
    }

    #[test]
    fn release_asset_requires_exact_tag_url_and_digest() {
        let json = serde_json::json!({
            "tag_name": "v0.2.0",
            "draft": false,
            "prerelease": false,
            "assets": [{
                "name": "DSHBox-windows-x64.exe",
                "state": "uploaded",
                "browser_download_url": "https://github.com/JeffioZ/dsh-box/releases/download/v0.2.0/DSHBox-windows-x64.exe",
                "digest": format!("sha256:{}", "ab".repeat(32))
            }]
        });
        let (url, digest) = parse_app_release_asset(&json, "0.2.0").unwrap();
        assert!(url.contains("/v0.2.0/"));
        assert_eq!(digest, "ab".repeat(32));
    }

    #[test]
    fn release_asset_rejects_missing_digest_and_version_drift() {
        let mut json = serde_json::json!({
            "tag_name": "v0.2.0",
            "draft": false,
            "prerelease": false,
            "assets": [{
                "name": "DSHBox-windows-x64.exe",
                "state": "uploaded",
                "browser_download_url": "https://github.com/JeffioZ/dsh-box/releases/download/v0.2.0/DSHBox-windows-x64.exe",
                "digest": null
            }]
        });
        assert!(parse_app_release_asset(&json, "0.2.0").is_err());
        json["assets"][0]["digest"] = serde_json::json!(format!("sha256:{}", "ab".repeat(32)));
        assert!(parse_app_release_asset(&json, "0.3.0").is_err());
    }

    #[test]
    fn release_asset_rejects_duplicate_windows_assets() {
        let asset = serde_json::json!({
            "name": "DSHBox-windows-x64.exe",
            "state": "uploaded",
            "browser_download_url": "https://github.com/JeffioZ/dsh-box/releases/download/v0.2.0/DSHBox-windows-x64.exe",
            "digest": format!("sha256:{}", "ab".repeat(32))
        });
        let json = serde_json::json!({
            "tag_name": "v0.2.0",
            "draft": false,
            "prerelease": false,
            "assets": [asset.clone(), asset]
        });
        assert!(parse_app_release_asset(&json, "0.2.0").is_err());
    }

    #[test]
    fn replacement_script_stages_and_atomically_replaces() {
        let script = windows_replace_script(
            std::path::Path::new(r"C:\cache\DSHBox.exe"),
            std::path::Path::new(r"C:\app\DSHBox.exe"),
            &"a".repeat(64),
        );
        assert!(script.contains("Copy-Item -LiteralPath $src -Destination $new"));
        assert!(script.contains("[System.IO.File]::Replace($new, $dst, $old, $true)"));
        assert!(script.contains("Get-FileHash -Algorithm SHA256"));
        assert!(!script.contains("Move-Item -LiteralPath $dst -Destination $old"));
    }
}
