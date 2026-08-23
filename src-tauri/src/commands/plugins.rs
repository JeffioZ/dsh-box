//! plugins IPC 转发。

use super::*;

// ---------- 插件管理（plugins 窗口调用） ----------

/// 已安装插件列表。
#[tauri::command]
pub fn plugin_list(
    app: AppHandle,
    webview: tauri::Webview,
) -> Result<Vec<crate::plugins::PluginInfo>, String> {
    ensure_local_origin(&webview)?;
    ensure_managed_service(&app)?;
    Ok(crate::plugins::list(&app))
}

/// 社区插件清单（当前未安装的项，供手动安装）。
#[tauri::command]
pub fn plugin_recommended(
    app: AppHandle,
    webview: tauri::Webview,
) -> Result<Vec<crate::plugins::RecommendedPlugin>, String> {
    ensure_local_origin(&webview)?;
    ensure_managed_service(&app)?;
    Ok(crate::plugins::recommended(&app))
}

/// 用户主动卸载后仍可手动装回的内置目录项。
#[tauri::command]
pub fn plugin_reinstallable_builtins(
    app: AppHandle,
    webview: tauri::Webview,
) -> Result<Vec<crate::plugins::ReinstallableBuiltinPlugin>, String> {
    ensure_local_origin(&webview)?;
    ensure_managed_service(&app)?;
    Ok(crate::plugins::reinstallable_builtins(&app))
}

/// npm 搜索插件。
#[tauri::command]
pub async fn plugin_search(
    app: AppHandle,
    webview: tauri::Webview,
    query: String,
) -> Result<Vec<crate::plugins::PluginInfo>, String> {
    ensure_local_origin(&webview)?;
    ensure_managed_service(&app)?;
    tauri::async_runtime::spawn_blocking(move || {
        crate::plugins::search(&query).map(|mut list| {
            // 标注已安装状态（与 list 结果合并）
            let installed: std::collections::HashMap<String, String> = crate::plugins::list(&app)
                .into_iter()
                .filter_map(|p| p.installed.map(|v| (p.name, v)))
                .collect();
            for p in &mut list {
                p.installed = installed.get(&p.name).cloned();
            }
            list
        })
    })
    .await
    .map_err(|e| {
        crate::locale::owned(
            format!("插件搜索任务异常结束：{e}"),
            format!("The plugin search task ended unexpectedly: {e}"),
        )
    })?
}

/// 安装插件（成功后进入待应用状态，可继续批量操作）。
#[tauri::command]
pub async fn plugin_install(
    app: AppHandle,
    webview: tauri::Webview,
    name: String,
) -> Result<(), String> {
    ensure_local_origin(&webview)?;
    ensure_managed_service(&app)?;
    tauri::async_runtime::spawn_blocking(move || crate::plugins::install(&app, &name))
        .await
        .map_err(|e| {
            crate::locale::owned(
                format!("插件安装任务异常结束：{e}"),
                format!("The plugin installation task ended unexpectedly: {e}"),
            )
        })?
}

/// 卸载插件（成功后进入待应用状态，可继续批量操作）。
#[tauri::command]
pub async fn plugin_remove(
    app: AppHandle,
    webview: tauri::Webview,
    name: String,
) -> Result<(), String> {
    ensure_local_origin(&webview)?;
    ensure_managed_service(&app)?;
    tauri::async_runtime::spawn_blocking(move || crate::plugins::remove(&app, &name))
        .await
        .map_err(|e| {
            crate::locale::owned(
                format!("插件卸载任务异常结束：{e}"),
                format!("The plugin removal task ended unexpectedly: {e}"),
            )
        })?
}

/// 检查内置插件（dshmarket/dsh-file-drop）是否有新版本（只读）。
#[tauri::command]
pub async fn plugin_updates(
    app: AppHandle,
    webview: tauri::Webview,
) -> Result<Vec<crate::plugins::UpdateStatus>, String> {
    ensure_local_origin(&webview)?;
    ensure_managed_service(&app)?;
    tauri::async_runtime::spawn_blocking(move || crate::plugins::check_updates(&app))
        .await
        .map_err(|e| {
            crate::locale::owned(
                format!("插件更新检查任务异常结束：{e}"),
                format!("The plugin update check ended unexpectedly: {e}"),
            )
        })?
}

/// 手动升级单个插件（绕过退避与门控；成功后进入待应用状态）。
#[tauri::command]
pub async fn plugin_update(
    app: AppHandle,
    webview: tauri::Webview,
    name: String,
) -> Result<crate::plugins::UpdateStatus, String> {
    ensure_local_origin(&webview)?;
    ensure_managed_service(&app)?;
    tauri::async_runtime::spawn_blocking(move || crate::plugins::update_pkg(&app, &name))
        .await
        .map_err(|e| {
            crate::locale::owned(
                format!("插件更新任务异常结束：{e}"),
                format!("The plugin update task ended unexpectedly: {e}"),
            )
        })?
}

#[tauri::command]
pub fn plugin_apply_status(
    app: AppHandle,
    webview: tauri::Webview,
) -> Result<crate::plugins::PluginApplyStatus, String> {
    ensure_local_origin(&webview)?;
    ensure_managed_service(&app)?;
    Ok(crate::plugins::plugin_apply_status())
}

#[tauri::command]
pub fn plugin_apply_changes(
    app: AppHandle,
    webview: tauri::Webview,
) -> Result<crate::plugins::PluginApplyStatus, String> {
    ensure_local_origin(&webview)?;
    ensure_managed_service(&app)?;
    Ok(crate::plugins::apply_plugin_changes(&app))
}
