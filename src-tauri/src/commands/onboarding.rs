//! onboarding IPC 转发。

use super::*;

// ---------- 首次使用配置（启动页调用） ----------

/// 拉取首次配置状态（needs_onboarding 等）。
#[tauri::command]
pub fn get_onboarding_state(
    app: AppHandle,
    webview: tauri::Webview,
) -> Result<crate::onboarding::OnboardingState, String> {
    ensure_local_origin(&webview)?;
    Ok(crate::onboarding::state(&app))
}

/// 保存首次配置并开始使用。
#[tauri::command]
pub fn save_onboarding(
    app: AppHandle,
    webview: tauri::Webview,
    payload: crate::onboarding::OnboardingPayload,
) -> Result<(), String> {
    ensure_local_origin(&webview)?;
    // 凭据与 settings.yaml 写入本地 $DSH_HOME，外部服务模式下禁止冒充外部配置
    ensure_local_service_scope(&app)?;
    crate::onboarding::save(&app, payload)
}

/// 启动页回报首次配置面板已显示：boot 等待据此切换为无限等待
/// （面板显示后等待用户明确完成；未显示时保留 60 秒防卡死兜底）。
#[tauri::command]
pub fn onboarding_shown(_app: AppHandle, webview: tauri::Webview) -> Result<(), String> {
    ensure_local_origin(&webview)?;
    crate::logging::log("boot: 首次配置面板已显示（无限等待用户操作）");
    crate::app_state::mark_onboarding_shown();
    Ok(())
}

/// Rust 主动探活后的显式 ACK。generation 防止旧页面或迟到 IPC 回包替新一轮
/// 探测作答；visible=true 同时完成正常的“面板已显示”回报。
#[tauri::command]
pub fn onboarding_probe_result(
    _app: AppHandle,
    webview: tauri::Webview,
    generation: u64,
    visible: bool,
) -> Result<(), String> {
    ensure_local_origin(&webview)?;
    let _ = crate::app_state::complete_onboarding_probe(generation, visible);
    Ok(())
}

/// 首次配置界面的主题实时预览：只切换窗口主题（不写 settings，
/// 不持久化）——保存时由 onboarding::save 正式应用。
#[tauri::command]
pub fn preview_theme(app: AppHandle, webview: tauri::Webview, theme: String) -> Result<(), String> {
    ensure_local_origin(&webview)?;
    let theme = match theme.as_str() {
        "light" => Some(tauri::Theme::Light),
        "dark" => Some(tauri::Theme::Dark),
        // 跟随系统：清除强制主题，窗口回到系统深浅色
        "system" => None,
        _ => return Err(crate::locale::text("未知主题。", "Unknown theme.").into()),
    };
    if let Some(win) = crate::main_window(&app) {
        let _ = win.set_theme(theme);
    }
    Ok(())
}

/// 首次配置界面的语言实时预览：切换内存语言并立即重推状态栏统计
/// （不写 config、不持久化）——保存时由 onboarding::save 正式应用。
#[tauri::command]
pub fn preview_language(
    app: AppHandle,
    webview: tauri::Webview,
    language: String,
) -> Result<(), String> {
    ensure_local_origin(&webview)?;
    if !matches!(language.as_str(), "zh-CN" | "en") {
        return Err(crate::locale::text("未知语言。", "Unknown language.").into());
    }
    // apply_language 内部：set_preference + 全窗口重译 + 状态栏统计重推
    crate::tray::apply_language(&app, &language);
    Ok(())
}
