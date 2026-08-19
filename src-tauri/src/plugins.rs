//! 插件市场：浏览/安装/卸载 dsh 插件（web profile）。
//!
//! 全部经由 dsh CLI 的 `plugin` 子命令（转发 pnpm 到 profile 目录），
//! 不改 dsh 代码；安装/卸载成功后重启服务使插件加载生效。

use serde::Serialize;
use tauri::AppHandle;
use tauri::Manager;

use crate::app_state::AppState;

#[derive(Serialize)]
pub struct PluginInfo {
    pub name: String,
    pub version: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// 已安装版本（未安装为 None）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub installed: Option<String>,
    /// 是否为 DSHBox 内置预装包（dshmarket/dsh-file-drop）。
    pub builtin: bool,
}

/// 已安装插件列表：读 web profile 的 package.json dependencies；
/// 描述从本地 node_modules/<pkg>/package.json 读取（零网络）。
pub fn list(app: &AppHandle) -> Vec<PluginInfo> {
    let config = app.state::<AppState>().config();
    let pkg = config.dsh_home().join("profiles/web/package.json");
    let Ok(text) = std::fs::read_to_string(&pkg) else {
        return vec![];
    };
    let Ok(json) = serde_json::from_str::<serde_json::Value>(&text) else {
        return vec![];
    };
    let mut out = vec![];
    if let Some(deps) = json.get("dependencies").and_then(|d| d.as_object()) {
        for (name, ver) in deps {
            let version = ver.as_str().unwrap_or("?").to_string();
            // 本地包描述：scope 包（@scope/name）的目录按嵌套路径拼接
            let description = std::fs::read_to_string(
                config
                    .dsh_home()
                    .join("profiles/web/node_modules")
                    .join(name)
                    .join("package.json"),
            )
            .ok()
            .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
            .and_then(|j| {
                j.get("description")
                    .and_then(|d| d.as_str())
                    .map(String::from)
            });
            out.push(PluginInfo {
                name: name.clone(),
                version: version.clone(),
                description,
                installed: Some(version),
                // 内置身份 = 在维护清单且用户未主动卸载过（卸载重装后
                // 不再显示内置标签）
                builtin: builtin_identity(
                    MARKET_PKGS.contains(&name.as_str()),
                    market_user_removed(&config, name),
                ),
            });
        }
    }
    out
}

/// npm registry 搜索 dsh 插件。
pub fn search(query: &str) -> Result<Vec<PluginInfo>, String> {
    let query = query.trim();
    if query.is_empty() {
        return Ok(vec![]);
    }
    let encoded: String = url::form_urlencoded::byte_serialize(query.as_bytes()).collect();
    let url = format!("https://registry.npmjs.org/-/v1/search?text={encoded}&size=24");
    let resp = crate::runtime::client()
        .get(&url)
        .header("User-Agent", "DSHBox")
        .call()
        .map_err(|e| format!("搜索失败：{e}"))?;
    use std::io::Read;
    let mut text = String::new();
    resp.into_body()
        .into_reader()
        .read_to_string(&mut text)
        .map_err(|e| format!("读取搜索响应失败：{e}"))?;
    let json: serde_json::Value =
        serde_json::from_str(&text).map_err(|e| format!("解析搜索响应失败：{e}"))?;
    let mut out = vec![];
    if let Some(objects) = json.get("objects").and_then(|v| v.as_array()) {
        for obj in objects {
            let pkg = obj.get("package");
            let (Some(name), Some(version)) = (
                pkg.and_then(|p| p.get("name")).and_then(|v| v.as_str()),
                pkg.and_then(|p| p.get("version")).and_then(|v| v.as_str()),
            ) else {
                continue;
            };
            out.push(PluginInfo {
                name: name.to_string(),
                version: version.to_string(),
                description: pkg
                    .and_then(|p| p.get("description"))
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string()),
                installed: None,
                builtin: false,
            });
        }
    }
    Ok(out)
}

/// 安装插件（dsh plugin --profile web add <pkg>），成功后重启服务。
pub fn install(app: &AppHandle, name: &str) -> Result<(), String> {
    let name = name.trim();
    if name.is_empty() {
        return Err(
            crate::locale::text("插件名不能为空。", "The package name must not be empty.").into(),
        );
    }
    run_dsh_plugin_auto(app, &["add", name])?;
    // 手动重装被强制下线的包 = 知情保留：记录标记，下次启动豁免清理
    let config = app.state::<AppState>().config();
    if MARKET_REMOVED.contains(&name) {
        market_mark_user_removed(&config, name);
        crate::logging::log(&format!(
            "plugins: 已记录 {name} 被手动重装（强制下线豁免）"
        ));
    }
    crate::logging::log(&format!("plugins: 已安装 {name}，重启服务生效"));
    restart_service_silently(app);
    Ok(())
}

/// 卸载插件（dsh plugin --profile web remove <pkg>），成功后重启服务。
/// 卸载内置包会记录"用户主动卸载"标记：之后即使重装也不再视为内置
/// （无内置标签、不自动更新、强制下线豁免）。
pub fn remove(app: &AppHandle, name: &str) -> Result<(), String> {
    let name = name.trim();
    if name.is_empty() {
        return Err(
            crate::locale::text("插件名不能为空。", "The package name must not be empty.").into(),
        );
    }
    run_dsh_plugin_auto(app, &["remove", name])?;
    // 仅用户主动卸载（本函数）写标记；强制下线清理走引导路径不经过这里
    let config = app.state::<AppState>().config();
    if MARKET_PKGS.contains(&name) {
        market_mark_user_removed(&config, name);
        crate::logging::log(&format!(
            "plugins: 已记录 {name} 被用户卸载（重装后不再视为内置）"
        ));
    }
    crate::logging::log(&format!("plugins: 已卸载 {name}，重启服务生效"));
    restart_service_silently(app);
    Ok(())
}

