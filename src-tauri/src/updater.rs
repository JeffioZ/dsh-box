//! 一键更新：检查/更新 dsh（npm 包）、Node（便携版），以及 Windows 的 PowerShell 7（可选增强）。

use std::io::Read;
use std::time::Duration;

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};

use crate::app_state::{AppState, BootPhase};
use crate::processes;
use crate::runtime::{self, base_envs};
use crate::update_txn;
use crate::versions;
use crate::{dsh, emit_status, navigate, SPLASH_ORIGIN};

/// 确保更新函数无论如何返回都会恢复更新标记。
struct UpdatingReset<'a>(&'a AppState);

impl Drop for UpdatingReset<'_> {
    fn drop(&mut self) {
        self.0.set_updating(false);
    }
}

#[derive(Serialize, Clone, Default)]
pub struct CheckResult {
    pub dsh: Option<VersionInfo>,
    pub node: Option<NodeInfo>,
    pub pwsh: Option<PwshInfo>,
    /// 应用自身更新（GitHub Releases）。
    pub app: Option<VersionInfo>,
    pub error: Option<String>,
}

#[derive(Serialize, Clone)]
pub struct VersionInfo {
    pub installed: String,
    pub latest: String,
    pub update_available: bool,
}

#[derive(Serialize, Clone)]
pub struct NodeInfo {
    pub managed: bool,
    pub installed: Option<String>,
    pub latest_lts: Option<String>,
    pub latest_error: Option<String>,
    pub update_available: bool,
}

#[derive(Serialize, Clone)]
pub struct PwshInfo {
    pub installed: Option<String>,
    pub latest: Option<String>,
    pub latest_error: Option<String>,
    /// true 表示需要操作（未安装或存在新版）。
    pub update_available: bool,
}

fn emit_progress(app: &AppHandle, message: &str) {
    // 事件之外同步写入状态：检查更新弹窗关闭再打开后，进行中的更新进度
    // 仍能经轮询（app_dialog_check_get）拉取——事件通道对隐藏窗口不可靠。
    app.state::<AppState>()
        .set_check_progress(Some(message.to_string()));
    let _ = app.emit("update-progress", serde_json::json!({ "message": message }));
}

// ---------- 检查 ----------

/// 检查并汇报（托盘/启动页共用）。
pub fn check_and_report(app: &AppHandle) -> Result<(), String> {
    emit_progress(
        app,
        crate::locale::text("正在检查更新…", "Checking for updates…"),
    );
    let result = check(app);
    let _ = app.emit("update-result", &result);
    Ok(())
}

/// 启动时静默检查一次 dsh 更新（不阻塞启动）：
/// - 有新版 → 弹一次提示（选“稍后”则本会话不再提示），不自动安装；
/// - 检查失败 / 无更新 → 完全静默（仅日志），不打扰。
pub fn silent_check(app: &AppHandle) {
    use std::sync::atomic::{AtomicBool, Ordering};
    static CHECKED: AtomicBool = AtomicBool::new(false);
    if CHECKED.swap(true, Ordering::SeqCst) {
        return; // 本进程只检查一次（看门狗重启后不重复弹窗）
    }
    let handle = app.clone();
    std::thread::spawn(move || {
        let result = check(&handle);
        match &result.dsh {
            Some(d) => {
                crate::logging::log(&format!(
                    "updater: 静默检查 dsh 当前 {}，最新 {}（{}）",
                    d.installed,
                    d.latest,
                    if d.update_available {
                        "可更新"
                    } else {
                        "已是最新"
                    }
                ));
                // 启动页若仍可见则展示结果
                let _ = handle.emit("update-result", &result);
                if d.update_available {
                    show_update_dialog(&handle, d);
                }
            }
            None => {
                if let Some(e) = &result.error {
                    crate::logging::log(&format!("updater: 静默检查跳过：{e}"));
                }
            }
        }
    });
}

/// 运行期周期检查（每 6 小时）：发现 dsh 新版时弹提示，不自动安装。
/// 与启动静默检查相互独立（不共享其一次性 CHECKED 标记）；退出中不再检查。
pub fn start_periodic_check(app: AppHandle) {
    const PERIODIC_CHECK_INTERVAL: std::time::Duration =
        std::time::Duration::from_secs(6 * 60 * 60);
    std::thread::spawn(move || loop {
        std::thread::sleep(PERIODIC_CHECK_INTERVAL);
        if app.state::<AppState>().is_quitting() {
            return;
        }
        let result = check(&app);
        match &result.dsh {
            Some(d) if d.update_available => {
                crate::logging::log(&format!(
                    "updater: 周期检查发现 dsh 新版 {}（当前 {}），提示用户",
                    d.latest, d.installed
                ));
                show_update_dialog(&app, d);
            }
            Some(d) => crate::logging::log(&format!(
                "updater: 周期检查 dsh 已是最新（{}）",
                d.installed
            )),
            None => {
                if let Some(e) = &result.error {
                    crate::logging::log(&format!("updater: 周期检查失败：{e}"));
                }
            }
        }
    });
}

