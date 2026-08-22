//! 插件管理入口：目录、命令与后台维护（web profile）。
//!
//! 全部经由 dsh CLI 的 `plugin` 子命令（转发 pnpm 到 profile 目录），
//! 不改 dsh 代码；变更先合并，用户确认或后台空闲后只重启一次服务使其生效。

mod maintenance;
mod runner;

pub use maintenance::start_market_bootstrap;
use maintenance::*;
use runner::run_dsh_plugin_auto;

use serde::Serialize;
use tauri::AppHandle;
use tauri::Manager;

use crate::app_state::AppState;

#[derive(Default)]
struct RestartState {
    generation: u64,
    pending: bool,
    applying: bool,
    deferred: bool,
    waiting_for_idle: bool,
    error: Option<String>,
}

static RESTART_STATE: std::sync::Mutex<RestartState> = std::sync::Mutex::new(RestartState {
    generation: 0,
    pending: false,
    applying: false,
    deferred: false,
    waiting_for_idle: false,
    error: None,
});
static RESTART_COORDINATOR_RUNNING: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

#[derive(Clone, Serialize)]
pub struct PluginApplyStatus {
    pub pending: bool,
    pub applying: bool,
    pub waiting_for_idle: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

fn apply_status() -> PluginApplyStatus {
    let state = RESTART_STATE.lock().unwrap_or_else(|e| e.into_inner());
    PluginApplyStatus {
        pending: state.pending,
        applying: state.applying,
        waiting_for_idle: state.waiting_for_idle,
        error: state.error.clone(),
    }
}

pub fn plugin_apply_status() -> PluginApplyStatus {
    apply_status()
}

fn mark_plugin_changes(app: &AppHandle, apply_when_idle: bool) {
    {
        let mut state = RESTART_STATE.lock().unwrap_or_else(|e| e.into_inner());
        state.generation = state.generation.wrapping_add(1);
        state.pending = true;
        state.error = None;
        if apply_when_idle {
            state.deferred = true;
        }
    }
    if apply_when_idle {
        start_restart_coordinator(app);
    }
}

pub fn apply_plugin_changes(app: &AppHandle) -> PluginApplyStatus {
    {
        let mut state = RESTART_STATE.lock().unwrap_or_else(|e| e.into_inner());
        if state.pending {
            state.deferred = true;
            state.error = None;
        }
    }
    start_restart_coordinator(app);
    apply_status()
}

fn start_restart_coordinator(app: &AppHandle) {
    use std::sync::atomic::Ordering;
    if RESTART_COORDINATOR_RUNNING.swap(true, Ordering::AcqRel) {
        return;
    }
    let handle = app.clone();
    std::thread::spawn(move || {
        loop {
            if handle.state::<AppState>().is_quitting() {
                break;
            }
            let should_apply = {
                let state = RESTART_STATE.lock().unwrap_or_else(|e| e.into_inner());
                state.pending && state.deferred && !state.applying
            };
            if !should_apply {
                break;
            }
            let config = handle.state::<AppState>().config();
            if crate::stats::session_activity(&config) != Some(false) {
                RESTART_STATE
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .waiting_for_idle = true;
                std::thread::sleep(std::time::Duration::from_secs(2));
                continue;
            }
            let generation = {
                let mut state = RESTART_STATE.lock().unwrap_or_else(|e| e.into_inner());
                state.applying = true;
                state.waiting_for_idle = false;
                state.generation
            };
            let result = crate::updater::restart_service(&handle);
            let mut state = RESTART_STATE.lock().unwrap_or_else(|e| e.into_inner());
            state.applying = false;
            match result {
                Ok(()) => {
                    state.error = None;
                    if state.generation == generation {
                        state.pending = false;
                        state.deferred = false;
                    }
                }
                Err(error) => {
                    crate::logging::log(&format!(
                        "plugins: 重启服务失败（插件变更仍待应用）：{error}"
                    ));
                    state.error = Some(error);
                    state.deferred = false;
                }
            }
        }
        RESTART_COORDINATOR_RUNNING.store(false, Ordering::Release);
        let needs_restart = {
            let state = RESTART_STATE.lock().unwrap_or_else(|e| e.into_inner());
            state.pending && state.deferred
        };
        if needs_restart {
            start_restart_coordinator(&handle);
        }
    });
}

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
                builtin: builtin_identity(is_market_pkg(name), market_user_removed(&config, name)),
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

/// 安装插件（dsh plugin --profile web add <pkg>），成功后等待批量应用。
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
    crate::logging::log(&format!("plugins: 已安装 {name}，等待统一应用"));
    mark_plugin_changes(app, false);
    Ok(())
}

