//! settings IPC 转发。

use super::*;

// ---------- 设置页（开关/行为/阈值与更新通道） ----------

/// 设置页开关状态快照。
#[derive(serde::Serialize)]
pub struct SettingsState {
    pub autostart: bool,
    pub hide_tool_calls: bool,
    pub hide_balance: bool,
    pub auto_update_plugins: bool,
    /// 主窗口不可见时，任务完成后是否发系统通知。
    pub task_notifications: bool,
    /// 每日用量提醒阈值（百万 token；None = 关闭）。
    pub usage_token_limit_m: Option<u64>,
    /// dsh 更新通道："latest" 或 "next"
    pub dsh_update_channel: String,
    pub close_behavior: String,
    pub launch_behavior: String,
    /// 外部服务的数据目录与生命周期不归 DSHBox 管理。
    pub external_service: bool,
}

fn settings_state(app: &AppHandle) -> SettingsState {
    let config = app.state::<AppState>().config();
    SettingsState {
        autostart: crate::autostart::is_enabled(),
        hide_tool_calls: config.hide_tool_calls,
        hide_balance: config.hide_balance,
        auto_update_plugins: config.auto_update_plugins,
        task_notifications: config.task_notifications,
        usage_token_limit_m: config.usage_token_limit_m,
        dsh_update_channel: config.dsh_update_channel.clone(),
        close_behavior: config.close_behavior.clone(),
        launch_behavior: config.launch_behavior.clone(),
        external_service: app.state::<AppState>().service_ownership().is_external(),
    }
}

#[tauri::command]
pub fn set_window_behavior(
    app: AppHandle,
    webview: tauri::Webview,
    key: String,
    value: String,
) -> Result<SettingsState, String> {
    ensure_local_origin(&webview)?;
    let state = app.state::<AppState>();
    match key.as_str() {
        "close_behavior" => state.set_close_behavior(&value)?,
        "launch_behavior" => state.set_launch_behavior(&value)?,
        _ => return Err(crate::locale::text("未知设置项。", "Unknown setting.").into()),
    }
    crate::logging::log(&format!("settings: {key}={value}"));
    let settings = settings_state(&app);
    crate::emit_signed(&app, "settings-changed", &settings);
    Ok(settings)
}

#[tauri::command]
pub fn settings_get(app: AppHandle, webview: tauri::Webview) -> Result<SettingsState, String> {
    ensure_local_origin(&webview)?;
    Ok(settings_state(&app))
}

/// 设置页开关切换：应用 + 持久化 + 即时下发到 dsh 页面，返回最新状态。
/// autostart 失败（注册表/文件写入）原样上报，前端就地显示错误。
#[tauri::command]
pub fn settings_set(
    app: AppHandle,
    webview: tauri::Webview,
    key: String,
    value: bool,
) -> Result<SettingsState, String> {
    ensure_local_origin(&webview)?;
    let state = app.state::<AppState>();
    match key.as_str() {
        "autostart" => {
            if let Err(e) = crate::autostart::set_enabled(value) {
                // 错误已回传前端就地提示；这里补后端日志，注册表错误码事后可查
                crate::logging::log(&format!("settings: 开机自启动设置失败：{e}"));
                return Err(e);
            }
        }
        "hide_tool_calls" => {
            state.set_hide_tool_calls(value)?;
            crate::apply_hide_tools(&app);
        }
        "hide_balance" => {
            state.set_hide_balance(value)?;
            // 即时生效：余额 chip 显示/隐藏由标题栏前端据此渲染
        }
        "auto_update_plugins" => {
            ensure_local_service_scope(&app)?;
            state.set_auto_update_plugins(value)?;
        }
        "task_notifications" => {
            state.set_task_notifications(value)?;
        }
        // 更新通道字段不改走 bool 开关逻辑（settings_set 的 value 是 bool，
        // 通道是字符串二选一），由 set_dsh_channel command 处理
        _ => return Err(crate::locale::text("未知设置项。", "Unknown setting.").into()),
    }
    crate::logging::log(&format!("settings: {key}={value}"));
    // 广播给其他内建窗口（标题栏据此隐藏/显示余额 chip）
    let st = settings_state(&app);
    crate::emit_signed(&app, "settings-changed", &st);
    Ok(st)
}

/// 设置每日用量提醒阈值（百万 token；None/0/空 = 关闭）。
#[tauri::command]
pub fn set_usage_token_limit(
    app: AppHandle,
    webview: tauri::Webview,
    limit_m: Option<u64>,
) -> Result<SettingsState, String> {
    ensure_local_origin(&webview)?;
    if limit_m.is_some_and(|m| m == 0 || m > 1_000_000) {
        return Err(crate::locale::text(
            "阈值需在 1–1,000,000（百万 token）之间。",
            "The threshold must be between 1 and 1,000,000 (million tokens).",
        )
        .into());
    }
    let state = app.state::<AppState>();
    state.set_usage_token_limit(limit_m)?;
    crate::logging::log(&format!("settings: usage_token_limit_m={limit_m:?}"));
    let settings = settings_state(&app);
    crate::emit_signed(&app, "settings-changed", &settings);
    Ok(settings)
}

/// 切换 dsh 内核更新通道（latest/next/alpha），持久化到 config.json。
#[tauri::command]
pub fn set_dsh_channel(
    app: AppHandle,
    webview: tauri::Webview,
    channel: String,
) -> Result<SettingsState, String> {
    ensure_local_origin(&webview)?;
    ensure_local_service_scope(&app)?;
    if !matches!(channel.as_str(), "latest" | "next" | "alpha") {
        return Err(crate::locale::text("未知更新通道。", "Unknown update channel.").into());
    }
    let state = app.state::<AppState>();
    if state.config().dsh_update_channel != channel {
        state.set_dsh_update_channel(&channel)?;
    }
    // 与其他设置命令一致：持久化后广播，其他内建窗口据此刷新
    let settings = settings_state(&app);
    crate::emit_signed(&app, "settings-changed", &settings);
    Ok(settings)
}
