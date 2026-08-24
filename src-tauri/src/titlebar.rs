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
/// 浮层高度上限；常规菜单仍按页面实测高度展开，窄窗口由 WebView 边界裁切。
pub const TITLEBAR_MENU_HEIGHT: f64 = 620.0;
/// 标题栏子 webview 的 label。
pub const TITLEBAR_LABEL: &str = "titlebar";
/// 状态栏高度（逻辑像素）：单行 12px 文本 + 上下留白，与 dsh 统计行同规格。
pub const STATUSBAR_HEIGHT: f64 = 26.0;
/// 状态栏子 webview 的 label。
pub const STATUSBAR_LABEL: &str = "statusbar";
const STATUSBAR_DARK_BG: tauri::window::Color = tauri::window::Color(0x18, 0x18, 0x19, 0xFF);
const STATUSBAR_LIGHT_BG: tauri::window::Color = tauri::window::Color(0xFC, 0xFC, 0xFD, 0xFF);

/// 当前标题栏子 WebView 高度；用整数逻辑像素即可，避免跨线程浮点原子。
static OVERLAY_HEIGHT: AtomicU64 = AtomicU64::new(TITLEBAR_HEIGHT as u64);

/// 标题栏页面初始化完成回报标记：启动自愈看门狗据此判断页面是否加载成功。
static READY: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// 状态栏页面就绪标记。状态栏没有 titlebar_ready 那样的页面侧回报命令
/// （自愈保持纯 Rust 实现，不改 ui/statusbar.js），由 init_statusbar 的
/// on_page_load(Finished) 事件置位。
static STATUSBAR_READY: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// 页面回报初始化完成（titlebar_ready 命令）。
pub fn mark_ready() {
    READY.store(true, Ordering::SeqCst);
}

