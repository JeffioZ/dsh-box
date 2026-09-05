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
    /// 仅 dsh 行使用：目标通道版本低于当前版本（切换通道后的降级/切换入口）。
    /// `update_available` 语义保持"有新版"，静默/周期弹窗不受降级影响。
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub downgrade_available: bool,
    /// 仅 dsh 行使用：其他通道存在更高版本的跨通道发现（仅手动检查页展示，
    /// 不参与 update_available 语义，静默/周期弹窗不触发）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub other_channel: Option<OtherChannelHint>,
}

/// 跨通道发现：当前通道之外某个 dist-tag 指向更高的版本。
#[derive(Serialize, Clone, PartialEq, Debug)]
pub struct OtherChannelHint {
    /// npm dist-tag 名（latest/next/alpha），前端映射为本地化通道名。
    pub channel: String,
    pub version: String,
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

/// dsh 版本行的方向判定：`update_available` 保持"有新版"语义（静默/周期
/// 弹窗据此提示）；目标低于当前时置 `downgrade_available`，仅供检查页
/// 手动展示"切换到 vX.Y.Z"入口。目标等于当前（含两通道指向同一版本的
/// 情形）两个标记都为 false，无事可做。
fn dsh_version_info(installed: String, latest: String) -> VersionInfo {
    let ordering = versions::compare_versions(&latest, &installed);
    VersionInfo {
        update_available: ordering == std::cmp::Ordering::Greater,
        downgrade_available: ordering == std::cmp::Ordering::Less,
        installed,
        latest,
        latest_error: None,
        other_channel: None,
    }
}

/// 跨通道发现：其他 dist-tag 通道存在比"已安装版本"和"当前通道目标"都高
/// 的版本时，返回其中最高者。比"当前通道目标"高才提示——否则当前通道
/// 本身就能装到，切换毫无信息量。只影响手动检查页的展示入口。
fn pick_higher_other_channel(
    installed: &str,
    current: runtime::DshChannel,
    tags: &runtime::DshDistTags,
) -> Option<OtherChannelHint> {
    let current_target = tags.get(current)?;
    let mut best: Option<OtherChannelHint> = None;
    for channel in [
        runtime::DshChannel::Latest,
        runtime::DshChannel::Next,
        runtime::DshChannel::Alpha,
    ] {
        if channel == current {
            continue;
        }
        let Some(version) = tags.get(channel) else {
            continue;
        };
        let higher_than_installed =
            versions::compare_versions(version, installed) == std::cmp::Ordering::Greater;
        let higher_than_current =
            versions::compare_versions(version, current_target) == std::cmp::Ordering::Greater;
        if !(higher_than_installed && higher_than_current) {
            continue;
        }
        let supersedes = best.as_ref().is_none_or(|hint| {
            versions::compare_versions(version, &hint.version) == std::cmp::Ordering::Greater
        });
        if supersedes {
            best = Some(OtherChannelHint {
                channel: channel.dist_tag().to_string(),
                version: version.to_string(),
            });
        }
    }
    best
}

/// 当前通道版本信息 + 跨通道发现（一次 dist-tags 请求同时携带三通道版本）。
fn dsh_version_info_with_tags(
    installed: String,
    current: runtime::DshChannel,
    tags: &runtime::DshDistTags,
) -> Result<VersionInfo, String> {
    let mut info = dsh_version_info(installed, tags.latest_of(current)?);
    info.other_channel = pick_higher_other_channel(&info.installed, current, tags);
    Ok(info)
}

// ---------- 检查 ----------

/// 检查并汇报（托盘/启动页共用）。
pub fn check_and_report(app: &AppHandle) -> Result<(), String> {
    emit_progress(
        app,
        crate::locale::text("正在检查更新…", "Checking for updates…"),
    );
    let result = check(app);
    crate::emit_signed(app, "update-result", &&result);
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
        let mut result = check(&handle);
        // dev 构建：有可更新的真实数据用它；没有（已最新/查询失败/无数据）
        // 则注入模拟，便于开发者验证两个提示弹窗及排队逻辑。正式版
        // dev_build() 恒为 false，不受影响。dev 下模拟 tag（9.9.9-dev）在
        // GitHub 不存在，点「查看更新内容/重启并更新」预期 404/失败，仅验证 UI。
        let mut dsh_simulated = false;
        let mut app_simulated = false;
        if crate::app_state::dev_build() {
            if !result.dsh.as_ref().is_some_and(|d| d.update_available) {
                dsh_simulated = true;
                result.dsh = Some(VersionInfo {
                    installed: "0.9.8".into(),
                    latest: "0.9.9-dev".into(),
                    update_available: true,
                    latest_error: None,
                    downgrade_available: false,
                    other_channel: None,
                });
            }
            if !result.app.as_ref().is_some_and(|a| a.update_available) {
                app_simulated = true;
                result.app = Some(VersionInfo {
                    installed: env!("CARGO_PKG_VERSION").into(),
                    latest: "9.9.9-dev".into(),
                    update_available: true,
                    latest_error: None,
                    downgrade_available: false,
                    other_channel: None,
                });
            }
        }
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
                crate::emit_signed(&handle, "update-result", &result);
                if d.update_available {
                    show_update_dialog(&handle, d, dsh_simulated);
                }
            }
            None => {
                if let Some(e) = &result.error {
                    crate::logging::log(&format!("updater: 静默检查跳过：{e}"));
                }
            }
        }
        // dev 模拟：应用更新就绪同样提示（正式版由 periodic 的 prefetch 触发）
        if crate::app_state::dev_build() {
            if let Some(app_info) = result.app.as_ref().filter(|info| info.update_available) {
                crate::control_center::open_update_prompt(
                    &handle,
                    crate::control_center::UpdatePrompt {
                        kind: "app".into(),
                        version: app_info.latest.clone(),
                        current: None,
                        release_url: Some(format!(
                            "https://github.com/{APP_REPO}/releases/tag/v{}",
                            app_info.latest
                        )),
                        simulated: app_simulated.then_some(true),
                    },
                );
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
                show_update_dialog(&app, d, false);
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

/// 有新版时的启动提示（自绘弹窗：立即更新 / 稍后 / 查看更新内容；与应用提示体验一致）。
/// 「立即更新」由弹窗前端走 app_dialog_update("dsh") → apply_dsh_update。
fn show_update_dialog(app: &AppHandle, d: &VersionInfo, simulated: bool) {
    // dsh 的 GitHub tag 形如 `dsh-v0.1.1-rc.2`（monorepo，前缀 dsh-v），
    // 与 DSHBox 应用自身的 `v` 前缀不同；`d.latest` 来自 npm 裸 semver（无 v）。
    let release_url = format!(
        "https://github.com/deepseek-ai/deepseek-harness/releases/tag/dsh-v{}",
        d.latest
    );
    crate::control_center::open_update_prompt(
        app,
        crate::control_center::UpdatePrompt {
            kind: "dsh".into(),
            version: d.latest.clone(),
            current: Some(d.installed.clone()),
            release_url: Some(release_url),
            simulated: simulated.then_some(true),
        },
    );
}

/// 执行 dsh 更新（统一入口：检查更新弹窗的更新按钮 与 更新提示弹窗的立即更新共用）。
pub(crate) fn apply_dsh_update(app: &AppHandle) {
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
            crate::emit_signed(&handle, "update-result", &result);
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
            crate::emit_signed(&handle, "update-result", &result);
            // 弹窗未显示时才弹原生兜底（弹窗开着已显示失败原因，不再重复打扰）
            if !crate::control_center::is_check_open(&handle) {
                crate::native_dialog::show_message(
                    &handle,
                    format!("{}: {e}", crate::locale::text("更新失败", "Update failed")),
                    crate::locale::text("更新", "Update"),
                    tauri_plugin_dialog::MessageDialogKind::Warning,
                );
            }
        }
    });
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
            // dist-tags 一次请求携带三通道版本：当前通道判"有新版"，
            // 其余通道供跨通道发现（仅检查页提示，不影响弹窗语义）
            match runtime::npm_dsh_dist_tags()
                .and_then(|tags| dsh_version_info_with_tags(installed.clone(), channel, &tags))
            {
                Ok(info) => (Some(info), None),
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
                            downgrade_available: false,
                            other_channel: None,
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
                    versions::compare_versions(
                        latest_lts.as_deref().expect("上面已判定 is_some"),
                        cur,
                    ) == std::cmp::Ordering::Greater
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
                        downgrade_available: false,
                        other_channel: None,
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
                            downgrade_available: false,
                            other_channel: None,
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

    match dsh_handle.join() {
        Ok((d, d_err)) => {
            result.dsh = d;
            if result.error.is_none() {
                result.error = d_err;
            }
        }
        Err(_) => crate::logging::log("updater: dsh 更新检查线程异常终止（panic）"),
    }
    match node_handle.join() {
        Ok((managed, installed, latest_lts, latest_error, update_available)) => {
            result.node = Some(NodeInfo {
                managed,
                installed,
                latest_lts,
                latest_error,
                update_available,
            });
        }
        Err(_) => crate::logging::log("updater: Node.js 更新检查线程异常终止（panic）"),
    }
    #[cfg(windows)]
    match pwsh_handle.join() {
        Ok(info) => result.pwsh = Some(info),
        Err(_) => crate::logging::log("updater: PowerShell 更新检查线程异常终止（panic）"),
    }
    match app_handle.join() {
        Ok(app_info) => result.app = app_info,
        Err(_) => crate::logging::log("updater: 应用更新检查线程异常终止（panic）"),
    }
    match npm_handle.join() {
        Ok((npm_info, npm_err)) => {
            result.npm = npm_info;
            if result.error.is_none() {
                result.error = npm_err;
            }
        }
        Err(_) => crate::logging::log("updater: npm 更新检查线程异常终止（panic）"),
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
            downgrade_available: false,
            other_channel: None,
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
    // 最近发布，可能未切 latest 的 rc 版，会误报"有更新"。与 pwsh 检查
    // 共用同一过滤实现（powershell::latest_stable_tag）。
    let Some(latest) = super::powershell::latest_stable_tag(&tags) else {
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
        downgrade_available: false,
        other_channel: None,
    })
}

