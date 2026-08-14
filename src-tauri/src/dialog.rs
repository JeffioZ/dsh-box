//! 统一消息对话框：
//! - 主窗口可见时设为模态父窗口（置顶显示，不会被主窗口遮挡）；
//! - 全局互斥：同一时刻只显示一个对话框，多处触发也不会重叠。
//!
//! 所有提示/询问必须经本模块，禁止直接调用 dialog()。

use std::sync::Mutex;

use tauri::AppHandle;
use tauri_plugin_dialog::{DialogExt, MessageDialogButtons, MessageDialogKind};

use crate::main_window;

/// 全局互斥锁：串行化全部对话框（阻塞等待前一个关闭，保证不重叠）。
static DIALOG_LOCK: Mutex<()> = Mutex::new(());

/// 底层显示：持有全局锁；返回用户是否点击左侧（确认）按钮。
fn show(
    app: &AppHandle,
    text: String,
    title: &str,
    kind: MessageDialogKind,
    buttons: Option<(String, String)>,
) -> bool {
    let _guard = DIALOG_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut builder = app.dialog().message(text).title(title).kind(kind);
    if let Some((yes, no)) = buttons {
        builder = builder.buttons(MessageDialogButtons::OkCancelCustom(yes, no));
    }
    // 主窗口可见时设为模态父窗口（置顶、模态化）；
    // 窗口隐藏（仅托盘）时不设父级，避免模态挂在隐藏窗口上。
    if let Some(win) = main_window(app) {
        if win.is_visible().unwrap_or(false) {
            builder = builder.parent(&win);
        }
    }
    builder.blocking_show()
}

/// 信息提示框（单按钮，阻塞调用线程）。
pub fn show_message(app: &AppHandle, text: String, title: &str, kind: MessageDialogKind) {
    show(app, text, title, kind, None);
}

/// 询问框（自定义按钮文字），返回用户是否点击左侧按钮（如“立即更新”）。
pub fn ask(
    app: &AppHandle,
    text: String,
    title: &str,
    kind: MessageDialogKind,
    yes_label: &str,
    no_label: &str,
) -> bool {
    show(
        app,
        text,
        title,
        kind,
        Some((yes_label.to_string(), no_label.to_string())),
    )
}
