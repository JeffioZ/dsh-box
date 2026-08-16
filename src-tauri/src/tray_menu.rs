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
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<TrayMenuItem>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub checked: Option<bool>,
}

impl TrayMenuItem {
    fn row(id: &str, label: &str) -> Self {
        Self {
            id: id.to_string(),
            label: label.to_string(),
            sep: false,
            children: Vec::new(),
            checked: None,
        }
    }
    fn choice(id: &str, label: &str, checked: bool) -> Self {
        Self {
            checked: Some(checked),
            ..Self::row(id, label)
        }
    }
    fn parent(id: &str, label: &str, children: Vec<TrayMenuItem>) -> Self {
        Self {
            children,
            ..Self::row(id, label)
        }
    }
    fn sep() -> Self {
        Self {
            id: String::new(),
            label: String::new(),
            sep: true,
            children: Vec::new(),
            checked: None,
        }
    }
}

/// 托盘与标题栏共用的菜单模型。标题栏版本不含窗口内已有的动作
/// （打开应用与余额入口）。
pub fn items(tray_surface: bool) -> Vec<TrayMenuItem> {
    let mut rows = vec![
        TrayMenuItem::row(
            "open",
            &format!(
                "{} {}",
                crate::locale::text("打开", "Open"),
                crate::APP_TITLE
            ),
        ),
        TrayMenuItem::row(
            "open_browser",
            crate::locale::text("在浏览器中打开", "Open in browser"),
        ),
        TrayMenuItem::sep(),
        TrayMenuItem::row(
            "restart",
            crate::locale::text("重启服务", "Restart service"),
        ),
        TrayMenuItem::row(
            "check_update",
            crate::locale::text("检查更新…", "Check for updates…"),
        ),
        TrayMenuItem::row(
            "plugins",
            crate::locale::text("插件管理…", "Plugin manager…"),
        ),
        TrayMenuItem::row(
            "session_diff",
            crate::locale::text("会话文件变更…", "Session file changes…"),
        ),
        TrayMenuItem::sep(),
        TrayMenuItem::row(
            "autostart",
            &format!(
                "{}: {}",
                crate::locale::text("开机自启动", "Launch at startup"),
                if autostart::is_enabled() {
                    crate::locale::text("已开启", "On")
                } else {
                    crate::locale::text("已关闭", "Off")
                }
            ),
        ),
        TrayMenuItem::row(
            "hide_tool_calls",
            &format!(
                "{}: {}",
                crate::locale::text("隐藏工具调用", "Hide tool calls"),
                if crate::app_state::Config::load().hide_tool_calls {
                    crate::locale::text("已开启", "On")
                } else {
                    crate::locale::text("已关闭", "Off")
                }
            ),
        ),
        TrayMenuItem::sep(),
        TrayMenuItem::parent(
            "language",
            crate::locale::text("语言", "Language"),
            vec![
                TrayMenuItem::choice("language_zh", "中文", crate::locale::is_chinese()),
                TrayMenuItem::choice("language_en", "English", !crate::locale::is_chinese()),
            ],
        ),
    ];
    // 多 profile 时追加“启动 profile”子菜单（单选，切换后重启生效）
    if let Some(profile_item) = profile_menu_item() {
        rows.push(profile_item);
    }
    rows.push(TrayMenuItem::sep());
    rows.push(TrayMenuItem::row(
        "about",
        crate::locale::text("关于", "About"),
    ));
    rows.push(TrayMenuItem::row(
        "quit",
        crate::locale::text("退出", "Quit"),
    ));
    if tray_surface {
        rows.insert(
            1,
            TrayMenuItem::row(
                "balance",
                crate::locale::text("查询 API 余额…", "Check API balance…"),
            ),
        );
    } else {
        rows.remove(0);
    }
    rows
}

