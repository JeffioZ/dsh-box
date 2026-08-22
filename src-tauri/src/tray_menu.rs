//! 自绘托盘菜单：无边框小窗（tray-menu.html + IPC），替代 Windows 原生
//! TrackPopupMenu 弹出菜单。
//!
//! 背景：原生弹出在本环境不稳定（首次右键不弹、弹出即闪退）。
//! 关键实现约束：窗口在启动时预创建一次、此后只 定位/显示/隐藏——
//! 绝不在事件回调里新建或销毁 WebView 窗口（否则主线程卡死）。

#[cfg(windows)]
use tauri::WebviewUrl;
use tauri::{AppHandle, Manager};

#[cfg(windows)]
use crate::app_state::AppState;

/// 托盘菜单窗口 label。
pub const TRAY_MENU_WINDOW: &str = "tray-menu";

#[cfg(windows)]
static TRAY_MENU_READY: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

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
    pub enabled: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub disabled_reason: Option<String>,
}

impl TrayMenuItem {
    fn row(id: &str, label: &str) -> Self {
        Self {
            id: id.to_string(),
            label: label.to_string(),
            sep: false,
            icon: None,
            enabled: true,
            disabled_reason: None,
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
            enabled: true,
            disabled_reason: None,
        }
    }
}

#[derive(Clone, Copy)]
struct MenuContext {
    phase: crate::app_state::BootPhase,
    ownership: crate::app_state::ServiceOwnership,
    updating: bool,
}

impl MenuContext {
    fn from_app(app: &AppHandle) -> Self {
        let state = app.state::<crate::app_state::AppState>();
        Self {
            phase: state.phase(),
            ownership: state.service_ownership(),
            updating: state.is_updating(),
        }
    }

    fn service_ready(self) -> bool {
        self.phase == crate::app_state::BootPhase::Ready
            && matches!(
                self.ownership,
                crate::app_state::ServiceOwnership::Managed
                    | crate::app_state::ServiceOwnership::External
            )
    }

    fn managed_ready(self) -> bool {
        self.phase == crate::app_state::BootPhase::Ready
            && self.ownership == crate::app_state::ServiceOwnership::Managed
    }
}

fn enabled_for(context: MenuContext, id: &str) -> bool {
    match id {
        "open_browser" => context.service_ready(),
        "restart" => context.managed_ready() && !context.updating,
        "plugins" => context.managed_ready() && !context.updating,
        // 余额直接查询 DeepSeek API，不依赖 dsh 进程是否已启动；但外部服务的
        // 凭据不归 DSHBox 管理，不能误用本地凭据展示另一套账户。
        "balance" => !context.ownership.is_external(),
        _ => true,
    }
}

fn disabled_reason(context: MenuContext, id: &str) -> Option<String> {
    if enabled_for(context, id) {
        return None;
    }
    if context.ownership.is_external() && matches!(id, "restart" | "plugins" | "balance") {
        return Some(
            crate::locale::text(
                "请在外部服务的原环境中管理",
                "Manage this in the external service's environment",
            )
            .into(),
        );
    }
    if matches!(id, "restart" | "plugins") && context.updating {
        return Some(
            crate::locale::text("更新完成后可用", "Available after the update finishes").into(),
        );
    }
    Some(
        crate::locale::text(
            "dsh 服务就绪后可用",
            "Available when the dsh service is ready",
        )
        .into(),
    )
}

pub fn action_enabled(app: &AppHandle, id: &str) -> bool {
    enabled_for(MenuContext::from_app(app), id)
}

pub fn managed_service_ready(app: &AppHandle) -> bool {
    MenuContext::from_app(app).managed_ready()
}

/// 原生托盘只在能力签名变化时重建，避免下载进度事件高频刷新菜单资源。
#[cfg(not(windows))]
pub fn capability_signature(app: &AppHandle) -> u8 {
    let context = MenuContext::from_app(app);
    u8::from(enabled_for(context, "open_browser"))
        | (u8::from(enabled_for(context, "restart")) << 1)
        | (u8::from(enabled_for(context, "balance")) << 2)
        | (u8::from(enabled_for(context, "plugins")) << 3)
}

