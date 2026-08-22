//! 内置插件清单、身份、引导、退避与定时同步。

use super::*;

// —— 内置预装包（自动预装与每日版本同步） ——

/// 单个内置插件的静态信息（对应 resources/builtin-plugins.json 条目）。
#[derive(serde::Deserialize, Clone)]
struct PresetPlugin {
    /// 主键 / 存储标记键 / UI builtin 判定键（即实际包名）
    id: String,
    /// 传给 `dsh plugin add` 的依赖形式（npm 包名或 git 依赖形式）；
    /// 与 id 可不一致（如 scoped 包 `@scope/name`）
    spec: String,
}

/// 解析 resources/builtin-plugins.json（编译期嵌入，运行期零 IO）。
/// 文件缺失/损坏时回落到空清单，绝不因清单问题阻断启动。
/// OnceLock 缓存避免热路径重复反序列化。
fn preset_plugins() -> &'static [PresetPlugin] {
    static CACHE: std::sync::OnceLock<Vec<PresetPlugin>> = std::sync::OnceLock::new();
    CACHE.get_or_init(|| {
        const JSON: &str = include_str!("../../resources/builtin-plugins.json");
        serde_json::from_str(JSON).unwrap_or_default()
    })
}

/// 内置包名列表（id = 实际包名，也是 state.json 中维护标记的稳定键）。
pub(super) fn market_pkg_ids() -> impl Iterator<Item = &'static str> {
    preset_plugins().iter().map(|p| p.id.as_str())
}

/// 是否内置包名（按 id 匹配）。
pub(super) fn is_market_pkg(name: &str) -> bool {
    market_pkg_ids().any(|p| p == name)
}

/// 按 id 取安装 spec（`dsh plugin add` 的依赖形式）。未找到回退用 id 本身。
pub(super) fn market_spec(id: &str) -> String {
    preset_plugins()
        .iter()
        .find(|p| p.id == id)
        .map(|p| p.spec.clone())
        .unwrap_or_else(|| id.to_string())
}

/// 更新必须绑定版本检查得到的精确版本，不能再次使用 latest/spec 范围；否则
/// registry tag 在检查与安装之间移动时，冷却校验与安装结果会指向不同版本。
pub(super) fn market_version_spec(id: &str, version: &str) -> Option<String> {
    semver::Version::parse(version).ok()?;
    Some(format!("{id}@{version}"))
}

/// 强制下线清单：曾内置、需要从用户机器移除的包（安全缺陷/与 DSHBox
/// 冲突等）。启动引导时检测到"已装且仍为内置身份"即自动卸载；用户卸载
/// 过又手动重装的包（market_user_removed 标记）豁免——尊重用户选择。
/// 初始为空，未来需要撤回插件时把包名加进来即可（发版生效）。
/// 约束：同一包不能同时出现在内置清单与 MARKET_REMOVED。
pub(super) const MARKET_REMOVED: &[&str] = &[];
/// 版本检查门控间隔（24 小时）。
pub(super) const MARKET_CHECK_INTERVAL: u64 = 86_400;
/// 引导（首次安装）失败后的重试退避：退避期内启动不再重试，避免
/// 每次启动都刷失败日志（上游 supply-chain 策略拦截是持续性的，
/// 短期反复重试必然失败）。
pub(super) const MARKET_BOOTSTRAP_RETRY: u64 = 6 * 3600;
/// 升级失败的通用退避（1 小时）：网络等瞬时错误，1h 后重试足够。
pub(super) const MARKET_UPGRADE_RETRY: u64 = 3600;
/// supply-chain 冷却期退避（24 小时）：pnpm 的 minimumReleaseAge 策略要求
/// 新发布包满冷却期（实测 24h）才允许安装，期间重试必然失败。
pub(super) const MARKET_SUPPLY_CHAIN_RETRY: u64 = 24 * 3600;

/// pnpm virtual store 错位：DSH_HOME 目录被整体迁移/复制后，
/// node_modules/.modules.yaml 里的 virtualStoreDir 绝对路径失效，
/// pnpm 拒绝一切写操作。可自愈：备份 node_modules 后让 pnpm 全新重建。
/// 同时匹配错误码行与详细说明行（pnpm 不同版本措辞有差异）。
pub(super) fn is_virtual_store_error(detail: &str) -> bool {
    detail.contains("ERR_PNPM_UNEXPECTED_VIRTUAL_STORE")
        || detail.contains("Unexpected virtual store location")
        || detail.contains("symlinked from the virtual store directory")
}