/// 重启请求合并窗口：窗口内到达的多个重启请求合并为一次执行——
/// 同一轮逻辑（并发升级、下线清理+引导安装）的多个请求只重启一次，
/// 避免 kill/spawn 竞态；窗口外的新请求（如用户随后手动安装/卸载）
/// 是最新请求，必定执行，手动变更即时生效。
const RESTART_MERGE_WINDOW_MS: u64 = 500;

static LAST_SERVICE_RESTART: std::sync::Mutex<Option<std::time::Instant>> =
    std::sync::Mutex::new(None);
/// 重启执行串行锁：多个执行线程先后等待，避免并发 kill/spawn。
static RESTART_EXEC_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// 重启服务（后台线程；失败仅记日志——插件已写入 profile，下次启动也会加载）。
/// 每个请求都 spawn 执行线程：等待合并窗口后检查自己是否仍为最新请求，
/// 被更新请求取代则退出（合并）；否则持执行锁重启（串行化，无双重启）。
fn restart_service_silently(app: &AppHandle) {
    let request_at = std::time::Instant::now();
    *LAST_SERVICE_RESTART
        .lock()
        .unwrap_or_else(|e| e.into_inner()) = Some(request_at);
    let handle = app.clone();
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(RESTART_MERGE_WINDOW_MS));
        let is_latest = LAST_SERVICE_RESTART
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .map(|t| t >= request_at)
            .unwrap_or(true);
        if !is_latest {
            return; // 已有更新的请求会执行，本次合并
        }
        let _guard = RESTART_EXEC_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        if let Err(e) = crate::updater::restart_service(&handle) {
            crate::logging::log(&format!("plugins: 重启服务失败（插件已保存）：{e}"));
        }
    });
}

// —— 内置预装包（dsh-market + dsh-file-drop）：自动预装与每日版本同步 ——

/// 内置预装包：插件市场（dshmarket）与文件拖拽（dsh-file-drop，BSD-3-Clause，
/// 与桌面壳场景直接相关）。均走 `dsh plugin` CLI 安装，失败静默重试。
/// 包名在此清单内即享受内置待遇（自动引导/每日同步/UI 内置标签），
/// 与安装来源无关；用户主动卸载后（见 market_user_removed）不再视为内置。
const MARKET_PKGS: &[&str] = &["dshmarket", "dsh-file-drop"];

/// 强制下线清单：曾内置、需要从用户机器移除的包（安全缺陷/与 DSHBox
/// 冲突等）。启动引导时检测到"已装且仍为内置身份"即自动卸载；用户卸载
/// 过又手动重装的包（market_user_removed 标记）豁免——尊重用户选择。
/// 初始为空，未来需要撤回插件时把包名加进来即可（发版生效）。
/// 约束：同一包不能同时出现在 MARKET_PKGS 与 MARKET_REMOVED。
const MARKET_REMOVED: &[&str] = &[];
/// 版本检查门控间隔（24 小时）。
const MARKET_CHECK_INTERVAL: u64 = 86_400;
/// 引导（首次安装）失败后的重试退避：退避期内启动不再重试，避免
/// 每次启动都刷失败日志（上游 supply-chain 策略拦截是持续性的，
/// 短期反复重试必然失败）。
const MARKET_BOOTSTRAP_RETRY: u64 = 6 * 3600;
/// 升级失败的通用退避（1 小时）：网络等瞬时错误，1h 后重试足够。
const MARKET_UPGRADE_RETRY: u64 = 3600;
/// supply-chain 冷却期退避（24 小时）：pnpm 的 minimumReleaseAge 策略要求
/// 新发布包满冷却期（实测 24h）才允许安装，期间重试必然失败。
const MARKET_SUPPLY_CHAIN_RETRY: u64 = 24 * 3600;

/// pnpm virtual store 错位：DSH_HOME 目录被整体迁移/复制后，
/// node_modules/.modules.yaml 里的 virtualStoreDir 绝对路径失效，
/// pnpm 拒绝一切写操作。可自愈：备份 node_modules 后让 pnpm 全新重建。
/// 同时匹配错误码行与详细说明行（pnpm 不同版本措辞有差异）。
fn is_virtual_store_error(detail: &str) -> bool {
    detail.contains("ERR_PNPM_UNEXPECTED_VIRTUAL_STORE")
        || detail.contains("Unexpected virtual store location")
        || detail.contains("symlinked from the virtual store directory")
}

/// pnpm supply-chain 策略拦截（minimumReleaseAge 冷却期）：新发布包在
/// 冷却期内不允许安装，重试无意义，须等冷却期过后。
fn is_supply_chain_error(detail: &str) -> bool {
    detail.contains("supply-chain") || detail.contains("minimumReleaseAge")
}

/// 环境拦截（安全软件杀进程等）：Windows 上子进程被外部终止时取不到
/// 退出码（run_dsh_plugin 会在错误文本中附加标记）。此类失败重试无意义，
/// 应长退避并提示用户配置信任，而不是每次启动重试。
fn is_environment_block_error(detail: &str) -> bool {
    detail.contains("进程被外部终止") || detail.contains("无退出码")
}

/// 所有 `dsh plugin`（pnpm）操作的互斥锁：引导、定时升级、手动升级、
/// 手动安装/卸载都可能并发触发 pnpm，串行化避免 pnpm 锁竞争与状态错乱。
static MARKET_PNPM_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// 已装包版本（web profile 的 package.json dependencies），未装为 None。
fn market_installed_version(config: &crate::app_state::Config, pkg: &str) -> Option<String> {
    let pkg_file = config.dsh_home().join("profiles/web/package.json");
    let text = std::fs::read_to_string(&pkg_file).ok()?;
    let json: serde_json::Value = serde_json::from_str(&text).ok()?;
    json.get("dependencies")?.get(pkg)?.as_str().map(|s| {
        s.trim_start_matches('^')
            .trim_start_matches('~')
            .to_string()
    })
}

