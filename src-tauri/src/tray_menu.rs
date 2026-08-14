//! 自绘托盘菜单：无边框小窗（tray-menu.html + IPC），替代 Windows 原生
//! TrackPopupMenu 弹出菜单。
//!
//! 背景：原生弹出在本环境不稳定（首次右键不弹、弹出即闪退）。
//! 关键实现约束：窗口在启动时预创建一次、此后只 定位/显示/隐藏——
//! 绝不在事件回调里新建或销毁 WebView 窗口（否则主线程卡死）。

use tauri::WebviewUrl;
use tauri::{AppHandle, Emitter, Manager};

use crate::autostart;

/// 托盘菜单窗口 label。
pub const TRAY_MENU_WINDOW: &str = "tray-menu";

/// 菜单条目（sep=true 渲染分隔线）。
#[derive(serde::Serialize, Clone)]
pub struct TrayMenuItem {
    pub id: String,
    pub label: String,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub sep: bool,
}

impl TrayMenuItem {
    fn row(id: &str, label: &str) -> Self {
        Self {
            id: id.to_string(),
            label: label.to_string(),
            sep: false,
        }
    }
    fn sep() -> Self {
        Self {
            id: String::new(),
            label: String::new(),
            sep: true,
        }
    }
}

/// 当前菜单条目（开机自启动文案动态生成）。
pub fn items() -> Vec<TrayMenuItem> {
    vec![
        TrayMenuItem::row("open", &format!("打开 {}", crate::APP_TITLE)),
        TrayMenuItem::row("balance", "查询 API 余额…"),
        TrayMenuItem::row("open_browser", "在浏览器中打开"),
        TrayMenuItem::sep(),
        TrayMenuItem::row("restart", "重启服务"),
        TrayMenuItem::row("check_update", "检查更新…"),
        TrayMenuItem::sep(),
        TrayMenuItem::row(
            "autostart",
            &format!(
                "开机自启动：{}",
                if autostart::is_enabled() {
                    "已开启"
                } else {
                    "已关闭"
                }
            ),
        ),
        TrayMenuItem::sep(),
        TrayMenuItem::row("about", "关于"),
        TrayMenuItem::row("quit", "退出"),
    ]
}

/// 菜单窗口尺寸：卡片内边距 4×2 + 行高 40 + 分隔线 9（与 dsh 菜单条目同规格），
/// 宽 264 容纳最长条目。
fn menu_size() -> (f64, f64) {
    let rows = items();
    let height = 8.0
        + rows
            .iter()
            .map(|r| if r.sep { 9.0 } else { 40.0 })
            .sum::<f64>();
    (264.0, height)
}

/// 启动时预创建（隐藏）：此后只定位/显示/隐藏，不再创建销毁。
pub fn precreate(app: &AppHandle) {
    let (w, h) = menu_size();
    match tauri::WebviewWindowBuilder::new(
        app,
        TRAY_MENU_WINDOW,
        WebviewUrl::App("tray-menu.html".into()),
    )
    .title("托盘菜单")
    .inner_size(w, h)
    .resizable(false)
    .decorations(false)
    .always_on_top(true)
    .skip_taskbar(true)
    .visible(false)
    .build()
    {
        Ok(_) => {}
        Err(e) => {
            crate::logging::log(&format!("tray-menu: 窗口预创建失败：{e}"));
        }
    }
}

/// 在光标处弹出菜单（调用方已在主线程）。`at` 为屏幕物理坐标。
pub fn open_menu(app: &AppHandle, at: (f64, f64)) {
    let Some(win) = app.get_webview_window(TRAY_MENU_WINDOW) else {
        crate::logging::log("tray-menu: 窗口不存在（预创建失败？）");
        return;
    };
    let (width, height) = menu_size();
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
            let cx = at.0 / scale;
            let cy = at.1 / scale;
            // 右下角贴点击点、向上展开，并在光标下方留 6px 空隙：
            // 光标停留在托盘图标上而非悬停在菜单行上（原生托盘菜单同款落位），
            // 也避免重开时某行被 hover 高亮误认为“选中态”
            let mut x = cx - width + 2.0;
            let mut y = cy - height - 6.0;
            if x < px / scale {
                x = cx - 2.0;
            }
            if y < py / scale {
                y = cy + 6.0;
            }
            (x.max(px / scale), y.max(py / scale))
        }
        None => (at.0 - width + 2.0, at.1 - height - 6.0),
    };
    crate::logging::log(&format!(
        "tray-menu: 点击=({:.0},{:.0}) 菜单=({x:.0},{y:.0}) 尺寸=({width:.0}x{height:.0})",
        at.0, at.1
    ));
    let _ = win.set_position(tauri::Position::Logical(tauri::LogicalPosition::new(x, y)));
    // 点击外部收起（物理矩形）
    bump_popup_gen();
    let scale = win.scale_factor().unwrap_or(1.0);
    if let Ok(size) = win.inner_size() {
        // 位置是逻辑坐标（乘缩放换物理）；inner_size 本身已是物理像素
        let (x0, y0) = ((x * scale) as i32, (y * scale) as i32);
        let (x1, y1) = (x0 + size.width as i32, y0 + size.height as i32);
        #[cfg(windows)]
        watch_outside_click(app.clone(), TRAY_MENU_WINDOW, (x0, y0, x1, y1));
    }
    // 先显示再发事件：隐藏窗口收不到 emit 的内容，事件仅作即时更新，
    // 页面另有 __dshdRefresh（Rust eval 直呼）作为确定性兜底
    let _ = win.show();
    if let Err(e) = app.emit_to(TRAY_MENU_WINDOW, "tray-menu-open", items()) {
        crate::logging::log(&format!("tray-menu: 事件下发失败：{e}"));
    }
    let _ = win.eval("window.__dshdRefresh && window.__dshdRefresh()");
    let _ = win.set_focus();
}

/// 隐藏菜单窗口（失焦/选中后）。
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
    std::thread::spawn(move || loop {
        std::thread::sleep(std::time::Duration::from_millis(70));
        if POPUP_GEN.load(Ordering::Relaxed) != gen {
            return; // 弹窗已关闭或重新打开
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
                crate::logging::log("tray-menu: 外部点击收起（watcher）");
                let _ = w.hide();
                return;
            }
        }
    });
}