/// 根因检测：node_modules/.modules.yaml 的 virtualStoreDir 指向的路径
/// 已不存在、或与当前 DSH_HOME 不符（目录被迁移/重命名后元数据失效）。
/// 比错误文本匹配更可靠——pnpm 新版本可能只输出堆栈尾部、不包含错误码行。
pub(super) fn virtual_store_stale(config: &crate::app_state::Config) -> bool {
    let modules_yaml = config
        .dsh_home()
        .join("profiles/web/node_modules/.modules.yaml");
    let Ok(text) = std::fs::read_to_string(&modules_yaml) else {
        return false;
    };
    // .modules.yaml 是 JSON 格式：直接解析 virtualStoreDir 字段
    let Ok(json) = serde_json::from_str::<serde_json::Value>(&text) else {
        return false;
    };
    let Some(vs) = json
        .get("virtualStoreDir")
        .and_then(|v| v.as_str())
        .map(str::to_string)
    else {
        return false;
    };
    // 路径已不存在 = 元数据失效
    if !std::path::Path::new(&vs).exists() {
        return true;
    }
    // 存在但指向旧 DSH_HOME 也为失效；比较前统一分隔符与大小写
    // （Windows 路径大小写不敏感，pnpm 可能写入正/反斜杠混用）
    let expected = config.dsh_home().join("profiles/web/node_modules/.pnpm");
    normalize_path(&vs) != normalize_path(&expected.to_string_lossy())
}

/// 路径规范化：Windows 大小写不敏感 + 正/反斜杠统一，供路径相等比较。
#[cfg(windows)]
pub(super) fn normalize_path(p: &str) -> String {
    p.replace('/', "\\").to_lowercase()
}

#[cfg(not(windows))]
pub(super) fn normalize_path(p: &str) -> String {
    p.to_string()
}

/// pnpm supply-chain 策略拦截（minimumReleaseAge 冷却期）：新发布包在
/// 冷却期内不允许安装，重试无意义，须等冷却期过后。
pub(super) fn is_supply_chain_error(detail: &str) -> bool {
    detail.contains("supply-chain") || detail.contains("minimumReleaseAge")
}

/// 环境拦截（安全软件杀进程等）：Windows 上子进程被外部终止时取不到
/// 退出码（run_dsh_plugin 会在错误文本中附加标记）。此类失败重试无意义，
/// 应长退避并提示用户配置信任，而不是每次启动重试。
pub(super) fn is_environment_block_error(detail: &str) -> bool {
    detail.contains("进程被外部终止") || detail.contains("无退出码")
}