pub fn contextual_items(app: &AppHandle, tray_surface: bool) -> Vec<TrayMenuItem> {
    let context = MenuContext::from_app(app);
    items(tray_surface)
        .into_iter()
        .map(|mut item| {
            if !item.sep {
                item.enabled = enabled_for(context, &item.id);
                item.disabled_reason = disabled_reason(context, &item.id);
            }
            item
        })
        .collect()
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
            crate::locale::text("打开 DSHBox", "Open DSHBox"),
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
        crate::locale::text("重启 dsh 服务", "Restart dsh service"),
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
            crate::locale::text("API 余额…", "API balance…"),
        ));
    }
    rows.push(TrayMenuItem::row_icon(
        "plugins",
        "puzzle",
        crate::locale::text("管理插件…", "Manage plugins…"),
    ));
    rows.push(TrayMenuItem::row_icon(
        "settings",
        "gear",
        crate::locale::text("设置…", "Settings…"),
    ));
    rows.push(TrayMenuItem::sep());
    // 关于/退出
    rows.push(TrayMenuItem::row_icon(
        "about",
        "info",
        crate::locale::text("关于 DSHBox", "About DSHBox"),
    ));
    rows.push(TrayMenuItem::row_icon(
        "quit",
        "exit",
        crate::locale::text("退出", "Quit"),
    ));
    rows
}

/// 透明宿主窗口的自绘阴影余量。与 tray-menu.html 的 body padding 同步；
/// 菜单使用 dsh shadow-lv3；透明余量只承载阴影，不参与卡片布局。
#[cfg(windows)]
const SHADOW_SIDES: f64 = crate::window::OVERLAY_SHADOW_SIDES;
#[cfg(windows)]
const SHADOW_TOP: f64 = crate::window::OVERLAY_SHADOW_TOP;
#[cfg(windows)]
const SHADOW_BOTTOM: f64 = crate::window::OVERLAY_SHADOW_BOTTOM;
#[cfg(windows)]
const MENU_CARD_WIDTH: f64 = 220.0;

/// 菜单卡片尺寸：完整 1px 描边、上下各 4px 内边距、行高 40、分隔线 9；宽 220
/// 与标题栏主菜单完全一致，容纳最长条目（含图标/内边距约 180px）。
/// 注意：Windows 自绘托盘菜单宽度与 ui/titlebar.html 的 .main-menu-panel
/// （220px）保持一致，改动需同步两处。
#[cfg(windows)]
fn menu_card_size() -> (f64, f64) {
    let rows = items(true);
    let height = 10.0
        + rows
            .iter()
            .map(|r| if r.sep { 9.0 } else { 40.0 })
            .sum::<f64>();
    (MENU_CARD_WIDTH, height)
}

/// 原生窗口尺寸 = 菜单卡片 + 自绘阴影透明余量。
#[cfg(windows)]
fn menu_size() -> (f64, f64) {
    let (width, height) = menu_card_size();
    (
        width + SHADOW_SIDES * 2.0,
        height + SHADOW_TOP + SHADOW_BOTTOM,
    )
}