/// “启动 profile”子菜单：多个可用 profile 时显示（单选，切换后重启生效）。
fn profile_menu_item() -> Option<TrayMenuItem> {
    let config = crate::app_state::Config::load();
    let profiles = crate::app_state::list_profiles(&config);
    if profiles.len() <= 1 {
        return None;
    }
    Some(TrayMenuItem::parent(
        "profile",
        crate::locale::text("启动 profile", "Launch profile"),
        profiles
            .into_iter()
            .map(|name| {
                TrayMenuItem::choice(&format!("profile_{name}"), &name, name == config.profile)
            })
            .collect(),
    ))
}

/// 窗口四周的阴影边距。暂为 0：透明窗口下无阴影，卡片直接占满窗口
/// （圆角外的四个角透明）；如后续恢复阴影方案再调大。
#[cfg(windows)]
const SHADOW_PAD: f64 = 0.0;

/// 菜单窗口尺寸（含四周阴影边距）：卡片内边距 4×2 + 行高 40
/// + 分隔线 9（与 dsh 菜单条目同规格），宽 264 容纳最长条目。
///
/// 卡片无描边（透明窗口模型，边界由圆角/底色/阴影承担），高度不含边框；
/// body 为 border-box，高度必须包含内边距，否则末行 hover 会被裁掉。
#[cfg(windows)]
fn menu_size(language_expanded: bool) -> (f64, f64) {
    let rows = items(true);
    let height = 8.0
        + rows
            .iter()
            .map(|r| if r.sep { 9.0 } else { 40.0 })
            .sum::<f64>()
        + if language_expanded { 80.0 } else { 0.0 };
    (264.0 + SHADOW_PAD * 2.0, height + SHADOW_PAD * 2.0)
}

/// 托盘菜单窗口几何基线（物理像素）：(base_w, base_h, current_h)。
/// 展开/收起围绕基线加减高度，宽度恒为基线宽，收起精确还原基线高——
/// 不读系统回报尺寸（inner/outer 语义与设置值有 1-2px 差，逐次累加
/// 会造成位置上移、宽度漂移、末行残余裁切）。
#[derive(Clone, Copy)]
struct Geometry {
    base_w: i32,
    base_h: i32,
    current_h: i32,
}
static GEOMETRY: std::sync::Mutex<Option<Geometry>> = std::sync::Mutex::new(None);

/// 启动时预创建（隐藏）：此后只定位/显示/隐藏，不再创建销毁。
#[cfg(windows)]
pub fn precreate(app: &AppHandle) {
    let (w, h) = menu_size(false);
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
            // 不透明窗口 + 卡片色背景：透明窗口的逐像素合成对圆角抗锯齿
            // 像素有缺陷（透明间隙/黑边/方块角），此路线已弃用；
            // 圆角交给 Win11 系统裁剪（Win10 直角），主题按 dsh 偏好固定
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
    let (width, height) = menu_size(false);
    let _ = win.set_size(tauri::Size::Logical(tauri::LogicalSize::new(width, height)));
    // 记录本次设置后的实际外尺寸基线，供展开/收起锚定（见 GEOMETRY 说明）
    if let Ok(size) = win.outer_size() {
        *GEOMETRY.lock().unwrap_or_else(|e| e.into_inner()) = Some(Geometry {
            base_w: size.width as i32,
            base_h: size.height as i32,
            current_h: size.height as i32,
        });
    }
    let _ = win.eval("window.__dshdCollapseMenu && window.__dshdCollapseMenu()");
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
        #[cfg(windows)]
        {
            let (x1, y1) = (x0 + size.width as i32, y0 + size.height as i32);
            watch_outside_click(app.clone(), TRAY_MENU_WINDOW, (x0, y0, x1, y1));
        }
    }
    // 先显示再发事件：隐藏窗口收不到 emit 的内容，事件仅作即时更新，
    // 页面另有 __dshdRefresh（Rust eval 直呼）作为确定性兜底
    let _ = win.show();
    if let Err(e) = app.emit_to(TRAY_MENU_WINDOW, "tray-menu-open", items(true)) {
        crate::logging::log(&format!("tray-menu: 事件下发失败：{e}"));
    }
    let _ = win.eval("window.__dshdRefresh && window.__dshdRefresh()");
    let _ = win.set_focus();
}

