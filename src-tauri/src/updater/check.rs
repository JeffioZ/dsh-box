//! 更新查询、周期检查与结果 DTO。

use super::powershell::parse_releases_atom;
use super::*;
use std::io::Read;

#[derive(Serialize, Clone, Default)]
pub struct CheckResult {
    pub dsh: Option<VersionInfo>,
    pub node: Option<NodeInfo>,
    pub pwsh: Option<PwshInfo>,
    /// npm 版本（Node 自带，可由用户单独维护）。
    pub npm: Option<VersionInfo>,
    /// 应用自身更新（GitHub Releases）。
    pub app: Option<VersionInfo>,
    pub error: Option<String>,
}

#[derive(Serialize, Clone)]
pub struct VersionInfo {
    pub installed: String,
    pub latest: String,
    pub update_available: bool,
    /// 版本查询失败原因（前端 hover tips 展示，与其他更新行统一）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latest_error: Option<String>,
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
        // 应用自身新版：后台预下载（下载完成后再提示重启）
        if let Some(app_info) = &result.app {
            if app_info.update_available {
                prefetch_app_update(&app);
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
    if crate::native_dialog::ask(
        app,
        msg,
        crate::locale::text("发现新版本", "Update available"),
        MessageDialogKind::Info,
        crate::locale::text("立即更新", "Update now"),
        crate::locale::text("稍后", "Later"),
    ) {
        // 打开弹窗作为进度载体：update-progress / update-result 事件在
        // check 页实时呈现，用户全程可见（下载→安装→重启服务），
        // 更新仍在后台线程执行，不阻塞界面
        crate::control_center::open_update_progress(app);
        let handle = app.clone();
        std::thread::spawn(move || match apply(&handle, "dsh") {
            Ok(()) => {
                // 成功后重查一次：弹窗显示新版本状态（已是最新）。
                // 结果同时写入状态（轮询通道）与事件（即时渲染）：
                // 事件早于弹窗页面就绪时被丢弃，轮询兜底保证不滞留
                emit_progress(
                    &handle,
                    crate::locale::text("正在确认新版本…", "Verifying the new version…"),
                );
                let result = check(&handle);
                let done_msg = crate::locale::text("dsh 更新完成。", "dsh was updated.");
                handle
                    .state::<AppState>()
                    .set_update_done(true, Some(done_msg.into()));
                handle
                    .state::<AppState>()
                    .set_last_check(Some(result.clone()));
                handle.state::<AppState>().set_check_progress(None);
                let _ = handle.emit("update-result", &result);
            }
            Err(e) => {
                let result = CheckResult {
                    error: Some(e.clone()),
                    ..CheckResult::default()
                };
                handle
                    .state::<AppState>()
                    .set_update_done(false, Some(e.clone()));
                handle
                    .state::<AppState>()
                    .set_last_check(Some(result.clone()));
                handle.state::<AppState>().set_check_progress(None);
                let _ = handle.emit("update-result", &result);
                // 弹窗未显示时才弹 win32 兜底（弹窗开着已显示失败原因，
                // 不再重复打扰）
                if !crate::control_center::is_check_open(&handle) {
                    crate::native_dialog::show_message(
                        &handle,
                        format!("{}: {e}", crate::locale::text("更新失败", "Update failed")),
                        crate::locale::text("更新", "Update"),
                        MessageDialogKind::Warning,
                    );
                }
            }
        });
    }
}

/// 查询 dsh 与 Node 是否有可用更新。
pub fn check(app: &AppHandle) -> CheckResult {
    let state = app.state::<AppState>();
    let external_service = state.service_ownership().is_external();
    let config = state.config();
    let mut result = CheckResult::default();

    if external_service {
        // 外部模式只检查 DSHBox 自身与可选系统工具；本地 dsh/Node/npm 既不
        // 代表当前连接，也不可从此处更新，连版本网络请求都不应发起。
        let app_handle = std::thread::spawn(check_app_update);
        #[cfg(windows)]
        let pwsh_handle = std::thread::spawn(check_pwsh_info);
        if let Ok(app_info) = app_handle.join() {
            result.app = app_info;
        }
        #[cfg(windows)]
        if let Ok(info) = pwsh_handle.join() {
            result.pwsh = Some(info);
        }
        return result;
    }

    // 三个独立检测（npm HTTP / Node 检测 + LTS HTTP / GitHub HTTP）并行执行，
    // 检查弹窗等待时间从“三者之和”缩短为“最慢者”。
    let dsh_cfg = config.clone();
    let dsh_handle = std::thread::spawn(move || match runtime::installed_dsh_version(&dsh_cfg) {
        Some(installed) => {
            let channel = runtime::DshChannel::from_config(&dsh_cfg);
            match runtime::npm_latest_dsh_version(channel) {
                Ok(latest) => (
                    Some(VersionInfo {
                        installed: installed.clone(),
                        latest: latest.clone(),
                        update_available: versions::compare_versions(&latest, &installed)
                            == std::cmp::Ordering::Greater,
                        latest_error: None,
                    }),
                    None,
                ),
                Err(e) => {
                    // 查询失败仍保留行：前端显示"暂无法获取版本信息"，
                    // hover 经 data-tip-extra 展示原因（与 node/pwsh 行统一）
                    let error = format!(
                        "{}: {e}",
                        crate::locale::text(
                            "查询 dsh 最新版本失败",
                            "Failed to query the latest dsh version"
                        )
                    );
                    (
                        Some(VersionInfo {
                            installed: installed.clone(),
                            latest: String::new(),
                            update_available: false,
                            latest_error: Some(error),
                        }),
                        None,
                    )
                }
            }
        }
        None => (
            None,
            Some(crate::locale::text("未检测到已安装的 dsh", "No installed dsh was found").into()),
        ),
    });

    // node：检测“当前实际使用的 Node”（DSHBox 便携优先，其次系统安装的 Node）。
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
    let pwsh_handle = std::thread::spawn(check_pwsh_info);
    // 应用自身更新（GitHub Releases；失败静默，不阻塞其他检查）
    let app_handle = std::thread::spawn(check_app_update);

    // npm 版本（Node 自带）：检测走 registry dist-tags latest，独立线程并行
    // （与 dsh/node/pwsh/app 一致）。dsh 本体安装已由自管 pnpm 负责。
    let npm_cfg = config.clone();
    let npm_handle = std::thread::spawn(move || {
        let installed = runtime::npm_version(&npm_cfg);
        match runtime::npm_latest_version() {
            Ok(latest) => {
                let update_available = installed
                    .as_deref()
                    .map(|cur| {
                        versions::compare_versions(&latest, cur) == std::cmp::Ordering::Greater
                    })
                    .unwrap_or(false);
                (
                    Some(VersionInfo {
                        installed: installed.unwrap_or_default(),
                        latest: latest.clone(),
                        update_available,
                        latest_error: None,
                    }),
                    None,
                )
            }
            Err(e) => {
                crate::logging::log(&format!("updater: npm 最新版本查询失败：{e}"));
                // 检查失败仍显示已装版本（若无则整行不显示，与 dsh 策略一致）
                if installed.is_some() {
                    (
                        Some(VersionInfo {
                            installed: installed.unwrap_or_default(),
                            latest: String::new(),
                            update_available: false,
                            latest_error: Some(format!(
                                "{}: {e}",
                                crate::locale::text(
                                    "查询 npm 最新版本失败",
                                    "Failed to query the latest npm version"
                                )
                            )),
                        }),
                        None,
                    )
                } else {
                    (None, None)
                }
            }
        }
    });

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
    if let Ok(info) = pwsh_handle.join() {
        result.pwsh = Some(info);
    }
    if let Ok(app_info) = app_handle.join() {
        result.app = app_info;
    }
    if let Ok((npm_info, npm_err)) = npm_handle.join() {
        result.npm = npm_info;
        if result.error.is_none() {
            result.error = npm_err;
        }
    }
    result
}

#[cfg(windows)]
fn check_pwsh_info() -> PwshInfo {
    let installed = pwsh_version();
    match latest_pwsh_version() {
        Ok(latest) => PwshInfo {
            installed: installed.clone(),
            latest: Some(latest.clone()),
            latest_error: None,
            update_available: installed
                .as_ref()
                .map(|current| {
                    versions::compare_versions(&latest, current) == std::cmp::Ordering::Greater
                })
                .unwrap_or(true),
        },
        Err(error) => {
            crate::logging::log(&format!("updater: PowerShell 最新版本查询失败：{error}"));
            PwshInfo {
                installed,
                latest: None,
                latest_error: Some(error),
                update_available: false,
            }
        }
    }
}

/// 应用自身更新检查：GitHub Releases latest 的版本号对比。
/// 检查失败（网络/仓库不存在/无 Release）静默返回 None，不打扰用户。
pub(super) fn check_app_update() -> Option<VersionInfo> {
    // 通过 releases.atom 页面（而非 api.github.com）查询，绕开未认证 API
    // 的 60 次/小时/IP 限流（本机实测 403）
    let url = format!("https://github.com/{APP_REPO}/releases.atom");
    // 查询失败仍返回带错误信息的行（前端显示"暂无法获取版本信息"，
    // hover tips 展示原因，与其他更新行统一）
    let fail = |e: String| {
        crate::logging::log(&format!("updater: 应用版本查询失败：{e}"));
        Some(VersionInfo {
            installed: env!("CARGO_PKG_VERSION").to_string(),
            latest: String::new(),
            update_available: false,
            latest_error: Some(crate::locale::owned(
                format!("查询应用最新版本失败：{e}"),
                format!("Failed to query the latest app version: {e}"),
            )),
        })
    };
    let resp = match runtime::check_client()
        .get(&url)
        .header("User-Agent", "DSHBox")
        .call()
    {
        Ok(r) => r,
        Err(e) => return fail(format!("{e}")),
    };
    let mut text = String::new();
    if resp
        .into_body()
        .into_reader()
        .read_to_string(&mut text)
        .is_err()
    {
        return fail("读取响应失败".into());
    }
    let tags = parse_releases_atom(&text);
    // 过滤 prerelease tag（-rc/-preview/-beta/-alpha）：atom 首个 entry 是
    // 最近发布，可能未切 latest 的 rc 版，会误报"有更新"
    let Some(latest) = tags
        .iter()
        .map(|t| t.trim_start_matches('v').to_string())
        .find(|t| {
            !t.contains("-rc")
                && !t.contains("-preview")
                && !t.contains("-beta")
                && !t.contains("-alpha")
        })
    else {
        return fail("更新源中未找到稳定发布版本".into());
    };
    let installed = env!("CARGO_PKG_VERSION").to_string();
    let update_available =
        versions::compare_versions(&latest, &installed) == std::cmp::Ordering::Greater;
    Some(VersionInfo {
        installed,
        latest,
        update_available,
        latest_error: None,
    })
}