/// 启动时预创建（隐藏）：此后只定位/显示/隐藏，不再创建销毁。
#[cfg(windows)]
pub fn precreate(app: &AppHandle) {
    use std::sync::atomic::Ordering;
    TRAY_MENU_READY.store(false, Ordering::Release);
    let (w, h) = menu_size();
    let theme = app.state::<AppState>().config().resolve_dsh_theme();
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
    // 与统一弹窗相同：透明性只在创建时设置一次。运行期重设背景色会让
    // WebView2 重建合成层，产生方底、闪烁或整窗透明。
    .background_color(tauri::window::Color(0, 0, 0, 0))
    .transparent(true)
    .shadow(false)
    .background_throttling(tauri::utils::config::BackgroundThrottlingPolicy::Disabled)
    .always_on_top(true)
    .skip_taskbar(true)
    .visible(false)
    .on_navigation(move |url| {
        let allowed = crate::is_local_app_url(url, crate::app_dev_origin(&navigation_app).as_ref());
        if allowed {
            TRAY_MENU_READY.store(false, Ordering::Release);
        }
        if !allowed {
            crate::logging::log(&format!("tray-menu: 已拦截非白名单导航 {url}"));
        }
        allowed
    })
    .on_page_load(|_webview, payload| {
        let finished = payload.event() == tauri::webview::PageLoadEvent::Finished;
        TRAY_MENU_READY.store(finished, Ordering::Release);
        if finished {
            crate::logging::log("tray-menu: 页面已就绪");
        }
    })
    .build()
    {
        Ok(win) => {
            if let Some(theme) = theme {
                let _ = win.set_theme(Some(theme));
            }
            #[cfg(windows)]
            crate::window::disable_system_rounded_corners(&win);
        }
        Err(e) => {
            crate::logging::log(&format!("tray-menu: 窗口预创建失败：{e}"));
        }
    }
}

/// 在光标处弹出菜单（调用方已在主线程）。`at` 为屏幕物理坐标。
#[cfg(windows)]
pub fn open_menu(app: &AppHandle, at: (f64, f64)) {
    open_menu_when_ready(app, at, 0);
}

