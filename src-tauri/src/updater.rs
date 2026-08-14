//! 一键更新：检查/更新 dsh（npm 包）、Node（便携版），以及 Windows 的 PowerShell 7（可选增强）。

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
    pub update_available: bool,
}

#[derive(Serialize, Clone)]
pub struct PwshInfo {
    pub installed: Option<String>,
    pub latest: Option<String>,
    /// true 表示需要操作（未安装或存在新版）。
    pub update_available: bool,
}

fn emit_progress(app: &AppHandle, message: &str) {
    let _ = app.emit("update-progress", serde_json::json!({ "message": message }));
}

// ---------- 检查 ----------

/// 检查并汇报（托盘/启动页共用）。
pub fn check_and_report(app: &AppHandle) -> Result<(), String> {
    emit_progress(app, "正在检查更新…");
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
                        "已最新"
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

/// 有新版时的启动提示（不自动安装，用户确认才更新）。
fn show_update_dialog(app: &AppHandle, d: &VersionInfo) {
    use tauri_plugin_dialog::MessageDialogKind;
    let msg = format!(
        "dsh 有新版本可用：\n{}（当前 {}）\n\n是否立即更新？",
        d.latest, d.installed
    );
    if crate::dialog::ask(
        app,
        msg,
        "发现新版本",
        MessageDialogKind::Info,
        "立即更新",
        "稍后",
    ) {
        if let Err(e) = apply(app, "dsh") {
            crate::dialog::show_message(
                app,
                format!("更新失败：{e}"),
                "更新",
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
            Err(e) => (None, Some(format!("查询 dsh 最新版本失败：{e}"))),
        },
        None => (None, Some("未找到已安装的 dsh".into())),
    });

    // node：检测“当前实际使用的 Node”（DSHDesktop 便携优先，其次系统安装的 Node）。
    let node_cfg = config.clone();
    let node_handle = std::thread::spawn(move || {
        let managed = node_cfg.node_exe().exists();
        let installed = runtime::current_node_version(&node_cfg);
        let latest_lts = runtime::latest_lts().ok();
        let update_available = managed
            && latest_lts.is_some()
            && installed
                .as_deref()
                .map(|cur| {
                    versions::compare_versions(latest_lts.as_deref().unwrap_or(""), cur)
                        == std::cmp::Ordering::Greater
                })
                .unwrap_or(false);
        (managed, installed, latest_lts, update_available)
    });

    // PowerShell 7（可选增强，仅 Windows——macOS/Linux 有各自的系统终端）。
    #[cfg(windows)]
    let pwsh_handle = std::thread::spawn(|| {
        let installed = pwsh_version();
        match latest_pwsh_version() {
            Ok(latest) => (
                installed.clone(),
                Some(latest.clone()),
                match &installed {
                    Some(cur) => {
                        versions::compare_versions(&latest, cur) == std::cmp::Ordering::Greater
                    }
                    None => true,
                },
            ),
            Err(_) => (installed, None, false),
        }
    });

    if let Ok((d, d_err)) = dsh_handle.join() {
        result.dsh = d;
        if result.error.is_none() {
            result.error = d_err;
        }
    }
    if let Ok((managed, installed, latest_lts, update_available)) = node_handle.join() {
        result.node = Some(NodeInfo {
            managed,
            installed,
            latest_lts,
            update_available,
        });
    }
    #[cfg(windows)]
    if let Ok((installed, latest, update_available)) = pwsh_handle.join() {
        result.pwsh = Some(PwshInfo {
            installed,
            latest,
            update_available,
        });
    }
    result
}

// ---------- PowerShell 7（可选增强，仅 Windows） ----------

/// 检测已安装的 PowerShell 7 版本（未安装返回 None）。
#[cfg(windows)]
fn pwsh_version() -> Option<String> {
    let mut cmd = std::process::Command::new("pwsh");
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

/// 查询 PowerShell 官方最新稳定版（GitHub Releases）。
#[cfg(windows)]
fn latest_pwsh_version() -> Result<String, String> {
    let resp = runtime::client()
        .get("https://api.github.com/repos/PowerShell/PowerShell/releases/latest")
        .set("User-Agent", "DSHDesktop")
        .set("Accept", "application/vnd.github+json")
        .call()
        .map_err(|e| format!("获取 PowerShell 版本信息失败：{e}"))?;
    let json: serde_json::Value = resp
        .into_json()
        .map_err(|e| format!("解析 PowerShell 版本信息失败：{e}"))?;
    json.get("tag_name")
        .and_then(|v| v.as_str())
        .map(|s| s.trim_start_matches('v').to_string())
        .ok_or_else(|| "响应中没有版本号".into())
}

/// 安装或更新 PowerShell 7（仅 Windows 有意义，其他平台给出明确提示）。
fn update_pwsh(app: &AppHandle) -> Result<(), String> {
    #[cfg(windows)]
    {
        update_pwsh_windows(app)
    }
    #[cfg(not(windows))]
    {
        Err("PowerShell 仅支持在 Windows 上安装更新；macOS/Linux 请使用系统自带终端。".into())
    }
}

/// 安装或更新 PowerShell 7（通过 winget；机器级安装会弹出 UAC 授权）。
#[cfg(windows)]
fn update_pwsh_windows(app: &AppHandle) -> Result<(), String> {
    use tauri_plugin_dialog::MessageDialogKind;
    crate::dialog::show_message(
        app,
        "安装/更新 PowerShell 需要管理员权限，\n接下来会弹出系统授权提示（UAC），请选择“是”。"
            .into(),
        "PowerShell",
        MessageDialogKind::Info,
    );

    // 前置：确认 winget（微软应用安装程序）可用
    let (code, _out, err) = processes::run_capture(
        std::path::Path::new("winget"),
        &["--version".to_string()],
        &[],
        None,
    )
    .map_err(|e| format!("运行 winget 失败：{e}"))?;
    if code != 0 {
        return Err(format!(
            "未找到 winget（微软应用安装程序）。\n请到微软官网下载 PowerShell 7 安装包手动安装。\n{}",
            crate::util::truncate(&err, 300)
        ));
    }

    let installed = pwsh_version().is_some();
    let action = if installed { "更新" } else { "安装" };
    let verb = if installed { "upgrade" } else { "install" };
    emit_progress(app, &format!("正在{action} PowerShell…"));
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
        processes::run_capture(std::path::Path::new("winget"), &args, &[], None)
            .map_err(|e| format!("运行 winget 失败：{e}"))?;
    if code != 0 {
        return Err(format!(
            "{action} PowerShell 失败（winget 退出码 {code}）：\n{}",
            crate::util::truncate(&err, 400)
        ));
    }
    match pwsh_version() {
        Some(v) => {
            crate::logging::log(&format!("updater: PowerShell 就绪 v{v}"));
            Ok(())
        }
        None => Err("winget 报告成功，但未检测到 pwsh，请打开新的终端确认。".into()),
    }
}

// ---------- 应用更新 ----------

/// 应用更新（which: "dsh" | "node" | "pwsh"）。
pub fn apply(app: &AppHandle, which: &str) -> Result<(), String> {
    let state = app.state::<AppState>();
    if !state.try_begin_update() {
        let msg = "启动或更新流程正在进行，请稍后再试。".to_string();
        emit_progress(app, &format!("更新失败：{msg}"));
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
    } else {
        Err(format!("未知更新目标：{which}"))
    };
    if let Err(msg) = &result {
        // 让启动页/托盘能看到失败原因
        emit_progress(app, &format!("更新失败：{msg}"));
    }
    result
}

// ---------- 重启服务 ----------

/// 重启服务并进入界面（托盘“重启服务”/更新后复用）。
/// 持有生命周期锁，与 boot_once 互斥，杜绝双服务并发。
pub(crate) fn restart_service(app: &AppHandle) -> Result<(), String> {
    let state = app.state::<AppState>();
    if state.is_updating() {
        return Err("更新流程正在进行，请稍后再重启。".into());
    }
    let _guard = state.lifecycle_guard();
    // 覆盖“检查后、拿锁前”更新刚好开始的竞争窗口。
    if state.is_updating() {
        return Err("更新流程正在进行，请稍后再重启。".into());
    }
    restart_service_locked(app)
}

/// 调用方已持有生命周期锁时使用。
fn restart_service_locked(app: &AppHandle) -> Result<(), String> {
    let state = app.state::<AppState>();
    let config = state.config();
    state.set_phase(BootPhase::Starting, "正在重启服务…", "");
    emit_status(app, BootPhase::Starting, "正在重启服务…", "");
    let result = (|| -> Result<(), String> {
        // 先停掉残留进程
        dsh::shutdown(app);
        std::thread::sleep(Duration::from_millis(800));
        let node_exe = if config.node_exe().exists() {
            config.node_exe()
        } else {
            runtime::find_system_node().ok_or("未找到 Node.js")?
        };
        let (pid, job) = runtime::start_server(app, &config, &node_exe)?;
        state.set_running(pid, job);
        if !dsh::wait_ready(config.port, Duration::from_secs(60)) {
            processes::kill_tree(pid);
            return Err("服务重启后未就绪".into());
        }
        Ok(())
    })();
    match &result {
        Ok(()) => {
            state.set_phase(BootPhase::Ready, "已就绪", "");
            emit_status(app, BootPhase::Ready, "已就绪", "");
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

// ---------- dsh 更新 ----------

/// 更新 dsh：停服务 → 备份 → npm 更新 → 重启（失败回滚）。
fn update_dsh(app: &AppHandle, config: &crate::app_state::Config) -> Result<(), String> {
    // 所有前置条件先检查完，再停止当前可用服务。
    let node_exe = if config.node_exe().exists() {
        config.node_exe()
    } else {
        runtime::find_system_node().ok_or("未找到 Node.js")?
    };
    let npm_cli = node_exe
        .parent()
        .unwrap()
        .join("node_modules/npm/bin/npm-cli.js");
    if !npm_cli.exists() {
        return Err("未找到 npm".into());
    }

    let current = config.dsh_dir();
    let backup = config.root.join(update_txn::DSH_BACKUP_DIR);
    let marker = config.root.join(update_txn::DSH_UPDATE_MARKER);
    if backup.exists() || marker.exists() {
        return Err(format!(
            "发现尚未处理的 dsh 更新状态，请重启应用后再试：{}",
            backup.display()
        ));
    }
    if !current.exists() {
        return Err("未找到当前 dsh 安装目录".into());
    }

    emit_progress(app, "正在停止 dsh 服务…");
    update_txn::create_marker(&marker)?;
    dsh::shutdown(app);
    navigate(app, SPLASH_ORIGIN);
    std::thread::sleep(Duration::from_millis(800));
    if let Err(e) = std::fs::rename(&current, &backup) {
        update_txn::remove_marker(&marker);
        let _ = restart_service_locked(app);
        return Err(format!("备份当前 dsh 失败：{e}"));
    }

    emit_progress(app, "正在更新 dsh 包…");
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
            .map_err(|e| format!("运行 npm 失败：{e}"))?;
        if code != 0 {
            return Err(format!(
                "更新 dsh 失败（npm 退出码 {code}）：\n{}",
                crate::util::truncate(&err, 600)
            ));
        }
        if !config.dsh_entry().exists() {
            return Err("更新完成但未找到 dsh 入口文件".into());
        }
        Ok(())
    })();
    if let Err(e) = install_result {
        if let Err(re) = update_txn::restore_directory(&current, &backup) {
            update_txn::remove_marker(&marker);
            return Err(format!(
                "{e}；旧版本自动恢复失败：{re}。\n下次启动将自动还原旧版本。"
            ));
        }
        update_txn::remove_marker(&marker);
        return match restart_service_locked(app) {
            Ok(()) => Err(format!("{e}；已恢复旧版本")),
            Err(re) => Err(format!("{e}；旧版本恢复后未能启动：{re}")),
        };
    }

    emit_progress(app, "更新完成，正在重启服务…");
    if let Err(e) = restart_service_locked(app) {
        dsh::shutdown(app);
        if let Err(re) = update_txn::restore_directory(&current, &backup) {
            update_txn::remove_marker(&marker);
            return Err(format!(
                "新版本启动失败：{e}；旧版本自动恢复也失败：{re}。\n\
                 服务已停止，下次启动将自动还原旧版本。"
            ));
        }
        update_txn::remove_marker(&marker);
        let restore_result = restart_service_locked(app);
        return match restore_result {
            Ok(()) => Err(format!("新版本启动失败，已恢复旧版本：{e}")),
            Err(re) => Err(format!("新版本启动失败：{e}；旧版本恢复后也未能启动：{re}")),
        };
    }
    std::fs::remove_file(&marker).map_err(|e| format!("提交 dsh 更新状态失败：{e}"))?;
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
        return Err("当前使用系统 Node，不自动更新。".into());
    }
    let old = config.node_dir();
    let backup = config.root.join(update_txn::NODE_BACKUP_DIR);
    let marker = config.root.join(update_txn::NODE_UPDATE_MARKER);
    if backup.exists() || marker.exists() {
        return Err(format!(
            "发现尚未处理的 Node 更新状态，请重启应用后再试：{}",
            backup.display()
        ));
    }

    emit_progress(app, "正在停止 dsh 服务…");
    update_txn::create_marker(&marker)?;
    dsh::shutdown(app);
    navigate(app, SPLASH_ORIGIN);
    std::thread::sleep(Duration::from_millis(800));
    if old.exists() {
        if let Err(e) = std::fs::rename(&old, &backup) {
            update_txn::remove_marker(&marker);
            let _ = restart_service_locked(app);
            return Err(format!("备份当前 Node 失败：{e}"));
        }
    }
    let result = (|| -> Result<(), String> {
        let exe = runtime::install_portable_node(app, config)?;
        let _ = exe;
        Ok(())
    })();
    if result.is_err() {
        // 回滚
        if old.exists() {
            std::fs::remove_dir_all(&old).map_err(|e| format!("清理新 Node 安装目录失败：{e}"))?;
        }
        if backup.exists() {
            std::fs::rename(&backup, &old).map_err(|e| format!("恢复旧 Node 失败：{e}"))?;
        }
        update_txn::remove_marker(&marker);
    }
    result?;

    emit_progress(app, "Node 更新完成，正在重启服务…");
    if let Err(e) = restart_service_locked(app) {
        dsh::shutdown(app);
        if let Err(re) = update_txn::restore_directory(&old, &backup) {
            update_txn::remove_marker(&marker);
            return Err(format!(
                "新 Node 启动失败：{e}；旧版本自动恢复失败：{re}。\n\
                 服务已停止，下次启动将自动还原旧版本。"
            ));
        }
        update_txn::remove_marker(&marker);
        let restore_result = restart_service_locked(app);
        return match restore_result {
            Ok(()) => Err(format!("新 Node 启动失败，已恢复旧版本：{e}")),
            Err(re) => Err(format!(
                "新 Node 启动失败：{e}；旧版本恢复后也未能启动：{re}"
            )),
        };
    }
    std::fs::remove_file(&marker).map_err(|e| format!("提交 Node 更新状态失败：{e}"))?;
    if backup.exists() {
        if let Err(e) = std::fs::remove_dir_all(&backup) {
            crate::logging::log(&format!(
                "updater: 清理 Node 备份失败（不影响当前版本）：{e}"
            ));
        }
    }
    Ok(())
}
