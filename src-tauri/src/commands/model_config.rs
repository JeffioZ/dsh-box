//! model config IPC 转发。

use super::*;

// ---------- 模型配置导入（启动页 onboarding 调用） ----------

/// 预览导入的模型配置：解析 + 校验 + 返回 provider 摘要与所需凭据引用
/// （只读，不写盘）。识别失败返回本地化错误，前端就地提示。
#[tauri::command]
pub fn preview_model_import(
    app: AppHandle,
    webview: tauri::Webview,
    yaml: String,
) -> Result<crate::model_config::ImportPreview, String> {
    ensure_local_origin(&webview)?;
    ensure_local_service_scope(&app)?;
    let config = app.state::<AppState>().config();
    crate::model_config::preview(&config, &yaml)
}

/// 应用导入的模型配置：写 settings.yaml 的 llm-pi-ai 段 + .credentials.yaml
/// 的凭据。配置与凭据均由 dsh 热加载，无需重启服务。
#[tauri::command]
pub fn apply_model_import(
    app: AppHandle,
    webview: tauri::Webview,
    payload: crate::model_config::ImportApplyPayload,
) -> Result<(), String> {
    ensure_local_origin(&webview)?;
    ensure_local_service_scope(&app)?;
    crate::model_config::apply(&app, payload)
}

/// 导出已配置的模型配置（settings.yaml 的 llm-pi-ai 段原文），供复制分享。
/// 返回 Option：None 表示当前无配置（前端显示"暂无配置"提示）。
#[tauri::command]
pub fn export_model_config(
    app: AppHandle,
    webview: tauri::Webview,
) -> Result<Option<String>, String> {
    ensure_local_origin(&webview)?;
    ensure_local_service_scope(&app)?;
    let config = app.state::<AppState>().config();
    crate::model_config::export_yaml(&config)
}
