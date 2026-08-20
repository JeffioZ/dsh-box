//! 自绘托盘菜单：无边框小窗（tray-menu.html + IPC），替代 Windows 原生
//! TrackPopupMenu 弹出菜单。
//!
//! 背景：原生弹出在本环境不稳定（首次右键不弹、弹出即闪退）。
//! 关键实现约束：窗口在启动时预创建一次、此后只 定位/显示/隐藏——
//! 绝不在事件回调里新建或销毁 WebView 窗口（否则主线程卡死）。

use tauri::{AppHandle, Manager};
#[cfg(windows)]
use tauri::{Emitter, WebviewUrl};

#[cfg(windows)]
use crate::app_state::AppState;

/// 托盘菜单窗口 label。
pub const TRAY_MENU_WINDOW: &str = "tray-menu";

/// 菜单条目（sep=true 渲染分隔线）。
#[derive(serde::Serialize, Clone)]
pub struct TrayMenuItem {
    pub id: String,
    pub label: String,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub sep: bool,
    /// 图标名（menu.js 的 ICONS 表）；None 不显示图标
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icon: Option<&'static str>,
}

impl TrayMenuItem {
    fn row(id: &str, label: &str) -> Self {
        Self {
            id: id.to_string(),
            label: label.to_string(),
            sep: false,
            icon: None,
        }
    }
    fn row_icon(id: &str, icon: &'static str, label: &str) -> Self {
        Self {
            icon: Some(icon),
            ..Self::row(id, label)
        }
    }
    fn sep() -> Self {
        Self {
            id: String::new(),
            label: String::new(),
            sep: true,
            icon: None,
        }
    }
}

/// 托盘与标题栏共用的菜单模型。标题栏版本不含窗口内已有的动作
/// （打开应用与余额入口）。
///
/// 分组：打开/访问 → 服务维护 → 管理与查询 → 关于/退出（危险动作
/// 与常规动作分离）。托盘比标题栏多“打开”与“查询 API 余额…”。
pub fn items(tray_surface: bool) -> Vec<TrayMenuItem> {
    let mut rows = Vec::new();
    // 打开/访问
    if tray_surface {
        rows.push(TrayMenuItem::row_icon(
            "open",
            "window",
            crate::locale::text("打开", "Open"),
        ));
    }
    rows.push(TrayMenuItem::row_icon(
        "open_browser",
        "globe",
        crate::locale::text("在浏览器中打开", "Open in browser"),
    ));
    rows.push(TrayMenuItem::sep());
    // 服务维护
    rows.push(TrayMenuItem::row_icon(
        "restart",
        "restart",
        crate::locale::text("重启服务", "Restart service"),
    ));
    rows.push(TrayMenuItem::row_icon(
        "check_update",
        "download",
        crate::locale::text("检查更新…", "Check for updates…"),
    ));
    rows.push(TrayMenuItem::sep());
    // 管理与查询
    if tray_surface {
        rows.push(TrayMenuItem::row_icon(
            "balance",
            "wallet",
            crate::locale::text("查询 API 余额…", "Check API balance…"),
        ));
    }
    rows.push(TrayMenuItem::row_icon(
        "plugins",
        "puzzle",
        crate::locale::text("插件管理…", "Plugin manager…"),
    ));
    rows.push(TrayMenuItem::row_icon(
        "settings",
        "gear",
        crate::locale::text("桌面端设置…", "Desktop settings…"),
    ));
    rows.push(TrayMenuItem::sep());
    // 关于/退出
    rows.push(TrayMenuItem::row_icon(
        "about",
        "info",
        crate::locale::text("关于", "About"),
    ));
    rows.push(TrayMenuItem::row_icon(
        "quit",
        "exit",
        crate::locale::text("退出", "Quit"),
    ));
    rows
}

/// 窗口四周的阴影边距：不透明窗口下卡片直接占满窗口（圆角由 Win11
/// 系统裁剪），无阴影空间。透明窗口渲染在本机 WebView2 下不可见
/// （is_visible=true 但内容透明），已回退不透明方案。
#[cfg(windows)]
const SHADOW_PAD: f64 = 0.0;

