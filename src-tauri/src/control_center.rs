//! 统一自绘弹窗（窗口标签 app-dialog）：
//! 用量与余额 / 插件 / 设置 / 检查更新（带进度）/ 关于，以及更新提示、
//! 更新应用确认、轻量提示三个紧凑视图。
//!
//! 替代原生消息框：立即出窗显示进度，网络查询后台进行、结果经事件下发，
//! 解决"点击检查更新后长时间没有响应"的问题。窗口启动时预创建、显示前同步
//! 渲染本次内容（show 第一帧即正确内容），与托盘菜单/选择器同一套机制
//! （不在事件回调里创建/销毁窗口）。

use tauri::WebviewUrl;
use tauri::{AppHandle, Manager};

use crate::app_state::AppState;
use crate::updater::APP_REPO;

static CHECKING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
static CHECK_GENERATION: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static APP_DIALOG_SHOW_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// 弹窗窗口 label。
pub const APP_DIALOG_WINDOW: &str = "app-dialog";

/// 弹窗统一为"左侧导航 + 右侧内容"布局。正常视口采用 dsh 的 800px 卡片，
/// 小屏/高 DPI 时按主窗口逻辑视口收窄、收短，右侧内容独立滚动；尺寸不按
/// kind 切换，避免导航时窗口在指针下跳变。
/// 自绘阴影余量（dsh shadow-lv3：上扩 20px、下 12px+32px、左右 32px 扩散）：
/// 透明窗口 = 卡片 + 阴影空间，阴影由 control-center.html 卡片层自绘；
/// 余量不足会被窗口边缘硬切（视觉不自然）。
const SHADOW_TOP: f64 = crate::window::OVERLAY_SHADOW_TOP;
const SHADOW_BOTTOM: f64 = crate::window::OVERLAY_SHADOW_BOTTOM;
const SHADOW_SIDES: f64 = crate::window::OVERLAY_SHADOW_SIDES;
const CARD_MIN_WIDTH: f64 = 560.0;
const CARD_MAX_WIDTH: f64 = 800.0;
const CARD_MIN_HEIGHT: f64 = 360.0;
const CARD_MAX_HEIGHT: f64 = 800.0;

fn fit_card_width(viewport_width: f64) -> f64 {
    (viewport_width - SHADOW_SIDES * 2.0).clamp(CARD_MIN_WIDTH, CARD_MAX_WIDTH)
}

fn fit_card_height(content_height: f64) -> f64 {
    (content_height - 48.0).clamp(CARD_MIN_HEIGHT, CARD_MAX_HEIGHT)
}

/// 状态栏高度（隐藏状态栏时为 0，与 sync_bounds 口径一致）。
fn status_bar_height(app: &AppHandle) -> f64 {
    if app.state::<AppState>().config().hide_statusbar {
        0.0
    } else {
        crate::titlebar::STATUSBAR_HEIGHT
    }
}

/// 主窗口内容区逻辑视口（宽, 内容高）。只含主窗口 getter，必须在 show 锁
/// 之外调用——getter 经事件循环往返阻塞等主线程服务，持锁调用即 H1 互锁
/// （见 show_with_update_token）。
fn main_content_viewport(app: &AppHandle) -> Option<(f64, f64)> {
    let w = crate::main_window(app)?;
    let scale = w.scale_factor().ok()?;
    let size = w.inner_size().ok()?;
    Some((
        size.width as f64 / scale,
        size.height as f64 / scale - crate::titlebar::TITLEBAR_HEIGHT - status_bar_height(app),
    ))
}

/// 由主窗口内容区视口推弹窗卡片尺寸（视口未知时用规格默认值兜底）。
/// 弹窗卡片逻辑宽度正常为 800px，窄窗口时把自绘阴影也完整收进主窗口；
/// 高度口径：dsh 的 100vh 指其页面视口 = 主窗口内容区（排除自绘标题栏与
/// 状态栏），而非整个主窗口高度——直接取主窗口高度会偏大。
fn card_size_for(viewport: Option<(f64, f64)>) -> (f64, f64) {
    match viewport {
        Some((vw, vh)) => (fit_card_width(vw), fit_card_height(vh)),
        None => (CARD_MAX_WIDTH, 640.0),
    }
}

/// 紧凑弹窗（update-prompt / app-restart / notice）高度按文案长度自适应：
/// CJK 感知宽度估算行数（文案列 = 400 卡片 − 左右边距 40 − 图标 34 −
/// 间距 12 ≈ 312px，13px 字号下约 48 个半角单元/行），基准 176px 容纳
/// 4 行，超出后每行 +20px（13px×1.55 行高），至多加 6 行；更长的内容
/// 交由页内滚动兜底（前端按溢出切换拖动/滚动）。
fn compact_extra_height(text: &str) -> f64 {
    const UNITS_PER_LINE: usize = 48;
    const BASE_LINES: usize = 4;
    const LINE_HEIGHT: f64 = 20.0;
    const MAX_EXTRA_LINES: usize = 6;
    let units: usize = text
        .chars()
        .map(|c| if ('\u{2E80}'..).contains(&c) { 2 } else { 1 })
        .sum();
    let lines = units.div_ceil(UNITS_PER_LINE).max(BASE_LINES);
    (lines - BASE_LINES).min(MAX_EXTRA_LINES) as f64 * LINE_HEIGHT
}

/// 从弹窗载荷提取紧凑尺寸的高度增量。
/// 模板文案在前端 i18n.js，此处用版本号 + 固定长度占位串近似估算。
fn compact_height_hint(kind: &str, initial: &serde_json::Value) -> f64 {
    match kind {
        "notice" => initial
            .get("message")
            .and_then(|v| v.as_str())
            .map(compact_extra_height)
            .unwrap_or(0.0),
        "app-restart" | "update-prompt" => {
            let version = initial
                .get("version")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let current = initial
                .get("current")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            compact_extra_height(&format!(
                "{version} {current} 新版本已就绪，将退出并自动重启完成更新，是否继续？查看更新内容"
            ))
        }
        _ => 0.0,
    }
}

/// 弹窗窗口尺寸 = 卡片 + 阴影余量。`update-prompt`、`app-restart`
/// （更新应用确认）与 `notice`（轻量提示）是紧凑尺寸（宽 400，高按
/// 文案自适应），其余 kind 用自适应大卡片。
fn dialog_size(app: &AppHandle, kind: &str, compact_extra: f64) -> (f64, f64) {
    dialog_size_with(
        card_size_for(main_content_viewport(app)),
        kind,
        compact_extra,
    )
}

