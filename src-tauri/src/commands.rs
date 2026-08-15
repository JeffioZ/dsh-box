//! Tauri 命令层：启动页/托盘通过 IPC 调用的全部命令（无业务实现，仅转发）。
//! 安全：所有命令仅允许内置 App 页面调用；dsh 页面及其他任意来源一律拒绝，
//! 避免 withGlobalTauri 暴露的 IPC 被远程内容用于控制桌面端。

use tauri::{AppHandle, Manager};

use crate::app_state::{self, AppState};
use crate::{dsh, logging, processes, updater};

/// 校验命令调用来源：只允许 Tauri 内置页面，不依赖可绕过的来源黑名单。
pub(crate) fn ensure_local_origin(webview: &tauri::Webview) -> Result<(), String> {
    let url = webview.url().map_err(|e| e.to_string())?;
    let dev = crate::app_dev_origin(webview.app_handle());
    if crate::is_local_app_url(&url, dev.as_ref()) {
        return Ok(());
    }
    logging::log(&format!("ipc: 拒绝非本地来源命令调用：{url}"));
    Err(crate::locale::text(
        "仅允许应用内置页面调用此操作。",
        "This action can only be invoked from the app's built-in pages.",
    )
    .into())
}

#[tauri::command]
pub fn get_status(
    app: AppHandle,
    webview: tauri::Webview,
) -> Result<app_state::StatusPayload, String> {
    ensure_local_origin(&webview)?;
    Ok(app.state::<AppState>().snapshot())
}

#[tauri::command]
pub fn retry_boot(app: AppHandle, webview: tauri::Webview) -> Result<(), String> {
    ensure_local_origin(&webview)?;
    app.state::<AppState>().signal_retry();
    Ok(())
}

#[tauri::command]
pub fn quit(app: AppHandle, webview: tauri::Webview) -> Result<(), String> {
    ensure_local_origin(&webview)?;
    app.state::<AppState>().set_quitting(true);
    dsh::shutdown(&app);
    app.exit(0);
    Ok(())
}

#[tauri::command]
pub fn open_logs(app: AppHandle, webview: tauri::Webview) -> Result<(), String> {
    ensure_local_origin(&webview)?;
    let dir = app.state::<AppState>().config().logs_dir();
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    processes::open_in_file_manager(&dir);
    Ok(())
}

#[tauri::command]
pub async fn check_updates(app: AppHandle, webview: tauri::Webview) -> Result<(), String> {
    ensure_local_origin(&webview)?;
    let handle = app.clone();
    std::thread::spawn(move || {
        let _ = updater::check_and_report(&handle);
    });
    Ok(())
}

#[tauri::command]
pub async fn apply_updates(
    app: AppHandle,
    webview: tauri::Webview,
    which: String,
) -> Result<(), String> {
    ensure_local_origin(&webview)?;
    // 启动页只提供 dsh 一键更新：pwsh 的 UAC 确认流程只在检查更新弹窗内
    // 可用（此通道直达 updater::apply 会挂起等待确认，启动页没有确认按钮）
    if which != "dsh" {
        return Err(crate::locale::text(
            "此页面仅支持更新 dsh。",
            "Only dsh updates are supported from this page.",
        )
        .into());
    }
    let handle = app.clone();
    std::thread::spawn(move || {
        let _ = updater::apply(&handle, &which);
    });
    Ok(())
}

// ---------- 自绘标题栏（titlebar 子 webview 调用） ----------

#[tauri::command]
pub fn titlebar_minimize(window: tauri::Window, webview: tauri::Webview) -> Result<(), String> {
    ensure_local_origin(&webview)?;
    window.minimize().map_err(|e| e.to_string())
}

/// 最大化/还原切换，返回新状态是否已最大化。
#[tauri::command]
pub fn titlebar_toggle_maximize(
    window: tauri::Window,
    webview: tauri::Webview,
) -> Result<bool, String> {
    ensure_local_origin(&webview)?;
    let maxed = window.is_maximized().map_err(|e| e.to_string())?;
    if maxed {
        window.unmaximize().map_err(|e| e.to_string())?;
    } else {
        window.maximize().map_err(|e| e.to_string())?;
    }
    Ok(!maxed)
}