/// 菜单窗口尺寸（含四周阴影边距）：卡片内边距 4×2、行高 40、
/// 分隔线 9（与 dsh 菜单条目同规格）；宽 220 与 dsh 菜单卡宽 218
/// 同规格，容纳最长条目（含图标/内边距约 180px）。
/// 注意：Windows 自绘托盘菜单宽度与 ui/titlebar.html 的 .main-menu-panel
/// （220px）保持一致，改动需同步两处。
///
/// 卡片无描边（透明窗口模型，边界由圆角/底色/阴影承担），高度不含边框；
/// body 为 border-box，高度必须包含内边距，否则末行 hover 会被裁掉。
#[cfg(windows)]
fn menu_size() -> (f64, f64) {
    let rows = items(true);
    let height = 8.0
        + rows
            .iter()
            .map(|r| if r.sep { 9.0 } else { 40.0 })
            .sum::<f64>();
    (220.0 + SHADOW_PAD * 2.0, height + SHADOW_PAD * 2.0)
}

/// 启动时预创建（隐藏）：此后只定位/显示/隐藏，不再创建销毁。
#[cfg(windows)]
pub fn precreate(app: &AppHandle) {
    let (w, h) = menu_size();
    // 导航白名单与主窗口一致：菜单内容只允许加载内置页面
    let navigation_app = app.clone();
    match tauri::WebviewWindowBuilder::new(
        app,
        TRAY_MENU_WINDOW,
        WebviewUrl::App("tray-menu.html".into()),
    )
    .title(crate::locale::text("托盘菜单", "Tray menu"))
    .initialization_script(crate::locale::init_script())
    .inner_size(w, h)
    .resizable(false)
    .decorations(false)
    .always_on_top(true)
    .skip_taskbar(true)
    .visible(false)
    .on_navigation(move |url| {
        let allowed = crate::is_local_app_url(url, crate::app_dev_origin(&navigation_app).as_ref());
        if !allowed {
            crate::logging::log(&format!("tray-menu: 已拦截非白名单导航 {url}"));
        }
        allowed
    })
    .build()
    {
        Ok(win) => {
            // 不透明窗口 + 卡片色背景：圆角由 Win11 系统裁剪（Win10 直角）
            let theme = app.state::<AppState>().config().resolve_dsh_theme();
            let light = if theme == Some(tauri::Theme::Light) {
                true
            } else if theme == Some(tauri::Theme::Dark) {
                false
            } else {
                win.theme().ok() == Some(tauri::Theme::Light)
            };
            if let Some(theme) = theme {
                let _ = win.set_theme(Some(theme));
            }
            let color = if light {
                crate::CARD_BG_LIGHT
            } else {
                crate::CARD_BG_DARK
            };
            let _ = win.set_background_color(Some(color));
            #[cfg(windows)]
            crate::window::enable_system_rounded_corners(&win);
        }
        Err(e) => {
            crate::logging::log(&format!("tray-menu: 窗口预创建失败：{e}"));
        }
    }
}

