//! 统一自绘弹窗（app-dialog 窗口 + dialog.html）：
//! 余额详情 / 检查更新（带进度）/ 关于。
//!
//! 替代原生消息框：立即出窗显示进度，网络查询后台进行、结果经事件下发，
//! 解决“点击检查更新后长时间没有响应”的问题。窗口启动时预创建、显示前同步
//! 渲染本次内容（show 第一帧即正确内容），与托盘菜单/选择器同一套机制
//! （不在事件回调里创建/销毁窗口）。

use tauri::WebviewUrl;
use tauri::{AppHandle, Emitter, Manager};

use crate::app_state::AppState;

/// 弹窗窗口 label。
pub const APP_DIALOG_WINDOW: &str = "app-dialog";

/// 弹窗统一为"左侧导航 + 右侧内容"布局：固定尺寸容纳导航栏与内容区。
/// 680 宽（导航 152 + 内容 528）、480 高——列表页（插件/会话文件）内容在
/// 内容区滚动浏览，轻量页（余额/设置/关于）不再因统一高度而大片留空。
/// 高度不做按 kind 切换（切换时窗口跳变观感差），统一折中。
fn dialog_size(_kind: &str) -> (f64, f64) {
    (680.0, 480.0)
}

fn main_is_presented(main: &tauri::Window) -> bool {
    main.is_visible().unwrap_or(false) && !main.is_minimized().unwrap_or(false)
}

/// 打开事件载荷：标题 + 类型 + 类型相关的初始数据。
#[derive(serde::Serialize, Clone)]
pub struct AppDialogOpen {
    pub title: String,
    /// balance / check / about
    pub kind: String,
    pub initial: serde_json::Value,
}

