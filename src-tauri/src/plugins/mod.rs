//! 插件管理入口：目录、命令与后台维护（web profile）。
//!
//! 全部经由 dsh CLI 的 `plugin` 子命令（转发 pnpm 到 profile 目录），
//! 不改 dsh 代码；变更先合并，用户确认或后台空闲后只重启一次服务使其生效。
//!
//! 模块分工：`transaction.rs` 管落盘事务标记与启动收敛（并发协议所在），
//! `restart.rs` 管变更合并与服务重启协调，`maintenance.rs` 管内置清单/
//! 定期同步/退避，`runner.rs` 管 CLI 执行与 pnpm virtual-store 自愈；
//! 本文件保留插件目录查询（list/search/recommended）、手动增删与更新检查。

mod maintenance;
mod restart;
mod runner;
mod transaction;

pub(crate) use maintenance::release_first_onboarding_bootstrap;
pub use maintenance::start_market_bootstrap;
use maintenance::*;
pub use maintenance::{RecommendedPlugin, ReinstallableBuiltinPlugin};
use runner::{run_dsh_plugin_auto, run_dsh_plugin_auto_user_remove};

// 拆分后的域内绑定：maintenance/runner/测试经 super::* 或 super::X 继续
// 引用这些名字；对 crate 其他模块的路径（crate::plugins::X）保持不变。
pub use restart::{apply_plugin_changes, plugin_apply_status, PluginApplyStatus};
pub(crate) use restart::{deferred_restart_pending, mark_plugin_changes};
use transaction::{
    clear_install_marker, save_install_marker, spec_package_name, try_mark_user_removed,
    PluginMutationKind,
};
pub(crate) use transaction::{
    clear_resolved_install_marker, recover_interrupted_plugin_mutation, try_acquire_pnpm_lock,
    PnpmGuard,
};

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
    /// 与新版 dsh 不兼容（曾导致更新回滚）。
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub incompatible: bool,
    /// 是否为当前 DSHBox 内置清单中的包。
    pub builtin: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub homepage: Option<String>,
}

fn github_repository_url(raw: &str) -> Option<String> {
    let raw = raw.trim();
    let normalized = if let Some(path) = raw.strip_prefix("git@github.com:") {
        format!("https://github.com/{path}")
    } else if let Some(path) = raw.strip_prefix("git://github.com/") {
        format!("https://github.com/{path}")
    } else {
        raw.strip_prefix("git+").unwrap_or(raw).to_string()
    };
    let url = url::Url::parse(&normalized).ok()?;
    if url.host_str()?.eq_ignore_ascii_case("github.com") {
        let mut segments = url.path_segments()?.filter(|segment| !segment.is_empty());
        let owner = segments.next()?;
        let repository = segments.next()?.trim_end_matches(".git");
        if !owner.is_empty() && !repository.is_empty() {
            return Some(format!("https://github.com/{owner}/{repository}"));
        }
    }
    None
}

fn npm_search_package_homepage(package: &serde_json::Value) -> Option<String> {
    let links = package.get("links")?;
    links
        .get("repository")
        .and_then(|value| value.as_str())
        .and_then(github_repository_url)
        .or_else(|| {
            links
                .get("homepage")
                .and_then(|value| value.as_str())
                .and_then(github_repository_url)
        })
}