/// 有新版时的启动提示（不自动安装，用户确认才更新）。
fn show_update_dialog(app: &AppHandle, d: &VersionInfo) {
    use tauri_plugin_dialog::MessageDialogKind;
    let msg = if crate::locale::is_chinese() {
        format!(
            "dsh 有新版本可用：\n{}（当前 {}）\n\n是否立即更新？",
            d.latest, d.installed
        )
    } else {
        format!(
            "A new dsh version is available:\n{} (current: {})\n\nUpdate now?",
            d.latest, d.installed
        )
    };
    if crate::dialog::ask(
        app,
        msg,
        crate::locale::text("发现新版本", "Update available"),
        MessageDialogKind::Info,
        crate::locale::text("立即更新", "Update now"),
        crate::locale::text("稍后", "Later"),
    ) {
        if let Err(e) = apply(app, "dsh") {
            crate::dialog::show_message(
                app,
                format!("{}: {e}", crate::locale::text("更新失败", "Update failed")),
                crate::locale::text("更新", "Update"),
                MessageDialogKind::Warning,
            );
        }
    }
}

/// 查询 dsh 与 Node 是否有可用更新。
pub fn check(app: &AppHandle) -> CheckResult {
    let config = app.state::<AppState>().config();
    let mut result = CheckResult::default();

    // 三个独立检测（npm HTTP / Node 检测 + LTS HTTP / GitHub HTTP）并行执行，
    // 检查弹窗等待时间从“三者之和”缩短为“最慢者”。
    let dsh_cfg = config.clone();
    let dsh_handle = std::thread::spawn(move || match runtime::installed_dsh_version(&dsh_cfg) {
        Some(installed) => match runtime::npm_latest_dsh_version() {
            Ok(latest) => (
                Some(VersionInfo {
                    installed: installed.clone(),
                    latest: latest.clone(),
                    update_available: versions::compare_versions(&latest, &installed)
                        == std::cmp::Ordering::Greater,
                }),
                None,
            ),
            Err(e) => (
                None,
                Some(format!(
                    "{}: {e}",
                    crate::locale::text(
                        "查询 dsh 最新版本失败",
                        "Failed to query the latest dsh version"
                    )
                )),
            ),
        },
        None => (
            None,
            Some(crate::locale::text("未检测到 dsh 安装", "Installed dsh was not found").into()),
        ),
    });

    // node：检测“当前实际使用的 Node”（DSHDesktop 便携优先，其次系统安装的 Node）。
    let node_cfg = config.clone();
    let node_handle = std::thread::spawn(move || {
        let managed = node_cfg.node_exe().exists();
        let installed = runtime::current_node_version(&node_cfg);
        let (latest_lts, latest_error) = match runtime::latest_lts() {
            Ok(version) => (Some(version), None),
            Err(error) => {
                crate::logging::log(&format!("updater: Node.js 最新版本查询失败：{error}"));
                (None, Some(error))
            }
        };
        let update_available = managed
            && latest_lts.is_some()
            && installed
                .as_deref()
                .map(|cur| {
                    versions::compare_versions(latest_lts.as_deref().unwrap_or(""), cur)
                        == std::cmp::Ordering::Greater
                })
                .unwrap_or(false);
        (
            managed,
            installed,
            latest_lts,
            latest_error,
            update_available,
        )
    });

    // PowerShell 7（可选增强，仅 Windows——macOS/Linux 有各自的系统终端）。
    #[cfg(windows)]
    let pwsh_handle = std::thread::spawn(|| {
        let installed = pwsh_version();
        match latest_pwsh_version() {
            Ok(latest) => (
                installed.clone(),
                Some(latest.clone()),
                None,
                match &installed {
                    Some(cur) => {
                        versions::compare_versions(&latest, cur) == std::cmp::Ordering::Greater
                    }
                    None => true,
                },
            ),
            Err(error) => {
                crate::logging::log(&format!("updater: PowerShell 最新版本查询失败：{error}"));
                (installed, None, Some(error), false)
            }
        }
    });
    // 应用自身更新（GitHub Releases；失败静默，不阻塞其他检查）
    let app_handle = std::thread::spawn(check_app_update);

    if let Ok((d, d_err)) = dsh_handle.join() {
        result.dsh = d;
        if result.error.is_none() {
            result.error = d_err;
        }
    }
    if let Ok((managed, installed, latest_lts, latest_error, update_available)) = node_handle.join()
    {
        result.node = Some(NodeInfo {
            managed,
            installed,
            latest_lts,
            latest_error,
            update_available,
        });
    }
    #[cfg(windows)]
    if let Ok((installed, latest, latest_error, update_available)) = pwsh_handle.join() {
        result.pwsh = Some(PwshInfo {
            installed,
            latest,
            latest_error,
            update_available,
        });
    }
    if let Ok(app_info) = app_handle.join() {
        result.app = app_info;
    }
    result
}

