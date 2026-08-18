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
    run_dsh_plugin(app, &["add", name])?;
    crate::logging::log(&format!("plugins: 已安装 {name}，重启服务生效"));
    restart_service_silently(app);
    Ok(())
}

/// 卸载插件（dsh plugin --profile web remove <pkg>），成功后重启服务。
pub fn remove(app: &AppHandle, name: &str) -> Result<(), String> {
    let name = name.trim();
    if name.is_empty() {
        return Err(
            crate::locale::text("插件名不能为空。", "The package name must not be empty.").into(),
        );
    }
    run_dsh_plugin(app, &["remove", name])?;
    crate::logging::log(&format!("plugins: 已卸载 {name}，重启服务生效"));
    restart_service_silently(app);
    Ok(())
}

/// 重启服务（后台线程；失败仅记日志——插件已写入 profile，下次启动也会加载）。
fn restart_service_silently(app: &AppHandle) {
    let handle = app.clone();
    std::thread::spawn(move || {
        if let Err(e) = crate::updater::restart_service(&handle) {
            crate::logging::log(&format!("plugins: 重启服务失败（插件已保存）：{e}"));
        }
    });
}

// —— 内置预装包（dsh-market + dsh-file-drop）：自动预装与每日版本同步 ——

/// 内置预装包：插件市场（dshmarket）与文件拖拽（dsh-file-drop，BSD-3-Clause，
/// 与桌面壳场景直接相关）。均走 `dsh plugin` CLI 安装，失败静默重试。
const MARKET_PKGS: &[&str] = &["dshmarket", "dsh-file-drop"];
/// 版本检查门控间隔（24 小时）。
const MARKET_CHECK_INTERVAL: u64 = 86_400;
/// 引导（首次安装）失败后的重试退避：退避期内启动不再重试，避免
/// 每次启动都刷失败日志（上游 supply-chain 策略拦截是持续性的，
/// 短期反复重试必然失败）。
const MARKET_BOOTSTRAP_RETRY: u64 = 6 * 3600;

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

/// npm registry 上指定包的最新版本。
fn market_latest_version(pkg: &str) -> Option<String> {
    use std::io::Read;
    let resp = crate::runtime::client()
        .get(&format!("https://registry.npmjs.org/{pkg}/latest"))
        .header("User-Agent", "DSHBox")
        .call()
        .ok()?;
    let mut text = String::new();
    resp.into_body()
        .into_reader()
        .read_to_string(&mut text)
        .ok()?;
    let json: serde_json::Value = serde_json::from_str(&text).ok()?;
    json.get("version")?.as_str().map(String::from)
}

fn market_last_check(root: &std::path::Path) -> Option<u64> {
    let text = std::fs::read_to_string(root.join("config.json")).ok()?;
    let json: serde_json::Value = serde_json::from_str(&text).ok()?;
    json.get("market_last_check")?.as_u64()
}

fn market_check_due(config: &crate::app_state::Config) -> bool {
    let now = market_unix_now();
    market_last_check(&config.root)
        .map(|t| now.saturating_sub(t) > MARKET_CHECK_INTERVAL)
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

/// 引导是否已完成（至少成功安装过一次内置包）。
/// 用户主动卸载后不再自动重装（尊重卸载意图，与 README 承诺一致）。
fn market_bootstrapped(config: &crate::app_state::Config) -> bool {
    let text = std::fs::read_to_string(config.root.join("config.json")).unwrap_or_default();
    serde_json::from_str::<serde_json::Value>(&text)
        .ok()
        .and_then(|j| j.get("market_bootstrapped").and_then(|v| v.as_bool()))
        .unwrap_or(false)
}

fn market_mark_bootstrapped(config: &crate::app_state::Config) {
    let _ = crate::app_state::save_config_value(
        &config.root,
        "market_bootstrapped",
        serde_json::json!(true),
    );
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

/// 内置预装包引导（后台线程）：dsh 服务就绪后——
/// 未安装的包逐个自动安装并重启服务；已安装的每 24h 检查一次 npm
/// 最新版，落后时后台升级（`dsh plugin add` 重复执行即升级语义）并重启。
/// 全部失败静默：安装/升级失败下次启动重试，不阻塞主流程。
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
        // 未安装的包逐个安装（仅在从未成功引导过时尝试：用户主动卸载
        // 后不自动重装）；全部包已存在或安装成功后才标记引导完成。
        // 上次引导失败后的退避期内直接跳过：上游 supply-chain 策略拦截
        // 是持续性的，短期反复重试必然失败，只会刷日志。
        let mut installed_any = false;
        if !market_bootstrapped(&config) && market_bootstrap_retry_due(&config) {
            let mut bootstrap_complete = true;
            for pkg in MARKET_PKGS {
                if market_installed_version(&config, pkg).is_some() {
                    continue;
                }
                crate::logging::log(&format!("market: 自动安装内置包 {pkg}"));
                match run_dsh_plugin(&app, &["add", pkg]) {
                    Ok(_) => {
                        crate::logging::log(&format!("market: {pkg} 安装完成"));
                        installed_any = true;
                    }
                    Err(e) => {
                        bootstrap_complete = false;
                        crate::logging::log(&format!("market: {pkg} 安装失败（退避后重试）：{e}"));
                    }
                }
            }
            if bootstrap_complete {
                market_mark_bootstrapped(&config);
                // 新安装来自 npm latest，无需同次启动再查询；若全部原本已安装，
                // 则保留检查门控原值，让下方正常执行版本同步。
                if installed_any {
                    market_mark_checked(&config);
                }
            } else {
                // 记退避：退避期内启动不再重试，避免刷屏
                market_mark_bootstrap_retry(&config);
            }
        }
        if installed_any {
            crate::logging::log("market: 重启服务使内置包生效");
            restart_service_silently(&app);
        }
        if !market_check_due(&config) {
            return;
        }
        // 已安装包的版本同步（每 24h）；缺失表示用户已卸载，必须跳过。
        // 与上方引导相互独立：dsh-file-drop 装不上（引导失败）不影响这里
        // 对已装 dshmarket 的升级检查——未装包直接 continue，不会计入失败。
        let mut upgraded_any = false;
        let mut check_complete = true;
        for pkg in MARKET_PKGS {
            let Some(installed) = market_installed_version(&config, pkg) else {
                continue;
            };
            let Some(latest) = market_latest_version(pkg) else {
                // 任一查询失败都不落全局门控：下次启动重试。
                check_complete = false;
                crate::logging::log(&format!("market: {pkg} 版本查询失败，跳过本次同步"));
                continue;
            };
            let needs_update = crate::versions::compare_versions(&installed, &latest).is_lt();
            if needs_update {
                crate::logging::log(&format!("market: 升级 {pkg} 到 {latest}"));
                match run_dsh_plugin(&app, &["add", pkg]) {
                    Ok(_) => {
                        crate::logging::log(&format!("market: {pkg} 升级完成"));
                        upgraded_any = true;
                    }
                    Err(e) => {
                        check_complete = false;
                        crate::logging::log(&format!("market: {pkg} 升级失败（下次重试）：{e}"));
                    }
                }
            }
        }
        if check_complete {
            market_mark_checked(&config);
        }
        if upgraded_any {
            crate::logging::log("market: 重启服务使升级生效");
            restart_service_silently(&app);
        }
    });
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
        return Err(if detail.is_empty() {
            crate::locale::text("dsh 插件命令执行失败。", "The dsh plugin command failed.").into()
        } else {
            detail
        });
    }
    Ok(tail)
}