#[cfg(test)]
mod dsh_direction_tests {
    use super::dsh_version_info;
    use super::dsh_version_info_with_tags;
    use super::pick_higher_other_channel;
    use crate::runtime::{DshChannel, DshDistTags};

    fn tags(latest: &str, next: &str, alpha: &str) -> DshDistTags {
        DshDistTags {
            latest: Some(latest.into()),
            next: Some(next.into()),
            alpha: Some(alpha.into()),
        }
    }

    #[test]
    fn dsh_direction_upgrade_downgrade_and_equal() {
        assert!(dsh_version_info("0.1.1-rc.2".into(), "0.1.2-alpha.3".into()).update_available);
        // 切回稳定通道：目标更低 → 仅降级入口，不触发"有新版"弹窗
        let downgrade = dsh_version_info("0.1.2-alpha.3".into(), "0.1.1-rc.2".into());
        assert!(!downgrade.update_available && downgrade.downgrade_available);
        // 两通道指向同一版本：无事可做
        let equal = dsh_version_info("0.1.1-rc.2".into(), "0.1.1-rc.2".into());
        assert!(!equal.update_available && !equal.downgrade_available);
        // 预发布后缀按 semver 优先级：0.1.2 > 0.1.2-alpha.3
        assert!(dsh_version_info("0.1.2-alpha.3".into(), "0.1.2".into()).update_available);
    }

