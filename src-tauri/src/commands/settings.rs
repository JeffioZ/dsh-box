//! settings IPC 转发。

use super::*;

// ---------- 设置页（统一弹窗内三开关） ----------

/// 设置页开关状态快照。
#[derive(serde::Serialize)]
pub struct SettingsState {
    pub api_key_set: bool,
    /// 环境变量优先级高于凭据文件；为 true 时设置页只读，避免“保存成功但不生效”。
    pub api_key_external: bool,
    /// settings.yaml 是否已有自定义模型路由（llm-pi-ai 段含 providers 键）。
    pub model_config_set: bool,
    pub autostart: bool,
    pub hide_tool_calls: bool,
    pub hide_stats_line: bool,
    pub hide_statusbar: bool,
    pub hide_balance: bool,
    pub auto_update_plugins: bool,
    /// 主窗口不可见时，任务完成后是否发系统通知。
    pub task_notifications: bool,
    /// dsh 更新通道："latest" 或 "next"
    pub dsh_update_channel: String,
    pub close_behavior: String,
    pub launch_behavior: String,
    /// 外部服务的数据目录与生命周期不归 DSHBox 管理。
    pub external_service: bool,
}

fn settings_state(app: &AppHandle) -> SettingsState {
    let config = app.state::<AppState>().config();
    let api_key_external = ["DSH_BOX_API_KEY", "DEEPSEEK_API_KEY"]
        .iter()
        .any(|name| std::env::var(name).is_ok_and(|value| !value.trim().is_empty()));
    SettingsState {
        api_key_set: api_key_external || crate::credentials::has(&config, "DEEPSEEK_API_KEY"),
        api_key_external,
        model_config_set: crate::model_config::has_custom_providers(&config),
        autostart: crate::autostart::is_enabled(),
        hide_tool_calls: config.hide_tool_calls,
        hide_stats_line: config.hide_stats_line,
        hide_statusbar: config.hide_statusbar,
        hide_balance: config.hide_balance,
        auto_update_plugins: config.auto_update_plugins,
        task_notifications: config.task_notifications,
        dsh_update_channel: config.dsh_update_channel.clone(),
        close_behavior: config.close_behavior.clone(),
        launch_behavior: config.launch_behavior.clone(),
        external_service: app.state::<AppState>().service_ownership().is_external(),
    }
}

/// 保存或清除 DeepSeek API Key。密钥仅进入 dsh 凭据文件，不进入 config.json，
/// 返回值也只暴露“是否已配置”，绝不回传密钥内容。
#[tauri::command]
pub fn set_deepseek_api_key(
    app: AppHandle,
    webview: tauri::Webview,
    api_key: Option<String>,
) -> Result<SettingsState, String> {
    ensure_local_origin(&webview)?;
    ensure_local_service_scope(&app)?;
    let current = settings_state(&app);
    if current.api_key_external {
        return Err(crate::locale::text(
            "当前密钥由环境变量管理，请在环境变量中修改。",
            "The current key is managed by an environment variable. Change it there.",
        )
        .into());
    }
    let config = app.state::<AppState>().config();
    match api_key.map(|value| value.trim().to_string()) {
        Some(value) if !value.is_empty() => {
            crate::onboarding::validate_api_key(&value)?;
            crate::credentials::save(&config, "DEEPSEEK_API_KEY", &value)?;
            crate::logging::log("settings: DeepSeek API Key 已更新");
        }
        _ => {
            crate::credentials::remove_saved(&config, "DEEPSEEK_API_KEY")?;
            crate::logging::log("settings: DeepSeek API Key 已清除");
        }
    }
    // 清掉余额弹窗的旧成功缓存，并让状态栏立即反映新凭据状态。
    app.state::<AppState>().set_last_balance(None);
    crate::balance::refresh_once(app.clone());
    let settings = settings_state(&app);
    crate::emit_signed(&app, "settings-changed", &settings);
    Ok(settings)
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
            crate::autostart::set_enabled(value)?;
        }
        "hide_tool_calls" => {
            state.set_hide_tool_calls(value)?;
            crate::apply_hide_tools(&app);
        }
        "hide_stats_line" => {
            state.set_hide_stats_line(value)?;
            crate::apply_hide_stats(&app);
            // 互斥：状态栏统计区随开关即时显示/隐藏（不等下一个轮询周期）
            crate::usage::refresh_once(app.clone());
        }
        "hide_statusbar" => {
            state.set_hide_statusbar(value)?;
            // 即时生效：重新同步三区块边界（隐藏时状态区 0 高、主区到底）。
            crate::titlebar::sync_bounds(&app);
        }
        "hide_balance" => {
            state.set_hide_balance(value)?;
            // 即时生效：余额 chip 显示/隐藏由状态栏前端据此渲染
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
    // 广播给其他内建窗口（状态栏据此隐藏/显示余额 chip）
    let st = settings_state(&app);
    crate::emit_signed(&app, "settings-changed", &st);
    Ok(st)
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