/// 在光标处弹出菜单（调用方已在主线程）。`at` 为屏幕物理坐标。
#[cfg(windows)]
pub fn open_menu(app: &AppHandle, at: (f64, f64)) {
    // 打开菜单时即时比对 dsh 设置（语言/主题）：用户在 dsh 里刚切换过，
    // 这次打开立即生效，不等 3s 轮询
    crate::tray::check_dsh_settings_now(app);
    let Some(win) = app.get_webview_window(TRAY_MENU_WINDOW) else {
        crate::logging::log("tray-menu: 窗口不存在（预创建失败？）");
        return;
    };
    let (width, height) = menu_size();
    let _ = win.set_size(tauri::Size::Logical(tauri::LogicalSize::new(width, height)));
    // 定位：仿 Windows 托盘菜单——菜单右下角贴点击点（向上/向左展开），放不下再翻转。
    // 关键：事件坐标是物理像素，窗口尺寸是逻辑像素——统一用目标显示器的缩放
    // 换算成逻辑坐标再计算（200% DPI 下混用会导致菜单位置偏移、鼠标落在菜单内）
    let monitors = app.available_monitors().unwrap_or_default();
    let target = monitors.iter().find_map(|m| {
        let wa = m.work_area();
        let (px, py) = (wa.position.x as f64, wa.position.y as f64);
        let (pw, ph) = (wa.size.width as f64, wa.size.height as f64);
        (at.0 >= px && at.0 < px + pw && at.1 >= py && at.1 < py + ph).then_some((
            px,
            py,
            pw,
            ph,
            m.scale_factor(),
        ))
    });
    let (x, y) = match target {
        Some((px, py, _pw, _ph, scale)) => {
            // at 是物理像素，px/py 是物理工作区；统一用物理计算，
            // 最后换回逻辑坐标传给 set_position(Logical)。
            let cx = at.0;
            let cy = at.1;
            let win_w = width * scale;
            let win_h = height * scale;
            // 右下角贴点击点、向上展开，留 2/6px 物理空隙
            let mut x = cx - win_w + 2.0 * scale;
            let mut y = cy - win_h - 6.0 * scale;
            if x < px {
                x = cx - 2.0 * scale;
            }
            if y < py {
                y = cy + 6.0 * scale;
            }
            (x.max(px) / scale, y.max(py) / scale)
        }
        None => {
            // 托盘图标在任务栏，任务栏在工作区外——普通右键命中此分支；
            // 用主显示器 scale 做物理→逻辑换算，位置贴光标右上角
            crate::logging::log(&format!(
                "tray-menu: 光标不在已枚举工作区 光标=({:.0},{:.0})",
                at.0, at.1
            ));
            let scale = app
                .primary_monitor()
                .ok()
                .flatten()
                .map(|m| m.scale_factor())
                .unwrap_or(1.0);
            // 物理坐标 → 逻辑（除以 scale）；set_position 用 Logical
            (
                (at.0 - width * scale + 2.0 * scale) / scale,
                (at.1 - height * scale - 6.0 * scale) / scale,
            )
        }
    };
    crate::logging::log(&format!(
        "tray-menu: 点击=({:.0},{:.0}) 菜单=({x:.0},{y:.0}) 尺寸=({width:.0}x{height:.0})",
        at.0, at.1
    ));
    let _ = win.set_position(tauri::Position::Logical(tauri::LogicalPosition::new(x, y)));
    // 先显示再发事件：隐藏窗口收不到 emit 的内容，事件仅作即时更新，
    // 页面另有 __dshdRefresh（Rust eval 直呼）作为确定性兜底
    if let Err(e) = win.show() {
        crate::logging::log(&format!("tray-menu: show 失败：{e}"));
    } else {
        crate::logging::log(&format!(
            "tray-menu: 已显示 is_visible={}",
            win.is_visible().unwrap_or(false)
        ));
    }
    // 点击外部收起（物理矩形）——必须在 show 之后启动：监控线程首查
    // is_visible，若在 show 前启动可能读到 false 提前退出，菜单"看似没显示"
    bump_popup_gen();
    let scale = win.scale_factor().unwrap_or(1.0);
    if let Ok(size) = win.inner_size() {
        // 位置是逻辑坐标（乘缩放换物理）；inner_size 本身已是物理像素
        let (x0, y0) = ((x * scale) as i32, (y * scale) as i32);
        #[cfg(windows)]
        {
            let (x1, y1) = (x0 + size.width as i32, y0 + size.height as i32);
            watch_outside_click(app.clone(), TRAY_MENU_WINDOW, (x0, y0, x1, y1));
        }
    }
    if let Err(e) = app.emit_to(TRAY_MENU_WINDOW, "tray-menu-open", items(true)) {
        crate::logging::log(&format!("tray-menu: 事件下发失败：{e}"));
    }
    let _ = win.eval("window.__dshdRefresh && window.__dshdRefresh()");
    let _ = win.set_focus();
}

/// 隐藏菜单窗口（失焦/选中后）。无动效（此前尝试淡出，托盘窗口
/// show/hide 时机与动画交互不理想，保持原生即时隐藏）。
pub fn hide_menu(app: &AppHandle) {
    bump_popup_gen();
    if let Some(w) = app.get_webview_window(TRAY_MENU_WINDOW) {
        let _ = w.hide();
    }
}