/// npm registry 上指定包的最新版本及其发布时间（epoch 秒）。
/// 发布时间用于判断 pnpm supply-chain 冷却期（minimumReleaseAge，实测
/// 24h）：冷却期内 `pnpm add` 必然失败或降级安装旧版，提前跳过可避免
/// 无谓拉起 node（安全软件弹窗/日志噪音）。
fn market_latest_info(pkg: &str) -> Option<(String, u64)> {
    use std::io::Read;
    // 完整 manifest（默认 Accept）才含 time 字段：install-v1 缩写版没有
    let resp = crate::runtime::client()
        .get(&format!("https://registry.npmjs.org/{pkg}"))
        .header("User-Agent", "DSHBox")
        .call()
        .ok()?;
    let mut text = String::new();
    resp.into_body()
        .into_reader()
        .read_to_string(&mut text)
        .ok()?;
    let json: serde_json::Value = serde_json::from_str(&text).ok()?;
    let latest = json.get("dist-tags")?.get("latest")?.as_str()?;
    let published = json.get("time")?.get(latest)?.as_str()?;
    Some((latest.to_string(), parse_rfc3339_epoch(published)?))
}

/// 版本发布时间是否仍在 pnpm supply-chain 冷却期内。
fn in_release_cooldown(published_epoch: u64, now: u64) -> bool {
    now.saturating_sub(published_epoch) < MARKET_SUPPLY_CHAIN_RETRY
}