/// 卸载插件（dsh plugin --profile web remove <pkg>），成功后等待批量应用。
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
    if is_market_pkg(name) {
        market_mark_user_removed(&config, name);
        crate::logging::log(&format!(
            "plugins: 已记录 {name} 被用户卸载（重装后不再视为内置）"
        ));
    }
    crate::logging::log(&format!("plugins: 已卸载 {name}，等待统一应用"));
    mark_plugin_changes(app, false);
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
    for p in market_pkg_ids() {
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
                        is_market_pkg(&pkg),
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
        let needs_update = match installed.as_deref() {
            // 未安装（用户已卸载）：不显示“有新版本”，避免出现无效更新按钮
            None => false,
            Some(i) => crate::versions::compare_versions(i, &latest).is_lt(),
        };
        let cooldown_until = (needs_update && in_release_cooldown(published, now))
            .then_some(published + MARKET_SUPPLY_CHAIN_RETRY);
        out.push(UpdateStatus {
            pkg: pkg.clone(),
            installed,
            latest,
            update_available: needs_update && cooldown_until.is_none(),
            builtin: builtin_identity(is_market_pkg(&pkg), market_user_removed(&config, &pkg)),
            cooldown_until,
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
    let builtin = builtin_identity(is_market_pkg(pkg), market_user_removed(&config, pkg));
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
    let target = market_version_spec(pkg, &latest).ok_or_else(|| {
        crate::locale::text(
            "版本服务返回了无效版本。",
            "The version service returned an invalid version.",
        )
    })?;
    crate::logging::log(&format!("plugins: 手动升级 {pkg} 到 {latest}"));
    match run_dsh_plugin_auto(app, &["add", &target]) {
        Ok(_) => {
            let actual = market_installed_version(&config, pkg);
            if actual.as_deref() != Some(latest.as_str()) {
                crate::logging::log(&format!(
                    "plugins: {pkg} 升级版本不符（实际 {}，预期 {latest}）",
                    actual.as_deref().unwrap_or("未知")
                ));
                Ok(UpdateStatus {
                    pkg: pkg.to_string(),
                    error: Some(format!(
                        "升级版本不符（实际 {}，预期 {latest}）",
                        actual.as_deref().unwrap_or("未知")
                    )),
                    installed: actual.or(Some(installed)),
                    latest,
                    update_available: true,
                    builtin,
                    cooldown_until: None,
                })
            } else {
                crate::logging::log(&format!("plugins: {pkg} 升级完成"));
                // 手动操作过：落检查门控（定时同步不再重复）+ 清除失败退避
                market_mark_checked(&config);
                let _ = crate::app_state::save_state_value(
                    &config.root,
                    "market_upgrade_retry_at",
                    serde_json::json!(0),
                );
                crate::logging::log("plugins: 升级完成，等待统一应用");
                mark_plugin_changes(app, false);
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
    fn exact_plugin_spec_supports_scoped_packages_and_rejects_ranges() {
        assert_eq!(
            market_version_spec("@scope/plugin", "1.2.3-beta.1").as_deref(),
            Some("@scope/plugin@1.2.3-beta.1")
        );
        assert!(market_version_spec("plugin", "latest").is_none());
        assert!(market_version_spec("plugin", "^1.2.3").is_none());
    }

    #[test]
    fn detect_virtual_store_error() {
        // 真实报错（DSH_HOME 迁移后 pnpm 拒绝写操作）
        assert!(is_virtual_store_error(
            "[ERR_PNPM_UNEXPECTED_VIRTUAL_STORE] Unexpected virtual store location"
        ));
        assert!(is_virtual_store_error(
            "The dependencies at \"C:\\Users\\u\\.dsh\\profiles\\web\\node_modules\" are currently symlinked from the virtual store directory at \"C:\\Users\\u\\.dsh\\profiles\\web\\node_modules\\.pnpm\"."
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
    fn normalize_path_handles_separators_and_case() {
        #[cfg(windows)]
        {
            assert_eq!(
                normalize_path("C:/Users/Jeff/.dsh"),
                "c:\\users\\jeff\\.dsh"
            );
            assert_eq!(
                normalize_path("C:\\Users\\JEFF\\.dsh"),
                "c:\\users\\jeff\\.dsh"
            );
        }
        #[cfg(not(windows))]
        {
            assert_eq!(normalize_path("/home/user/.dsh"), "/home/user/.dsh");
        }
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
    fn preset_plugins_loads_from_embedded_json() {
        let ids: Vec<&str> = market_pkg_ids().collect();
        assert!(ids.contains(&"dshmarket"));
        assert!(ids.contains(&"dsh-file-drop"));
        assert!(!ids.is_empty());
    }

    #[test]
    fn market_pkg_matching_and_spec() {
        assert!(is_market_pkg("dshmarket"));
        assert!(!is_market_pkg("some-random-plugin"));
        // spec 默认与 id 一致（当前两个预设均无 scoped 分离）
        assert_eq!(market_spec("dshmarket"), "dshmarket");
        // 未收录的包 spec 回退为 id 本身
        assert_eq!(market_spec("unknown"), "unknown");
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

    #[test]
    fn builtin_plugin_consent_requires_an_explicit_choice() {
        let root =
            std::env::temp_dir().join(format!("dshbox-plugin-consent-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let mut config = crate::app_state::Config::load();
        config.root = root.clone();

        assert_eq!(builtin_plugins_consent(&config), None);

        crate::app_state::save_state_value(
            &root,
            "builtin_plugins_enabled",
            serde_json::json!(true),
        )
        .unwrap();
        assert!(builtin_plugins_enabled(&config));

        crate::app_state::save_state_value(
            &root,
            "builtin_plugins_enabled",
            serde_json::json!(false),
        )
        .unwrap();
        assert!(!builtin_plugins_enabled(&config));
        let _ = std::fs::remove_dir_all(root);
    }
}