/// 应用自身更新检查：GitHub Releases latest 的版本号对比。
/// 检查失败（网络/仓库不存在/无 Release）静默返回 None，不打扰用户。
fn check_app_update() -> Option<VersionInfo> {
    const REPO: &str = "JeffioZ/dsh-desktop";
    let url = format!("https://api.github.com/repos/{REPO}/releases/latest");
    let resp = match runtime::client()
        .get(&url)
        .header("User-Agent", "DSHDesktop")
        .call()
    {
        Ok(r) => r,
        Err(e) => {
            crate::logging::log(&format!("updater: 应用版本查询失败：{e}"));
            return None;
        }
    };
    let mut text = String::new();
    if resp
        .into_body()
        .into_reader()
        .read_to_string(&mut text)
        .is_err()
    {
        return None;
    }
    let json: serde_json::Value = match serde_json::from_str(&text) {
        Ok(v) => v,
        Err(_) => return None,
    };
    let latest = json
        .get("tag_name")
        .and_then(|v| v.as_str())
        .map(|t| t.trim_start_matches('v').to_string())?;
    let installed = env!("CARGO_PKG_VERSION").to_string();
    let update_available =
        versions::compare_versions(&latest, &installed) == std::cmp::Ordering::Greater;
    Some(VersionInfo {
        installed,
        latest,
        update_available,
    })
}

// ---------- PowerShell 7（可选增强，仅 Windows） ----------

/// 检测已安装的 PowerShell 7 版本（未安装返回 None）。
#[cfg(windows)]
fn pwsh_version() -> Option<String> {
    // pwsh 用绝对路径优先：应用启动后才安装的 pwsh 不在 PATH 快照里
    let mut cmd = processes::pwsh_command();
    cmd.args([
        "-NoProfile",
        "-Command",
        "$PSVersionTable.PSVersion.ToString()",
    ]);
    processes::hide_console(&mut cmd);
    let out = cmd.output().ok()?;
    if !out.status.success() {
        return None;
    }
    let v = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if v.is_empty() {
        None
    } else {
        Some(v)
    }
}

/// 从 GitHub 官方 metadata 解析稳定版本号（仅 Windows 的 PowerShell 检测使用；
/// 单测跨平台引用，故非 Windows 下仅抑制 dead_code）。
#[cfg_attr(not(windows), allow(dead_code))]
fn parse_pwsh_metadata(json: &serde_json::Value) -> Result<String, String> {
    json.get("StableReleaseTag")
        .or_else(|| json.get("ReleaseTag"))
        .and_then(|value| value.as_str())
        .map(|value| value.trim_start_matches('v').to_string())
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "metadata has no stable release tag".into())
}

/// 查询 PowerShell 官方最新稳定版。
///
/// 主用 GitHub Releases 列表（取最高的非预览 tag）：官方 metadata.json 的
/// StableReleaseTag 更新滞后于发布（实测 7.6.5 发布后仍停留在 7.6.4），
/// 只在其上兜底会在补丁发布后漏报。GitHub API 失败时回退 metadata。
#[cfg(windows)]
fn latest_pwsh_version() -> Result<String, String> {
    match github_latest_stable() {
        Ok(version) => return Ok(version),
        Err(github_error) => {
            crate::logging::log(&format!(
                "updater: PowerShell GitHub Releases 查询失败，回退官方 metadata：{github_error}"
            ));
        }
    }

    let metadata_result = runtime::client()
        .get("https://raw.githubusercontent.com/PowerShell/PowerShell/master/tools/metadata.json")
        .header("User-Agent", "DSHDesktop")
        .call()
        .map_err(|e| e.to_string())
        .and_then(|response| {
            response
                .into_body()
                .read_json::<serde_json::Value>()
                .map_err(|e| e.to_string())
        })
        .and_then(|json| parse_pwsh_metadata(&json));
    match metadata_result {
        Ok(version) => Ok(version),
        Err(metadata_error) => Err(format!(
            "{}: {metadata_error}",
            crate::locale::text(
                "获取 PowerShell 版本信息失败",
                "Failed to retrieve PowerShell version information"
            )
        )),
    }
}

