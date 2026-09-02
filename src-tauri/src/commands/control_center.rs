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
pub fn app_dialog_open_usage(app: AppHandle, webview: tauri::Webview) -> Result<(), String> {
    ensure_local_origin(&webview)?;
    crate::control_center::open_usage(&app);
    Ok(())
}

#[tauri::command]
pub async fn session_stats_get(
    app: AppHandle,
    webview: tauri::Webview,
) -> Result<crate::usage::StatsPayload, String> {
    ensure_local_origin(&webview)?;
    if !crate::tray_menu::managed_service_ready(&app) {
        return Err(crate::locale::text(
            "dsh 服务就绪后才能读取会话统计。",
            "Session statistics are available when the dsh service is ready.",
        )
        .into());
    }
    let config = app.state::<AppState>().config();
    tauri::async_runtime::spawn_blocking(move || crate::usage::snapshot(&config))
        .await
        .map_err(|e| {
            crate::locale::owned(
                format!("会话统计任务异常结束：{e}"),
                format!("The session statistics task ended unexpectedly: {e}"),
            )
        })
}

/// 订阅额度快照（阶段 3 的只读入口；缓存优先，空缓存回退同步查询）。
#[tauri::command]
pub async fn usage_subscriptions_get(
    app: AppHandle,
    webview: tauri::Webview,
) -> Result<Vec<crate::usage::SubscriptionSnapshot>, String> {
    ensure_local_origin(&webview)?;
    if !crate::tray_menu::managed_service_ready(&app) {
        return Err(crate::locale::text(
            "dsh 服务就绪后才能读取订阅额度。",
            "Subscription quotas are available when the dsh service is ready.",
        )
        .into());
    }
    let config = app.state::<AppState>().config();
    tauri::async_runtime::spawn_blocking(move || {
        crate::usage::cached_subscriptions().unwrap_or_else(|| crate::usage::subscriptions(&config))
    })
    .await
    .map_err(|e| {
        crate::locale::owned(
            format!("订阅额度查询任务异常结束：{e}"),
            format!("The subscription query task ended unexpectedly: {e}"),
        )
    })
}

/// 供应商账户快照（阶段 2 的只读入口；缓存优先，空缓存回退同步查询）。
#[tauri::command]
pub async fn usage_accounts_get(
    app: AppHandle,
    webview: tauri::Webview,
) -> Result<Vec<crate::usage::AccountSnapshot>, String> {
    ensure_local_origin(&webview)?;
    if !crate::tray_menu::managed_service_ready(&app) {
        return Err(crate::locale::text(
            "dsh 服务就绪后才能读取账户信息。",
            "Account information is available when the dsh service is ready.",
        )
        .into());
    }
    let config = app.state::<AppState>().config();
    tauri::async_runtime::spawn_blocking(move || {
        if let Some(cached) = crate::usage::cached_accounts() {
            return Ok(cached);
        }
        crate::usage::accounts(&config)
    })
    .await
    .map_err(|e| {
        crate::locale::owned(
            format!("账户查询任务异常结束：{e}"),
            format!("The account query task ended unexpectedly: {e}"),
        )
    })?
}

/// 手动触发账户全量刷新：single-flight 合并、立即返回，结果经
/// `usage-accounts-updated` 事件推送（后台监测线程旁路的同一通道）。
#[tauri::command]
pub fn usage_accounts_refresh(app: AppHandle, webview: tauri::Webview) -> Result<(), String> {
    ensure_local_origin(&webview)?;
    ensure_managed_service(&app)?;
    crate::usage::request_account_refresh(app);
    Ok(())
}

/// 历史用量聚合报告（阶段 1 的只读入口；跨会话按日/模型聚合本地日志）。
#[tauri::command]
pub async fn usage_report_get(
    app: AppHandle,
    webview: tauri::Webview,
) -> Result<crate::usage::UsageReport, String> {
    ensure_local_origin(&webview)?;
    if !crate::tray_menu::managed_service_ready(&app) {
        return Err(crate::locale::text(
            "dsh 服务就绪后才能读取用量统计。",
            "Usage statistics are available when the dsh service is ready.",
        )
        .into());
    }
    let config = app.state::<AppState>().config();
    tauri::async_runtime::spawn_blocking(move || crate::usage::report(&config))
        .await
        .map_err(|e| {
            crate::locale::owned(
                format!("用量统计任务异常结束：{e}"),
                format!("The usage statistics task ended unexpectedly: {e}"),
            )
        })?
}