/// 启动时预创建（隐藏）：此后只定位/显示/隐藏。
pub fn precreate(app: &AppHandle) {
    // 导航白名单与主窗口一致：弹窗内容只允许加载内置页面（IPC 另有来源
    // 校验兜底，此处堵住内容本身被导航到任意远程地址的口子）
    let navigation_app = app.clone();
    match tauri::WebviewWindowBuilder::new(
        app,
        APP_DIALOG_WINDOW,
        WebviewUrl::App("dialog.html".into()),
    )
    .title(crate::APP_TITLE)
    .inner_size(680.0, 480.0)
    .initialization_script(crate::locale::init_script())
    .resizable(false)
    .decorations(false)
    .always_on_top(true)
    .skip_taskbar(true)
    .visible(false)
    .on_navigation(move |url| {
        let allowed = crate::is_local_app_url(url, crate::app_dev_origin(&navigation_app).as_ref());
        if !allowed {
            crate::logging::log(&format!("app-dialog: 已拦截非白名单导航 {url}"));
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
            crate::logging::log(&format!("app-dialog: 窗口预创建失败：{e}"));
        }
    }
}

/// 定位并显示（调用方已在主线程）。
fn show(app: &AppHandle, title: &str, kind: &str, initial: serde_json::Value) {
    let Some(win) = app.get_webview_window(APP_DIALOG_WINDOW) else {
        crate::logging::log("app-dialog: 窗口不存在（预创建失败？）");
        return;
    };
    // 代次 +1：若上次关闭的延迟隐藏尚未执行，令其失效，避免误藏本次弹窗
    app.state::<AppState>().bump_dialog_gen();
    let (ww, wh) = dialog_size(kind);
    let _ = win.set_size(tauri::Size::Logical(tauri::LogicalSize::new(ww, wh)));
    let main = crate::main_window(app);
    let main_presented = main.as_ref().is_some_and(main_is_presented);
    if main_presented {
        // 主窗口正常显示时沿用原行为，相对主窗口居中。
        if let Some(main) = main.as_ref() {
            if let (Ok(mp), Ok(ms)) = (main.outer_position(), main.outer_size()) {
                let scale = main.scale_factor().unwrap_or(1.0);
                let mlx = mp.x as f64 / scale;
                let mly = mp.y as f64 / scale;
                let mlw = ms.width as f64 / scale;
                let mlh = ms.height as f64 / scale;
                let _ = win.set_position(tauri::Position::Logical(tauri::LogicalPosition::new(
                    mlx + (mlw - ww) / 2.0,
                    mly + (mlh - wh) / 2.0,
                )));
            }
        }
    } else {
        // 仅托盘运行时，按鼠标所在屏幕的工作区居中（避开任务栏）。
        let monitor = app
            .cursor_position()
            .ok()
            .and_then(|p| app.monitor_from_point(p.x, p.y).ok().flatten())
            .or_else(|| app.primary_monitor().ok().flatten());
        if let Some(monitor) = monitor {
            let scale = monitor.scale_factor();
            let area = monitor.work_area();
            let width = (ww * scale).round() as u32;
            let height = (wh * scale).round() as u32;
            let x = area.position.x + (area.size.width.saturating_sub(width) / 2) as i32;
            let y = area.position.y + (area.size.height.saturating_sub(height) / 2) as i32;
            let _ = win.set_size(tauri::Size::Physical(tauri::PhysicalSize::new(
                width, height,
            )));
            let _ = win.set_position(tauri::Position::Physical(tauri::PhysicalPosition::new(
                x, y,
            )));
        } else {
            let _ = win.center();
        }
    }
    // 统一注入版本信息：导航栏底部与“关于”页从任何入口切换过去都可用。
    let mut initial = if initial.is_object() {
        initial
    } else {
        serde_json::json!({})
    };
    let obj = initial
        .as_object_mut()
        .expect("dialog initial must be an object");
    obj.insert(
        "app_version".into(),
        serde_json::json!(env!("CARGO_PKG_VERSION")),
    );
    let config = app.state::<AppState>().config();
    let dsh_version = crate::runtime::installed_dsh_version(&config)
        .unwrap_or_else(|| crate::locale::text("未知", "Unknown").into());
    obj.insert("dsh_version".into(), serde_json::json!(dsh_version));
    let payload = AppDialogOpen {
        title: title.to_string(),
        kind: kind.to_string(),
        initial,
    };
    // 存入状态供页面拉取（隐藏窗口收不到 emit，Rust eval 直呼页面刷新兜底）
    app.state::<AppState>().set_last_dialog(payload.clone());
    // 先把本次内容同步渲染进隐藏窗口，再显示：show 的第一帧就是正确内容，
    // 不会先把上一弹窗的残影亮出一帧。载荷内联进 eval（无 IPC 往返），
    // 事件通道对隐藏窗口不可靠，下方 emit 仅作兜底（页面按载荷印章去重）
    let json = serde_json::to_string(&payload).unwrap_or_default();
    let _ = win.eval(format!("window.__dshdOpen && window.__dshdOpen({json})"));
    let _ = win.show();
    if let Err(e) = app.emit_to(APP_DIALOG_WINDOW, "app-dialog-open", payload) {
        crate::logging::log(&format!("app-dialog: 事件下发失败：{e}"));
    }
    let _ = win.set_focus();
    // 模态：主窗口可见时禁用之（点击主窗口无效，符合系统模态语义）；
    // 主窗口隐藏（仅托盘运行）时不处理，弹窗独立显示。关闭时恢复。
    if let Some(main) = main {
        if main_presented {
            let _ = main.set_enabled(false);
            app.state::<AppState>().set_main_disabled(true);
        }
    }
}

/// 隐藏弹窗（关闭按钮/动作完成后）：恢复主窗口可用状态。
///
/// 页面已先把内容淡出（内容不可见），这里清空内容后再等一帧绘制：
/// 淡出+清空后的中性表面会成为下次 show 的第一帧，
/// 否则下次打开会先闪出上一弹窗的残影。
pub fn close(app: &AppHandle) {
    // 代次 +1：令挂起的延迟隐藏失效（关闭后立刻重开不会被误藏）
    let gen = app.state::<AppState>().bump_dialog_gen();
    // 关闭弹窗视为取消待确认的 UAC 预告
    app.state::<AppState>().set_pwsh_pending(false);
    if app.state::<AppState>().main_disabled() {
        app.state::<AppState>().set_main_disabled(false);
        if let Some(main) = crate::main_window(app) {
            let _ = main.set_enabled(true);
        }
    }
    if let Some(w) = app.get_webview_window(APP_DIALOG_WINDOW) {
        let _ = w.eval("window.__dshdReset && window.__dshdReset()");
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
}

// ---------- 余额 ----------

/// 打开余额弹窗：立即出窗显示“查询中…”，查询在后台执行、结果写入状态，
/// 页面轮询拉取（事件通道对该窗口不可靠）。
pub fn open_balance(app: &AppHandle) {
    show(
        app,
        crate::locale::text("API 余额", "API balance"),
        "balance",
        serde_json::json!(null),
    );
    let handle = app.clone();
    let config = app.state::<AppState>().config();
    // stale-while-revalidate：保留上次缓存立即渲染（打开不长时间转圈），
    // 后台刷新完成后替换。首次打开无缓存时短暂显示“查询中…”，
    // 查询 ≤10s 短超时内完成
    std::thread::spawn(move || {
        let payload = crate::balance::query_balance(&config);
        handle.state::<AppState>().set_last_balance(Some(payload));
    });
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
    app.get_webview_window(APP_DIALOG_WINDOW)
        .is_some_and(|w| w.is_visible().unwrap_or(false))
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
    let handle = app.clone();
    handle.state::<AppState>().set_last_check(None);
    handle.state::<AppState>().set_update_done(false, None);
    handle.state::<AppState>().set_check_progress(Some(
        crate::locale::text("正在检查更新…", "Checking for updates…").into(),
    ));
    std::thread::spawn(move || {
        let result = crate::updater::check(&handle);
        handle.state::<AppState>().set_last_check(Some(result));
        handle.state::<AppState>().set_check_progress(None);
    });
}

/// 弹窗内点击“更新/安装”：后台执行并写入结果状态。
pub fn apply_update(app: &AppHandle, which: &str) {
    let handle = app.clone();
    let which = which.to_string();
    std::thread::spawn(move || {
        let success_message = match which.as_str() {
            "dsh" => crate::locale::text("dsh 更新完成。", "dsh was updated."),
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
    show(
        app,
        crate::locale::text("插件管理", "Plugin manager"),
        "plugins",
        serde_json::json!({}),
    );
}

/// 设置（统一弹窗内）：四个开关（开机自启 / 隐藏工具调用 / 隐藏统计行 /
/// 隐藏状态栏），状态与切换由前端经 settings_get / settings_set 完成。
pub fn open_settings(app: &AppHandle) {
    show(
        app,
        crate::locale::text("桌面端设置", "Desktop settings"),
        "settings",
        serde_json::json!({}),
    );
}
