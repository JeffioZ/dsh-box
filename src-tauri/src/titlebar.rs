//! 自绘标题栏：
//! - Windows/Linux：去掉系统标题栏，子 webview 顶条承载产品名/余额/窗口按钮；
//! - macOS：保留系统装饰（titleBarStyle: Overlay 悬浮红绿灯），标题栏左侧留白；
//! - 主 webview 让出顶部与底部条，窗口缩放时同步三个 webview 的边界。

use std::sync::atomic::{AtomicU64, Ordering};

use tauri::webview::WebviewBuilder;
use tauri::{AppHandle, Manager, WebviewUrl};

use crate::app_state::AppState;
use crate::MAIN_WINDOW;

/// 标题栏高度（逻辑像素）。
pub const TITLEBAR_HEIGHT: f64 = 36.0;
/// 浮层高度默认值：页面未提供实测高度时使用（36px 标题栏 + 浮层 + 阴影余量）。
pub const TITLEBAR_EXPANDED_HEIGHT: f64 = 260.0;
/// 浮层高度上限：主窗口最小高度为 620px；完整主菜单也能容纳，
/// 常规菜单仍按页面实测高度展开。
pub const TITLEBAR_MENU_HEIGHT: f64 = 620.0;
/// 标题栏子 webview 的 label。
pub const TITLEBAR_LABEL: &str = "titlebar";
/// 状态栏高度（逻辑像素）：单行 12px 文本 + 上下留白，与 dsh 统计行同规格。
pub const STATUSBAR_HEIGHT: f64 = 26.0;
/// 状态栏子 webview 的 label。
pub const STATUSBAR_LABEL: &str = "statusbar";

/// 当前标题栏子 WebView 高度；用整数逻辑像素即可，避免跨线程浮点原子。
static OVERLAY_HEIGHT: AtomicU64 = AtomicU64::new(TITLEBAR_HEIGHT as u64);

/// 标题栏页面初始化完成回报标记：启动自愈看门狗据此判断页面是否加载成功。
static READY: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// 页面回报初始化完成（titlebar_ready 命令）。
pub fn mark_ready() {
    READY.store(true, Ordering::SeqCst);
}

/// 强制子 webview 重建合成层：间歇性「标题栏渲染空白」的自动修复。
/// WebView2 的合成层失效时 DOM 正常、仅画面空白，应用层无法直接探测，
/// 故在窗口焦点变化与周期看门狗中触发重绘脉冲（页面侧 __dshdRepaint
/// 通过强制创建/销毁合成层恢复渲染）。标题栏与状态栏同一修复通道。
pub fn repaint_pulse(app: &AppHandle) {
    let Some(window) = app.get_window(MAIN_WINDOW) else {
        return;
    };
    for wv in window.webviews() {
        if wv.label() == TITLEBAR_LABEL || wv.label() == STATUSBAR_LABEL {
            let _ = wv.eval("window.__dshdRepaint && window.__dshdRepaint()");
        }
    }
}

pub fn is_ready() -> bool {
    READY.load(Ordering::SeqCst)
}

/// 重新加载标题栏页面（自愈：首次加载失败时重试）。
pub fn reload(app: &AppHandle) {
    let Some(window) = app.get_window(MAIN_WINDOW) else {
        return;
    };
    for wv in window.webviews() {
        if wv.label() != TITLEBAR_LABEL {
            continue;
        }
        // 优先重试当前 URL（加载失败时 URL 通常仍是目标地址）；
        // 不合法时退回内置 App 路径
        let current = wv
            .url()
            .ok()
            .filter(|u| u.scheme() == "http" || u.scheme() == "https" || u.scheme() == "tauri");
        match current {
            Some(u) => {
                let _ = wv.navigate(u);
            }
            None => {
                #[cfg(windows)]
                let fallback = "http://tauri.localhost/titlebar.html";
                #[cfg(not(windows))]
                let fallback = "tauri://localhost/titlebar.html";
                if let Ok(u) = url::Url::parse(fallback) {
                    let _ = wv.navigate(u);
                }
            }
        }
    }
}

/// 展开/收起标题栏浮层（余额或主菜单），并立即同步边界。
pub fn set_expanded(app: &AppHandle, expanded: bool, requested_height: Option<f64>) {
    let height = if expanded {
        requested_height
            .unwrap_or(TITLEBAR_EXPANDED_HEIGHT)
            .clamp(TITLEBAR_HEIGHT, TITLEBAR_MENU_HEIGHT)
    } else {
        TITLEBAR_HEIGHT
    };
    OVERLAY_HEIGHT.store(height.round() as u64, Ordering::SeqCst);
    sync_bounds(app);
}

/// 初始化底部状态栏：独立子 webview（会话统计 + 余额 + 设置入口），
/// 固定为不透明 26px 高度，避免透明子 WebView 动态合成产生绘制残影。
pub fn init_statusbar(app: &AppHandle) -> tauri::Result<()> {
    let window = main_window(app)?;
    let navigation_app = app.clone();
    let child = WebviewBuilder::new(STATUSBAR_LABEL, WebviewUrl::App("statusbar.html".into()))
        // 禁用后台节流：状态栏实时更新（会话统计/余额），失焦节流会导致
        // 首次渲染滞后（loading 界面先出、状态栏后出的跳跃感）
        .background_throttling(tauri::utils::config::BackgroundThrottlingPolicy::Disabled)
        .initialization_script(crate::locale::init_script())
        .on_navigation(move |url| {
            let allowed =
                crate::is_local_app_url(url, crate::app_dev_origin(&navigation_app).as_ref());
            if !allowed {
                crate::logging::log(&format!("statusbar: 已拦截非白名单导航 {url}"));
            }
            allowed
        });
    let size = window.inner_size()?;
    let scale = window.scale_factor().unwrap_or(1.0);
    let w = size.width as f64 / scale;
    let h = size.height as f64 / scale;
    window.add_child(
        child,
        tauri::LogicalPosition::new(0.0, h - STATUSBAR_HEIGHT),
        tauri::LogicalSize::new(w, STATUSBAR_HEIGHT),
    )?;
    sync_bounds(app);
    Ok(())
}