/// [`dialog_size`] 的锁外快照版：cards 由调用方在取 show 锁前读取（H1）。
fn dialog_size_with(cards: (f64, f64), kind: &str, compact_extra: f64) -> (f64, f64) {
    if matches!(kind, "update-prompt" | "app-restart" | "notice") {
        // 宽度：仅容纳最长英文文案一行（约 342px @12.5px）+ 左右 padding 40px；
        // 极长版本号由 overflow-wrap 折行兜底，不为罕见冗余预留大宽度。
        const PROMPT_CARD_WIDTH: f64 = 400.0;
        const PROMPT_CARD_HEIGHT: f64 = 176.0;
        const PROMPT_CARD_MAX_HEIGHT: f64 = 340.0;
        let height = (PROMPT_CARD_HEIGHT + compact_extra).min(PROMPT_CARD_MAX_HEIGHT);
        return (
            PROMPT_CARD_WIDTH + SHADOW_SIDES * 2.0,
            height + SHADOW_TOP + SHADOW_BOTTOM,
        );
    }
    (
        cards.0 + SHADOW_SIDES * 2.0,
        cards.1 + SHADOW_TOP + SHADOW_BOTTOM,
    )
}

/// 主窗口内容区逻辑矩形（x/y 为内容区左上角，w/h 为内容区尺寸）。
/// 统一取 inner 口径（inner_position + inner_size），与 dialog_card_width/height
/// 同源：macOS 保留系统装饰时 outer 会多算约 28px 原生标题栏，inner/outer
/// 混用会让居中结果垂直偏移其一半（约 14px）；无边框平台 inner==outer，行为不变。
fn main_inner_logical_rect(app: &AppHandle) -> Option<(f64, f64, f64, f64)> {
    let w = crate::main_window(app)?;
    let scale = w.scale_factor().ok()?;
    let pos = w.inner_position().ok()?;
    let size = w.inner_size().ok()?;
    Some((
        pos.x as f64 / scale,
        pos.y as f64 / scale,
        size.width as f64 / scale,
        size.height as f64 / scale,
    ))
}

/// 弹窗窗口（含阴影余量）相对主窗口内容区居中后的左上角逻辑坐标。
/// 注意按卡片视觉中心对齐：窗口含不对称阴影空间（上 24/下 48/左右 36），
/// 直接按窗口矩形居中会让卡片视觉中心偏下。
fn centered_dialog_pos(
    main: (f64, f64, f64, f64),
    status_h: f64,
    dialog_w: f64,
    dialog_h: f64,
) -> (f64, f64) {
    let (mlx, mly, mlw, mlh) = main;
    // dsh 弹窗对齐的是主窗口内容区（去标题栏/状态栏），非整个窗口
    let content_h = mlh - crate::titlebar::TITLEBAR_HEIGHT - status_h;
    let content_y = mly + crate::titlebar::TITLEBAR_HEIGHT;
    let dx = mlx + (mlw - (dialog_w - SHADOW_SIDES * 2.0)) / 2.0 - SHADOW_SIDES;
    let dy = content_y + (content_h - (dialog_h - SHADOW_TOP - SHADOW_BOTTOM)) / 2.0 - SHADOW_TOP;
    (dx, dy)
}

fn main_is_presented(main: &tauri::Window) -> bool {
    main.is_visible().unwrap_or(false) && !main.is_minimized().unwrap_or(false)
}

/// 打开事件载荷：标题 + 类型 + 类型相关的初始数据。
#[derive(serde::Serialize, Clone)]
pub struct AppDialogOpen {
    pub title: String,
    /// usage / plugins / settings / check / about / update-prompt / app-restart / notice
    pub kind: String,
    pub initial: serde_json::Value,
}

/// 启动时预创建（隐藏）：此后只定位/显示/隐藏。仅在主线程（启动路径）
/// 调用——内部读取主窗口几何（getter）。
pub fn precreate(app: &AppHandle) {
    // 弹窗窗口 = 自适应卡片 + 自绘阴影余量；大视口仍严格对齐 dsh 的
    // width 800 / height min(800px, 100vh-48px)。
    // 创建时即算好位置（相对主窗口内容区居中）——show 时的异步 set_position
    // 有窗口期（日志实锤：显示前位置仍是默认值），首帧错位；创建参数同步生效
    let (dialog_w, dialog_h) = dialog_size(app, "default", 0.0);
    let initial_pos = main_inner_logical_rect(app)
        .map(|rect| centered_dialog_pos(rect, status_bar_height(app), dialog_w, dialog_h));
    precreate_sized(app, (dialog_w, dialog_h), initial_pos);
}

