//! control center IPC 转发。

use super::*;

// ---------- 统一自绘弹窗（dialog 窗口调用；内容预渲染+轮询为主，事件兜底） ----------

/// 标题栏余额 chip 点击：打开余额弹窗。
#[tauri::command]
pub fn app_dialog_open_balance(app: AppHandle, webview: tauri::Webview) -> Result<(), String> {
    ensure_local_origin(&webview)?;
    crate::control_center::open_balance(&app);
    Ok(())
}

/// 打开设置页（统一弹窗）。
#[tauri::command]
pub fn app_dialog_open_settings(app: AppHandle, webview: tauri::Webview) -> Result<(), String> {
    ensure_local_origin(&webview)?;
    crate::control_center::open_settings(&app);
    Ok(())
}

#[tauri::command]
pub fn app_dialog_open_stats(
    app: AppHandle,
    webview: tauri::Webview,
    group: Option<String>,
) -> Result<(), String> {
    ensure_local_origin(&webview)?;
    crate::control_center::open_stats(&app, group.as_deref());
    Ok(())
}

#[tauri::command]
pub async fn session_stats_get(
    app: AppHandle,
    webview: tauri::Webview,
) -> Result<crate::stats::StatsPayload, String> {
    ensure_local_origin(&webview)?;
    let config = app.state::<AppState>().config();
    tauri::async_runtime::spawn_blocking(move || crate::stats::snapshot(&config))
        .await
        .map_err(|e| format!("会话统计任务异常结束：{e}"))
}

/// 余额弹窗内“刷新”按钮：后台重新查询，结果经轮询通道返回。
/// 不清空旧结果：刷新期间弹窗继续显示上次数据。
#[tauri::command]
pub fn app_dialog_refresh_balance(app: AppHandle, webview: tauri::Webview) -> Result<(), String> {
    ensure_local_origin(&webview)?;
    std::thread::spawn(move || {
        let config = app.state::<AppState>().config();
        let payload = crate::balance::query_balance(&config);
        app.state::<AppState>().set_last_balance(Some(payload));
    });
    Ok(())
}

/// 弹窗页面主动拉取最近一次打开载荷（隐藏窗口收不到 emit 时的兜底）。
#[tauri::command]
pub fn app_dialog_get(
    app: AppHandle,
    webview: tauri::Webview,
) -> Result<Option<crate::control_center::AppDialogOpen>, String> {
    ensure_local_origin(&webview)?;
    Ok(app.state::<AppState>().last_dialog())
}

/// 余额弹窗轮询拉取：最近一次查询结果（None=查询中）。
#[tauri::command]
pub fn app_dialog_balance_get(
    app: AppHandle,
    webview: tauri::Webview,
) -> Result<Option<crate::balance::BalancePayload>, String> {
    ensure_local_origin(&webview)?;
    Ok(app.state::<AppState>().last_balance())
}

/// 检查更新弹窗轮询拉取：进度文案 + 检查结果 + 更新完成文案 + UAC 确认状态。
#[tauri::command]
pub fn app_dialog_check_get(
    app: AppHandle,
    webview: tauri::Webview,
) -> Result<serde_json::Value, String> {
    ensure_local_origin(&webview)?;
    let state = app.state::<AppState>();
    let done = state.update_done();
    Ok(serde_json::json!({
        "progress": state.check_progress(),
        "result": state.last_check(),
        "done": done.map(|(ok, message)| serde_json::json!({ "ok": ok, "message": message })),
        "pwsh_pending": state.pwsh_pending(),
        "updating": state.is_updating(),
    }))
}

/// 弹窗内 UAC 预告的“继续”确认：置位后更新线程继续执行 winget。
#[tauri::command]
pub fn app_dialog_pwsh_confirm(app: AppHandle, webview: tauri::Webview) -> Result<(), String> {
    ensure_local_origin(&webview)?;
    let state = app.state::<AppState>();
    state.set_pwsh_confirmed(true);
    state.set_pwsh_pending(false);
    Ok(())
}

/// 弹窗内导航切到"检查更新"时触发一次检查（不重复 show）。
#[tauri::command]
pub fn app_dialog_run_check(app: AppHandle, webview: tauri::Webview) -> Result<(), String> {
    ensure_local_origin(&webview)?;
    crate::control_center::run_check(&app);
    Ok(())
}

/// 弹窗关闭（✕/Esc/关闭按钮）。
#[tauri::command]
pub fn app_dialog_close(app: AppHandle, webview: tauri::Webview) -> Result<(), String> {
    ensure_local_origin(&webview)?;
    crate::control_center::close(&app);
    Ok(())
}

/// 弹窗内“更新/安装”按钮：后台执行，结果由检查更新弹窗轮询（app_dialog_check_get）拉取。
#[tauri::command]
pub fn app_dialog_update(
    app: AppHandle,
    webview: tauri::Webview,
    which: String,
) -> Result<(), String> {
    ensure_local_origin(&webview)?;
    crate::control_center::apply_update(&app, &which);
    Ok(())
}