/// 从 GitHub Releases 列表取最高的非预览 tag。
#[cfg(windows)]
fn github_latest_stable() -> Result<String, String> {
    let response = runtime::client()
        .get("https://api.github.com/repos/PowerShell/PowerShell/releases?per_page=30")
        .header("User-Agent", "DSHDesktop")
        .header("Accept", "application/vnd.github+json")
        .call()
        .map_err(|e| format!("GitHub API: {e}"))?;
    let json: serde_json::Value = response
        .into_body()
        .read_json()
        .map_err(|e| format!("GitHub API: {e}"))?;
    let entries = json
        .as_array()
        .ok_or_else(|| "GitHub API: releases 格式错误".to_string())?;
    let mut best: Option<semver::Version> = None;
    let mut best_tag = String::new();
    for entry in entries {
        if entry
            .get("prerelease")
            .and_then(|value| value.as_bool())
            .unwrap_or(false)
            || entry
                .get("draft")
                .and_then(|value| value.as_bool())
                .unwrap_or(false)
        {
            continue;
        }
        let Some(tag) = entry.get("tag_name").and_then(|value| value.as_str()) else {
            continue;
        };
        let Ok(version) = semver::Version::parse(tag.trim_start_matches('v')) else {
            continue;
        };
        let newer = best
            .as_ref()
            .is_none_or(|current| version.cmp_precedence(current) == std::cmp::Ordering::Greater);
        if newer {
            best = Some(version);
            best_tag = tag.trim_start_matches('v').to_string();
        }
    }
    best.map(|_| best_tag)
        .ok_or_else(|| "GitHub API: 未找到稳定版本".to_string())
}

/// 安装或更新 PowerShell 7（仅 Windows 有意义，其他平台给出明确提示）。
fn update_pwsh(app: &AppHandle) -> Result<(), String> {
    #[cfg(windows)]
    {
        update_pwsh_windows(app)
    }
    #[cfg(not(windows))]
    {
        let _ = app;
        Err(crate::locale::text(
            "PowerShell 更新仅支持 Windows。",
            "PowerShell updates are supported only on Windows.",
        )
        .into())
    }
}

/// 安装或更新 PowerShell 7（通过 winget；机器级安装会弹出 UAC 授权）。
#[cfg(windows)]
fn update_pwsh_windows(app: &AppHandle) -> Result<(), String> {
    // UAC 预告在检查更新弹窗内展示并等待确认（不再用原生消息框——
    // 原生框无法可靠锚定到自绘弹窗上，位置/层级不可控）。
    // 弹窗关闭视为取消。
    let state = app.state::<AppState>();
    state.set_pwsh_confirmed(false);
    state.set_pwsh_pending(true);
    loop {
        if app.state::<AppState>().is_quitting() {
            state.set_pwsh_pending(false);
            return Err(crate::locale::text("应用已退出", "The app has quit").into());
        }
        if state.pwsh_confirmed() {
            break;
        }
        if !state.pwsh_pending() {
            return Err(crate::locale::text("已取消", "Cancelled").into());
        }
        std::thread::sleep(std::time::Duration::from_millis(200));
    }

    // 前置：确认 winget（微软应用安装程序）可用
    let (code, _out, err) = processes::run_capture(
        std::path::Path::new("winget"),
        &["--version".to_string()],
        &[],
        None,
    )
    .map_err(|e| {
        crate::locale::owned(
            format!("运行 winget 失败：{e}"),
            format!("Failed to run winget: {e}"),
        )
    })?;
    if code != 0 {
        let detail = crate::util::truncate(&err, 300);
        return Err(crate::locale::owned(
            format!(
                "未找到 winget（微软应用安装程序）。\n请到微软官网下载 PowerShell 7 安装包手动安装。\n{detail}"
            ),
            format!(
                "winget (App Installer) was not found.\nDownload and install PowerShell 7 manually from Microsoft.\n{detail}"
            ),
        ));
    }

    let installed = pwsh_version().is_some();
    let action = if installed {
        crate::locale::text("更新", "Update")
    } else {
        crate::locale::text("安装", "Install")
    };
    let verb = if installed { "upgrade" } else { "install" };
    let progress = if installed {
        crate::locale::text("正在更新 PowerShell…", "Updating PowerShell…")
    } else {
        crate::locale::text("正在安装 PowerShell…", "Installing PowerShell…")
    };
    emit_progress(app, progress);
    let args = vec![
        verb.into(),
        "--id".into(),
        "Microsoft.PowerShell".into(),
        "--exact".into(),
        "--silent".into(),
        "--accept-package-agreements".into(),
        "--accept-source-agreements".into(),
    ];
    let (code, _out, err) =
        processes::run_capture(std::path::Path::new("winget"), &args, &[], None).map_err(|e| {
            crate::locale::owned(
                format!("运行 winget 失败：{e}"),
                format!("Failed to run winget: {e}"),
            )
        })?;
    if code != 0 {
        let detail = crate::util::truncate(&err, 400);
        return Err(if crate::locale::is_chinese() {
            format!("{action} PowerShell 失败（winget 退出码 {code}）：\n{detail}")
        } else {
            format!("PowerShell {action} failed (winget exit code {code}):\n{detail}")
        });
    }
    match pwsh_version() {
        Some(v) => {
            crate::logging::log(&format!("updater: PowerShell 就绪 v{v}"));
            Ok(())
        }
        None => Err(crate::locale::text(
            "winget 报告成功，但尚未检测到 pwsh，请稍后重试或重新打开 PowerShell 确认。",
            "winget reported success, but pwsh was not detected. Please retry later or reopen PowerShell to confirm.",
        )
        .into()),
    }
}