/// 以给定尺寸/位置创建弹窗窗口。几何参数由调用方预先读取：show 锁内的
/// 现场重建（可能来自后台线程）不得读主窗口 getter（H1），其几何随后由
/// 同序列的 set_size/set_position 立即修正，且窗口保持隐藏至延迟 show。
fn precreate_sized(app: &AppHandle, size: (f64, f64), initial_pos: Option<(f64, f64)>) {
    // 导航白名单与主窗口一致：弹窗内容只允许加载内置页面（IPC 另有来源
    // 校验兜底，此处堵住内容本身被导航到任意远程地址的口子）
    let navigation_app = app.clone();
    let (dialog_w, dialog_h) = size;
    // 基础链拆成可重复构造的闭包：owner 挂接与回退置顶两条路径各自需要
    // 一条完整的 builder 链。
    let base = || {
        let mut builder = tauri::WebviewWindowBuilder::new(
            app,
            APP_DIALOG_WINDOW,
            WebviewUrl::App("control-center.html".into()),
        )
        .title(crate::APP_TITLE)
        .inner_size(dialog_w, dialog_h);
        if let Some((dx, dy)) = initial_pos {
            builder = builder.position(dx, dy);
        }
        // 透明窗口：仅有 Windows/Linux 提供 Public API（macOS 需 macos-private-api
        // feature，未启用）。macOS 上跳过 transparent，卡片层在非透明窗口内以
        // 24px 圆角自绘，效果一致（仅系统阴影差异）。
        #[cfg(not(target_os = "macos"))]
        {
            builder = builder
                .background_color(tauri::window::Color(0, 0, 0, 0))
                .transparent(true)
                .shadow(false);
        }
        #[cfg(target_os = "macos")]
        {
            builder = builder.shadow(false);
        }
        builder
            .initialization_script(crate::locale::init_script())
            .resizable(false)
            .decorations(false)
            // 固定尺寸卡片：禁止最大化/最小化，防止 Win+Up 把透明宿主窗口
            // 拉成全屏不可见点击阻挡层、Win+Down 让 skip_taskbar 弹窗无处召回
            .maximizable(false)
            .minimizable(false)
            .skip_taskbar(true)
            .visible(false)
            // 同托盘菜单：预创建的隐藏弹窗不得在 WebView2 初始化时抢焦点
            // （否则主窗口 Focused 振荡、启动页闪变淡）；打开弹窗时显式
            // set_focus 提供键盘焦点
            .focused(false)
    };
    // 层级：非 macOS 把主窗口挂为 owner（Windows）/transient（Linux）——弹窗
    // 恒在主窗口之上、随应用整体激活或退到后台，替代原先的全系统置顶。
    // macOS 的 parent 是 addChildWindow：弹窗会随主窗口隐藏而消失，仅托盘
    // 场景直接不可用，维持置顶旧语义；主窗口缺失或挂接失败同样回退置顶。
    #[cfg(not(target_os = "macos"))]
    let builder = match crate::main_window(app) {
        Some(main) => {
            // 主窗口经 get_window 获取（带子 webview 时 get_webview_window
            // 返回 None），层级挂接落到平台的 owner/transient 原语上
            #[cfg(windows)]
            let parented = main.hwnd().map(|h| base().owner_raw(h));
            #[cfg(not(windows))]
            let parented = main.gtk_window().map(|g| base().transient_for_raw(&g));
            match parented {
                Ok(parented) => parented,
                Err(e) => {
                    crate::logging::log(&format!("app-dialog: owner 挂接失败，回退置顶：{e}"));
                    base().always_on_top(true)
                }
            }
        }
        None => {
            crate::logging::log("app-dialog: 主窗口不存在，回退置顶");
            base().always_on_top(true)
        }
    };
    #[cfg(target_os = "macos")]
    let builder = base().always_on_top(true);
    match builder
        .on_navigation(move |url| {
            let allowed =
                crate::is_local_app_url(url, crate::app_dev_origin(&navigation_app).as_ref());
            if !allowed {
                crate::logging::log(&format!("app-dialog: 已拦截非白名单导航 {url}"));
            }
            allowed
        })
        .build()
    {
        Ok(win) => {
            // 透明窗口：背景色由 control-center.html 的卡片层自绘（含 24px 圆角），
            // 窗口本身不设背景色；关闭系统圆角裁剪避免与自绘圆角叠加
            let theme = app.state::<AppState>().config().resolve_dsh_theme();
            if let Some(theme) = theme {
                let _ = win.set_theme(Some(theme));
            }
            #[cfg(windows)]
            crate::window::disable_system_rounded_corners(&win);
        }
        Err(e) => {
            crate::logging::log(&format!("app-dialog: 窗口预创建失败：{e}"));
        }
    }
}

/// 定位并显示（调用方已在主线程）。
fn show(app: &AppHandle, title: &str, kind: &str, initial: serde_json::Value) {
    show_with_update_token(app, title, kind, initial, None);
}