#[cfg(windows)]
fn open_menu_when_ready(app: &AppHandle, at: (f64, f64), attempt: u8) {
    use std::sync::atomic::Ordering;
    // 首次页面尚未完成加载时不展示透明空窗；保留最后一次右键请求，最多等待
    // 1 秒。新请求会推进代次，使旧重试自动失效。
    if !TRAY_MENU_READY.load(Ordering::Acquire) {
        if attempt >= 25 {
            crate::logging::log("tray-menu: 页面 1s 内未就绪，取消本次显示");
            return;
        }
        let gen = bump_popup_gen();
        let scheduler = app.clone();
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(40));
            if POPUP_GEN.load(Ordering::Relaxed) != gen {
                return;
            }
            let handle = scheduler.clone();
            let _ = scheduler.run_on_main_thread(move || {
                if POPUP_GEN.load(Ordering::Relaxed) == gen {
                    open_menu_when_ready(&handle, at, attempt + 1);
                }
            });
        });
        return;
    }
    // 打开菜单时即时比对 dsh 设置（语言/主题）：用户在 dsh 里刚切换过，
    // 这次打开立即生效，不等 3s 轮询
    crate::tray::check_dsh_settings_now(app);
    let Some(win) = app.get_webview_window(TRAY_MENU_WINDOW) else {
        crate::logging::log("tray-menu: 窗口不存在（预创建失败？）");
        return;
    };
    // 新一代打开使尚未执行的退场隐藏失效；窗口当前可见时先静默收起，
    // 避免透明宿主在重新定位过程中被用户看到横跨屏幕移动。
    let generation = bump_popup_gen();
    if win.is_visible().unwrap_or(false) {
        let _ = win.hide();
        let _ = win.eval("window.__dshdReset && window.__dshdReset()");
    }
    let (card_width, card_height) = menu_card_size();
    let (width, height) = menu_size();
    // 托盘点击点位于任务栏，通常不在 monitor.work_area() 内；直接按点找到
    // 所在显示器，并全程使用物理像素，避免隐藏窗口按旧屏幕 DPI 二次换算。
    let monitor = app
        .monitor_from_point(at.0, at.1)
        .ok()
        .flatten()
        .or_else(|| app.primary_monitor().ok().flatten());
    let (x, y, physical_width, physical_height, scale, opens_up) = if let Some(monitor) = monitor {
        let scale = monitor.scale_factor();
        let area = monitor.work_area();
        let monitor_position = monitor.position();
        let monitor_size = monitor.size();
        let physical_width = (width * scale).round() as u32;
        let physical_height = (height * scale).round() as u32;
        let physical_card_width = (card_width * scale).round();
        let physical_card_height = (card_height * scale).round();
        let shadow_left = SHADOW_SIDES * scale;
        let shadow_top = SHADOW_TOP * scale;
        let work_left = area.position.x as f64;
        let work_top = area.position.y as f64;
        let work_right = work_left + area.size.width as f64;
        let work_bottom = work_top + area.size.height as f64;
        let screen_left = monitor_position.x as f64;
        let screen_top = monitor_position.y as f64;
        let screen_right = screen_left + monitor_size.width as f64;
        let screen_bottom = screen_top + monitor_size.height as f64;
        let win_w = physical_width as f64;
        let win_h = physical_height as f64;
        // 先按视觉卡片定位，再向外扩出透明阴影窗口；这样阴影余量不会改变
        // 菜单相对托盘图标的锚点。卡片限制在工作区，透明阴影宿主限制在
        // 完整屏幕；否则扩大阴影余量会把菜单整体推离任务栏约 48px。
        let preferred_card_x = at.0 - physical_card_width + 2.0 * scale;
        let card_x =
            preferred_card_x.clamp(work_left, (work_right - physical_card_width).max(work_left));
        let preferred_card_y = at.1 - physical_card_height - 6.0 * scale;
        let opens_up = preferred_card_y >= work_top;
        let preferred_card_y = if opens_up {
            preferred_card_y
        } else {
            at.1 + 6.0 * scale
        };
        let card_y =
            preferred_card_y.clamp(work_top, (work_bottom - physical_card_height).max(work_top));
        let x = (card_x - shadow_left)
            .clamp(screen_left, (screen_right - win_w).max(screen_left))
            .round() as i32;
        let y = (card_y - shadow_top)
            .clamp(screen_top, (screen_bottom - win_h).max(screen_top))
            .round() as i32;
        (x, y, physical_width, physical_height, scale, opens_up)
    } else {
        let scale = win.scale_factor().unwrap_or(1.0);
        let physical_width = (width * scale).round() as u32;
        let physical_height = (height * scale).round() as u32;
        (
            (at.0 - card_width * scale + 2.0 * scale - SHADOW_SIDES * scale).round() as i32,
            (at.1 - card_height * scale - 6.0 * scale - SHADOW_TOP * scale).round() as i32,
            physical_width,
            physical_height,
            scale,
            true,
        )
    };
    crate::logging::log(&format!(
        "tray-menu: 点击=({:.0},{:.0}) 菜单=({x},{y}) 尺寸=({physical_width}x{physical_height}) scale={scale:.2}",
        at.0, at.1,
    ));
    if let Err(e) = win.set_position(tauri::Position::Physical(tauri::PhysicalPosition::new(
        x, y,
    ))) {
        crate::logging::log(&format!("tray-menu: 定位失败：{e}"));
    }
    if let Err(e) = win.set_size(tauri::Size::Physical(tauri::PhysicalSize::new(
        physical_width,
        physical_height,
    ))) {
        crate::logging::log(&format!("tray-menu: 设置尺寸失败：{e}"));
    }
    // 隐藏窗口内先同步填入菜单并复位入场初态。eval 本身只代表脚本已下发，
    // eval_with_callback 才保证 JS 已执行完；在回调前 show 是首次展示残留底边/
    // 旧 hover 层的根因。
    let rows = contextual_items(app, true);
    let json = serde_json::to_string(&rows).unwrap_or_else(|_| "[]".to_string());
    let direction = if opens_up { "up" } else { "down" };
    // 点击外部收起按视觉卡片矩形判定，透明阴影区域不算菜单内部。
    let card_x = x + (SHADOW_SIDES * scale).round() as i32;
    let card_y = y + (SHADOW_TOP * scale).round() as i32;
    let card_w = (card_width * scale).round() as i32;
    let card_h = (card_height * scale).round() as i32;
    let card_rect = (card_x, card_y, card_x + card_w, card_y + card_h);
    let script =
        format!("(() => window.__dshdOpen ? window.__dshdOpen({json}, {direction:?}) : false)()");
    wait_for_geometry_then_prepare(
        app.clone(),
        win,
        generation,
        (x, y, physical_width, physical_height),
        card_rect,
        script,
        0,
    );
}