/// 初始化自绘标题栏：去掉系统标题栏（macOS 除外）、创建子 webview、同步边界。
pub fn init(app: &AppHandle) -> tauri::Result<()> {
    let window = main_window(app)?;
    // macOS 保留系统装饰（Overlay 红绿灯）；其他平台去掉系统标题栏。
    // 主窗口不启用 set_shadow：见 lib.rs 的说明（阴影 insets 导致窗口
    // 尺寸记忆逐次累积变大，并附加 1px 白边）。
    #[cfg(not(target_os = "macos"))]
    window.set_decorations(false)?;

    // 子 webview 透明：浮层展开加高时透出下层的 dsh 界面（浮层“盖在”其上而非推挤）；
    // 导航白名单与主窗口一致（IPC 另有来源校验兜底）
    let navigation_app = app.clone();
    let child = WebviewBuilder::new(TITLEBAR_LABEL, WebviewUrl::App("titlebar.html".into()))
        .initialization_script(crate::locale::init_script())
        .on_navigation(move |url| {
            let allowed =
                crate::is_local_app_url(url, crate::app_dev_origin(&navigation_app).as_ref());
            if !allowed {
                crate::logging::log(&format!("titlebar: 已拦截非白名单导航 {url}"));
            }
            allowed
        });
    // 子 webview 透明：浮层展开加高时透出下层的 dsh 界面（浮层“盖在”其上而非推挤）。
    // macOS 上 transparent 需要 macos-private-api feature，暂不启用私有 API，
    // 子 webview 透明仅在 Windows/Linux 生效
    #[cfg(not(target_os = "macos"))]
    let child = child.transparent(true);
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

/// 同步三个 webview 的边界：标题栏浮层可向下覆盖主 webview，状态栏固定
/// 占底部 26px；主 webview 夹在两者之间。
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
    let top_h = OVERLAY_HEIGHT.load(Ordering::SeqCst) as f64;
    // 隐藏状态栏时高度为 0、主 webview 直到底部；重新开启恢复固定高度。
    let status_h = if app.state::<AppState>().config().hide_statusbar {
        0.0
    } else {
        STATUSBAR_HEIGHT
    };
    let top = tauri::Rect {
        position: tauri::Position::Logical((0.0, 0.0).into()),
        size: tauri::Size::Logical((w, top_h).into()),
    };
    // 主 webview 从标题栏底部开始，并为可见状态栏让出固定高度。
    let main = tauri::Rect {
        position: tauri::Position::Logical((0.0, TITLEBAR_HEIGHT).into()),
        size: tauri::Size::Logical((w, (h - TITLEBAR_HEIGHT - status_h).max(0.0)).into()),
    };
    // 状态栏始终贴底，不因 hover 改变边界。
    let status = tauri::Rect {
        position: tauri::Position::Logical((0.0, (h - status_h).max(TITLEBAR_HEIGHT)).into()),
        size: tauri::Size::Logical((w, status_h).into()),
    };
    for wv in window.webviews() {
        let is_titlebar = wv.label() == TITLEBAR_LABEL;
        let is_statusbar = wv.label() == STATUSBAR_LABEL;
        let rect = if is_titlebar {
            top
        } else if is_statusbar {
            status
        } else {
            main
        };
        // 矩形未变时跳过 set_bounds：重复设置会触发无谓的重布局/重绘，
        // 是标题栏文案偶发闪烁的来源之一（tauri::Rect 无 PartialEq，
        // 以逻辑分量记录上次设置值比较）
        let key = rect_key(&rect);
        let guard = if is_titlebar {
            &LAST_TOP_KEY
        } else if is_statusbar {
            &LAST_STATUS_KEY
        } else {
            &LAST_MAIN_KEY
        };
        let mut last = guard.lock().unwrap_or_else(|e| e.into_inner());
        if *last == Some(key) {
            continue;
        }
        *last = Some(key);
        let _ = wv.set_bounds(rect);
    }
}

fn rect_key(rect: &tauri::Rect) -> (f64, f64, f64) {
    let (w, h) = match rect.size {
        tauri::Size::Logical(l) => (l.width, l.height),
        tauri::Size::Physical(p) => (p.width as f64, p.height as f64),
    };
    (w, h, rect_y(rect))
}

fn rect_y(rect: &tauri::Rect) -> f64 {
    match rect.position {
        tauri::Position::Logical(l) => l.y,
        tauri::Position::Physical(p) => p.y as f64,
    }
}

/// 上次设置的标题栏/主 webview/状态栏矩形（宽、高、y 的逻辑分量），
/// 供冗余 set_bounds 跳过。
static LAST_TOP_KEY: std::sync::Mutex<Option<(f64, f64, f64)>> = std::sync::Mutex::new(None);
static LAST_MAIN_KEY: std::sync::Mutex<Option<(f64, f64, f64)>> = std::sync::Mutex::new(None);
static LAST_STATUS_KEY: std::sync::Mutex<Option<(f64, f64, f64)>> = std::sync::Mutex::new(None);

fn main_window(app: &AppHandle) -> tauri::Result<tauri::Window> {
    app.get_window(MAIN_WINDOW).ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::NotFound, "main window is unavailable").into()
    })
}