fn show_with_update_token(
    app: &AppHandle,
    title: &str,
    kind: &str,
    initial: serde_json::Value,
    update_prompt_token: Option<u64>,
) {
    // —— H1：所有依赖主线程事件循环的窗口 getter（is_visible / inner_size /
    // inner_position / cursor_position / monitor_from_point 等）必须在取 show
    // 锁之前读取。Windows 上同步 IPC 命令在主线程执行且会取这把锁
    // （app_dialog_close / app_dialog_open_*），而后台线程同样会走到这里
    // （托盘重启失败的 open_notice、dev 预览）：持锁调用 getter 即形成
    // “后台线程持锁等主线程 + 主线程等锁”的全应用互锁。几何快照允许毫秒
    // 级陈旧（只影响居中位置），锁内只保留非阻塞的 setter 与状态提交。
    let main = crate::main_window(app);
    let mut main_presented = main.as_ref().is_some_and(main_is_presented);
    // 模态前置：主窗口隐藏/最小化（仅托盘运行）时先拉起再读几何。拉起是
    // 非阻塞 setter，不违反 H1；且必须先拉起再取 rect——否则隐藏态下
    // main_presented=false 走“指针所在屏居中”分支，多显示器下弹窗会脱离
    // owner 窗口（回到原位置的主窗口可能在另一块屏）。极小概率的
    // “stale 提交 + 主窗口被拉起”可接受：提交失败意味着已有其他弹窗在
    // 展示，彼时主窗口必然已呈现，两条件近乎互斥。
    if !main_presented {
        if let Some(w) = main.as_ref() {
            let _ = w.show();
            let _ = w.unminimize();
            main_presented = true;
            crate::logging::log("app-dialog: 主窗口未呈现，已拉起后再显示弹窗");
        }
    }
    // 最小化窗口在 Windows 上 inner_position 报 (-32000,-32000)：unminimize
    // 是异步派发，极端时序下仍可能读到哨兵值，按哨兵过滤退回屏幕居中兜底
    let main_rect = main_inner_logical_rect(app).filter(|&(_, y, _, _)| y > -20000.0);
    let cards = card_size_for(main_content_viewport(app));
    // 统一注入版本信息：导航栏底部与“关于”页从任何入口切换过去都可用。
    let mut initial = if initial.is_object() {
        initial
    } else {
        serde_json::json!({})
    };
    let Some(obj) = initial.as_object_mut() else {
        crate::logging::log("app-dialog: 初始载荷无法转换为对象，已取消显示");
        return;
    };
    obj.insert(
        "app_version".into(),
        serde_json::json!(env!("CARGO_PKG_VERSION")),
    );
    let config = app.state::<AppState>().config();
    let dsh_version = crate::runtime::installed_dsh_version(&config)
        .unwrap_or_else(|| crate::locale::text("未知", "Unknown").into());
    obj.insert("dsh_version".into(), serde_json::json!(dsh_version));
    let state = app.state::<AppState>();
    let ownership = state.service_ownership();
    obj.insert("service_mode".into(), serde_json::json!(ownership.as_str()));
    obj.insert(
        "service_ready".into(),
        serde_json::json!(
            state.phase() == crate::app_state::BootPhase::Ready
                && matches!(
                    ownership,
                    crate::app_state::ServiceOwnership::Managed
                        | crate::app_state::ServiceOwnership::External
                )
        ),
    );
    // 紧凑弹窗高度增量需在 initial 被 payload 移动前计算
    let compact_hint = compact_height_hint(kind, &initial);
    let payload = AppDialogOpen {
        title: title.to_string(),
        kind: kind.to_string(),
        initial,
    };
    let (ww, wh) = dialog_size_with(cards, kind, compact_hint);
    let mut pending_size = tauri::Size::Logical(tauri::LogicalSize::new(ww, wh));
    let mut pending_pos: Option<tauri::Position> = None;
    let mut center_fallback = false;
    let mut target_pos: Option<(f64, f64)> = None;
    if main_presented {
        // 主窗口正常显示时相对主窗口内容区居中（inner 口径，与卡片尺寸同源）。
        if let Some((mlx, mly, mlw, mlh)) = main_rect {
            let (dx, dy) =
                centered_dialog_pos((mlx, mly, mlw, mlh), status_bar_height(app), ww, wh);
            crate::logging::log(&format!(
                "app-dialog: 居中 main=({mlx:.0},{mly:.0} {mlw:.0}x{mlh:.0}) dialog=({dx:.0},{dy:.0} {ww:.0}x{wh:.0})"
            ));
            target_pos = Some((dx, dy));
            pending_pos = Some(tauri::Position::Logical(tauri::LogicalPosition::new(
                dx, dy,
            )));
        }
    } else {
        // 主窗口不存在（启动极早期/异常兜底）：按鼠标所在屏幕的工作区居中
        crate::logging::log("app-dialog: 主窗口不存在，屏幕居中");
        let monitor = app
            .cursor_position()
            .ok()
            .and_then(|p| app.monitor_from_point(p.x, p.y).ok().flatten())
            .or_else(|| app.primary_monitor().ok().flatten());
        if let Some(monitor) = monitor {
            let scale = monitor.scale_factor();
            let area = monitor.work_area();
            // 卡片视觉居中（窗口含不对称阴影空间，需按卡片尺寸计算并补偿偏移）
            let card_w = ((ww - SHADOW_SIDES * 2.0) * scale).round() as u32;
            let card_h = ((wh - SHADOW_TOP - SHADOW_BOTTOM) * scale).round() as u32;
            let x = area.position.x + (area.size.width.saturating_sub(card_w) / 2) as i32
                - (SHADOW_SIDES * scale) as i32;
            let y = area.position.y + (area.size.height.saturating_sub(card_h) / 2) as i32
                - (SHADOW_TOP * scale) as i32;
            pending_size = tauri::Size::Physical(tauri::PhysicalSize::new(
                (ww * scale).round() as u32,
                (wh * scale).round() as u32,
            ));
            target_pos = Some((x as f64 / scale, y as f64 / scale));
            pending_pos = Some(tauri::Position::Physical(tauri::PhysicalPosition::new(
                x, y,
            )));
        } else {
            center_fallback = true;
        }
    }
    // —— 状态提交、尺寸/位置与隐藏窗口内容注入必须在同一持锁序列内完成；
    // 否则一个刚被普通页面抢占的更新提示仍可能晚到并覆盖新页面。——
    let _show_guard = APP_DIALOG_SHOW_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let win = match app.get_webview_window(APP_DIALOG_WINDOW) {
        Some(w) => w,
        None => {
            // 兜底：窗口被销毁或创建失败（如主窗口恢复前用户极早打开），
            // 现场重建——重建不得读主窗口 getter（H1），几何由本序列随后
            // 的 set_size/set_position 立即修正，窗口保持隐藏至延迟 show
            crate::logging::log("app-dialog: 窗口不存在，现场重建");
            precreate_sized(app, (ww, wh), None);
            // build 从非主线程调用只是把 CreateWindow 消息入队，紧随的
            // get 可能仍为 None：短暂重试几轮再放弃（放弃后更新提示的
            // token 已占用，只能靠下一次普通 show 的 re-queue 找回）
            let mut rebuilt = None;
            for _ in 0..10 {
                if let Some(w) = app.get_webview_window(APP_DIALOG_WINDOW) {
                    rebuilt = Some(w);
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            let Some(win) = rebuilt else {
                crate::logging::log("app-dialog: 窗口重建失败");
                return;
            };
            win
        }
    };
    // 存入状态供页面拉取（隐藏窗口收不到 emit，Rust eval 直呼页面刷新兜底）
    let committed = if let Some(token) = update_prompt_token {
        state.commit_update_prompt_show(token, payload.clone())
    } else {
        state.set_last_dialog(payload.clone());
        true
    };
    if !committed {
        crate::logging::log("app-dialog: 更新提示展示权已失效，取消旧展示");
        return;
    }
    // 任何把视图切离 check 的成功提交都取消待确认的 UAC：确认 UI 已随页面
    // 切换消失，pwsh 等待线程应立即以“已取消”退出并释放 lifecycle 锁，
    // 而不是悬置到下一次任意弹窗关闭
    if kind != "check" {
        state.set_pwsh_pending(false);
    }
    // 更新提示 token 过期的路径已在上方门控返回：以下窗口副作用（尺寸/
    // 位置、代次）都只属于有效展示。主窗口的模态前置拉起已在锁外完成
    // （见函数开头）；弹窗以主窗口为 owner，统一走“内容区居中 + 禁用
    // 主窗口”的模态路径，不脱离主窗口悬浮。
    // 通过提交门控后才产生副作用：代次 +1（若上次关闭的延迟隐藏尚未执行，
    // 令其失效，避免误藏本次弹窗），并应用尺寸/位置。
    let dialog_gen = state.bump_dialog_gen();
    let _ = win.set_size(pending_size);
    if let Some(pos) = pending_pos {
        let _ = win.set_position(pos);
    } else if center_fallback {
        let _ = win.center();
    }
    // 先把本次内容同步渲染进隐藏窗口，再显示：show 的第一帧就是正确内容，
    // 不会先把上一弹窗的残影亮出一帧。载荷内联进 eval（无 IPC 往返），
    // 事件通道对隐藏窗口不可靠，下方 emit 仅作兜底（页面按载荷印章去重）
    let json = serde_json::to_string(&payload).unwrap_or_default();
    let _ = win.eval(format!("window.__dshdOpen && window.__dshdOpen({json})"));
    crate::emit_signed_to(app, APP_DIALOG_WINDOW, "app-dialog-open", &payload);
    // 尺寸/位置设置交给事件循环处理一帧后再显示，既保留首帧位置稳定性，
    // 又不在主线程用最多 1.2 秒的 sleep 轮询阻塞标题栏与菜单响应。
    let handle = app.clone();
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(16));
        let dispatch = handle.clone();
        let _ = handle.run_on_main_thread(move || {
            if dispatch.state::<AppState>().dialog_gen() != dialog_gen {
                return;
            }
            let Some(win) = dispatch.get_webview_window(APP_DIALOG_WINDOW) else {
                return;
            };
            if let Some((tx, ty)) = target_pos {
                if let Ok(position) = win.outer_position() {
                    let scale = win.scale_factor().unwrap_or(1.0);
                    crate::logging::log(&format!(
                        "app-dialog: 显示前位置 ({:.0},{:.0})，目标 ({tx:.0},{ty:.0})",
                        position.x as f64 / scale,
                        position.y as f64 / scale,
                    ));
                }
            }
            if win.show().is_ok() {
                let _ = win.set_focus();
                // WebView2：窗口级 set_focus 不把键盘焦点送进控制器——首次
                // 打开按 Esc 无反应，需先点一下弹窗。补一次 webview 级聚焦
                //（wry → MoveFocus(Programmatic)；UFCS 消 AsRef 歧义）
                let _ = tauri::Webview::set_focus(win.as_ref());
                // 模态：只在弹窗真正显示的同一代次禁用主窗口；关闭/快速重开
                // 让旧代次失效，不会留下主窗口被禁用的孤立状态。
                if main_presented {
                    if let Some(main) = crate::main_window(&dispatch) {
                        let _ = main.set_enabled(false);
                        dispatch.state::<AppState>().set_main_disabled(true);
                    }
                }
            } else {
                // show 失败（窗口异常销毁等）：不留下"主窗口被禁用且无弹窗
                // 可关"的孤立态，直接恢复主窗口
                restore_main_after_dialog(&dispatch);
            }
        });
    });
}