#[cfg(windows)]
fn wait_for_geometry_then_prepare(
    app: AppHandle,
    win: tauri::WebviewWindow,
    generation: u64,
    target: (i32, i32, u32, u32),
    card_rect: (i32, i32, i32, i32),
    script: String,
    attempt: u8,
) {
    use std::sync::atomic::Ordering;
    if POPUP_GEN.load(Ordering::Relaxed) != generation {
        return;
    }
    let (x, y, width, height) = target;
    let position_ok = win
        .outer_position()
        .map(|position| position.x == x && position.y == y)
        .unwrap_or(false);
    let size_ok = win
        .inner_size()
        .map(|size| size.width == width && size.height == height)
        .unwrap_or(false);

    // SetWindowPos 在 WebView2 上可能异步完成。不能在 UI 主线程 sleep 轮询，
    // 否则恰好阻塞几何消息和应用其他窗口；短延迟后重入主线程等待首帧稳定。
    if (!position_ok || !size_ok) && attempt < 30 {
        let scheduler = app.clone();
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(20));
            if POPUP_GEN.load(Ordering::Relaxed) != generation {
                return;
            }
            let handle = scheduler.clone();
            let _ = scheduler.run_on_main_thread(move || {
                wait_for_geometry_then_prepare(
                    handle,
                    win,
                    generation,
                    target,
                    card_rect,
                    script,
                    attempt + 1,
                );
            });
        });
        return;
    }
    if !position_ok || !size_ok {
        crate::logging::log("tray-menu: 几何 600ms 内未稳定，按最新位置继续显示");
    }

    let callback_app = app.clone();
    let callback_win = win.clone();
    if let Err(e) = win.eval_with_callback(script, move |result| {
        if result != "true" {
            crate::logging::log(&format!("tray-menu: 首帧准备失败，取消显示：{result}"));
            return;
        }
        present_prepared_menu(&callback_app, &callback_win, generation, card_rect);
    }) {
        crate::logging::log(&format!("tray-menu: 首帧脚本下发失败：{e}"));
    }
}

#[cfg(windows)]
fn present_prepared_menu(
    app: &AppHandle,
    win: &tauri::WebviewWindow,
    generation: u64,
    card_rect: (i32, i32, i32, i32),
) {
    use std::sync::atomic::Ordering;
    // JS 回调可能晚于下一次打开/关闭请求；旧代次绝不能把已取消窗口重新显示。
    if POPUP_GEN.load(Ordering::Relaxed) != generation {
        return;
    }
    let _ = win.set_ignore_cursor_events(false);
    if let Err(e) = win.show() {
        crate::logging::log(&format!("tray-menu: show 失败：{e}"));
        return;
    }
    crate::logging::log(&format!(
        "tray-menu: 已显示 is_visible={}",
        win.is_visible().unwrap_or(false)
    ));
    watch_outside_click(app.clone(), TRAY_MENU_WINDOW, card_rect);
    let _ = win.set_focus();
}

/// 播放退场动效后隐藏。代次校验确保快速重开可中断旧动效，最终状态不依赖
/// transitionend（页面卡顿/减弱动态效果均不会留下可见窗口）。
pub fn hide_menu(app: &AppHandle) {
    use std::sync::atomic::Ordering;
    let gen = bump_popup_gen();
    let Some(w) = app.get_webview_window(TRAY_MENU_WINDOW) else {
        return;
    };
    if !w.is_visible().unwrap_or(false) {
        return;
    }
    let _ = w.eval("window.__dshdClose && window.__dshdClose()");
    // 退场期间立即穿透鼠标，透明宿主不会为了 90ms 动效阻塞底层交互；
    // 快速重开会在 show 前恢复命中。
    let _ = w.set_ignore_cursor_events(true);
    let handle = app.clone();
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(90));
        let h2 = handle.clone();
        let _ = handle.run_on_main_thread(move || {
            if POPUP_GEN.load(Ordering::Relaxed) != gen {
                return;
            }
            if let Some(w) = h2.get_webview_window(TRAY_MENU_WINDOW) {
                let _ = w.hide();
                let _ = w.eval("window.__dshdReset && window.__dshdReset()");
            }
        });
    });
}

