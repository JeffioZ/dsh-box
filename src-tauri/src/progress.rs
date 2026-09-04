//! 任务栏进度指示（Windows ITaskbarList3 / macOS Dock / Linux Unity）：
//! 更新包下载期间在主窗口的任务栏按钮上叠加进度条——用户切离应用
//! 干别的事时也能一眼看到下载进展。
//!
//! 只挂接有数值回调的下载（应用更新包 / Node 安装包）；dsh 更新走
//! npm CLI 无逐字节进度，不在此列。清除点：各下载函数收尾 +
//! updater::apply 的 UpdatingReset（覆盖全部更新流程的兜底）。

/// 更新主窗口任务栏进度（0-100）。窗口缺失/平台不支持时静默。
pub fn set(app: &tauri::AppHandle, done: u64, total: u64) {
    if total == 0 {
        return;
    }
    let Some(win) = crate::main_window(app) else {
        return;
    };
    let pct = ((done as f64 / total as f64) * 100.0) as u64;
    let _ = win.set_progress_bar(tauri::window::ProgressBarState {
        status: Some(tauri::window::ProgressBarStatus::Normal),
        progress: Some(pct),
    });
}

/// 清除进度，恢复正常任务栏按钮。完成/失败/取消路径都必须调用；
/// 重复调用无害（幂等）。
pub fn clear(app: &tauri::AppHandle) {
    let Some(win) = crate::main_window(app) else {
        return;
    };
    let _ = win.set_progress_bar(tauri::window::ProgressBarState {
        status: None,
        progress: None,
    });
}