/// 已装包版本（web profile 的 package.json dependencies），未装为 None。
pub(super) fn market_installed_version(
    config: &crate::app_state::Config,
    pkg: &str,
) -> Option<String> {
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
pub(super) fn market_latest_info(pkg: &str) -> Option<(String, u64)> {
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
pub(super) fn in_release_cooldown(published_epoch: u64, now: u64) -> bool {
    now.saturating_sub(published_epoch) < MARKET_SUPPLY_CHAIN_RETRY
}

/// 解析 npm registry 的发布时间（RFC3339 UTC，形如
/// `2026-08-19T01:16:46.446Z`）为 epoch 秒。仅支持 UTC（registry 恒定
/// 输出 UTC），无需引入时间库。
pub(super) fn parse_rfc3339_epoch(s: &str) -> Option<u64> {
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

pub(super) fn market_last_check(root: &std::path::Path) -> Option<u64> {
    crate::app_state::load_state_value(root, "market_last_check")?.as_u64()
}

pub(super) fn market_check_due(config: &crate::app_state::Config) -> bool {
    let now = market_unix_now();
    market_last_check(&config.root)
        .map(|t| now.saturating_sub(t) >= MARKET_CHECK_INTERVAL)
        .unwrap_or(true)
}

pub(super) fn market_unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

pub(super) fn market_mark_checked(config: &crate::app_state::Config) {
    let _ = crate::app_state::save_state_value(
        &config.root,
        "market_last_check",
        serde_json::json!(market_unix_now()),
    );
}

/// 某内置包是否曾成功引导安装（按包粒度）。曾装过的包被用户卸载后
/// 不再自动重装（尊重卸载意图，与 README 承诺一致）。
pub(super) fn market_pkg_bootstrapped(config: &crate::app_state::Config, pkg: &str) -> bool {
    crate::app_state::load_state_value(&config.root, &format!("market_bootstrapped_{pkg}"))
        .and_then(|value| value.as_bool())
        .unwrap_or(false)
}

pub(super) fn market_mark_pkg_bootstrapped(config: &crate::app_state::Config, pkg: &str) {
    let _ = crate::app_state::save_state_value(
        &config.root,
        &format!("market_bootstrapped_{pkg}"),
        serde_json::json!(true),
    );
}

/// 用户是否主动卸载过该内置包（plugins::remove 卸载内置包时写入，重装
/// 不清除）。存在此标记 = 用户放弃内置待遇：不再显示内置标签、不再
/// 自动更新、强制下线清理豁免。
pub(super) fn market_user_removed(config: &crate::app_state::Config, pkg: &str) -> bool {
    crate::app_state::load_state_value(&config.root, &format!("market_user_removed_{pkg}"))
        .and_then(|value| value.as_bool())
        .unwrap_or(false)
}

pub(super) fn market_mark_user_removed(config: &crate::app_state::Config, pkg: &str) {
    let _ = crate::app_state::save_state_value(
        &config.root,
        &format!("market_user_removed_{pkg}"),
        serde_json::json!(true),
    );
}

/// 内置身份判定：包在当前维护清单中，且用户未曾主动卸载过。
/// （不含 bootstrapped 条件——引导从未成功的包仍需显示重装入口。）
pub(super) fn builtin_identity(in_market: bool, user_removed: bool) -> bool {
    in_market && !user_removed
}

/// 强制下线清理条件：在下线清单、已安装、且仍为内置身份。
/// 用户卸载过又手动重装的包豁免——尊重用户对已装插件的所有权。
pub(super) fn should_retire(in_removed: bool, installed: bool, user_removed: bool) -> bool {
    in_removed && installed && !user_removed
}

/// 引导失败退避时间戳：上次引导失败时写入 `now + MARKET_BOOTSTRAP_RETRY`，
/// 该时刻前启动不再重试。
pub(super) fn market_bootstrap_retry_due(config: &crate::app_state::Config) -> bool {
    let retry_at = crate::app_state::load_state_value(&config.root, "market_bootstrap_retry_at")
        .and_then(|value| value.as_u64());
    retry_at.map(|t| market_unix_now() >= t).unwrap_or(true)
}

pub(super) fn market_mark_bootstrap_retry(config: &crate::app_state::Config) {
    let _ = crate::app_state::save_state_value(
        &config.root,
        "market_bootstrap_retry_at",
        serde_json::json!(market_unix_now() + MARKET_BOOTSTRAP_RETRY),
    );
}

/// 升级失败退避：退避期内跳过版本同步（不落检查门控，到期后自动恢复）。
/// 通用失败 1h；supply-chain 冷却期 24h（minimumReleaseAge 实测 24h）。
pub(super) fn market_upgrade_retry_due(config: &crate::app_state::Config) -> bool {
    let retry_at = crate::app_state::load_state_value(&config.root, "market_upgrade_retry_at")
        .and_then(|value| value.as_u64());
    retry_at.map(|t| market_unix_now() >= t).unwrap_or(true)
}

pub(super) fn market_mark_upgrade_retry(config: &crate::app_state::Config, backoff_secs: u64) {
    let _ = crate::app_state::save_state_value(
        &config.root,
        "market_upgrade_retry_at",
        serde_json::json!(market_unix_now() + backoff_secs),
    );
}

/// `None` 表示首次引导仍未提交选择；只有显式 true/false 才生效。
pub(super) fn builtin_plugins_consent(config: &crate::app_state::Config) -> Option<bool> {
    crate::app_state::load_state_value(&config.root, "builtin_plugins_enabled")
        .and_then(|value| value.as_bool())
}

pub(super) fn builtin_plugins_enabled(config: &crate::app_state::Config) -> bool {
    builtin_plugins_consent(config).unwrap_or(false)
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
        // 必须等用户明确选择，不能把“尚未提交”误当作默认开启。
        while builtin_plugins_consent(&config).is_none() {
            std::thread::sleep(std::time::Duration::from_secs(1));
        }
        // 未安装的包逐个安装（按包记录引导完成：曾装过、被用户主动卸载的
        // 包不再自动重装）；上次引导失败后的退避期内直接跳过：上游
        // supply-chain 策略拦截是持续性的，短期反复重试必然失败，只会刷日志。
        if bootstrap_market_pkgs(&app, &config) {
            crate::logging::log("market: 内置插件已变更，将在会话空闲时应用");
            mark_plugin_changes(&app, true);
        }
        // 版本同步：首次延迟 90s（避开启动期——安全软件弹窗/网络波动，
        // 不在用户刚打开应用时打扰），此后每 24h 循环（应用常驻期间持续
        // 生效；应用退出线程随之结束，下次启动重新开始）。
        std::thread::sleep(std::time::Duration::from_secs(90));
        loop {
            if builtin_plugins_enabled(&config)
                && market_check_due(&config)
                && sync_market_versions(&app, &config)
            {
                crate::logging::log("market: 插件升级完成，将在会话空闲时应用");
                mark_plugin_changes(&app, true);
            }
            std::thread::sleep(std::time::Duration::from_secs(MARKET_CHECK_INTERVAL));
        }
    });
}

/// 未安装的内置包逐个安装。返回是否有新装包（调用方据此重启服务）。
pub(super) fn bootstrap_market_pkgs(app: &AppHandle, config: &crate::app_state::Config) -> bool {
    if !market_bootstrap_retry_due(config) {
        return false;
    }
    let mut installed_any = false;
    let mut failed = false;
    // 先清理强制下线清单：已装且仍为内置身份的包自动卸载。
    // 失败计入引导退避（6h 后重试），避免每次启动刷失败日志。
    let (removed_any, cleanup_failed) = remove_retired_market_pkgs(app, config);
    if cleanup_failed {
        failed = true;
    }
    if !builtin_plugins_enabled(config) {
        crate::logging::log("market: 用户未启用内置插件，跳过自动安装");
        return removed_any;
    }
    for pkg in market_pkg_ids() {
        if market_installed_version(config, pkg).is_some() {
            // 已装（含用户手动安装）即视为该包引导完成
            market_mark_pkg_bootstrapped(config, pkg);
            continue;
        }
        if market_pkg_bootstrapped(config, pkg) {
            continue; // 曾装过、用户主动卸载 → 尊重卸载意图，不重装
        }
        crate::logging::log(&format!("market: 自动安装内置包 {pkg}"));
        match run_dsh_plugin_auto(app, &["add", &market_spec(pkg)]) {
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
pub(super) fn remove_retired_market_pkgs(
    app: &AppHandle,
    config: &crate::app_state::Config,
) -> (bool, bool) {
    if MARKET_REMOVED.is_empty() {
        return (false, false);
    }
    let mut removed_any = false;
    let mut failed = false;
    for pkg in MARKET_REMOVED {
        if is_market_pkg(pkg) {
            // 配置错误防抖：同一包不能同时在维护与下线清单，否则每次
            // 启动"卸载→引导重装"抖动；跳过并提示
            crate::logging::log(&format!(
                "market: 配置错误：{pkg} 同时存在于内置清单与 MARKET_REMOVED，跳过清理"
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
pub(super) fn sync_market_versions(app: &AppHandle, config: &crate::app_state::Config) -> bool {
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
    for pkg in market_pkg_ids() {
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
        let Some(target) = market_version_spec(pkg, &latest) else {
            check_complete = false;
            crate::logging::log(&format!("market: {pkg} 返回了无效版本 {latest}，跳过"));
            continue;
        };
        crate::logging::log(&format!("market: 升级 {pkg} 到 {latest}"));
        match run_dsh_plugin_auto(app, &["add", &target]) {
            Ok(_) => {
                // 必须等于检查过的精确版本；仅判断“不再等于旧版”会把错误版本
                // 或 tag 漂移误报成成功。
                let actual = market_installed_version(config, pkg);
                if actual.as_deref() != Some(latest.as_str()) {
                    check_complete = false;
                    crate::logging::log(&format!(
                        "market: {pkg} 升级版本不符（实际 {}，预期 {latest}），退避后重试",
                        actual.as_deref().unwrap_or("未知")
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