/// 解析 npm registry 的发布时间（RFC3339 UTC，形如
/// `2026-08-19T01:16:46.446Z`）为 epoch 秒。仅支持 UTC（registry 恒定
/// 输出 UTC），无需引入时间库。
fn parse_rfc3339_epoch(s: &str) -> Option<u64> {
    let (date, time) = s.split_once('T')?;
    let mut dit = date.split('-');
    let year: i64 = dit.next()?.parse().ok()?;
    let month: u64 = dit.next()?.parse().ok()?;
    let day: u64 = dit.next()?.parse().ok()?;
    let mut tit = time.split(':');
    let hour: u64 = tit.next()?.parse().ok()?;
    let min: u64 = tit.next()?.parse().ok()?;
    // 秒可能带小数与时区后缀（46.446Z / 46Z），只取数字前缀
    let sec: u64 = tit
        .next()?
        .split(['.', 'Z', 'z', '+'])
        .next()?
        .parse()
        .ok()?;
    if !(1..=12).contains(&month) || day == 0 || day > 31 || hour > 23 || min > 59 || sec > 60 {
        return None;
    }
    // days from civil（Howard Hinnant 算法）
    let y = if month <= 2 { year - 1 } else { year };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let doy = (153 * (if month > 2 { month - 3 } else { month + 9 }) + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy as i64;
    let days = era * 146_097 + doe - 719_468;
    Some((days * 86_400 + hour as i64 * 3_600 + min as i64 * 60 + sec as i64) as u64)
}

fn market_last_check(root: &std::path::Path) -> Option<u64> {
    let text = std::fs::read_to_string(root.join("config.json")).ok()?;
    let json: serde_json::Value = serde_json::from_str(&text).ok()?;
    json.get("market_last_check")?.as_u64()
}

fn market_check_due(config: &crate::app_state::Config) -> bool {
    let now = market_unix_now();
    market_last_check(&config.root)
        .map(|t| now.saturating_sub(t) >= MARKET_CHECK_INTERVAL)
        .unwrap_or(true)
}

fn market_unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn market_mark_checked(config: &crate::app_state::Config) {
    let _ = crate::app_state::save_config_value(
        &config.root,
        "market_last_check",
        serde_json::json!(market_unix_now()),
    );
}

/// 某内置包是否曾成功引导安装（按包粒度）。曾装过的包被用户卸载后
/// 不再自动重装（尊重卸载意图，与 README 承诺一致）。
fn market_pkg_bootstrapped(config: &crate::app_state::Config, pkg: &str) -> bool {
    let text = std::fs::read_to_string(config.root.join("config.json")).unwrap_or_default();
    serde_json::from_str::<serde_json::Value>(&text)
        .ok()
        .and_then(|j| {
            j.get(format!("market_bootstrapped_{pkg}"))
                .and_then(|v| v.as_bool())
        })
        .unwrap_or(false)
}

fn market_mark_pkg_bootstrapped(config: &crate::app_state::Config, pkg: &str) {
    let _ = crate::app_state::save_config_value(
        &config.root,
        &format!("market_bootstrapped_{pkg}"),
        serde_json::json!(true),
    );
}

/// 用户是否主动卸载过该内置包（plugins::remove 卸载内置包时写入，重装
/// 不清除）。存在此标记 = 用户放弃内置待遇：不再显示内置标签、不再
/// 自动更新、强制下线清理豁免。
fn market_user_removed(config: &crate::app_state::Config, pkg: &str) -> bool {
    let text = std::fs::read_to_string(config.root.join("config.json")).unwrap_or_default();
    serde_json::from_str::<serde_json::Value>(&text)
        .ok()
        .and_then(|j| {
            j.get(format!("market_user_removed_{pkg}"))
                .and_then(|v| v.as_bool())
        })
        .unwrap_or(false)
}

fn market_mark_user_removed(config: &crate::app_state::Config, pkg: &str) {
    let _ = crate::app_state::save_config_value(
        &config.root,
        &format!("market_user_removed_{pkg}"),
        serde_json::json!(true),
    );
}

/// 内置身份判定：包在当前维护清单中，且用户未曾主动卸载过。
/// （不含 bootstrapped 条件——引导从未成功的包仍需显示重装入口。）
fn builtin_identity(in_market: bool, user_removed: bool) -> bool {
    in_market && !user_removed
}

/// 强制下线清理条件：在下线清单、已安装、且仍为内置身份。
/// 用户卸载过又手动重装的包豁免——尊重用户对已装插件的所有权。
fn should_retire(in_removed: bool, installed: bool, user_removed: bool) -> bool {
    in_removed && installed && !user_removed
}

/// 引导失败退避时间戳：上次引导失败时写入 `now + MARKET_BOOTSTRAP_RETRY`，
/// 该时刻前启动不再重试。
fn market_bootstrap_retry_due(config: &crate::app_state::Config) -> bool {
    let text = std::fs::read_to_string(config.root.join("config.json")).unwrap_or_default();
    let retry_at = serde_json::from_str::<serde_json::Value>(&text)
        .ok()
        .and_then(|j| j.get("market_bootstrap_retry_at").and_then(|v| v.as_u64()));
    retry_at.map(|t| market_unix_now() >= t).unwrap_or(true)
}

fn market_mark_bootstrap_retry(config: &crate::app_state::Config) {
    let _ = crate::app_state::save_config_value(
        &config.root,
        "market_bootstrap_retry_at",
        serde_json::json!(market_unix_now() + MARKET_BOOTSTRAP_RETRY),
    );
}

/// 升级失败退避：退避期内跳过版本同步（不落检查门控，到期后自动恢复）。
/// 通用失败 1h；supply-chain 冷却期 24h（minimumReleaseAge 实测 24h）。
fn market_upgrade_retry_due(config: &crate::app_state::Config) -> bool {
    let text = std::fs::read_to_string(config.root.join("config.json")).unwrap_or_default();
    let retry_at = serde_json::from_str::<serde_json::Value>(&text)
        .ok()
        .and_then(|j| j.get("market_upgrade_retry_at").and_then(|v| v.as_u64()));
    retry_at.map(|t| market_unix_now() >= t).unwrap_or(true)
}

fn market_mark_upgrade_retry(config: &crate::app_state::Config, backoff_secs: u64) {
    let _ = crate::app_state::save_config_value(
        &config.root,
        "market_upgrade_retry_at",
        serde_json::json!(market_unix_now() + backoff_secs),
    );
}

/// 内置预装包引导（后台线程）：dsh 服务就绪后——
/// 未安装的包逐个自动安装并重启服务；此后每 24h 检查一次 npm 最新版，
/// 落后时后台升级（`dsh plugin add` 重复执行即升级语义）并重启。
/// 全部失败静默：安装/升级失败退避后重试，不阻塞主流程。
pub fn start_market_bootstrap(app: AppHandle) {
    std::thread::spawn(move || {
        let config = app.state::<AppState>().config();
        // 等待 dsh 服务就绪（最多 5 分钟）：插件命令依赖 dsh CLI 与 profile
        // 结构；超时放弃，下次启动再试
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(300);
        loop {
            if crate::dsh::health_check(config.port) {
                break;
            }
            if std::time::Instant::now() > deadline {
                crate::logging::log("market: dsh 服务 5 分钟内未就绪，跳过本次引导");
                return;
            }
            std::thread::sleep(std::time::Duration::from_secs(5));
        }
        // 未安装的包逐个安装（按包记录引导完成：曾装过、被用户主动卸载的
        // 包不再自动重装）；上次引导失败后的退避期内直接跳过：上游
        // supply-chain 策略拦截是持续性的，短期反复重试必然失败，只会刷日志。
        if bootstrap_market_pkgs(&app, &config) {
            crate::logging::log("market: 重启服务使内置包生效");
            restart_service_silently(&app);
        }
        // 版本同步：首次延迟 90s（避开启动期——安全软件弹窗/网络波动，
        // 不在用户刚打开应用时打扰），此后每 24h 循环（应用常驻期间持续
        // 生效；应用退出线程随之结束，下次启动重新开始）。
        std::thread::sleep(std::time::Duration::from_secs(90));
        loop {
            if market_check_due(&config) && sync_market_versions(&app, &config) {
                crate::logging::log("market: 重启服务使升级生效");
                restart_service_silently(&app);
            }
            std::thread::sleep(std::time::Duration::from_secs(MARKET_CHECK_INTERVAL));
        }
    });
}

/// 未安装的内置包逐个安装。返回是否有新装包（调用方据此重启服务）。
fn bootstrap_market_pkgs(app: &AppHandle, config: &crate::app_state::Config) -> bool {
    if !market_bootstrap_retry_due(config) {
        return false;
    }
    let mut installed_any = false;
    let mut failed = false;
    // 先清理强制下线清单：已装且仍为内置身份的包自动卸载。
    // 失败计入引导退避（6h 后重试），避免每次启动刷失败日志。
    let (removed_any, cleanup_failed) = remove_retired_market_pkgs(app, config);
    if removed_any {
        crate::logging::log("market: 重启服务使强制下线卸载生效");
        restart_service_silently(app);
    }
    if cleanup_failed {
        failed = true;
    }
    for pkg in MARKET_PKGS {
        if market_installed_version(config, pkg).is_some() {
            // 已装（含用户手动安装）即视为该包引导完成
            market_mark_pkg_bootstrapped(config, pkg);
            continue;
        }
        if market_pkg_bootstrapped(config, pkg) {
            continue; // 曾装过、用户主动卸载 → 尊重卸载意图，不重装
        }
        crate::logging::log(&format!("market: 自动安装内置包 {pkg}"));
        match run_dsh_plugin_auto(app, &["add", pkg]) {
            Ok(_) => {
                crate::logging::log(&format!("market: {pkg} 安装完成"));
                market_mark_pkg_bootstrapped(config, pkg);
                installed_any = true;
            }
            Err(e) => {
                failed = true;
                crate::logging::log(&format!("market: {pkg} 安装失败（退避后重试）：{e}"));
            }
        }
    }
    if failed {
        // 记退避：退避期内启动不再重试，避免刷屏
        market_mark_bootstrap_retry(config);
    } else if installed_any || removed_any {
        // 新安装/下线卸载来自 npm latest/清理，无需同次启动再查询；
        // 若全部原本已安装，则保留检查门控原值，让版本同步正常执行。
        market_mark_checked(config);
    }
    installed_any || removed_any
}

/// 强制下线清理：遍历 MARKET_REMOVED，对"已装且仍为内置身份"的包自动
/// 卸载（用户卸载过又手动重装的豁免）。返回 (是否有卸载, 是否有失败)。
fn remove_retired_market_pkgs(app: &AppHandle, config: &crate::app_state::Config) -> (bool, bool) {
    if MARKET_REMOVED.is_empty() {
        return (false, false);
    }
    let mut removed_any = false;
    let mut failed = false;
    for pkg in MARKET_REMOVED {
        if MARKET_PKGS.contains(pkg) {
            // 配置错误防抖：同一包不能同时在维护与下线清单，否则每次
            // 启动"卸载→引导重装"抖动；跳过并提示
            crate::logging::log(&format!(
                "market: 配置错误：{pkg} 同时存在于 MARKET_PKGS 与 MARKET_REMOVED，跳过清理"
            ));
            continue;
        }
        let installed = market_installed_version(config, pkg).is_some();
        let user_removed = market_user_removed(config, pkg);
        if !should_retire(true, installed, user_removed) {
            if installed && user_removed {
                crate::logging::log(&format!(
                    "market: {pkg} 已下线但用户主动重装过，尊重用户选择，跳过清理"
                ));
            }
            continue;
        }
        crate::logging::log(&format!("market: 强制下线：卸载 {pkg}"));
        match run_dsh_plugin_auto(app, &["remove", pkg]) {
            Ok(_) => {
                crate::logging::log(&format!("market: {pkg} 已卸载（强制下线）"));
                removed_any = true;
            }
            Err(e) => {
                failed = true;
                crate::logging::log(&format!(
                    "market: {pkg} 强制下线卸载失败（退避后重试）：{e}"
                ));
            }
        }
    }
    (removed_any, failed)
}

/// 已安装包的版本同步（每 24h 门控由调用方控制）；缺失表示用户已卸载，
/// 必须跳过。与引导相互独立：某包装不上（引导失败）不影响这里对已装
/// 其他包的升级检查——未装包直接 continue，不会计入失败。
/// 返回是否有包升级成功。
fn sync_market_versions(app: &AppHandle, config: &crate::app_state::Config) -> bool {
    // 用户关闭自动升级后静默跳过（首次预装引导不受此开关影响）
    if !config.auto_update_plugins {
        return false;
    }
    // 升级失败退避期内跳过（不落检查门控，退避到期后自动恢复）
    if !market_upgrade_retry_due(config) {
        crate::logging::log("market: 升级失败退避期内，跳过版本同步");
        return false;
    }
    let mut upgraded_any = false;
    let mut check_complete = true;
    for pkg in MARKET_PKGS {
        // 用户卸载过又重装的包：不再视为内置，不自动更新
        // （仍可在插件管理页手动检查/更新）
        if market_user_removed(config, pkg) {
            continue;
        }
        let Some(installed) = market_installed_version(config, pkg) else {
            continue;
        };
        let Some((latest, published)) = market_latest_info(pkg) else {
            // 任一查询失败都不落全局门控：下次周期重试。
            check_complete = false;
            crate::logging::log(&format!("market: {pkg} 版本查询失败，跳过本次同步"));
            continue;
        };
        let needs_update = crate::versions::compare_versions(&installed, &latest).is_lt();
        if !needs_update {
            continue;
        }
        // 新版仍在 supply-chain 冷却期：pnpm add 必然失败或降级安装，
        // 提前跳过（不拉起 node），冷却期满后自动重试。仅在确有升级
        // 需求时判断，避免无需升级的包也写入退避。
        if in_release_cooldown(published, market_unix_now()) {
            check_complete = false;
            crate::logging::log(&format!(
                "market: {pkg} 新版 {latest} 仍在发布冷却期内，跳过（冷却期满后自动重试）"
            ));
            market_mark_upgrade_retry(config, MARKET_SUPPLY_CHAIN_RETRY);
            continue;
        }
        crate::logging::log(&format!("market: 升级 {pkg} 到 {latest}"));
        match run_dsh_plugin_auto(app, &["add", pkg]) {
            Ok(_) => {
                // 验证真的升级了：pnpm 在冷却期可能“成功”但降级安装旧版
                if market_installed_version(config, pkg).as_deref() == Some(installed.as_str()) {
                    check_complete = false;
                    crate::logging::log(&format!(
                        "market: {pkg} 升级未生效（版本仍为 {installed}），退避后重试"
                    ));
                    market_mark_upgrade_retry(config, MARKET_SUPPLY_CHAIN_RETRY);
                } else {
                    crate::logging::log(&format!("market: {pkg} 升级完成"));
                    upgraded_any = true;
                }
            }
            Err(e) => {
                check_complete = false;
                crate::logging::log(&format!("market: {pkg} 升级失败（退避后重试）：{e}"));
                // 环境拦截（安全软件）与 supply-chain 冷却期都是持续性的，
                // 长退避 24h 才可能成功；其余瞬时错误 1h 足够
                let backoff = if is_supply_chain_error(&e) {
                    MARKET_SUPPLY_CHAIN_RETRY
                } else if is_environment_block_error(&e) {
                    crate::logging::log(
                        "market: 疑似安全软件拦截插件命令，24h 内不再自动重试；可在设置中关闭自动升级，或将 DSHBox 目录加入安全软件信任",
                    );
                    MARKET_SUPPLY_CHAIN_RETRY
                } else {
                    MARKET_UPGRADE_RETRY
                };
                market_mark_upgrade_retry(config, backoff);
            }
        }
    }
    if check_complete {
        market_mark_checked(config);
    }
    upgraded_any
}

/// 调用 dsh CLI 的 plugin 子命令（阻塞至完成，5 分钟超时）。
fn run_dsh_plugin(app: &AppHandle, args: &[&str]) -> Result<String, String> {
    let config = app.state::<AppState>().config();
    // 便携 Node 优先，其次系统 Node（与 dsh 服务启动的运行时选择一致）
    let node = if config.node_exe().exists() {
        config.node_exe()
    } else {
        crate::runtime::find_system_node().ok_or_else(|| {
            crate::locale::text(
                "Node.js 运行时未就绪。",
                "The Node.js runtime is not ready.",
            )
        })?
    };
    let mut cmd = std::process::Command::new(&node);
    cmd.arg(config.dsh_entry())
        .arg("plugin")
        .arg("--profile")
        .arg("web")
        .args(args);
    for (k, v) in crate::runtime::base_envs(&node, &config) {
        cmd.env(k, v);
    }
    // 正式版是 GUI 子系统；必须隐藏 node 控制台，否则首次插件引导会闪窗。
    crate::processes::hide_console(&mut cmd);
    // 输出重定向到临时文件：npm 输出可能远超管道缓冲（64KB），
    // 若不持续读取会让子进程写阻塞，误触 5 分钟超时。
    // 文件名做安全化 + 唯一随机后缀：scope 包名含 @ / 等字符（Windows 文件名
    // 不允许），同 pid 并发调用不共文件
    let safe_args: String = args
        .join("_")
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.') {
                c
            } else {
                '_'
            }
        })
        .collect();
    let mut nonce = [0u8; 6];
    let _ = getrandom::fill(&mut nonce);
    let nonce_hex: String = nonce.iter().map(|b| format!("{b:02x}")).collect();
    let out_path = std::env::temp_dir().join(format!(
        "dshd-plugin-{}-{}-{}.log",
        std::process::id(),
        safe_args,
        nonce_hex
    ));
    let out_file =
        std::fs::File::create(&out_path).map_err(|e| format!("创建输出文件失败：{e}"))?;
    cmd.stdout(
        out_file
            .try_clone()
            .map_err(|e| format!("复制输出句柄失败：{e}"))?,
    )
    .stderr(out_file);
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            let _ = std::fs::remove_file(&out_path);
            return Err(format!("启动 dsh 插件命令失败：{e}"));
        }
    };
    // 插件命令也纳入进程树守卫：应用退出或超时时一并回收 npm 后代进程。
    let _guard = crate::processes::TreeGuard::from_child(&child);
    // 5 分钟超时（npm 安装可能较慢）；超时杀掉避免线程悬挂
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(300);
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {
                if std::time::Instant::now() > deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    let _ = std::fs::remove_file(&out_path);
                    return Err(crate::locale::text(
                        "插件操作超时（超过 5 分钟），已中止。",
                        "The plugin operation timed out after 5 minutes and was aborted.",
                    )
                    .into());
                }
                std::thread::sleep(std::time::Duration::from_millis(500));
            }
            Err(e) => {
                let _ = std::fs::remove_file(&out_path);
                return Err(format!("等待插件命令失败：{e}"));
            }
        }
    };
    // 输出（含 stderr 尾部）作为错误详情返回（stdout/stderr 已重定向到临时文件）
    let tail = std::fs::read_to_string(&out_path).unwrap_or_default();
    let _ = std::fs::remove_file(&out_path);
    if !status.success() {
        let detail = tail.trim().to_string();
        let mut err = if detail.is_empty() {
            crate::locale::text("dsh 插件命令执行失败。", "The dsh plugin command failed.").into()
        } else {
            detail
        };
        // Windows 上被外部进程终止（如安全软件拦截）时取不到退出码；
        // 附注便于识别与诊断（is_environment_block_error 据此分类）
        if status.code().is_none() {
            err.push_str("\n（进程被外部终止，无退出码——可能被安全软件拦截）");
        }
        return Err(err);
    }
    Ok(tail)
}

