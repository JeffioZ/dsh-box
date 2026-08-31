//! window menu IPC 转发。

use super::*;

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

/// 关闭按钮：close() 触发 CloseRequested，由 close_behavior 决定隐藏到托盘或退出。
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

/// 状态栏页面初始化完成回报：与 titlebar_ready 同款就绪握手语义。
#[tauri::command]
pub fn statusbar_ready(webview: tauri::Webview) -> Result<(), String> {
    ensure_local_origin(&webview)?;
    crate::titlebar::mark_statusbar_ready();
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
    app: AppHandle,
    webview: tauri::Webview,
    tray_surface: bool,
) -> Result<Vec<crate::tray_menu::TrayMenuItem>, String> {
    ensure_local_origin(&webview)?;
    Ok(crate::tray_menu::contextual_items(&app, tray_surface))
}

/// 两处菜单共用动作分发。
#[tauri::command]
pub fn menu_choose(app: AppHandle, webview: tauri::Webview, id: String) -> Result<(), String> {
    ensure_local_origin(&webview)?;
    crate::logging::log(&format!("menu: 选择 {id}"));
    crate::tray_menu::run_action(&app, &id);
    Ok(())
}

/// 托盘菜单通过 Escape 请求关闭；统一走 Rust 生命周期，确保外部点击、
/// 菜单选择与键盘关闭都使用同一退场动画和竞态保护。
#[tauri::command]
pub fn tray_menu_close(app: AppHandle, webview: tauri::Webview) -> Result<(), String> {
    ensure_local_origin(&webview)?;
    crate::tray_menu::hide_menu(&app);
    Ok(())
}

/// 更新 Win11 贴边浮层覆盖层位置（按钮矩形为标题栏视口内的 CSS 像素；
/// Rust 侧叠加 webview 偏移换算到窗口客户区）。非 Windows 平台 no-op。
#[tauri::command]
pub fn snap_overlay_update(
    window: tauri::Window,
    webview: tauri::Webview,
    x: i32,
    y: i32,
    width: i32,
    height: i32,
) -> Result<(), String> {
    ensure_local_origin(&webview)?;
    #[cfg(windows)]
    crate::snap_layout::update(&webview, &window, x, y, width, height);
    // cfg 剔除实现后参数悬空：显式消音，避免非 Windows CI 的 -D warnings
    #[cfg(not(windows))]
    let _ = (window, x, y, width, height);
    Ok(())
}

/// 移除贴边浮层覆盖层（标题栏页面卸载时调用）。非 Windows 平台 no-op。
#[tauri::command]
pub fn snap_overlay_detach(window: tauri::Window, webview: tauri::Webview) -> Result<(), String> {
    ensure_local_origin(&webview)?;
    #[cfg(windows)]
    crate::snap_layout::detach(&window);
    #[cfg(not(windows))]
    let _ = window;
    Ok(())
}
