//! 自绘标题栏：
//! - Windows/Linux：去掉系统标题栏，子 webview 顶条承载产品名/余额/窗口按钮；
//! - macOS：保留系统装饰（titleBarStyle: Overlay 悬浮红绿灯），标题栏左侧留白；
//! - 主 webview 让出顶部条，窗口缩放时同步两个 webview 的边界。

use std::sync::atomic::{AtomicBool, Ordering};

use tauri::webview::WebviewBuilder;
use tauri::{AppHandle, Manager, WebviewUrl};

use crate::MAIN_WINDOW;

/// 标题栏高度（逻辑像素）。
pub const TITLEBAR_HEIGHT: f64 = 36.0;
/// 余额浮层展开时标题栏 webview 的总高度（36px 标题栏 + 浮层 + 阴影余量）。
pub const TITLEBAR_EXPANDED_HEIGHT: f64 = 260.0;
/// 标题栏子 webview 的 label。
pub const TITLEBAR_LABEL: &str = "titlebar";

/// 余额浮层是否展开（决定标题栏 webview 高度，供 sync_bounds 读取）。
static EXPANDED: AtomicBool = AtomicBool::new(false);

/// 展开/收起余额浮层（由 titlebar 前端 hover 时调用），并立即同步边界。
pub fn set_expanded(app: &AppHandle, expanded: bool) {
    EXPANDED.store(expanded, Ordering::SeqCst);
    sync_bounds(app);
}

/// 初始化自绘标题栏：去掉系统标题栏（macOS 除外）、创建子 webview、同步边界。
pub fn init(app: &AppHandle) -> tauri::Result<()> {
    let window = app.get_window(MAIN_WINDOW).expect("主窗口不存在");
    // macOS 保留系统装饰（Overlay 红绿灯）；其他平台去掉系统标题栏
    #[cfg(not(target_os = "macos"))]
    window.set_decorations(false)?;

    // 子 webview 透明：浮层展开加高时透出下层的 dsh 界面（浮层“盖在”其上而非推挤）
    let child = WebviewBuilder::new(TITLEBAR_LABEL, WebviewUrl::App("titlebar.html".into()))
        .transparent(true);
    let size = window.inner_size()?;
    let scale = window.scale_factor().unwrap_or(1.0);
    let w = size.width as f64 / scale;
    window.add_child(
        child,
        tauri::LogicalPosition::new(0.0, 0.0),
        tauri::LogicalSize::new(w, TITLEBAR_HEIGHT),
    )?;
    sync_bounds(app);
    Ok(())
}

/// 同步两个 webview 的边界：标题栏占顶部条（浮层展开时子 webview 加高但主 webview
/// 不推挤——浮层透明覆盖在 dsh 界面上方；代价是浮层区域在展开期间拦截鼠标）。
pub fn sync_bounds(app: &AppHandle) {
    let Some(window) = app.get_window(MAIN_WINDOW) else {
        return;
    };
    let Ok(size) = window.inner_size() else {
        return;
    };
    let scale = window.scale_factor().unwrap_or(1.0);
    let w = size.width as f64 / scale;
    let h = size.height as f64 / scale;
    let top_h = if EXPANDED.load(Ordering::SeqCst) {
        TITLEBAR_EXPANDED_HEIGHT
    } else {
        TITLEBAR_HEIGHT
    };
    let top = tauri::Rect {
        position: tauri::Position::Logical((0.0, 0.0).into()),
        size: tauri::Size::Logical((w, top_h).into()),
    };
    // 主 webview 始终从 36px 开始：浮层只是视觉覆盖，不改变 dsh 布局
    let main = tauri::Rect {
        position: tauri::Position::Logical((0.0, TITLEBAR_HEIGHT).into()),
        size: tauri::Size::Logical((w, (h - TITLEBAR_HEIGHT).max(0.0)).into()),
    };
    for wv in window.webviews() {
        let rect = if wv.label() == TITLEBAR_LABEL {
            top
        } else {
            main
        };
        let _ = wv.set_bounds(rect);
    }
}