/// 用量导出：`format` = "csv"（每日明细）或 "json"（全量），保存对话框
/// 选路径后写文件；用户取消对话框视为成功（不报错）。
#[tauri::command]
pub async fn usage_export(
    app: AppHandle,
    webview: tauri::Webview,
    format: String,
) -> Result<(), String> {
    ensure_local_origin(&webview)?;
    if !crate::tray_menu::managed_service_ready(&app) {
        return Err(crate::locale::text(
            "dsh 服务就绪后才能导出用量统计。",
            "Usage export is available when the dsh service is ready.",
        )
        .into());
    }
    let config = app.state::<AppState>().config();
    tauri::async_runtime::spawn_blocking(move || {
        let report = crate::usage::report(&config)?;
        let today = crate::usage::day_key_now();
        let (content, file_name) = match format.as_str() {
            "csv" => (
                crate::usage::export::daily_csv(&report),
                format!("dshbox-usage-daily-{today}.csv"),
            ),
            "json" => (
                crate::usage::export::export_json(&report),
                format!("dshbox-usage-export-{today}.json"),
            ),
            other => {
                return Err(crate::locale::owned(
                    format!("未知的导出格式：{other}"),
                    format!("Unknown export format: {other}"),
                ))
            }
        };
        use tauri_plugin_dialog::DialogExt;
        let mut builder = app.dialog().file().set_file_name(&file_name);
        if let Some(window) = crate::main_window(&app) {
            if window.is_visible().unwrap_or(false) {
                builder = builder.set_parent(&window);
            }
        }
        let Some(dest) = builder
            .blocking_save_file()
            .and_then(|d| d.into_path().ok())
        else {
            return Ok(()); // 用户取消
        };
        std::fs::write(&dest, content.as_bytes()).map_err(|e| {
            crate::locale::owned(
                format!("写入导出文件失败：{e}"),
                format!("Failed to write the export file: {e}"),
            )
        })?;
        crate::logging::log(&format!("usage: 已导出 {file_name} → {}", dest.display()));
        Ok(())
    })
    .await
    .map_err(|e| {
        crate::locale::owned(
            format!("导出任务异常结束：{e}"),
            format!("The export task ended unexpectedly: {e}"),
        )
    })?
}

/// 今日用量消耗速度预测（缓存优先；预警后台任务每 10 分钟刷新一次）。
#[tauri::command]
pub fn usage_prediction_get(
    app: AppHandle,
    webview: tauri::Webview,
) -> Result<crate::usage::PredictionPayload, String> {
    ensure_local_origin(&webview)?;
    if !crate::tray_menu::managed_service_ready(&app) {
        return Err(crate::locale::text(
            "dsh 服务就绪后才能读取用量预测。",
            "Usage prediction is available when the dsh service is ready.",
        )
        .into());
    }
    let config = app.state::<AppState>().config();
    Ok(crate::usage::cached_payload(&config))
}

/// 当前会话路由上下文（只读入口；无活动会话时三字段全 null）。
#[tauri::command]
pub async fn usage_session_context_get(
    app: AppHandle,
    webview: tauri::Webview,
) -> Result<crate::usage::SessionContext, String> {
    ensure_local_origin(&webview)?;
    if !crate::tray_menu::managed_service_ready(&app) {
        return Err(crate::locale::text(
            "dsh 服务就绪后才能读取会话上下文。",
            "The session context is available when the dsh service is ready.",
        )
        .into());
    }
    let config = app.state::<AppState>().config();
    tauri::async_runtime::spawn_blocking(move || crate::usage::session_context(&config))
        .await
        .map_err(|e| {
            crate::locale::owned(
                format!("会话上下文任务异常结束：{e}"),
                format!("The session context task ended unexpectedly: {e}"),
            )
        })
}

/// 余额弹窗内“刷新”按钮：后台重新查询，结果经轮询通道返回。
/// 不清空旧结果：刷新期间弹窗继续显示上次数据。
#[tauri::command]
pub fn app_dialog_refresh_balance(app: AppHandle, webview: tauri::Webview) -> Result<(), String> {
    ensure_local_origin(&webview)?;
    if !crate::tray_menu::action_enabled(&app, "balance") {
        return Err(crate::locale::text(
            "余额由外部 dsh 管理。",
            "Balance is managed by the external dsh service.",
        )
        .into());
    }
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