// ---------- 应用更新 ----------

/// 应用更新（which: "dsh" | "node" | "pwsh"）。
pub fn apply(app: &AppHandle, which: &str) -> Result<(), String> {
    let state = app.state::<AppState>();
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

/// 重启服务并进入界面（托盘“重启服务”/更新后复用）。
/// 持有生命周期锁，与 boot_once 互斥，杜绝双服务并发。
pub(crate) fn restart_service(app: &AppHandle) -> Result<(), String> {
    let state = app.state::<AppState>();
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
fn restart_service_locked(app: &AppHandle) -> Result<(), String> {
    let state = app.state::<AppState>();
    let config = state.config();
    let restarting = crate::locale::text("正在重启服务…", "Restarting the service…");
    state.set_phase(BootPhase::Starting, restarting, "");
    emit_status(app, BootPhase::Starting, restarting, "");
    let result = (|| -> Result<(), String> {
        // 先停掉残留进程
        dsh::shutdown(app);
        std::thread::sleep(Duration::from_millis(800));
        let node_exe = runtime::ensure_node(app, &config)?;
        let (pid, job) = runtime::start_server(app, &config, &node_exe)?;
        state.set_running(pid, job);
        if !dsh::wait_ready(config.port, Duration::from_secs(60)) {
            processes::kill_tree(pid);
            return Err(crate::locale::text(
                "重启后服务未就绪",
                "The service did not become ready after restarting",
            )
            .into());
        }
        Ok(())
    })();
    match &result {
        Ok(()) => {
            let ready = crate::locale::text("已就绪", "Ready");
            state.set_phase(BootPhase::Ready, ready, "");
            emit_status(app, BootPhase::Ready, ready, "");
            // 唤醒可能阻塞在错误页等待的 boot_loop，让其重入引导（复用本服务）进入看门狗
            state.signal_retry();
            // 给启动页淡出动画留余量，再跳转（与 boot_once 一致，无白闪）
            std::thread::sleep(Duration::from_millis(320));
            navigate(app, &config.web_url());
        }
        Err(msg) => {
            state.set_phase(BootPhase::Error, msg, "");
            emit_status(app, BootPhase::Error, msg, "");
            // 用户此刻可能在 dsh 界面：导航回启动页让错误与重试按钮可见
            navigate(app, SPLASH_ORIGIN);
        }
    }
    result
}

// ---------- 应用自身更新（Windows：下载 → 替换 → 重启） ----------

/// 更新应用本体 exe：下载 GitHub Releases 的 DSHDesktop.exe → 基础校验 →
/// 用户确认 → 写替换脚本并退出（脚本在进程退出后替换并重启新版本）。
///
/// 仅 Windows 支持（单文件分发场景）；macOS/Linux 提示从官网下载。
fn update_app_exe(app: &AppHandle, config: &crate::app_state::Config) -> Result<(), String> {
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
        const ASSET_URL: &str =
            "https://github.com/JeffioZ/dsh-desktop/releases/latest/download/DSHDesktop.exe";
        let dir = config.root.join("exe-update");
        std::fs::create_dir_all(&dir).map_err(|e| format!("创建更新目录失败：{e}"))?;
        let target = dir.join("DSHDesktop.exe");

        // 1) 下载（流式写盘，1 小时整体预算，与 Node 归档下载同一客户端）
        emit_progress(
            app,
            crate::locale::text("正在下载应用更新…", "Downloading the app update…"),
        );
        let resp = runtime::download_client()
            .get(ASSET_URL)
            .header("User-Agent", "DSHDesktop")
            .call()
            .map_err(|e| format!("下载失败：{e}"))?;
        // 单文件 exe 上限 512MB：防止异常响应/恶意源写满磁盘
        const MAX_APP_EXE_BYTES: u64 = 512 * 1024 * 1024;
        let mut reader = resp.into_body().into_reader().take(MAX_APP_EXE_BYTES + 1);
        let mut file = std::fs::File::create(&target).map_err(|e| format!("写入失败：{e}"))?;
        let copied = std::io::copy(&mut reader, &mut file).map_err(|e| format!("下载中断：{e}"))?;
        if copied > MAX_APP_EXE_BYTES {
            return Err(crate::locale::text(
                "下载内容超出预期大小，已取消更新。",
                "The downloaded content exceeds the expected size. Update cancelled.",
            )
            .into());
        }

        // 2) 基础校验：PE 头（MZ）+ 合理体积。HTML 错误页/截断文件在此被拦截
        let bytes = std::fs::read(&target).map_err(|e| format!("读取下载文件失败：{e}"))?;
        if bytes.len() < 1024 * 1024 {
            return Err(crate::locale::text(
                "下载的文件大小异常，已取消更新。",
                "The downloaded file size looks wrong. Update cancelled.",
            )
            .into());
        }
        if bytes.get(0..2) != Some(b"MZ") {
            return Err(crate::locale::text(
                "下载的文件不是有效的程序，已取消更新。",
                "The downloaded file is not a valid program. Update cancelled.",
            )
            .into());
        }
        // 3) 确认：更新需要退出并自动重启
        use tauri_plugin_dialog::MessageDialogKind;
        if !crate::dialog::ask(
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

        // 4) 写替换脚本（进程退出后：等锁释放 → 备份当前 exe → 复制新版 → 启动；
        //    复制失败自动把备份移回，避免 exe 被移走留下损坏安装）
        let exe = std::env::current_exe().map_err(|e| format!("无法定位当前程序路径：{e}"))?;
        let backup = dir.join("DSHDesktop.exe.old");
        let script = dir.join("replace.ps1");
        // PowerShell 单引号字符串内转义：' → ''
        let ps_quote = |s: &std::path::Path| s.to_string_lossy().replace('\'', "''");
        let script_text = format!(
            "$ErrorActionPreference = 'Continue'\n\
             Start-Sleep -Seconds 2\n\
             $src = '{}'\n\
             $dst = '{}'\n\
             $old = '{}'\n\
             $i = 0\n\
             while ($i -lt 60) {{ try {{ Move-Item -LiteralPath $dst -Destination $old -Force -ErrorAction Stop; break }} catch {{ Start-Sleep -Milliseconds 500; $i++ }} }}\n\
             $copied = $false\n\
             $i = 0\n\
             while ($i -lt 60) {{ try {{ Copy-Item -LiteralPath $src -Destination $dst -Force -ErrorAction Stop; $copied = $true; break }} catch {{ Start-Sleep -Milliseconds 500; $i++ }} }}\n\
             if (-not $copied) {{ $j = 0; while ($j -lt 60) {{ try {{ Move-Item -LiteralPath $old -Destination $dst -Force -ErrorAction Stop; break }} catch {{ Start-Sleep -Milliseconds 500; $j++ }} }} }}\n\
             Start-Process -FilePath $dst\n",
            ps_quote(&target),
            ps_quote(&exe),
            ps_quote(&backup),
        );
        std::fs::write(&script, script_text).map_err(|e| format!("写入替换脚本失败：{e}"))?;

        // 5) 启动替换脚本（隐藏、独立于本进程），保存窗口状态后退出
        let spawn = std::process::Command::new("powershell")
            .args([
                "-NoProfile",
                "-WindowStyle",
                "Hidden",
                "-ExecutionPolicy",
                "Bypass",
                "-File",
            ])
            .arg(&script)
            .spawn();
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
}

// ---------- dsh 更新 ----------

/// 更新 dsh：停服务 → 备份 → npm 更新 → 重启（失败回滚）。
fn update_dsh(app: &AppHandle, config: &crate::app_state::Config) -> Result<(), String> {
    // 所有前置条件先检查完，再停止当前可用服务。
    let node_exe = if config.node_exe().exists() {
        config.node_exe()
    } else {
        runtime::find_system_node()
            .ok_or_else(|| crate::locale::text("未找到 Node.js", "Node.js was not found"))?
    };
    let npm_cli = node_exe
        .parent()
        .ok_or_else(|| {
            crate::locale::text(
                "Node.js 可执行文件路径无父目录",
                "The Node.js executable path has no parent directory",
            )
        })?
        .join("node_modules/npm/bin/npm-cli.js");
    if !npm_cli.exists() {
        return Err(crate::locale::text("未找到 npm", "npm was not found").into());
    }

    let current = config.dsh_dir();
    let backup = config.root.join(update_txn::DSH_BACKUP_DIR);
    let marker = config.root.join(update_txn::DSH_UPDATE_MARKER);
    if backup.exists() || marker.exists() {
        return Err(crate::locale::owned(
            format!(
                "检测到未完成的 dsh 更新，请重启应用后重试：{}",
                backup.display()
            ),
            format!(
                "An unfinished dsh update was found. Restart the app before trying again: {}",
                backup.display()
            ),
        ));
    }
    if !current.exists() {
        return Err(crate::locale::text(
            "未找到当前 dsh 安装目录",
            "The current dsh installation directory was not found",
        )
        .into());
    }

    emit_progress(
        app,
        crate::locale::text("正在停止 dsh 服务…", "Stopping the dsh service…"),
    );
    update_txn::create_marker(&marker)?;
    dsh::shutdown(app);
    navigate(app, SPLASH_ORIGIN);
    std::thread::sleep(Duration::from_millis(800));
    if let Err(e) = std::fs::rename(&current, &backup) {
        update_txn::remove_marker(&marker);
        let _ = restart_service_locked(app);
        return Err(crate::locale::owned(
            format!("备份当前 dsh 失败：{e}"),
            format!("Failed to back up the current dsh installation: {e}"),
        ));
    }

    emit_progress(
        app,
        crate::locale::text("正在更新 dsh 包…", "Updating the dsh package…"),
    );
    let args = vec![
        npm_cli.to_string_lossy().into_owned(),
        "install".into(),
        "--prefix".into(),
        config.dsh_dir().to_string_lossy().into_owned(),
        "@deepseek-ai/dsh@latest".into(),
        "--dangerously-allow-all-scripts".into(),
        "--no-audit".into(),
        "--no-fund".into(),
    ];
    let envs = base_envs(&node_exe, config);
    let install_result = (|| -> Result<(), String> {
        let (code, _out, err) = processes::run_capture(&node_exe, &args, &envs, Some(&config.root))
            .map_err(|e| {
                crate::locale::owned(
                    format!("运行 npm 失败：{e}"),
                    format!("Failed to run npm: {e}"),
                )
            })?;
        if code != 0 {
            let detail = crate::util::truncate(&err, 600);
            return Err(crate::locale::owned(
                format!("更新 dsh 失败（npm 退出码 {code}）：\n{detail}"),
                format!("Failed to update dsh (npm exit code {code}):\n{detail}"),
            ));
        }
        if !config.dsh_entry().exists() {
            return Err(crate::locale::text(
                "更新完成但未找到 dsh 入口文件",
                "The update completed, but the dsh entry file was not found",
            )
            .into());
        }
        Ok(())
    })();
    if let Err(e) = install_result {
        if let Err(re) = update_txn::rollback_directory(&current, &backup, &marker) {
            return Err(crate::locale::owned(
                format!("{e}；旧版本自动恢复失败：{re}。\n下次启动将自动还原旧版本。"),
                format!(
                    "{e}; automatic rollback failed: {re}.\nThe previous version will be restored on the next launch."
                ),
            ));
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

    emit_progress(
        app,
        crate::locale::text(
            "更新完成，正在重启服务…",
            "Update complete. Restarting the service…",
        ),
    );
    if let Err(e) = restart_service_locked(app) {
        dsh::shutdown(app);
        if let Err(re) = update_txn::rollback_directory(&current, &backup, &marker) {
            return Err(crate::locale::owned(
                format!(
                    "新版本启动失败：{e}；旧版本自动恢复也失败：{re}。\n\
                     服务已停止，下次启动将自动还原旧版本。"
                ),
                format!(
                    "The new version did not start: {e}; automatic rollback also failed: {re}.\n\
                     The service is stopped; the previous version will be restored on the next launch."
                ),
            ));
        }
        let restore_result = restart_service_locked(app);
        return match restore_result {
            Ok(()) => Err(crate::locale::owned(
                format!("新版本启动失败，已恢复旧版本：{e}"),
                format!("The new version did not start; the previous version was restored: {e}"),
            )),
            Err(re) => Err(crate::locale::owned(
                format!("新版本启动失败：{e}；旧版本恢复后也未能启动：{re}"),
                format!(
                    "The new version did not start: {e}; the restored version also failed to start: {re}"
                ),
            )),
        };
    }
    std::fs::remove_file(&marker).map_err(|e| {
        crate::locale::owned(
            format!("提交 dsh 更新状态失败：{e}"),
            format!("Failed to commit the dsh update state: {e}"),
        )
    })?;
    if let Err(e) = std::fs::remove_dir_all(&backup) {
        crate::logging::log(&format!(
            "updater: 清理 dsh 备份失败（不影响当前版本）：{e}"
        ));
    }
    Ok(())
}

// ---------- Node 更新 ----------

/// 更新 Node：停服务 → 下载新版便携 Node → 换目录 → 重启（失败回滚）。
fn update_node(app: &AppHandle, config: &crate::app_state::Config) -> Result<(), String> {
    if !config.node_exe().exists() {
        return Err(crate::locale::text(
            "当前使用的是系统 Node.js，应用不会自动更新它。",
            "The app is using the system Node.js installation and will not update it automatically.",
        )
        .into());
    }
    let old = config.node_dir();
    let backup = config.root.join(update_txn::NODE_BACKUP_DIR);
    let marker = config.root.join(update_txn::NODE_UPDATE_MARKER);
    if backup.exists() || marker.exists() {
        return Err(crate::locale::owned(
            format!(
                "检测到未完成的 Node 更新，请重启应用后重试：{}",
                backup.display()
            ),
            format!(
                "An unfinished Node.js update was found. Restart the app before trying again: {}",
                backup.display()
            ),
        ));
    }

    emit_progress(
        app,
        crate::locale::text("正在停止 dsh 服务…", "Stopping the dsh service…"),
    );
    update_txn::create_marker(&marker)?;
    dsh::shutdown(app);
    navigate(app, SPLASH_ORIGIN);
    std::thread::sleep(Duration::from_millis(800));
    if old.exists() {
        if let Err(e) = std::fs::rename(&old, &backup) {
            update_txn::remove_marker(&marker);
            let _ = restart_service_locked(app);
            return Err(crate::locale::owned(
                format!("备份当前 Node 失败：{e}"),
                format!("Failed to back up the current Node.js installation: {e}"),
            ));
        }
    }
    let result = (|| -> Result<(), String> {
        let exe = runtime::install_portable_node(app, config)?;
        let _ = exe;
        Ok(())
    })();
    if let Err(e) = result {
        if let Err(re) = update_txn::rollback_directory(&old, &backup, &marker) {
            return Err(crate::locale::owned(
                format!(
                    "{e}；旧 Node 自动恢复失败：{re}。\n\
                     已保留备份和事务标记，下次启动将再次恢复。"
                ),
                format!(
                    "{e}; automatic Node.js rollback failed: {re}.\n\
                     The backup and transaction marker were kept for recovery on the next launch."
                ),
            ));
        }
        return match restart_service_locked(app) {
            Ok(()) => Err(crate::locale::owned(
                format!("{e}；已恢复旧 Node"),
                format!("{e}; the previous Node.js version was restored"),
            )),
            Err(re) => Err(crate::locale::owned(
                format!("{e}；旧 Node 恢复后未能启动：{re}"),
                format!("{e}; the restored Node.js version did not start: {re}"),
            )),
        };
    }

    emit_progress(
        app,
        crate::locale::text(
            "Node 更新完成，正在重启服务…",
            "Node.js update complete. Restarting the service…",
        ),
    );
    if let Err(e) = restart_service_locked(app) {
        dsh::shutdown(app);
        if let Err(re) = update_txn::rollback_directory(&old, &backup, &marker) {
            return Err(crate::locale::owned(
                format!(
                    "新 Node 启动失败：{e}；旧版本自动恢复失败：{re}。\n\
                     服务已停止，下次启动将自动还原旧版本。"
                ),
                format!(
                    "The new Node.js version did not start: {e}; automatic rollback failed: {re}.\n\
                     The service is stopped; the previous version will be restored on the next launch."
                ),
            ));
        }
        let restore_result = restart_service_locked(app);
        return match restore_result {
            Ok(()) => Err(crate::locale::owned(
                format!("新 Node 启动失败，已恢复旧版本：{e}"),
                format!(
                    "The new Node.js version did not start; the previous version was restored: {e}"
                ),
            )),
            Err(re) => Err(crate::locale::owned(
                format!("新 Node 启动失败：{e}；旧版本恢复后也未能启动：{re}"),
                format!(
                    "The new Node.js version did not start: {e}; the restored version also failed to start: {re}"
                ),
            )),
        };
    }
    std::fs::remove_file(&marker).map_err(|e| {
        crate::locale::owned(
            format!("提交 Node 更新状态失败：{e}"),
            format!("Failed to commit the Node.js update state: {e}"),
        )
    })?;
    if backup.exists() {
        if let Err(e) = std::fs::remove_dir_all(&backup) {
            crate::logging::log(&format!(
                "updater: 清理 Node 备份失败（不影响当前版本）：{e}"
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::parse_pwsh_metadata;

    #[test]
    fn parses_official_powershell_stable_tag() {
        let metadata = serde_json::json!({ "StableReleaseTag": "v7.6.4" });
        assert_eq!(parse_pwsh_metadata(&metadata).unwrap(), "7.6.4");
    }
}