/// 隐藏弹窗（关闭按钮/动作完成后）：恢复主窗口可用状态。
///
/// 页面已先把内容淡出（内容不可见），这里清空内容后再等一帧绘制：
/// 淡出+清空后的中性表面会成为下次 show 的第一帧，
/// 否则下次打开会先闪出上一弹窗的残影。
pub fn close(app: &AppHandle) {
    let closed_kind = app.state::<AppState>().dialog_kind().unwrap_or_default();
    // close 的副作用（eval 复位/状态收尾/延迟隐藏）必须与 show 的“提交+注入”
    // 临界区同锁序列化：此前 close 不取锁，其副作用可切入 show 序列中间，
    // 产生空白弹窗或“状态与可见性脱节”（提示静默丢失）。锁内已无主线程
    // getter（H1 修复），主线程取锁不会与后台 show 互锁。
    let pending = {
        let _close_guard = APP_DIALOG_SHOW_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        // 代次 +1：令挂起的延迟隐藏失效（关闭后立刻重开不会被误藏）
        let gen = app.state::<AppState>().bump_dialog_gen();
        // 关闭弹窗视为取消待确认的 UAC 预告
        app.state::<AppState>().set_pwsh_pending(false);
        let pending = if let Some(w) = app.get_webview_window(APP_DIALOG_WINDOW) {
            let _ = w.eval("window.__dshdReset && window.__dshdReset()");
            let pending = app.state::<AppState>().finish_dialog_close(&closed_kind);
            if pending.is_none() {
                // 先恢复主窗口，让 WebView2 在弹窗仍占据前台时完成一帧合成；
                // 下一帧再隐藏弹窗，避免露出主窗口后面的内容。
                restore_main_after_dialog(app);
                let handle = app.clone();
                std::thread::spawn(move || {
                    // 仅等一帧确保清空已提交；此前等待 50ms 会让空卡片明显停顿。
                    std::thread::sleep(std::time::Duration::from_millis(16));
                    let h2 = handle.clone();
                    let _ = handle.run_on_main_thread(move || {
                        if h2.state::<AppState>().dialog_gen() == gen {
                            if let Some(w) = h2.get_webview_window(APP_DIALOG_WINDOW) {
                                let _ = w.hide();
                            }
                        }
                    });
                });
            }
            pending
        } else {
            let pending = app.state::<AppState>().finish_dialog_close(&closed_kind);
            if pending.is_none() {
                restore_main_after_dialog(app);
            }
            pending
        };
        pending
    };
    // 连续提示的换页展示在锁外发起（present → show 自身取锁；std Mutex
    // 不可重入），同样在当前前台窗口内直接换页，不产生 hide/show 层级断档。
    if let Some((next, token)) = pending {
        present_update_prompt(app, next, token);
    }
}

fn restore_main_after_dialog(app: &AppHandle) {
    if !app.state::<AppState>().main_disabled() {
        return;
    }
    app.state::<AppState>().set_main_disabled(false);
    if let Some(main) = crate::main_window(app) {
        let _ = main.set_enabled(true);
        let _ = main.set_focus();
    }
}

/// 主窗口唤醒路径（托盘打开/单实例/Resumed）的焦点归还：弹窗模态打开期间
/// 主窗口被禁用，唤醒时应聚焦弹窗本身而非不可交互的主窗口。返回是否聚焦了弹窗。
pub fn focus_dialog_if_visible(app: &AppHandle) -> bool {
    match app.get_webview_window(APP_DIALOG_WINDOW) {
        Some(w) if w.is_visible().unwrap_or(false) => {
            let _ = w.set_focus();
            // 与 show 路径同因：WebView2 需 webview 级聚焦才有键盘输入
            let _ = tauri::Webview::set_focus(w.as_ref());
            true
        }
        _ => false,
    }
}

// ---------- 检查更新 ----------

/// 打开"更新进行中"视图：作为 win32 提示框确认后的更新进度载体——
/// 不重跑检查，页面显示"正在更新…"并实时承接 update-progress /
/// update-result 事件（弹窗内更新按钮路径复用同一机制）。
pub fn open_update_progress(app: &AppHandle) {
    // 对齐 run_check 的 reset 语义：清旧检查结果与完成状态，避免弹窗
    // 轮询用旧 last_check 覆盖"正在更新…"视图（旧结果行会闪现可更新按钮）
    let state = app.state::<AppState>();
    state.set_last_check(None);
    state.set_update_done(false, None);
    state.set_check_progress(Some(crate::locale::text("正在更新…", "Updating…").into()));
    show(
        app,
        crate::locale::text("检查更新", "Check for updates"),
        "check",
        serde_json::json!({ "updating": true }),
    );
}