/// 关闭按钮：close() 触发 CloseRequested，走“最小化到托盘”逻辑。
#[tauri::command]
pub fn titlebar_close(window: tauri::Window, webview: tauri::Webview) -> Result<(), String> {
    ensure_local_origin(&webview)?;
    window.close().map_err(|e| e.to_string())
}

/// 查询窗口是否最大化（标题栏同步按钮图标；拖动标题栏退出最大化也会触发）。
#[tauri::command]
pub fn titlebar_is_maximized(
    window: tauri::Window,
    webview: tauri::Webview,
) -> Result<bool, String> {
    ensure_local_origin(&webview)?;
    window.is_maximized().map_err(|e| e.to_string())
}

/// 标题栏页面初始化完成回报：启动自愈看门狗据此判断页面是否加载成功。
#[tauri::command]
pub fn titlebar_ready(webview: tauri::Webview) -> Result<(), String> {
    ensure_local_origin(&webview)?;
    crate::titlebar::mark_ready();
    Ok(())
}

/// 标题栏浮层（余额浮层/主菜单）展开/收起：hover 时扩展标题栏 webview 以承载浮层。
#[tauri::command]
pub fn titlebar_expand(
    app: AppHandle,
    webview: tauri::Webview,
    expand: bool,
    height: Option<f64>,
) -> Result<(), String> {
    ensure_local_origin(&webview)?;
    crate::titlebar::set_expanded(&app, expand, height);
    Ok(())
}

// ---------- 共享菜单（托盘菜单 + 标题栏主菜单） ----------

/// 两处菜单读取同一模型；模型按托盘/标题栏场景处理少量专属项。
#[tauri::command]
pub fn menu_get(
    _app: AppHandle,
    webview: tauri::Webview,
    tray_surface: bool,
) -> Result<Vec<crate::tray_menu::TrayMenuItem>, String> {
    ensure_local_origin(&webview)?;
    Ok(crate::tray_menu::items(tray_surface))
}

/// 两处菜单共用动作分发。
#[tauri::command]
pub fn menu_choose(app: AppHandle, webview: tauri::Webview, id: String) -> Result<(), String> {
    ensure_local_origin(&webview)?;
    crate::logging::log(&format!("menu: 选择 {id}"));
    crate::tray_menu::run_action(&app, &id);
    Ok(())
}

/// 托盘窗口的语言子菜单展开后需要同步调整原生窗口高度。
#[tauri::command]
pub fn tray_menu_set_language_expanded(
    app: AppHandle,
    webview: tauri::Webview,
    expanded: bool,
) -> Result<(), String> {
    ensure_local_origin(&webview)?;
    crate::tray_menu::set_language_expanded(&app, expanded);
    Ok(())
}

// ---------- 统一自绘弹窗（dialog 窗口调用；内容预渲染+轮询为主，事件兜底） ----------

/// 标题栏余额 chip 点击：打开余额弹窗。
#[tauri::command]
pub fn app_dialog_open_balance(app: AppHandle, webview: tauri::Webview) -> Result<(), String> {
    ensure_local_origin(&webview)?;
    crate::app_dialog::open_balance(&app);
    Ok(())
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
) -> Result<Option<crate::app_dialog::AppDialogOpen>, String> {
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

/// 弹窗关闭（✕/Esc/关闭按钮）。
#[tauri::command]
pub fn app_dialog_close(app: AppHandle, webview: tauri::Webview) -> Result<(), String> {
    ensure_local_origin(&webview)?;
    crate::app_dialog::close(&app);
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
    crate::app_dialog::apply_update(&app, &which);
    Ok(())
}

/// 生成 invoke_handler（由 run() 挂载）。
pub fn invoke_handler() -> impl Fn(tauri::ipc::Invoke<tauri::Wry>) -> bool + Send + Sync + 'static {
    tauri::generate_handler![
        get_status,
        retry_boot,
        quit,
        open_logs,
        check_updates,
        apply_updates,
        crate::balance::api_balance,
        titlebar_minimize,
        titlebar_toggle_maximize,
        titlebar_close,
        titlebar_is_maximized,
        titlebar_ready,
        titlebar_expand,
        menu_get,
        menu_choose,
        tray_menu_set_language_expanded,
        app_dialog_open_balance,
        app_dialog_refresh_balance,
        app_dialog_get,
        app_dialog_balance_get,
        app_dialog_check_get,
        app_dialog_pwsh_confirm,
        app_dialog_close,
        app_dialog_update,
    ]
}