/// 子菜单展开/收起时调整托盘窗口高度，底缘始终锚定托盘点击点。
/// `id` 为父菜单项 id（language / profile）；未展开时精确还原基线高。
pub fn set_submenu_expanded(app: &AppHandle, id: &str, expanded: bool) {
    let Some(win) = app.get_webview_window(TRAY_MENU_WINDOW) else {
        return;
    };
    let Ok(position) = win.outer_position() else {
        return;
    };
    let scale = win.scale_factor().unwrap_or(1.0);
    // 子菜单展开高度增量（逻辑像素）：语言 2 行；profile 每项一行
    let expand_rows: i32 = match id {
        "language" => 2,
        "profile" => {
            let config = crate::app_state::Config::load();
            crate::app_state::list_profiles(&config).len() as i32
        }
        _ => 0,
    };
    // 以基线几何为准：宽度恒定、高度只加减子菜单的物理增量，
    // 收起时精确还原基线高（消除残余裁切与宽度漂移）
    let geo = *GEOMETRY.lock().unwrap_or_else(|e| e.into_inner());
    let (base_w, base_h, current_h) = match geo {
        Some(g) => (g.base_w, g.base_h, g.current_h),
        None => {
            let Ok(size) = win.outer_size() else {
                return;
            };
            (size.width as i32, size.height as i32, size.height as i32)
        }
    };
    let new_width = base_w;
    let new_height = if expanded {
        base_h + (expand_rows as f64 * 40.0 * scale).round() as i32
    } else {
        base_h
    };
    // 底缘锚定：只动上缘，底边始终贴住托盘点击点
    let bottom = position.y + current_h;
    let new_y = bottom - new_height;
    *GEOMETRY.lock().unwrap_or_else(|e| e.into_inner()) = Some(Geometry {
        base_w,
        base_h,
        current_h: new_height,
    });

    #[cfg(windows)]
    {
        // 单次 SetWindowPos 原子完成移动+缩放：分两步会出现
        // “先长高/缩短、再上移/下移”的中间帧，底缘跳动即闪烁来源
        if let Ok(hwnd) = win.hwnd() {
            use windows_sys::Win32::UI::WindowsAndMessaging::SetWindowPos;
            let ok = unsafe {
                SetWindowPos(
                    hwnd.0,
                    std::ptr::null_mut(),
                    position.x,
                    new_y,
                    new_width,
                    new_height,
                    0x0004u32 | 0x0010u32, // SWP_NOZORDER | SWP_NOACTIVATE
                )
            };
            if ok != 0 {
                bump_popup_gen();
                watch_outside_click(
                    app.clone(),
                    TRAY_MENU_WINDOW,
                    (
                        position.x,
                        new_y,
                        position.x + new_width,
                        new_y + new_height,
                    ),
                );
                return;
            }
        }
    }

    // 兜底路径（非 Windows 或 hwnd 不可用）：先移上缘再改高度
    let _ = win.set_position(tauri::Position::Physical(tauri::PhysicalPosition::new(
        position.x, new_y,
    )));
    let _ = win.set_size(tauri::Size::Physical(tauri::PhysicalSize::new(
        new_width as u32,
        new_height as u32,
    )));

    bump_popup_gen();
    #[cfg(windows)]
    watch_outside_click(
        app.clone(),
        TRAY_MENU_WINDOW,
        (
            position.x,
            new_y,
            position.x + new_width,
            new_y + new_height,
        ),
    );
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
                crate::logging::log("tray-menu: 检测到外部点击，收起菜单");
                let _ = w.hide();
                return;
            }
        }
    });
}