/// 检查更新弹窗当前是否可见（更新失败时决定是否弹 win32 兜底提示，
/// 避免弹窗内已显示失败原因时重复打扰）。
pub fn is_check_open(app: &AppHandle) -> bool {
    app.state::<AppState>().dialog_kind().as_deref() == Some("check")
}

/// 更新提示载荷：dsh 与应用自身两个场景共用同一自绘弹窗。
#[derive(serde::Serialize, Clone)]
pub struct UpdatePrompt {
    /// "dsh"（发现新版，立即更新）或 "app"（已下载就绪，重启并更新）。
    pub kind: String,
    /// 新版本号。
    pub version: String,
    /// 当前版本号（dsh 场景展示；app 场景可省略）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current: Option<String>,
    /// 新版本 release 页面（有则显示「查看更新内容」链接）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub release_url: Option<String>,
    /// dev 构建注入的模拟数据：标题栏追加「模拟数据」标识，避免与真实
    /// 更新提示混淆。正式版恒为 None。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub simulated: Option<bool>,
}

/// 打开更新提示弹窗（替代原生 msgbox：可点链接、深浅色/reduced-motion 齐全）。
/// dsh 与应用两个更新可能几乎同时触发；单窗无法并排，正在显示时入队，
/// 关闭当前后依次出队，避免后弹者覆盖前者导致选择丢失。
pub fn open_update_prompt(app: &AppHandle, prompt: UpdatePrompt) {
    // 原子申请展示权（单次加锁内判断+置位/入队），规避并发 TOCTOU 与
    // show() 16ms 延迟显示造成的时序盲区。
    if let Some(token) = app
        .state::<AppState>()
        .acquire_update_prompt_show(prompt.clone())
    {
        present_update_prompt(app, prompt, token);
    }
}

/// 实际展示单个更新提示。
fn present_update_prompt(app: &AppHandle, prompt: UpdatePrompt, token: u64) {
    let mut title: std::borrow::Cow<'static, str> = if prompt.kind == "app" {
        crate::locale::text("应用更新已就绪", "App update ready").into()
    } else {
        crate::locale::text("发现新版本", "Update available").into()
    };
    if prompt.simulated == Some(true) {
        // dev 模拟数据：标题栏追加标识，避免与真实更新提示混淆
        title = format!(
            "{title}（{}）",
            crate::locale::text("模拟数据", "Simulated")
        )
        .into();
    }
    let initial =
        serde_json::to_value(prompt.clone()).unwrap_or(serde_json::json!({ "kind": prompt.kind }));
    show_with_update_token(app, &title, "update-prompt", initial, Some(token));
}

/// 打开检查更新弹窗：立即出窗显示进度，检查在后台执行、结果写入状态，
/// 页面轮询拉取。
pub fn open_check(app: &AppHandle) {
    // 更新仍在后台执行时：不重置状态、不重跑检查，轮询直接拉取进行中的
    // 进度与结果（进度已同步写入 check_progress 状态，事件通道对隐藏窗口
    // 不可靠），按钮保持禁用，避免并发更新。
    let updating = app.state::<AppState>().is_updating();
    if updating {
        // 更新中重开：清旧结果与完成状态，轮询不会用旧 last_check
        // 覆盖"正在更新…"视图（与 open_update_progress 一致）
        let state = app.state::<AppState>();
        state.set_last_check(None);
        state.set_update_done(false, None);
    }
    show(
        app,
        crate::locale::text("检查更新", "Check for updates"),
        "check",
        serde_json::json!({ "updating": updating }),
    );
    if !updating {
        run_check(app);
    }
}

/// 触发一次更新检查（导航切到"检查更新"页时调用；弹窗内不重复 show）。
/// 更新执行中不重置状态、不并发检查（与 open_check 行为一致）。
pub fn run_check(app: &AppHandle) {
    if app.state::<AppState>().is_updating() {
        return;
    }
    // 每次请求都推进代次。已有检查不并发启动，但完成后会发现代次变化并按
    // 最新配置重跑；旧通道的晚到结果因此既不会覆盖，也不会让新请求丢失。
    CHECK_GENERATION.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let handle = app.clone();
    handle.state::<AppState>().set_last_check(None);
    handle.state::<AppState>().set_update_done(false, None);
    handle.state::<AppState>().set_check_progress(Some(
        crate::locale::text("正在检查更新…", "Checking for updates…").into(),
    ));
    if CHECKING.swap(true, std::sync::atomic::Ordering::SeqCst) {
        return;
    }
    std::thread::spawn(move || {
        loop {
            let generation = CHECK_GENERATION.load(std::sync::atomic::Ordering::SeqCst);
            let result = crate::updater::check(&handle);
            if CHECK_GENERATION.load(std::sync::atomic::Ordering::SeqCst) != generation {
                continue;
            }
            handle.state::<AppState>().set_last_check(Some(result));
            handle.state::<AppState>().set_check_progress(None);
            CHECKING.store(false, std::sync::atomic::Ordering::SeqCst);
            // 覆盖“结果提交”和释放 CHECKING 之间到达的新请求。若另一线程已
            // 抢到检查权，本线程退出；否则继续复用当前线程完成最新代次。
            if CHECK_GENERATION.load(std::sync::atomic::Ordering::SeqCst) == generation
                || CHECKING.swap(true, std::sync::atomic::Ordering::SeqCst)
            {
                break;
            }
            handle.state::<AppState>().set_last_check(None);
            handle.state::<AppState>().set_check_progress(Some(
                crate::locale::text("正在检查更新…", "Checking for updates…").into(),
            ));
        }
    });
}