/// 两处自绘菜单的动作分发（menu_choose 调用）。
pub fn run_action(app: &AppHandle, id: &str) {
    // 菜单页面共用同一套 70ms 按压反馈与关闭状态机；后端只负责立即分发动作。
    if !action_enabled(app, id) {
        crate::logging::log(&format!("menu: 已忽略当前不可用的动作 {id}"));
        return;
    }
    crate::tray::run_action(app, id);
}

// ---------- 弹窗通用：点击外部收起 ----------

static POPUP_GEN: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// 每次弹窗打开/关闭时递增：旧监视线程与延迟隐藏据此失效，避免误关新弹窗。
pub(crate) fn bump_popup_gen() -> u64 {
    use std::sync::atomic::Ordering;
    POPUP_GEN.fetch_add(1, Ordering::Relaxed) + 1
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
        // 初始视为按住：忽略触发菜单的那次右键，待全部松开后只响应下一次
        // 鼠标按下沿，避免菜单刚显示就被同一次右键误收起。
        let mut was_down = true;
        loop {
            std::thread::sleep(std::time::Duration::from_millis(20));
            if POPUP_GEN.load(Ordering::Relaxed) != gen {
                return;
            }
            let Some(w) = app.get_webview_window(label) else {
                return;
            };
            if !w.is_visible().unwrap_or(false) {
                return;
            }
            let any_down = unsafe {
                GetAsyncKeyState(0x01) < 0
                    || GetAsyncKeyState(0x02) < 0
                    || GetAsyncKeyState(0x04) < 0
            };
            let pressed = any_down && !was_down;
            was_down = any_down;
            if !pressed {
                continue;
            }
            let mut pt = windows_sys::Win32::Foundation::POINT { x: 0, y: 0 };
            unsafe { GetCursorPos(&mut pt) };
            let (x0, y0, x1, y1) = rect;
            if pt.x < x0 || pt.x >= x1 || pt.y < y0 || pt.y >= y1 {
                crate::logging::log("tray-menu: 检测到外部点击，收起菜单");
                hide_menu(&app);
                return;
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

    #[test]
    fn service_actions_follow_ownership_and_phase() {
        let starting = MenuContext {
            phase: crate::app_state::BootPhase::Starting,
            ownership: crate::app_state::ServiceOwnership::None,
            updating: false,
        };
        assert!(!enabled_for(starting, "open_browser"));
        assert!(!enabled_for(starting, "restart"));
        assert!(!enabled_for(starting, "plugins"));
        assert!(enabled_for(starting, "balance"));

        let managed = MenuContext {
            phase: crate::app_state::BootPhase::Ready,
            ownership: crate::app_state::ServiceOwnership::Managed,
            updating: false,
        };
        assert!(enabled_for(managed, "open_browser"));
        assert!(enabled_for(managed, "restart"));
        assert!(enabled_for(managed, "plugins"));

        let updating = MenuContext {
            updating: true,
            ..managed
        };
        assert!(!enabled_for(updating, "restart"));
        assert!(!enabled_for(updating, "plugins"));

        let external = MenuContext {
            phase: crate::app_state::BootPhase::Ready,
            ownership: crate::app_state::ServiceOwnership::External,
            updating: false,
        };
        assert!(enabled_for(external, "open_browser"));
        assert!(!enabled_for(external, "restart"));
        assert!(!enabled_for(external, "plugins"));
        assert!(!enabled_for(external, "balance"));
    }

    #[cfg(windows)]
    #[test]
    fn transparent_window_reserves_exact_shadow_margins() {
        let (card_width, card_height) = menu_card_size();
        let (window_width, window_height) = menu_size();
        assert_eq!(window_width - card_width, SHADOW_SIDES * 2.0);
        assert_eq!(window_height - card_height, SHADOW_TOP + SHADOW_BOTTOM);
    }
}