/// 复位就绪标记：reload 前调用，要求重载后的页面重新完成就绪握手。
/// READY 一旦置位永不复位的话，子 webview 崩溃/重载后的再次加载失败
/// 对看门狗不可见，自愈通道就此失效。
fn reset_ready() {
    READY.store(false, Ordering::SeqCst);
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

/// 同步状态栏子 WebView 的原生底色。主窗口主题变化时只改 Window 底色并
/// 不会自动更新子 WebView；显式同步可避免后续缩放再次露出旧主题底色。
pub fn set_statusbar_theme_background(app: &AppHandle, light: bool) {
    let Some(window) = app.get_window(MAIN_WINDOW) else {
        return;
    };
    if let Some(statusbar) = window
        .webviews()
        .into_iter()
        .find(|webview| webview.label() == STATUSBAR_LABEL)
    {
        let _ = statusbar.set_background_color(Some(statusbar_background(light)));
    }
}

fn statusbar_background(light: bool) -> tauri::window::Color {
    if light {
        STATUSBAR_LIGHT_BG
    } else {
        STATUSBAR_DARK_BG
    }
}

pub fn is_ready() -> bool {
    READY.load(Ordering::SeqCst)
}

/// 状态栏页面是否已就绪（启动自愈看门狗据此判断页面是否加载成功）。
pub fn statusbar_is_ready() -> bool {
    STATUSBAR_READY.load(Ordering::SeqCst)
}

/// 状态栏页面加载完成（on_page_load Finished 事件）。
fn mark_statusbar_ready() {
    STATUSBAR_READY.store(true, Ordering::SeqCst);
}

/// 复位状态栏就绪标记：语义同 reset_ready。
fn reset_statusbar_ready() {
    STATUSBAR_READY.store(false, Ordering::SeqCst);
}

/// 重新加载标题栏页面（自愈：首次加载失败时重试）。
pub fn reload(app: &AppHandle) {
    reset_ready();
    reload_child(app, TITLEBAR_LABEL, "titlebar.html");
}

/// 重新加载状态栏页面（自愈：看门狗检测到页面未就绪时重试一次）。
pub fn reload_statusbar(app: &AppHandle) {
    reset_statusbar_ready();
    reload_child(app, STATUSBAR_LABEL, "statusbar.html");
}

/// 重新加载指定子 webview 的页面。调用方须先复位对应就绪标记，
/// 使本次重载重新走就绪握手。
fn reload_child(app: &AppHandle, label: &str, page: &str) {
    let Some(window) = app.get_window(MAIN_WINDOW) else {
        return;
    };
    for wv in window.webviews() {
        if wv.label() != label {
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
                let fallback = format!("http://tauri.localhost/{page}");
                #[cfg(not(windows))]
                let fallback = format!("tauri://localhost/{page}");
                if let Ok(u) = url::Url::parse(&fallback) {
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
    let background = statusbar_background(window.theme().ok() == Some(tauri::Theme::Light));
    let navigation_app = app.clone();
    let child = WebviewBuilder::new(STATUSBAR_LABEL, WebviewUrl::App("statusbar.html".into()))
        // 子 WebView 有独立的原生底色；创建时即与主窗口一致，缩放期间即使
        // WebView2 尚未完成一帧合成，也不会从透明缝隙露出默认白色。
        .background_color(background)
        // 禁用后台节流：状态栏实时更新（会话统计/余额），失焦节流会导致
        // 首次渲染滞后（loading 界面先出、状态栏后出的跳跃感）
        .background_throttling(tauri::utils::config::BackgroundThrottlingPolicy::Disabled)
        .initialization_script(crate::locale::init_script())
        // 状态栏没有页面侧就绪回报命令（区别于标题栏的 titlebar_ready，
        // 不改 ui/statusbar.js）：以页面加载完成事件作为就绪信号，
        // 供 bootstrap 的一次性自愈看门狗判断
        .on_page_load(|_, payload| {
            if payload.event() == tauri::webview::PageLoadEvent::Finished {
                mark_statusbar_ready();
            }
        })
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

    // 主 WebView 默认会跟随父窗口自动缩放，而本模块还必须为标题栏和状态栏
    // 手动让位；保留自动缩放会让一次 Resized 触发两轮 SetBounds/重排。
    // 关闭后由 sync_bounds 成为三个 WebView 唯一的布局所有者。
    if let Some(main) = crate::main_webview(app) {
        main.set_auto_resize(false)?;
    }

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
    sync_bounds_for_size(app, size);
}

/// 使用 Resized 事件携带的物理尺寸同步边界，避免再读一次可能已经变化的窗口
/// 几何。所有分区先在物理像素中取整，再把主内容设为精确余量，因此任意 DPI
/// 下标题栏、内容区、状态栏都能无缝拼合。
pub fn sync_bounds_for_size(app: &AppHandle, size: tauri::PhysicalSize<u32>) {
    let Some(window) = app.get_window(MAIN_WINDOW) else {
        return;
    };
    let scale = window.scale_factor().unwrap_or(1.0);
    // 隐藏状态栏时高度为 0、主 webview 直到底部；重新开启恢复固定高度。
    let status_visible = !app.state::<AppState>().config().hide_statusbar;
    let layout = ChromeLayout::new(
        size,
        scale,
        OVERLAY_HEIGHT.load(Ordering::SeqCst) as f64,
        status_visible,
    );
    let top = tauri::Rect {
        position: tauri::Position::Physical((0, 0).into()),
        size: tauri::Size::Physical((size.width, layout.overlay_height).into()),
    };
    // 主 webview 从标题栏底部开始，并为可见状态栏让出固定高度。
    let main = tauri::Rect {
        position: tauri::Position::Physical((0, layout.titlebar_height as i32).into()),
        size: tauri::Size::Physical((size.width, layout.main_height).into()),
    };
    // 状态栏始终贴底，不因 hover 改变边界。
    let status = tauri::Rect {
        position: tauri::Position::Physical((0, layout.status_y as i32).into()),
        size: tauri::Size::Physical((size.width, layout.status_height).into()),
    };

    // 固定顺序：先铺满主内容，再盖标题栏，最后盖状态栏。单次 resize 不再
    // 依赖 HashMap 的遍历顺序，能缩短边缘短暂露出宿主底色的时间窗口。
    let webviews = window.webviews();
    for (label, rect, guard) in [
        (MAIN_WINDOW, main, &LAST_MAIN_KEY),
        (TITLEBAR_LABEL, top, &LAST_TOP_KEY),
        (STATUSBAR_LABEL, status, &LAST_STATUS_KEY),
    ] {
        let Some(wv) = webviews.iter().find(|webview| webview.label() == label) else {
            continue;
        };
        // 矩形未变时跳过 set_bounds：重复设置会触发无谓的重布局/重绘，
        // 是标题栏文案偶发闪烁的来源之一。
        let key = rect_key(&rect);
        let mut last = guard.lock().unwrap_or_else(|e| e.into_inner());
        if *last == Some(key) {
            continue;
        }
        *last = Some(key);
        let _ = wv.set_bounds(rect);
    }
}

#[derive(Debug, PartialEq, Eq)]
struct ChromeLayout {
    titlebar_height: u32,
    overlay_height: u32,
    main_height: u32,
    status_y: u32,
    status_height: u32,
}

impl ChromeLayout {
    fn new(
        size: tauri::PhysicalSize<u32>,
        scale: f64,
        overlay_height: f64,
        status_visible: bool,
    ) -> Self {
        let physical = |logical: f64| (logical * scale).round().max(0.0) as u32;
        let titlebar_height = physical(TITLEBAR_HEIGHT).min(size.height);
        let overlay_height = physical(overlay_height).min(size.height);
        let status_height = if status_visible {
            physical(STATUSBAR_HEIGHT).min(size.height.saturating_sub(titlebar_height))
        } else {
            0
        };
        let status_y = size.height.saturating_sub(status_height);
        let main_height = status_y.saturating_sub(titlebar_height);
        Self {
            titlebar_height,
            overlay_height,
            main_height,
            status_y,
            status_height,
        }
    }
}

fn rect_key(rect: &tauri::Rect) -> (u32, u32, i32) {
    let (w, h) = match rect.size {
        tauri::Size::Logical(l) => (
            l.width.round().max(0.0) as u32,
            l.height.round().max(0.0) as u32,
        ),
        tauri::Size::Physical(p) => (p.width, p.height),
    };
    (w, h, rect_y(rect))
}

fn rect_y(rect: &tauri::Rect) -> i32 {
    match rect.position {
        tauri::Position::Logical(l) => l.y.round() as i32,
        tauri::Position::Physical(p) => p.y,
    }
}

/// 上次设置的标题栏/主 webview/状态栏矩形（宽、高、y 的物理分量），
/// 供冗余 set_bounds 跳过。
static LAST_TOP_KEY: std::sync::Mutex<Option<(u32, u32, i32)>> = std::sync::Mutex::new(None);
static LAST_MAIN_KEY: std::sync::Mutex<Option<(u32, u32, i32)>> = std::sync::Mutex::new(None);
static LAST_STATUS_KEY: std::sync::Mutex<Option<(u32, u32, i32)>> = std::sync::Mutex::new(None);

fn main_window(app: &AppHandle) -> tauri::Result<tauri::Window> {
    app.get_window(MAIN_WINDOW).ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::NotFound, "main window is unavailable").into()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn physical_layout_tiles_window_without_gaps_at_fractional_dpi() {
        let layout = ChromeLayout::new(tauri::PhysicalSize::new(1280, 1000), 1.25, 36.0, true);
        assert_eq!(layout.titlebar_height, 45);
        assert_eq!(layout.status_height, 33);
        assert_eq!(layout.main_height, 922);
        assert_eq!(
            layout.titlebar_height + layout.main_height + layout.status_height,
            1000
        );
        assert_eq!(layout.status_y, 967);
    }

    #[test]
    fn hidden_statusbar_gives_its_exact_pixels_to_main_webview() {
        let layout = ChromeLayout::new(tauri::PhysicalSize::new(900, 575), 1.5, 240.0, false);
        assert_eq!(layout.titlebar_height, 54);
        assert_eq!(layout.status_height, 0);
        assert_eq!(layout.main_height, 521);
        assert_eq!(layout.status_y, 575);
    }

    #[test]
    fn titlebar_ready_rearms_after_reset() {
        // reload 路径复位后，页面重新完成就绪握手必须能再次置位
        mark_ready();
        assert!(is_ready());
        reset_ready();
        assert!(!is_ready());
        mark_ready();
        assert!(is_ready());
        reset_ready();
    }

    #[test]
    fn statusbar_ready_rearms_after_reset() {
        mark_statusbar_ready();
        assert!(statusbar_is_ready());
        reset_statusbar_ready();
        assert!(!statusbar_is_ready());
        mark_statusbar_ready();
        assert!(statusbar_is_ready());
        reset_statusbar_ready();
    }
}