/// 已安装插件列表：读 web profile 的 package.json dependencies；
/// 描述与项目主页从本地 node_modules/<pkg>/package.json 读取（零网络）。
pub fn list(app: &AppHandle) -> Vec<PluginInfo> {
    let config = app.state::<AppState>().config();
    let builtin_consent = builtin_plugins_enabled(&config);
    let update_conflict = plugin_update_conflict(&config);
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
            // 本地元数据：scope 包（@scope/name）的目录按嵌套路径拼接。
            let local_manifest = std::fs::read_to_string(
                config
                    .dsh_home()
                    .join("profiles/web/node_modules")
                    .join(name)
                    .join("package.json"),
            )
            .ok()
            .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok());
            let description = local_manifest
                .as_ref()
                .and_then(|manifest| manifest.get("description"))
                .and_then(|value| value.as_str())
                .map(String::from);
            let homepage = local_manifest
                .as_ref()
                .and_then(|manifest| manifest.get("homepage"))
                .and_then(|value| value.as_str())
                .and_then(github_repository_url)
                .or_else(|| {
                    local_manifest
                        .as_ref()
                        .and_then(|manifest| manifest.get("repository"))
                        .and_then(|repository| {
                            repository
                                .as_str()
                                .or_else(|| repository.get("url").and_then(|value| value.as_str()))
                        })
                        .and_then(github_repository_url)
                })
                .or_else(|| {
                    maintenance::known_plugin_homepage(name)
                        .as_deref()
                        .and_then(github_repository_url)
                });
            out.push(PluginInfo {
                name: name.clone(),
                version: version.clone(),
                description,
                installed: Some(version),
                // 只有首次引导明确授权的清单项才具有内置身份；卸载重装后
                // 不再显示内置标签，也不参与自动维护。
                builtin: builtin_identity(
                    builtin_consent,
                    is_market_pkg(name),
                    effective_market_user_removed(&config, name),
                ),
                // 该插件曾导致 dsh 更新回滚（与新 dsh 版本不兼容）
                incompatible: update_conflict.as_deref() == Some(name.as_str()),
                homepage,
            });
        }
    }
    // 排序规则：内置插件靠前，其余按名称
    // 字典序稳定排序，保证已安装列表展示确定性。
    out.sort_by(|a, b| b.builtin.cmp(&a.builtin).then_with(|| a.name.cmp(&b.name)));
    out
}

/// 社区插件清单（尚未安装的项）。与 builtin 预装完全分离：
/// 仅展示供用户手动安装，不自动安装、不自动升级、卸载后回推荐区。
pub fn recommended(app: &AppHandle) -> Vec<maintenance::RecommendedPlugin> {
    let installed: std::collections::HashSet<String> =
        list(app).into_iter().map(|p| p.name).collect();
    maintenance::recommended_not_installed(&installed)
}

/// 用户主动卸载且当前仍未安装的内置目录项。仅提供手动重装入口，既有
/// user_removed 标记保持不变，因此重装后仍由用户自行维护。
pub fn reinstallable_builtins(app: &AppHandle) -> Vec<maintenance::ReinstallableBuiltinPlugin> {
    let config = app.state::<AppState>().config();
    let installed: std::collections::HashSet<String> =
        list(app).into_iter().map(|plugin| plugin.name).collect();
    maintenance::reinstallable_builtin_plugins(&config, &installed)
}

/// npm registry 搜索 dsh 插件。
pub fn search(query: &str) -> Result<Vec<PluginInfo>, String> {
    let query = query.trim();
    if query.is_empty() {
        return Ok(vec![]);
    }
    let encoded: String = url::form_urlencoded::byte_serialize(query.as_bytes()).collect();
    let url = format!("https://registry.npmjs.org/-/v1/search?text={encoded}&size=24");
    let resp = crate::runtime::check_client()
        .get(&url)
        .header("User-Agent", "DSHBox")
        .call()
        .map_err(|e| crate::locale::error("搜索失败", "Search failed", e))?;
    use std::io::Read;
    let mut text = String::new();
    resp.into_body()
        .into_reader()
        .read_to_string(&mut text)
        .map_err(|e| {
            crate::locale::error("读取搜索响应失败", "Failed to read the search response", e)
        })?;
    let json: serde_json::Value = serde_json::from_str(&text).map_err(|e| {
        crate::locale::error("解析搜索响应失败", "Failed to parse the search response", e)
    })?;
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
            let homepage = pkg.and_then(npm_search_package_homepage);
            out.push(PluginInfo {
                name: name.to_string(),
                version: version.to_string(),
                description: pkg
                    .and_then(|p| p.get("description"))
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string()),
                installed: None,
                builtin: false,
                incompatible: false,
                homepage,
            });
        }
    }
    Ok(out)
}