/// 执行 dsh plugin 命令；若失败原因是 pnpm virtual store 错位（DSH_HOME
/// 被整体迁移/复制后 node_modules 元数据里的绝对路径失效），自动备份并
/// 重建 node_modules 后重试一次。安装/升级/卸载共用，遇错自愈。
/// 所有 pnpm 操作在此串行化（互斥锁），避免与定时同步/手动操作并发。
fn run_dsh_plugin_auto(app: &AppHandle, args: &[&str]) -> Result<String, String> {
    let _guard = MARKET_PNPM_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    match run_dsh_plugin(app, args) {
        Ok(out) => Ok(out),
        Err(e) if is_virtual_store_error(&e) => {
            crate::logging::log(
                "plugins: 检测到 pnpm virtual store 错位，备份并重建 node_modules 后重试",
            );
            let config = app.state::<AppState>().config();
            if let Err(re) = recover_virtual_store(&config) {
                return Err(format!("{e}\n（自愈失败：{re}）"));
            }
            match run_dsh_plugin(app, args) {
                Ok(out) => {
                    crate::logging::log("plugins: virtual store 自愈成功，node_modules 已重建");
                    Ok(out)
                }
                Err(e2) => Err(format!(
                    "{e2}\n（已重建 node_modules 修复 virtual store 错位，仍失败；旧数据保留在 node_modules.vstore-bak）"
                )),
            }
        }
        Err(e) => Err(e),
    }
}