/// 托盘菜单项动作分发（与旧原生菜单一致，tray_menu_choose 调用）。
pub fn run_action(app: &AppHandle, id: &str) {
    // 菜单延迟 ~180ms 收起：让按压高亮清晰可见（原生菜单的选中反馈节奏）
    let handle = app.clone();
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(180));
        hide_menu(&handle);
    });
    crate::tray::run_action(app, id);
}

// ---------- 弹窗通用：点击外部收起 ----------

static POPUP_GEN: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// 每次弹窗打开/关闭时递增：旧的外部点击监视线程据此退出，避免误关新弹窗。
pub(crate) fn bump_popup_gen() {
    use std::sync::atomic::Ordering;
    POPUP_GEN.fetch_add(1, Ordering::Relaxed);
}

/// 外部点击监视：光标在弹窗矩形外按下任意鼠标键即隐藏。
/// 不依赖焦点（弹窗可能拿不到焦点），轮询 GetAsyncKeyState 全局按键。
/// rect 为物理像素矩形。
#[cfg(windows)]
pub(crate) fn watch_outside_click(app: AppHandle, label: &'static str, rect: (i32, i32, i32, i32)) {
    use std::sync::atomic::Ordering;
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState;
    use windows_sys::Win32::UI::WindowsAndMessaging::GetCursorPos;
    let gen = POPUP_GEN.load(Ordering::Relaxed);
    std::thread::spawn(move || {
        // 托盘菜单由右键触发：弹出时右键通常仍按住（GetAsyncKeyState 一直为
        // true），若立即按"按键按下 + 光标在外"判定会瞬间误收菜单（菜单
        // 一 出就消失）。改为：先等所有鼠标按键都释放，再从"按键按下沿"
        // 检测真正的外部点击——右键菜单的标准收起语义。
        loop {
            std::thread::sleep(std::time::Duration::from_millis(70));
            if POPUP_GEN.load(Ordering::Relaxed) != gen {
                return;
            }
            // 等全部按键释放（0x01 左 / 0x02 右 / 0x04 中）
            let any_down = unsafe {
                GetAsyncKeyState(0x01) < 0
                    || GetAsyncKeyState(0x02) < 0
                    || GetAsyncKeyState(0x04) < 0
            };
            if any_down {
                continue; // 仍按住（含触发菜单的右键），不判定
            }
            let Some(w) = app.get_webview_window(label) else {
                return;
            };
            if !w.is_visible().unwrap_or(false) {
                return;
            }
            let mut pt = windows_sys::Win32::Foundation::POINT { x: 0, y: 0 };
            unsafe { GetCursorPos(&mut pt) };
            let (x0, y0, x1, y1) = rect;
            if pt.x < x0 || pt.x >= x1 || pt.y < y0 || pt.y >= y1 {
                let down = unsafe {
                    GetAsyncKeyState(0x01) < 0
                        || GetAsyncKeyState(0x02) < 0
                        || GetAsyncKeyState(0x04) < 0
                };
                if down {
                    crate::logging::log("tray-menu: 检测到外部点击，收起菜单");
                    let _ = w.hide();
                    return;
                }
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 固化托盘菜单分组顺序（空串 = 分隔线）。tray.rs 的 macOS/Linux
    /// 原生菜单手写顺序必须与此保持一致，改动需同步两处。
    #[test]
    fn tray_menu_grouping_order() {
        let rows = items(true);
        let ids: Vec<&str> = rows.iter().map(|r| r.id.as_str()).collect();
        assert_eq!(
            ids,
            [
                "open",
                "open_browser",
                "",
                "restart",
                "check_update",
                "",
                "balance",
                "plugins",
                "settings",
                "",
                "about",
                "quit",
            ]
        );
    }

    /// 标题栏主菜单：不含托盘专属项（打开应用、查询余额），分组同托盘。
    #[test]
    fn titlebar_menu_grouping_order() {
        let rows = items(false);
        let ids: Vec<&str> = rows.iter().map(|r| r.id.as_str()).collect();
        assert_eq!(
            ids,
            [
                "open_browser",
                "",
                "restart",
                "check_update",
                "",
                "plugins",
                "settings",
                "",
                "about",
                "quit",
            ]
        );
    }
}