/// 弹窗内点击“更新/安装”：后台执行并写入结果状态。
pub fn apply_update(app: &AppHandle, which: &str) {
    // 更新已在执行时直接忽略后续请求（M3）：按钮禁用生效前到达的重复 IPC
    // 可能以 "check" 来源进入（第一次请求已把视图切到进度页），没有
    // update-prompt 的 token 门控，会重复启动更新并闪现“正在进行”假失败
    if app.state::<AppState>().is_updating() {
        crate::logging::log(&format!("app-dialog: 更新进行中，忽略重复请求：{which}"));
        return;
    }
    let handle = app.clone();
    let which = which.to_string();
    // 只消费用户正在确认的这一条提示；dsh 与应用更新各自排队，互不丢弃。
    let state = handle.state::<AppState>();
    if state.dialog_kind().as_deref() == Some("update-prompt")
        && !state.consume_active_update_prompt(&which)
    {
        crate::logging::log(&format!("app-dialog: 忽略已过期或重复的更新确认：{which}"));
        return;
    }
    if which == "dsh" {
        // dsh 更新统一走进度载体（内部会 open_update_progress + 结果处理），
        // 与更新提示弹窗的“立即更新”共用同一入口，体验一致。
        crate::updater::apply_dsh_update(&handle);
        return;
    }
    if which == "app" {
        // 应用更新入口分流：
        // - 更新提示弹窗（应用更新已就绪）：按钮即“重启并更新”，已获确认，直接执行；
        // - 检查更新页：按钮只表达更新意图，先弹自绘确认（应用将退出并重启）；
        // - 自绘确认弹窗（app-restart）：确认按钮，直接执行。
        // 退出并重启的确认全部走自绘弹窗，不再弹原生 msgbox。
        match handle.state::<AppState>().dialog_kind().as_deref() {
            Some("update-prompt") | Some("app-restart") => {}
            Some("check") => {
                let version = handle
                    .state::<AppState>()
                    .last_check()
                    .and_then(|r| r.app)
                    .filter(|info| info.update_available && !info.latest.is_empty())
                    .map(|info| info.latest);
                open_app_restart_confirm(&handle, version, false);
                return;
            }
            other => {
                crate::logging::log(&format!(
                    "app-dialog: 忽略未知来源的应用更新请求（当前弹窗：{other:?}）"
                ));
                return;
            }
        }
        // 应用更新也是异步操作；切入统一进度页后由轮询呈现真实失败和重试信息。
        open_update_progress(&handle);
    }
    std::thread::spawn(move || {
        let success_message = match which.as_str() {
            "node" => crate::locale::text("Node.js 更新完成。", "Node.js was updated."),
            "pwsh" => crate::locale::text(
                "PowerShell 7 安装或更新完成。",
                "PowerShell 7 was installed or updated.",
            ),
            _ => crate::locale::text("操作完成。", "Operation completed."),
        };
        let (ok, message) = match crate::updater::apply(&handle, &which) {
            Ok(()) => (true, success_message.to_string()),
            Err(e) => (false, e),
        };
        handle
            .state::<AppState>()
            .set_update_done(ok, Some(message));
    });
}

// ---------- 更新应用确认 ----------

/// 打开自绘"更新应用"确认弹窗（替代原生 msgbox：应用将退出并自动重启）。
/// `version`：检查更新页路径可带最新版本号；dev 效果测试用 `simulated` 标记。
pub fn open_app_restart_confirm(app: &AppHandle, version: Option<String>, simulated: bool) {
    let mut title = crate::locale::text("更新应用", "Update app").to_string();
    if simulated {
        title = format!(
            "{title}（{}）",
            crate::locale::text("模拟数据", "Simulated")
        );
    }
    show(
        app,
        &title,
        "app-restart",
        serde_json::json!({ "version": version, "simulated": simulated }),
    );
}

/// 确认弹窗取消：不触发新检查，仅回到检查更新视图（沿用上次结果渲染）。
pub fn open_check_view(app: &AppHandle) {
    // nonce：返回视图的载荷可能与上次完全相同（检查结果未变），
    // 前端 applyOpen 按载荷印章去重会跳过重渲染，确认弹窗内容残留
    show(
        app,
        crate::locale::text("检查更新", "Check for updates"),
        "check",
        serde_json::json!({
            "updating": app.state::<AppState>().is_updating(),
            "nonce": app.state::<AppState>().dialog_gen(),
        }),
    );
}

/// 打开轻量提示弹窗（自绘，单"关闭"按钮）：替代托盘动作失败/拒绝类
/// 原生 msgbox。`severity`："warn"（拒绝/失败，琥珀三角）或 "info"
/// （中性说明，蓝色圆 i）。更新提示/应用更新确认正在展示时不抢占
/// 统一弹窗（顶掉会丢失那条提示），罕见冲突路径回落原生框。
pub fn open_notice(app: &AppHandle, title: &str, message: String, severity: &str) {
    let info = severity == "info";
    if matches!(
        app.state::<AppState>().dialog_kind().as_deref(),
        Some("update-prompt") | Some("app-restart")
    ) {
        crate::logging::log("app-dialog: 更新提示展示中，轻量提示回落原生框");
        // blocking_show 阻塞调用线程直到用户点掉；托盘事件在主线程处理，
        // 直接调用会冻结全部 UI——放后台线程执行（内部 getter 等主线程
        // 服务，彼时主线程空闲，无互锁风险）
        let title_owned = title.to_string();
        let kind = if info {
            tauri_plugin_dialog::MessageDialogKind::Info
        } else {
            tauri_plugin_dialog::MessageDialogKind::Warning
        };
        let handle = app.clone();
        std::thread::spawn(move || {
            crate::native_dialog::show_message(&handle, message, &title_owned, kind);
        });
        return;
    }
    show(
        app,
        title,
        "notice",
        serde_json::json!({ "message": message, "severity": if info { "info" } else { "warn" } }),
    );
}