/// virtual store 自愈：把 node_modules（与 pnpm-lock.yaml）改名备份，
/// 让 pnpm 视其为全新目录重建。重试成功后由调用方继续；失败时旧数据
/// 保留在备份目录，下次自愈会先清理同名残留。
fn recover_virtual_store(config: &crate::app_state::Config) -> Result<(), String> {
    let dir = config.dsh_home().join("profiles/web");
    let nm = dir.join("node_modules");
    let lock = dir.join("pnpm-lock.yaml");
    let bak = dir.join("node_modules.vstore-bak");
    if bak.exists() {
        std::fs::remove_dir_all(&bak).map_err(|e| format!("清理上次自愈残留失败：{e}"))?;
    }
    std::fs::rename(&nm, &bak).map_err(|e| format!("备份 node_modules 失败：{e}"))?;
    if lock.exists() {
        let _ = std::fs::rename(&lock, dir.join("pnpm-lock.yaml.bak"));
    }
    Ok(())
}

// —— 手动检查/升级插件（插件管理页入口：覆盖全部已安装插件 + 未装内置包） ——

/// 单个插件的更新状态（手动“检查更新/立即更新”用）。
#[derive(serde::Serialize)]
pub struct UpdateStatus {
    pub pkg: String,
    pub installed: Option<String>,
    pub latest: String,
    pub update_available: bool,
    /// 是否为 DSHBox 内置预装包（dshmarket/dsh-file-drop）。
    pub builtin: bool,
    /// 新版仍在 supply-chain 冷却期：此时间戳（epoch 秒）前不应执行升级。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cooldown_until: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// 已安装插件名列表（web profile 的 package.json dependencies 全部 key）。
fn installed_pkgs(config: &crate::app_state::Config) -> Vec<String> {
    let pkg_file = config.dsh_home().join("profiles/web/package.json");
    let Ok(text) = std::fs::read_to_string(&pkg_file) else {
        return vec![];
    };
    let Ok(json) = serde_json::from_str::<serde_json::Value>(&text) else {
        return vec![];
    };
    json.get("dependencies")
        .and_then(|d| d.as_object())
        .map(|d| d.keys().cloned().collect())
        .unwrap_or_default()
}

/// 只读检查全部已安装插件（含未安装的内置包，保持重装入口）是否有新版
/// （不执行安装）。
pub fn check_updates(app: &AppHandle) -> Result<Vec<UpdateStatus>, String> {
    let config = app.state::<AppState>().config();
    // 已安装插件 + 未安装的内置包（重装入口可见）
    let mut pkgs = installed_pkgs(&config);
    for p in MARKET_PKGS {
        if !pkgs.iter().any(|x| x == p) {
            pkgs.push((*p).to_string());
        }
    }
    let now = market_unix_now();
    let mut out = vec![];
    for pkg in pkgs {
        let installed = market_installed_version(&config, &pkg);
        let (latest, published) = match market_latest_info(&pkg) {
            Some(v) => v,
            None => {
                out.push(UpdateStatus {
                    pkg: pkg.clone(),
                    installed,
                    latest: String::new(),
                    update_available: false,
                    builtin: builtin_identity(
                        MARKET_PKGS.contains(&pkg.as_str()),
                        market_user_removed(&config, &pkg),
                    ),
                    cooldown_until: None,
                    error: Some(
                        crate::locale::text(
                            "版本查询失败。",
                            "Failed to query the latest version.",
                        )
                        .into(),
                    ),
                });
                continue;
            }
        };
        // 冷却期内的新版视为“没有新版”：装不了就不提示，冷却期满后
        // 下次检查自然出现“有新版本”。
        let update_available = match installed.as_deref() {
            // 未安装（用户已卸载）：不显示“有新版本”，避免出现无效更新按钮
            None => false,
            Some(i) => {
                crate::versions::compare_versions(i, &latest).is_lt()
                    && !in_release_cooldown(published, now)
            }
        };
        out.push(UpdateStatus {
            pkg: pkg.clone(),
            installed,
            latest,
            update_available,
            builtin: builtin_identity(
                MARKET_PKGS.contains(&pkg.as_str()),
                market_user_removed(&config, &pkg),
            ),
            cooldown_until: None,
            error: None,
        });
    }
    Ok(out)
}

/// 手动升级单个插件（绕过退避与 24h 门控；仅限已安装包；pnpm 操作由
/// run_dsh_plugin_auto 的互斥锁与定时同步串行化）。升级成功后落检查
/// 门控并清除失败退避，避免定时同步紧接着再跑/被旧退避压制。
/// 冷却期内拒绝执行（pnpm minimumReleaseAge 安全策略，不提供绕过）。
pub fn update_pkg(app: &AppHandle, pkg: &str) -> Result<UpdateStatus, String> {
    let config = app.state::<AppState>().config();
    let Some(installed) = market_installed_version(&config, pkg) else {
        return Err(crate::locale::text(
            "该插件未安装，无法更新。",
            "This plugin is not installed.",
        )
        .into());
    };
    let builtin = builtin_identity(
        MARKET_PKGS.contains(&pkg),
        market_user_removed(&config, pkg),
    );
    let (latest, published) = market_latest_info(pkg).ok_or_else(|| {
        crate::locale::text("版本查询失败。", "Failed to query the latest version.")
    })?;
    if !crate::versions::compare_versions(&installed, &latest).is_lt() {
        return Ok(UpdateStatus {
            pkg: pkg.to_string(),
            installed: Some(installed),
            latest,
            update_available: false,
            builtin,
            cooldown_until: None,
            error: None,
        });
    }
    // 冷却期内：不拉起 pnpm，明确告知等待
    if in_release_cooldown(published, market_unix_now()) {
        let cooldown_until = published + MARKET_SUPPLY_CHAIN_RETRY;
        crate::logging::log(&format!(
            "plugins: {pkg} 新版 {latest} 仍在发布冷却期，跳过手动升级"
        ));
        return Ok(UpdateStatus {
            pkg: pkg.to_string(),
            installed: Some(installed),
            latest,
            update_available: true,
            builtin,
            cooldown_until: Some(cooldown_until),
            error: None,
        });
    }
    crate::logging::log(&format!("plugins: 手动升级 {pkg} 到 {latest}"));
    match run_dsh_plugin_auto(app, &["add", pkg]) {
        Ok(_) => {
            // 验证真的升级了：pnpm 在冷却期可能“成功”但降级安装旧版
            if market_installed_version(&config, pkg).as_deref() == Some(installed.as_str()) {
                crate::logging::log(&format!(
                    "plugins: {pkg} 升级未生效（版本仍为 {installed}）"
                ));
                Ok(UpdateStatus {
                    pkg: pkg.to_string(),
                    // 先借用在 format，再 move 进结构体（字段按书写顺序求值）
                    error: Some(format!("升级未生效（版本仍为 {installed}）")),
                    installed: Some(installed),
                    latest,
                    update_available: true,
                    builtin,
                    cooldown_until: None,
                })
            } else {
                crate::logging::log(&format!("plugins: {pkg} 升级完成"));
                // 手动操作过：落检查门控（定时同步不再重复）+ 清除失败退避
                market_mark_checked(&config);
                let _ = crate::app_state::save_config_value(
                    &config.root,
                    "market_upgrade_retry_at",
                    serde_json::json!(0),
                );
                crate::logging::log("plugins: 重启服务使升级生效");
                restart_service_silently(app);
                Ok(UpdateStatus {
                    pkg: pkg.to_string(),
                    installed: Some(latest.clone()),
                    latest,
                    update_available: false,
                    builtin,
                    cooldown_until: None,
                    error: None,
                })
            }
        }
        Err(e) => Ok(UpdateStatus {
            pkg: pkg.to_string(),
            installed: Some(installed),
            latest,
            update_available: true,
            builtin,
            cooldown_until: None,
            error: Some(format!("升级失败：{e}")),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_virtual_store_error() {
        // 真实报错（DSH_HOME 迁移后 pnpm 拒绝写操作）
        assert!(is_virtual_store_error(
            "[ERR_PNPM_UNEXPECTED_VIRTUAL_STORE] Unexpected virtual store location"
        ));
        assert!(is_virtual_store_error(
            "The dependencies at \"C:\\Users\\u\\.dsh-box\\profiles\\web\\node_modules\" are currently symlinked from the virtual store directory at \"C:\\Users\\u\\.dsh\\profiles\\web\\node_modules\\.pnpm\"."
        ));
        // 其他错误不应误判
        assert!(!is_virtual_store_error("npm ERR! code E404"));
        assert!(!is_virtual_store_error("npm ERR! code ETIMEDOUT"));
    }

    #[test]
    fn detect_supply_chain_error() {
        // 真实报错（pnpm minimumReleaseAge 冷却期拦截）
        assert!(is_supply_chain_error(
            "? Verifying lockfile against supply-chain policies (3 entries)..."
        ));
        assert!(is_supply_chain_error(
            "✗ Lockfile failed supply-chain policy check (3 entries in 348ms)"
        ));
        assert!(is_supply_chain_error(
            "dshmarket@1.11.0 was published at 2026-08-17T03:53:35.000Z, within the minimumReleaseAge cutoff"
        ));
        assert!(!is_supply_chain_error(
            "[ERR_PNPM_UNEXPECTED_VIRTUAL_STORE] Unexpected virtual store location"
        ));
    }

    #[test]
    fn detect_environment_block_error() {
        // run_dsh_plugin 附加的“无退出码”标记（Windows 上被安全软件杀进程）
        assert!(is_environment_block_error(
            "（进程被外部终止，无退出码——可能被安全软件拦截）"
        ));
        assert!(is_environment_block_error(
            "dsh 插件命令执行失败。\n（进程被外部终止，无退出码——可能被安全软件拦截）"
        ));
        // 其他错误不误判
        assert!(!is_environment_block_error(
            "[ERR_PNPM_UNEXPECTED_VIRTUAL_STORE] Unexpected virtual store location"
        ));
        assert!(!is_environment_block_error("npm ERR! code E404"));
    }

    #[test]
    fn parse_npm_publish_time() {
        // npm registry 的 RFC3339 UTC 格式（真实值：dshmarket 1.14.1）
        let epoch = parse_rfc3339_epoch("2026-08-19T01:16:46.446Z").unwrap();
        // 冷却期（24h）内：发布后 4.5h
        assert!(in_release_cooldown(epoch, epoch + 4 * 3600 + 30 * 60));
        // 冷却期外：发布后 25h
        assert!(!in_release_cooldown(epoch, epoch + 25 * 3600));
        // 边界：恰好满 24h 视为冷却结束
        assert!(!in_release_cooldown(epoch, epoch + 24 * 3600));
        // 异常输入
        assert!(parse_rfc3339_epoch("garbage").is_none());
        assert!(parse_rfc3339_epoch("2026-13-01T00:00:00Z").is_none());
        assert!(parse_rfc3339_epoch("2026-08-19T24:00:00Z").is_none());
    }

    #[test]
    fn rfc3339_epoch_matches_known_value() {
        // 用系统 date 校验过的真实值（date -u -d "2026-08-19 01:16:46 UTC" +%s）
        assert_eq!(
            parse_rfc3339_epoch("2026-08-19T01:16:46Z"),
            Some(1787102206)
        );
        assert_eq!(parse_rfc3339_epoch("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(
            parse_rfc3339_epoch("2026-08-19T01:16:46.446Z"),
            Some(1787102206)
        );
    }

    #[test]
    fn builtin_identity_requires_no_user_removal() {
        // 内置身份 = 在维护清单 && 用户未主动卸载过
        assert!(builtin_identity(true, false));
        // 用户卸载过（重装与否）→ 非内置
        assert!(!builtin_identity(true, true));
        // 不在维护清单 → 非内置
        assert!(!builtin_identity(false, false));
        assert!(!builtin_identity(false, true));
    }

    #[test]
    fn retired_cleanup_condition() {
        // 强制下线清理 = 在下线清单 && 已装 && 仍为内置身份
        assert!(should_retire(true, true, false));
        // 用户卸载过又重装 → 豁免（尊重用户选择）
        assert!(!should_retire(true, true, true));
        // 未装（含卸载未重装）→ 无操作
        assert!(!should_retire(true, false, false));
        assert!(!should_retire(true, false, true));
        // 不在下线清单 → 不动
        assert!(!should_retire(false, true, false));
    }
}
