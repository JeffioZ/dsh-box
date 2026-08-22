//! Tauri 命令入口：来源校验、基础命令与领域命令注册。
//! 安全：所有命令仅允许内置 App 页面调用；dsh 页面及其他任意来源一律拒绝，
//! 避免 withGlobalTauri 暴露的 IPC 被远程内容用于控制桌面端。

use tauri::{AppHandle, Emitter, Manager};

use crate::app_state::{self, AppState, InstallAction};
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

pub(crate) fn ensure_managed_service(app: &AppHandle) -> Result<(), String> {
    let state = app.state::<AppState>();
    if state.is_updating() {
        return Err(crate::locale::text(
            "更新正在进行，请稍后再试。",
            "An update is in progress. Please try again later.",
        )
        .into());
    }
    match state.service_ownership() {
        crate::app_state::ServiceOwnership::Managed
            if state.phase() == crate::app_state::BootPhase::Ready =>
        {
            Ok(())
        }
        ownership if ownership.is_external() => Err(crate::locale::text(
            "当前连接由外部 dsh 服务管理，请在原服务环境中修改这项配置。",
            "The current connection is managed by an external dsh service. Change this setting in that service's environment.",
        )
        .into()),
        _ => Err(crate::locale::text(
            "本地 dsh 服务尚未就绪，请稍后再试。",
            "The local dsh service is not ready yet. Please try again shortly.",
        )
        .into()),
    }
}

/// 本地配置文件可在 dsh 尚未安装/启动时预先写入；仅外部服务必须隔离。
pub(crate) fn ensure_local_service_scope(app: &AppHandle) -> Result<(), String> {
    if app.state::<AppState>().service_ownership().is_external() {
        return Err(crate::locale::text(
            "当前连接由外部 dsh 服务管理，请在原服务环境中修改这项配置。",
            "The current connection is managed by an external dsh service. Change this setting in that service's environment.",
        )
        .into());
    }
    Ok(())
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
    let state = app.state::<AppState>();
    state.signal_retry();
    Ok(())
}

#[tauri::command]
pub fn choose_service(
    app: AppHandle,
    webview: tauri::Webview,
    generation: u64,
    reuse: bool,
) -> Result<bool, String> {
    ensure_local_origin(&webview)?;
    Ok(app
        .state::<AppState>()
        .request_service_choice(generation, reuse))
}

#[tauri::command]
pub fn use_local_service(app: AppHandle, webview: tauri::Webview) -> Result<(), String> {
    ensure_local_origin(&webview)?;
    dsh::forget_external_service(&app)
}

#[tauri::command]
pub fn cancel_install(
    app: AppHandle,
    webview: tauri::Webview,
    generation: u64,
) -> Result<bool, String> {
    ensure_local_origin(&webview)?;
    Ok(app
        .state::<AppState>()
        .request_install_action(generation, InstallAction::Cancel))
}

#[tauri::command]
pub fn set_install_source(
    app: AppHandle,
    webview: tauri::Webview,
    source: String,
    generation: u64,
) -> Result<bool, String> {
    ensure_local_origin(&webview)?;
    app.state::<AppState>()
        .request_install_source(generation, &source)
}

#[tauri::command]
pub fn startup_transition_done(app: AppHandle, webview: tauri::Webview) -> Result<(), String> {
    ensure_local_origin(&webview)?;
    app.state::<AppState>().finish_startup_transition();
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
pub fn check_updates(app: AppHandle, webview: tauri::Webview) -> Result<(), String> {
    ensure_local_origin(&webview)?;
    let handle = app.clone();
    std::thread::spawn(move || {
        let _ = updater::check_and_report(&handle);
    });
    Ok(())
}

#[tauri::command]
pub fn apply_updates(app: AppHandle, webview: tauri::Webview, which: String) -> Result<(), String> {
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

pub fn invoke_handler() -> impl Fn(tauri::ipc::Invoke<tauri::Wry>) -> bool + Send + Sync + 'static {
    tauri::generate_handler![
        get_status,
        retry_boot,
        choose_service,
        use_local_service,
        cancel_install,
        set_install_source,
        startup_transition_done,
        quit,
        open_logs,
        check_updates,
        apply_updates,
        onboarding::get_onboarding_state,
        onboarding::save_onboarding,
        onboarding::onboarding_shown,
        onboarding::preview_theme,
        onboarding::preview_language,
        model_config::preview_model_import,
        model_config::apply_model_import,
        model_config::export_model_config,
        plugins::plugin_list,
        plugins::plugin_search,
        plugins::plugin_install,
        plugins::plugin_remove,
        plugins::plugin_updates,
        plugins::plugin_update,
        plugins::plugin_apply_status,
        plugins::plugin_apply_changes,
        crate::balance::api_balance,
        window_menu::titlebar_minimize,
        window_menu::titlebar_toggle_maximize,
        window_menu::titlebar_close,
        window_menu::titlebar_is_maximized,
        window_menu::titlebar_ready,
        crate::heartbeat::page_heartbeat,
        window_menu::titlebar_expand,
        window_menu::menu_get,
        window_menu::menu_choose,
        window_menu::tray_menu_close,
        control_center::app_dialog_open_balance,
        control_center::app_dialog_refresh_balance,
        control_center::app_dialog_get,
        control_center::app_dialog_balance_get,
        control_center::app_dialog_check_get,
        control_center::app_dialog_run_check,
        control_center::app_dialog_pwsh_confirm,
        control_center::app_dialog_close,
        control_center::app_dialog_update,
        control_center::app_dialog_open_settings,
        control_center::app_dialog_open_stats,
        control_center::session_stats_get,
        settings::settings_get,
        settings::settings_set,
        settings::set_deepseek_api_key,
        settings::set_dsh_channel,
        settings::set_window_behavior,
    ]
}
mod control_center;
mod model_config;
mod onboarding;
mod plugins;
mod settings;
mod window_menu;
