//! Tauri 命令层：启动页/托盘通过 IPC 调用的全部命令（无业务实现，仅转发）。
//! 安全：所有命令校验调用来源，拒绝从 dsh 远程页面（http://127.0.0.1:*）发起的调用，
//! 防止官方页面被注入后越权控制桌面端（withGlobalTauri 会向远程页面注入 __TAURI__）。

use tauri::{AppHandle, Manager};

use crate::app_state::{self, AppState};
use crate::{dsh, logging, processes, updater};

/// 校验命令调用来源：titlebar/启动页（tauri://localhost）允许；dsh 页面拒绝。
fn ensure_local_origin(webview: &tauri::Webview) -> Result<(), String> {
    let url = webview.url().map_err(|e| e.to_string())?;
    if url.as_str().starts_with("http://127.0.0.1:") {
        logging::log(&format!("ipc: 拒绝远程来源命令调用：{url}"));
        Err("此操作不允许从页面发起。".into())
    } else {
        Ok(())
    }
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

/// 余额浮层展开/收起（hover 时扩展标题栏 webview 以承载浮层）。
#[tauri::command]
pub fn titlebar_expand(
    app: AppHandle,
    webview: tauri::Webview,
    expand: bool,
) -> Result<(), String> {
    ensure_local_origin(&webview)?;
    crate::titlebar::set_expanded(&app, expand);
    Ok(())
}

// ---------- 自绘托盘菜单（tray-menu 窗口调用，内容经 tray-menu-open 事件下发） ----------

/// 托盘菜单页面主动拉取条目（事件可能在页面监听就绪前被漏掉）。
#[tauri::command]
pub fn tray_menu_get(
    _app: AppHandle,
    webview: tauri::Webview,
) -> Result<Vec<crate::tray_menu::TrayMenuItem>, String> {
    ensure_local_origin(&webview)?;
    Ok(crate::tray_menu::items())
}

/// 托盘菜单项点击：分发动作并隐藏菜单窗口。
#[tauri::command]
pub fn tray_menu_choose(app: AppHandle, webview: tauri::Webview, id: String) -> Result<(), String> {
    ensure_local_origin(&webview)?;
    crate::logging::log(&format!("tray-menu: 选择 {id}"));
    crate::tray_menu::run_action(&app, &id);
    Ok(())
}

// ---------- 统一自绘弹窗（dialog 窗口调用，内容经事件下发） ----------

/// 标题栏余额 chip 点击：打开余额弹窗。
#[tauri::command]
pub fn app_dialog_open_balance(app: AppHandle, webview: tauri::Webview) -> Result<(), String> {
    ensure_local_origin(&webview)?;
    crate::app_dialog::open_balance(&app);
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

/// 检查更新弹窗轮询拉取：进度文案 + 检查结果 + 更新完成文案。
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
    }))
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
        titlebar_expand,
        tray_menu_get,
        tray_menu_choose,
        app_dialog_open_balance,
        app_dialog_get,
        app_dialog_balance_get,
        app_dialog_check_get,
        app_dialog_close,
        app_dialog_update,
    ]
}
