//! balance IPC 转发。

use super::*;

/// 标题栏余额查询：拒绝非本地来源与外部服务模式（外部 dsh 的凭据在原
/// 环境查询）后，转发真实查询（阻塞线程执行网络请求）。
#[tauri::command]
pub async fn api_balance(
    app: AppHandle,
    webview: tauri::Webview,
) -> crate::balance::BalancePayload {
    if let Err(error) = ensure_local_origin(&webview) {
        return crate::balance::denied_payload(error);
    }
    if app.state::<AppState>().service_ownership().is_external() {
        return crate::balance::denied_payload(
            crate::locale::text(
                "余额使用外部 dsh 的凭据，请在原服务环境中查询。",
                "The external dsh service manages the credentials used for balance queries. Check the balance in that service's environment.",
            )
            .into(),
        );
    }
    crate::balance::run_balance_query(app).await
}
