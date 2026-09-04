//! 原生消息对话框兜底通道（自绘弹窗为默认交互载体，见 control_center）：
//! 仅保留三类场景——更新失败且检查更新弹窗未开（check.rs）、dsh 页面内
//! 打开可执行文件的安全确认（webview/protocol.rs）、自绘更新提示展示中
//! 被轻量提示抢占的罕见冲突（control_center::open_notice）。
//!
//! - 主窗口可见时设为模态父窗口（置顶显示，不会被主窗口遮挡）；
//! - 全局互斥：同一时刻只显示一个对话框，多处触发也不会重叠。
//!
//! 新增提示优先走自绘弹窗；确需原生时经本模块，禁止直接调用 dialog()。

use std::sync::Mutex;

use tauri::{AppHandle, Manager};
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
    // 父窗口选择：自绘弹窗打开时以其为父（自绘弹窗常驻置顶，
    // 挂主窗口会被它遮住——PowerShell 更新提示曾出现在弹窗下方）；
    // 否则挂主窗口。两者都不可见（仅托盘）时不设父级。
    let dialog_open = app
        .get_webview_window(crate::control_center::APP_DIALOG_WINDOW)
        .is_some_and(|win| win.is_visible().unwrap_or(false));
    if dialog_open {
        if let Some(win) = app.get_webview_window(crate::control_center::APP_DIALOG_WINDOW) {
            builder = builder.parent(&win);
        }
    } else if let Some(win) = main_window(app).filter(|win| win.is_visible().unwrap_or(false)) {
        builder = builder.parent(&win);
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
