//! 插件市场：浏览/安装/卸载 dsh 插件（web profile）。
//!
//! 全部经由 dsh CLI 的 `plugin` 子命令（转发 pnpm 到 profile 目录），
//! 不改 dsh 代码；安装/卸载成功后重启服务使插件加载生效。

use serde::Serialize;
use tauri::AppHandle;
use tauri::Manager;

use crate::app_state::AppState;

/// 插件管理窗口 label。
pub const PLUGINS_WINDOW: &str = "plugins";

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

/// 打开插件管理窗口（已存在则聚焦）。
pub fn open(app: &AppHandle) {
    if let Some(w) = app.get_webview_window(PLUGINS_WINDOW) {
        let _ = w.show();
        let _ = w.set_focus();
        return;
    }
    let navigation_app = app.clone();
    match tauri::WebviewWindowBuilder::new(
        app,
        PLUGINS_WINDOW,
        tauri::WebviewUrl::App("plugins.html".into()),
    )
    .title(crate::locale::text("插件管理", "Plugin manager"))
    .inner_size(560.0, 640.0)
    .min_inner_size(420.0, 480.0)
    .resizable(true)
    .on_navigation(move |url| {
        let allowed = crate::is_local_app_url(url, crate::app_dev_origin(&navigation_app).as_ref());
        if !allowed {
            crate::logging::log(&format!("plugins: 已拦截非白名单导航 {url}"));
        }
        allowed
    })
    .build()
    {
        Ok(w) => {
            let _ = w.show();
            crate::logging::log("plugins: 插件管理窗口已打开");
        }
        Err(e) => crate::logging::log(&format!("plugins: 打开失败：{e}")),
    }
}

/// 已安装插件列表：读 web profile 的 package.json dependencies。
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
            out.push(PluginInfo {
                name: name.clone(),
                version: version.clone(),
                description: None,
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
        .header("User-Agent", "DSHDesktop")
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
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("启动 dsh 插件命令失败：{e}"))?;
    // 5 分钟超时（npm 安装可能较慢）；超时杀掉避免线程悬挂
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(300);
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {
                if std::time::Instant::now() > deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(crate::locale::text(
                        "插件操作超时（超过 5 分钟），已中止。",
                        "The plugin operation timed out after 5 minutes and was aborted.",
                    )
                    .into());
                }
                std::thread::sleep(std::time::Duration::from_millis(500));
            }
            Err(e) => return Err(format!("等待插件命令失败：{e}")),
        }
    };
    // 输出（含 stderr 尾部）作为错误详情返回
    let mut stdout = String::new();
    use std::io::Read;
    if let Some(mut out) = child.stdout.take() {
        let _ = out.read_to_string(&mut stdout);
    }
    let mut stderr = String::new();
    if let Some(mut err) = child.stderr.take() {
        let _ = err.read_to_string(&mut stderr);
    }
    if !status.success() {
        let tail = format!("{stdout}\n{stderr}").trim().to_string();
        return Err(if tail.is_empty() {
            crate::locale::text("dsh 插件命令执行失败。", "The dsh plugin command failed.").into()
        } else {
            tail
        });
    }
    Ok(stdout)
}