/// dev 效果预览：依序弹出自绘弹窗的各视图（均带模拟数据标记），每弹一个
/// 等用户关闭后再弹下一个。正式构建不会调用（bootstrap 以 devUrl 门控）。
pub fn dev_preview_dialogs(app: &AppHandle) {
    std::thread::sleep(std::time::Duration::from_millis(1500));
    // 真实数据优先：预览弹窗先做一次真实更新检查，有可用更新则展示真实
    // 版本与链接（非模拟）；查询失败或无更新时才回退 9.9.9 模拟数据
    let real = crate::updater::check(app);
    let dsh_real = real.dsh.as_ref().filter(|d| d.update_available);
    let app_real = real.app.as_ref().filter(|a| a.update_available);
    let dsh_version = dsh_real
        .map(|d| d.latest.clone())
        .unwrap_or_else(|| "9.9.9".into());
    let dsh_simulated = dsh_real.is_none();
    let app_version = app_real
        .map(|a| a.latest.clone())
        .unwrap_or_else(|| "9.9.9".into());
    let app_simulated = app_real.is_none();

    open_app_restart_confirm(app, Some(dsh_version.clone()), dsh_simulated);
    wait_dialog_closed(app);
    open_notice(
        app,
        crate::locale::text("重启 dsh 服务", "Restart dsh service"),
        crate::locale::text(
            "更新流程正在进行，请稍后再重启。",
            "An update is in progress. Please restart the service later.",
        )
        .into(),
        "warn",
    );
    wait_dialog_closed(app);
    open_update_prompt(
        app,
        UpdatePrompt {
            kind: "app".into(),
            version: app_version,
            current: None,
            release_url: app_real
                .map(|a| format!("https://github.com/{APP_REPO}/releases/tag/v{}", a.latest)),
            simulated: app_simulated.then_some(true),
        },
    );
    wait_dialog_closed(app);
    open_update_prompt(
        app,
        UpdatePrompt {
            kind: "dsh".into(),
            version: dsh_version,
            current: Some(
                dsh_real
                    .map(|d| d.installed.clone())
                    .unwrap_or_else(|| "1.1.0".into()),
            ),
            release_url: None,
            simulated: dsh_simulated.then_some(true),
        },
    );
    wait_dialog_closed(app);
    // 检查更新视图（含 spinner；不触发真实检查）
    // 检查更新视图：预览收尾切到真实检查入口（open_check 会发起网络
    // 检查）；此前用 open_check_view 只出页面不检查，spinner 永远不停
    open_check(app);
}

/// 等待统一弹窗关闭（dev 预览序列用）。
fn wait_dialog_closed(app: &AppHandle) {
    loop {
        std::thread::sleep(std::time::Duration::from_millis(400));
        if app.state::<AppState>().dialog_kind().is_none() {
            return;
        }
    }
}

// ---------- 关于 ----------

/// 打开关于弹窗。
pub fn open_about(app: &AppHandle) {
    show(
        app,
        crate::locale::text("关于", "About"),
        "about",
        serde_json::json!({}),
    );
}

/// 插件管理（统一弹窗内）：内容由前端拉取，无需初始载荷。
pub fn open_plugins(app: &AppHandle) {
    if !crate::tray_menu::action_enabled(app, "plugins") {
        return;
    }
    show(
        app,
        crate::locale::text("管理插件", "Plugins"),
        "plugins",
        serde_json::json!({}),
    );
}

/// 打开统一的「用量与余额」弹窗。
pub fn open_usage(app: &AppHandle) {
    if !crate::tray_menu::managed_service_ready(app) {
        return;
    }
    show(
        app,
        crate::locale::text("用量与余额", "Usage & balance"),
        "usage",
        serde_json::json!({}),
    );
}

/// 设置（统一弹窗内）：桌面行为、界面显示、本地凭据、dsh/插件与模型配置。
/// 状态与切换由前端经各领域命令完成。
pub fn open_settings(app: &AppHandle) {
    show(
        app,
        crate::locale::text("设置", "Settings"),
        "settings",
        serde_json::json!({}),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compact_height_stays_at_base_for_short_text() {
        // ≤4 行（含中英混排）不加高
        assert_eq!(compact_extra_height("dsh 服务当前未运行。"), 0.0);
        assert_eq!(
            compact_extra_height(&"x".repeat(48 * 4)), // 恰好 4 行
            0.0
        );
        assert_eq!(compact_height_hint("check", &serde_json::json!({})), 0.0);
    }

    #[test]
    fn compact_height_grows_per_line_and_caps() {
        // 5 行：+19；中文字符按 2 单元计
        assert_eq!(compact_extra_height(&"x".repeat(48 * 4 + 1)), 20.0);
        assert_eq!(compact_extra_height(&"汉".repeat(24 * 4 + 1)), 20.0);
        // 至多加 6 行（10 行以上封顶）
        assert_eq!(compact_extra_height(&"x".repeat(48 * 20)), 6.0 * 20.0);
        // 载荷提取：notice 用 message 原文，其余 kind 为 0
        assert_eq!(
            compact_height_hint("notice", &serde_json::json!({ "message": "短" })),
            0.0
        );
        // 常规版本号的确认弹窗（模板约 2 行）维持基准高度
        assert_eq!(
            compact_height_hint("app-restart", &serde_json::json!({ "version": "1.0.0" })),
            0.0
        );
    }

    #[test]
    fn dialog_card_keeps_dsh_size_on_roomy_viewports() {
        assert_eq!(fit_card_width(1280.0), 800.0);
        assert_eq!(fit_card_height(900.0), 800.0);
    }

    #[test]
    fn dialog_card_fits_compact_logical_viewports() {
        assert_eq!(fit_card_width(720.0) + SHADOW_SIDES * 2.0, 720.0);
        assert_eq!(fit_card_height(456.0), 408.0);
    }

    #[test]
    fn dialog_card_has_usable_lower_bounds() {
        assert_eq!(fit_card_width(400.0), CARD_MIN_WIDTH);
        assert_eq!(fit_card_height(300.0), CARD_MIN_HEIGHT);
    }

    #[test]
    fn dialog_center_aligns_card_visual_center_with_content_area() {
        // 主窗口内容区逻辑矩形 (100,200 1200x900)，状态栏可见（26px）
        let dialog_w = 800.0 + SHADOW_SIDES * 2.0;
        let dialog_h = 640.0 + SHADOW_TOP + SHADOW_BOTTOM;
        let (dx, dy) = centered_dialog_pos((100.0, 200.0, 1200.0, 900.0), 26.0, dialog_w, dialog_h);
        // 卡片视觉中心必须落在内容区（去标题栏/状态栏）中心
        let card_cx = dx + SHADOW_SIDES + 800.0 / 2.0;
        let card_cy = dy + SHADOW_TOP + 640.0 / 2.0;
        let content_cx = 100.0 + 1200.0 / 2.0;
        let content_cy = 200.0
            + crate::titlebar::TITLEBAR_HEIGHT
            + (900.0 - crate::titlebar::TITLEBAR_HEIGHT - 26.0) / 2.0;
        assert!(
            (card_cx - content_cx).abs() < 1e-9,
            "水平未对齐：{card_cx} vs {content_cx}"
        );
        assert!(
            (card_cy - content_cy).abs() < 1e-9,
            "垂直未对齐：{card_cy} vs {content_cy}"
        );
    }
}