    #[test]
    fn other_channel_hint_covers_next_higher_than_alpha() {
        // 用户场景：alpha 通道用户，上游把更高版本挂在 next——
        // 当前通道"已是最新"，但其他通道可发现
        let t = tags("1.2.0", "2.0.0-rc.2", "1.2.1-alpha.1");
        let info = dsh_version_info_with_tags("1.2.1-alpha.1".into(), DshChannel::Alpha, &t)
            .expect("当前通道 alpha 有目标版本");
        assert!(!info.update_available && !info.downgrade_available);
        assert_eq!(
            info.other_channel,
            Some(super::OtherChannelHint {
                channel: "next".into(),
                version: "2.0.0-rc.2".into(),
            })
        );
    }

    #[test]
    fn other_channel_hint_requires_higher_than_current_target() {
        // 其他通道虽比已安装高，但不比当前通道目标高：切了也装不到更高，不提示
        // （当前稳定通道目标已到 1.5.0，next/alpha 的 1.3.0/1.2.9 低于目标）
        let t = tags("1.5.0", "1.3.0-rc.1", "1.2.9-alpha.1");
        let hint = pick_higher_other_channel("1.2.0", DshChannel::Latest, &t);
        assert_eq!(hint, None);
    }

    #[test]
    fn other_channel_hint_picks_the_highest_candidate() {
        // 多个通道都符合：取最高者（next 与 alpha 均高于当前通道目标）
        let t = tags("1.2.0", "1.4.0-rc.1", "1.3.0-alpha.1");
        let hint = pick_higher_other_channel("1.2.0", DshChannel::Latest, &t);
        assert_eq!(
            hint,
            Some(super::OtherChannelHint {
                channel: "next".into(),
                version: "1.4.0-rc.1".into(),
            })
        );
    }

    #[test]
    fn other_channel_hint_ignores_missing_and_equal_versions() {
        // 缺失的通道跳过；与已安装版本相同的通道不提示
        let t = DshDistTags {
            latest: Some("1.2.0".into()),
            next: None,
            alpha: Some("1.2.0".into()),
        };
        assert_eq!(
            pick_higher_other_channel("1.2.0", DshChannel::Latest, &t),
            None
        );
    }

    #[test]
    fn dsh_version_info_with_tags_reports_missing_channel_target() {
        // 上游未发布该通道（如 alpha 尚无 tag）：报错而非 panic
        let t = DshDistTags {
            latest: Some("1.2.0".into()),
            next: None,
            alpha: None,
        };
        assert!(dsh_version_info_with_tags("1.0.0".into(), DshChannel::Alpha, &t).is_err());
    }
}