/// 校验用户输入的插件名：非空且不得以 `-` 开头（防止被 dsh CLI 解析成
/// 命令行 flag 注入）。返回 trim 后的名字。
fn checked_plugin_name(name: &str) -> Result<&str, String> {
    let name = name.trim();
    if name.is_empty() {
        return Err(
            crate::locale::text("插件名不能为空。", "The package name must not be empty.").into(),
        );
    }
    if name.starts_with('-') {
        return Err(crate::locale::text(
            "插件名不能以「-」开头。",
            "The package name must not start with '-'.",
        )
        .into());
    }
    Ok(name)
}

/// 安装插件（dsh plugin --profile web add <pkg>），成功后等待批量应用。
pub fn install(app: &AppHandle, name: &str) -> Result<(), String> {
    let name = checked_plugin_name(name)?;
    let config = app.state::<AppState>().config();
    run_dsh_plugin_auto(app, &["add", name])?;
    // 手动重装已下线或被替换的包 = 知情保留：记录标记，下次启动豁免清理
    if let Some(package) = spec_package_name(name).filter(|package| is_retired_market_pkg(package))
    {
        try_mark_user_removed(&config, package).map_err(|error| {
            crate::locale::owned(
                format!(
                    "插件已安装，但保存用户管理状态失败：{error}。请勿重启应用，并在插件页重试安装。"
                ),
                format!(
                    "The plugin was installed, but its user-managed state could not be saved: {error}. Do not restart the app; retry the installation from the Plugins page."
                ),
            )
        })?;
        crate::logging::log(&format!(
            "plugins: 已记录 {package} 被手动重装（自动清理豁免）"
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
    let name = checked_plugin_name(name)?;
    run_dsh_plugin_auto_user_remove(app, &["remove", name])?;
    // 卸载的正是此前导致 dsh 更新回滚的冲突插件时，同步清掉诊断记录。
    if let Some(package) = spec_package_name(name) {
        if clear_plugin_update_conflict_if(&app.state::<AppState>().config(), package) {
            crate::logging::log(&format!(
                "plugins: 已清除 {package} 的 dsh 更新冲突记录（插件已卸载）"
            ));
        }
    }
    // 用户管理状态由卸载事务在提交前写入；强制下线清理走普通 runner，
    // 不会写用户主动卸载标记。判定按解析后的包名（原始输入可能带 @version）。
    if spec_package_name(name).is_some_and(is_market_pkg) {
        crate::logging::log(&format!(
            "plugins: 已记录 {name} 被用户卸载（重装后不再视为内置）"
        ));
    }
    crate::logging::log(&format!("plugins: 已卸载 {name}，等待统一应用"));
    mark_plugin_changes(app, false);
    Ok(())
}

// —— 手动检查/升级插件（插件管理页入口：覆盖已安装插件与待修复内置包） ——

/// 单个插件的更新状态（手动“检查更新/立即更新”用）。
#[derive(serde::Serialize)]
pub struct UpdateStatus {
    pub pkg: String,
    pub installed: Option<String>,
    pub latest: String,
    pub update_available: bool,
    /// 是否为当前 DSHBox 内置清单中的包。
    pub builtin: bool,
    /// 新版仍在 supply-chain 冷却期：此时间戳（epoch 秒）前不应执行升级。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cooldown_until: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

fn manifest_package_names(
    text: &str,
) -> Option<(
    std::collections::HashSet<String>,
    std::collections::HashSet<String>,
)> {
    let json: serde_json::Value = serde_json::from_str(text).ok()?;
    let dependencies = json
        .get("dependencies")
        .and_then(|value| value.as_object())
        .map(|values| values.keys().cloned().collect())
        .unwrap_or_default();
    let bundles = json
        .get("dsh")
        .and_then(|value| value.get("profile"))
        .and_then(|value| value.get("bundles"))
        .and_then(|value| value.as_array())
        .map(|values| {
            values
                .iter()
                .filter_map(|value| value.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    Some((dependencies, bundles))
}

// —— dsh 更新冲突记录 ——

/// state.json 键：dsh 更新因该插件加载崩溃而回滚时记录的包名。
/// 检查更新页据此给出“卸载并重试”的引导入口，插件页据此展示不兼容标记。
const PLUGIN_UPDATE_CONFLICT_KEY: &str = "plugin_update_conflict";

pub(crate) fn set_plugin_update_conflict(
    config: &crate::app_state::Config,
    package: &str,
) -> Result<(), String> {
    crate::app_state::save_state_value(
        &config.root,
        PLUGIN_UPDATE_CONFLICT_KEY,
        serde_json::json!(package),
    )
}

pub(crate) fn plugin_update_conflict(config: &crate::app_state::Config) -> Option<String> {
    crate::app_state::load_state_value(&config.root, PLUGIN_UPDATE_CONFLICT_KEY)
        .and_then(|value| value.as_str().map(String::from))
}

/// 记录的冲突插件被卸载后清除对应记录；包名不一致时保留（可能另有其人）。
/// 返回是否真的清除，便于调用方只在命中时对外反馈。
pub(crate) fn clear_plugin_update_conflict_if(
    config: &crate::app_state::Config,
    package: &str,
) -> bool {
    if plugin_update_conflict(config).as_deref() != Some(package) {
        return false;
    }
    match crate::app_state::remove_state_value(&config.root, PLUGIN_UPDATE_CONFLICT_KEY) {
        Ok(()) => true,
        Err(e) => {
            crate::logging::log(&format!("plugins: 清除插件更新冲突记录失败：{e}"));
            false
        }
    }
}

/// 新一轮 dsh 更新开始时清空旧记录：无论成败，旧诊断都不再指导当前操作。
pub(crate) fn reset_plugin_update_conflict(config: &crate::app_state::Config) {
    let _ = crate::app_state::remove_state_value(&config.root, PLUGIN_UPDATE_CONFLICT_KEY);
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

/// 只读检查全部已安装插件，并为已授权但安装缺失的内置包保留修复入口
/// （不执行安装）。用户主动卸载的内置包由独立目录提供手动重装入口。
pub fn check_updates(app: &AppHandle) -> Result<Vec<UpdateStatus>, String> {
    let config = app.state::<AppState>().config();
    let builtin_consent = builtin_plugins_enabled(&config);
    // 已安装插件 + 已授权但缺失的内置包（修复入口可见）。用户主动
    // 卸载或从未授权的目录项不参与查询，避免无意义的网络请求与身份混淆。
    let mut pkgs = installed_pkgs(&config);
    for p in market_pkg_ids() {
        if builtin_identity(
            builtin_consent,
            true,
            effective_market_user_removed(&config, p),
        ) && !pkgs.iter().any(|x| x == p)
        {
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
                        builtin_consent,
                        is_market_pkg(&pkg),
                        effective_market_user_removed(&config, &pkg),
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
            builtin: builtin_identity(
                builtin_consent,
                is_market_pkg(&pkg),
                effective_market_user_removed(&config, &pkg),
            ),
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
    let builtin = builtin_identity(
        builtin_plugins_enabled(&config),
        is_market_pkg(pkg),
        effective_market_user_removed(&config, pkg),
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
                    error: Some(crate::locale::owned(
                        format!(
                            "升级版本不符（实际 {}，预期 {latest}）",
                            actual.as_deref().unwrap_or("未知")
                        ),
                        format!(
                            "The upgraded version does not match (actual {}, expected {latest})",
                            actual.as_deref().unwrap_or("unknown")
                        ),
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
            error: Some(crate::locale::owned(
                format!("升级失败：{e}"),
                format!("Upgrade failed: {e}"),
            )),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::restart::*;
    use super::transaction::*;
    use super::*;

    #[test]
    fn restart_backoff_grows_exponentially_with_cap() {
        assert_eq!(restart_backoff_secs(1), 30);
        assert_eq!(restart_backoff_secs(2), 60);
        assert_eq!(restart_backoff_secs(3), 120);
        assert_eq!(restart_backoff_secs(6), 600);
        // 连续失败很多次后封顶在 10 分钟，不再增长
        assert_eq!(restart_backoff_secs(20), 600);
    }

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
    fn package_name_parser_handles_versions_scopes_and_rejects_urls() {
        assert_eq!(spec_package_name("plugin@1.2.3"), Some("plugin"));
        assert_eq!(
            spec_package_name("@scope/plugin@2.0.0-beta.1"),
            Some("@scope/plugin")
        );
        assert_eq!(spec_package_name("plugin#next"), Some("plugin"));
        assert_eq!(spec_package_name("https://example.com/plugin.tgz"), None);
        assert_eq!(
            spec_package_name("git+https://example.com/plugin.git"),
            None
        );
        assert_eq!(spec_package_name("./plugin"), None);
    }

    #[test]
    fn project_url_normalizes_only_github_repositories() {
        assert_eq!(
            github_repository_url("git+https://github.com/example/plugin.git").as_deref(),
            Some("https://github.com/example/plugin")
        );
        assert_eq!(
            github_repository_url("git@github.com:example/plugin.git").as_deref(),
            Some("https://github.com/example/plugin")
        );
        assert_eq!(
            github_repository_url("https://github.com/example/plugin/releases/latest").as_deref(),
            Some("https://github.com/example/plugin")
        );
        assert!(github_repository_url("https://gitlab.com/example/plugin").is_none());
        assert!(github_repository_url("file:///tmp/plugin").is_none());
    }

    #[test]
    fn npm_search_project_url_prefers_a_github_repository() {
        let package = serde_json::json!({
            "links": {
                "repository": "git+https://github.com/example/plugin.git",
                "homepage": "https://example.com/plugin"
            }
        });
        assert_eq!(
            npm_search_package_homepage(&package).as_deref(),
            Some("https://github.com/example/plugin")
        );

        let fallback = serde_json::json!({
            "links": {
                "repository": "https://gitlab.com/example/plugin",
                "homepage": "https://github.com/example/fallback#readme"
            }
        });
        assert_eq!(
            npm_search_package_homepage(&fallback).as_deref(),
            Some("https://github.com/example/fallback")
        );
    }

    #[test]
    fn recommended_manifest_is_localized_and_uses_real_dependency_ids() {
        let plugins = recommended_plugins();
        assert!(!plugins.is_empty());
        for plugin in plugins {
            assert_eq!(spec_package_name(&plugin.spec), Some(plugin.id.as_str()));
            assert!(!plugin.description_zh.is_empty());
            assert!(!plugin.description_en.is_empty());
            assert!(plugin.homepage.starts_with("https://github.com/"));
        }
        assert!(parse_recommended_plugins("[{\"id\":\"missing-fields\"}]").is_err());
    }

    #[test]
    fn interrupted_install_recovery_requires_the_recorded_package() {
        let root = std::env::temp_dir().join(format!(
            "dshbox-plugin-recovery-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let mut config = crate::app_state::Config::load();
        config.root = root.join("app");
        config.dsh_home = root.join("home");
        let profile = config.dsh_home().join("profiles/web");
        std::fs::create_dir_all(&profile).unwrap();
        let manifest = profile.join("package.json");
        std::fs::write(
            &manifest,
            r#"{
  "dependencies": { "broken-plugin": "1.0.0", "keep-plugin": "2.0.0" },
  "dsh": { "profile": { "bundles": ["broken-plugin", "keep-plugin"] } }
}"#,
        )
        .unwrap();
        save_install_marker(
            &config,
            "broken-plugin@1.0.0",
            Some("broken-plugin"),
            PluginMutationKind::Add,
            false,
            Some(
                r#"{
  "dependencies": { "keep-plugin": "2.0.0" },
  "dsh": { "profile": { "bundles": ["keep-plugin"] } }
}"#,
            ),
        )
        .unwrap();

        assert!(!recover_interrupted_plugin_mutation(&config, "keep-plugin").unwrap());
        let unchanged = std::fs::read_to_string(&manifest).unwrap();
        assert!(unchanged.contains("broken-plugin"));
        assert!(unchanged.contains("keep-plugin"));

        assert!(recover_interrupted_plugin_mutation(&config, "broken-plugin").unwrap());
        let repaired = std::fs::read_to_string(&manifest).unwrap();
        assert!(!repaired.contains("broken-plugin"));
        assert!(repaired.contains("keep-plugin"));
        assert!(install_marker(&config).is_none());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn interrupted_user_removal_finishes_without_restoring_builtin_identity() {
        let root = std::env::temp_dir().join(format!(
            "dshbox-plugin-remove-recovery-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let mut config = crate::app_state::Config::load();
        config.root = root.join("app");
        config.dsh_home = root.join("home");
        let profile = config.dsh_home().join("profiles/web");
        std::fs::create_dir_all(&profile).unwrap();
        let manifest = profile.join("package.json");
        let original = r#"{
  "dependencies": { "dshmarket": "1.0.0", "keep-plugin": "2.0.0" },
  "dsh": { "profile": { "bundles": ["dshmarket", "keep-plugin"] } }
}"#;
        std::fs::write(
            &manifest,
            r#"{
  "dependencies": { "keep-plugin": "2.0.0" },
  "dsh": { "profile": { "bundles": ["dshmarket", "keep-plugin"] } }
}"#,
        )
        .unwrap();
        save_install_marker(
            &config,
            "dshmarket",
            Some("dshmarket"),
            PluginMutationKind::Remove,
            true,
            Some(original),
        )
        .unwrap();

        assert!(recover_interrupted_plugin_mutation(&config, "dshmarket").unwrap());
        let repaired = std::fs::read_to_string(&manifest).unwrap();
        assert!(!repaired.contains("dshmarket"));
        assert!(repaired.contains("keep-plugin"));
        assert!(market_user_removed(&config, "dshmarket"));
        assert!(install_marker(&config).is_none());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn unfinished_plugin_marker_cannot_be_overwritten() {
        let root = std::env::temp_dir().join(format!(
            "dshbox-plugin-marker-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let mut config = crate::app_state::Config::load();
        config.root = root.clone();
        save_install_marker(
            &config,
            "first-plugin",
            Some("first-plugin"),
            PluginMutationKind::Add,
            false,
            None,
        )
        .unwrap();
        assert!(save_install_marker(
            &config,
            "second-plugin",
            Some("second-plugin"),
            PluginMutationKind::Add,
            false,
            None,
        )
        .is_err());
        assert_eq!(install_marker(&config).unwrap().spec, "first-plugin");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn plugin_name_rejects_empty_and_flag_like_input() {
        assert!(checked_plugin_name("dshmarket").is_ok());
        assert_eq!(checked_plugin_name("  plugin  ").unwrap(), "plugin");
        assert!(checked_plugin_name("").is_err());
        assert!(checked_plugin_name("   ").is_err());
        // 以 - 开头的输入会被 dsh CLI 当成 flag（注入风险），必须拒绝
        assert!(checked_plugin_name("--force").is_err());
        assert!(checked_plugin_name("-x").is_err());
        assert!(checked_plugin_name("  --evil  ").is_err());
    }

    #[test]
    fn resolved_marker_convergence_marks_removal_only_when_effective() {
        let root = std::env::temp_dir().join(format!(
            "dshbox-plugin-resolve-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let mut config = crate::app_state::Config::load();
        config.root = root.join("app");
        config.dsh_home = root.join("home");
        let profile = config.dsh_home().join("profiles/web");
        std::fs::create_dir_all(&profile).unwrap();

        // 卸载未生效（依赖仍在）：不记录用户卸载状态，事务标记仍被收敛清理
        std::fs::write(
            profile.join("package.json"),
            r#"{"dependencies":{"dshmarket":"1.0.0"}}"#,
        )
        .unwrap();
        save_install_marker(
            &config,
            "dshmarket",
            Some("dshmarket"),
            PluginMutationKind::Remove,
            true,
            None,
        )
        .unwrap();
        clear_resolved_install_marker_locked(&config, install_marker(&config).unwrap());
        assert!(!market_user_removed(&config, "dshmarket"));
        assert!(install_marker(&config).is_none());

        // 卸载已生效（依赖不在）：补记用户卸载状态后清理标记
        std::fs::write(profile.join("package.json"), r#"{"dependencies":{}}"#).unwrap();
        save_install_marker(
            &config,
            "dshmarket",
            Some("dshmarket"),
            PluginMutationKind::Remove,
            true,
            None,
        )
        .unwrap();
        clear_resolved_install_marker_locked(&config, install_marker(&config).unwrap());
        assert!(market_user_removed(&config, "dshmarket"));
        assert!(install_marker(&config).is_none());
        let _ = std::fs::remove_dir_all(root);
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
        // 1970 年前：epoch 为负会回绕成巨大 u64，必须拒绝
        assert!(parse_rfc3339_epoch("1969-12-31T23:59:59Z").is_none());
        assert!(parse_rfc3339_epoch("0001-01-01T00:00:00Z").is_none());
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
    fn builtin_identity_requires_consent_and_no_user_removal() {
        assert!(builtin_identity(true, true, false));
        // 未授权时，手动安装同名包也由用户自行维护
        assert!(!builtin_identity(false, true, false));
        // 用户卸载过（重装与否）→ 非内置
        assert!(!builtin_identity(true, true, true));
        // 不在维护清单 → 非内置
        assert!(!builtin_identity(true, false, false));
    }

    #[test]
    fn preset_plugins_loads_from_embedded_json() {
        let ids: Vec<&str> = market_pkg_ids().collect();
        assert!(ids.contains(&"dshmarket"));
        assert!(ids.contains(&"dsh-file-upload"));
        assert!(!ids.contains(&"dsh-file-drop"));
        assert!(is_retired_market_pkg("dsh-file-drop"));
        assert!(!ids.is_empty());

        let current: std::collections::HashSet<&str> = ids.iter().copied().collect();
        assert_eq!(current.len(), ids.len());
        let retired: Vec<&str> = retired_market_pkg_ids().collect();
        let unique_retired: std::collections::HashSet<&str> = retired.iter().copied().collect();
        assert_eq!(unique_retired.len(), retired.len());
        assert!(current.is_disjoint(&unique_retired));
    }

    #[test]
    fn replacement_inherits_user_removal_and_blocks_parallel_install() {
        use std::time::{SystemTime, UNIX_EPOCH};

        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "dshbox-plugin-replacement-{}-{nonce}",
            std::process::id()
        ));
        let mut config = crate::app_state::Config::load();
        config.root = root.join("app");
        config.dsh_home = root.join("home");

        try_mark_user_removed(&config, "dsh-file-drop").unwrap();
        assert!(effective_market_user_removed(&config, "dsh-file-upload"));
        assert!(
            reinstallable_builtin_plugins(&config, &std::collections::HashSet::new())
                .iter()
                .any(|plugin| plugin.id == "dsh-file-upload")
        );

        let profile = config.dsh_home().join("profiles/web");
        std::fs::create_dir_all(&profile).unwrap();
        std::fs::write(
            profile.join("package.json"),
            r#"{"dependencies":{"dsh-file-drop":"^1.0.0"}}"#,
        )
        .unwrap();
        assert_eq!(
            installed_replacement_predecessor(&config, "dsh-file-upload"),
            Some("dsh-file-drop")
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn removed_builtin_is_reinstallable_only_while_missing() {
        use std::collections::HashSet;
        use std::time::{SystemTime, UNIX_EPOCH};

        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "dshbox-reinstallable-builtin-{}-{nonce}",
            std::process::id()
        ));
        let mut config = crate::app_state::Config::load();
        config.root = root.clone();
        let mut installed = HashSet::new();

        assert!(reinstallable_builtin_plugins(&config, &installed).is_empty());
        try_mark_user_removed(&config, "dshmarket").unwrap();

        let available = reinstallable_builtin_plugins(&config, &installed);
        let market = available
            .iter()
            .find(|plugin| plugin.id == "dshmarket")
            .expect("removed built-in should remain available for manual reinstall");
        assert_eq!(market.spec, "dshmarket");
        assert!(!market.description_zh.is_empty());
        assert!(!market.description_en.is_empty());
        assert!(market.homepage.starts_with("https://github.com/"));
        // 卸载标记仍在，因此手动装回后不会重新获得内置身份。
        assert!(!builtin_identity(
            true,
            true,
            market_user_removed(&config, "dshmarket")
        ));

        installed.insert("dshmarket".to_string());
        assert!(reinstallable_builtin_plugins(&config, &installed)
            .iter()
            .all(|plugin| plugin.id != "dshmarket"));
        let _ = std::fs::remove_dir_all(root);
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
    fn preset_install_state_requires_dependency_package_and_bundle() {
        use std::time::{SystemTime, UNIX_EPOCH};

        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "dshbox-market-state-{}-{nonce}",
            std::process::id()
        ));
        let mut config = crate::app_state::Config::load();
        config.root = root.join("app");
        config.dsh_home = root.join("home");
        let profile = config.dsh_home().join("profiles/web");
        let package_dir = profile.join("node_modules/dshmarket");
        std::fs::create_dir_all(&package_dir).unwrap();

        assert_eq!(
            market_install_state(&config, "dshmarket"),
            MarketInstallState::MissingDependency
        );

        std::fs::write(
            profile.join("package.json"),
            r#"{"dependencies":{"dshmarket":"^1.0.0"},"dsh":{"profile":{"bundles":[]}}}"#,
        )
        .unwrap();
        assert_eq!(
            market_install_state(&config, "dshmarket"),
            MarketInstallState::MissingPackage
        );

        std::fs::write(package_dir.join("package.json"), r#"{"name":"dshmarket"}"#).unwrap();
        assert_eq!(
            market_install_state(&config, "dshmarket"),
            MarketInstallState::MissingBundleDeclaration
        );

        std::fs::write(
            package_dir.join("package.json"),
            r#"{"name":"dshmarket","dsh":{"bundle":{"patch":"cordis.patch.yml"}}}"#,
        )
        .unwrap();
        assert_eq!(
            market_install_state(&config, "dshmarket"),
            MarketInstallState::MissingBundleEntry
        );

        std::fs::write(
            profile.join("package.json"),
            r#"{"dependencies":{"dshmarket":"^1.0.0"},"dsh":{"profile":{"bundles":["dshmarket"]}}}"#,
        )
        .unwrap();
        assert_eq!(
            market_install_state(&config, "dshmarket"),
            MarketInstallState::Ready
        );

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn preset_repair_uses_explicit_user_removal_only() {
        assert!(should_bootstrap_market_pkg(
            MarketInstallState::MissingDependency,
            false
        ));
        assert!(should_bootstrap_market_pkg(
            MarketInstallState::MissingBundleEntry,
            false
        ));
        assert!(!should_bootstrap_market_pkg(
            MarketInstallState::MissingDependency,
            true
        ));
        assert!(!should_bootstrap_market_pkg(
            MarketInstallState::Ready,
            false
        ));
    }

    #[test]
    fn retired_cleanup_condition() {
        // 强制下线清理 = 曾授权 && 在下线清单 && 已装 && 仍为内置身份
        assert!(should_retire(true, true, true, false));
        // 从未授权时，手动安装的同名包归用户所有
        assert!(!should_retire(false, true, true, false));
        // 用户卸载过又重装 → 豁免（尊重用户选择）
        assert!(!should_retire(true, true, true, true));
        // 未装（含卸载未重装）→ 无操作
        assert!(!should_retire(true, true, false, false));
        assert!(!should_retire(true, true, false, true));
        // 不在下线清单 → 不动
        assert!(!should_retire(true, false, true, false));
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
