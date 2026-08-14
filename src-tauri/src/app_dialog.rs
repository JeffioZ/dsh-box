//! 统一自绘弹窗（app-dialog 窗口 + dialog.html）：
//! 余额详情 / 检查更新（带进度）/ 关于。
//!
//! 替代原生消息框：立即出窗显示进度，网络查询后台进行、结果经事件下发，
//! 解决“点了检查更新等半天没反应”的问题。窗口启动时预创建、显示前同步
//! 渲染本次内容（show 第一帧即正确内容），与托盘菜单/选择器同一套机制
//! （不在事件回调里创建/销毁窗口）。

use tauri::WebviewUrl;
use tauri::{AppHandle, Emitter, Manager};

use crate::app_state::AppState;

/// 弹窗窗口 label。
pub const APP_DIALOG_WINDOW: &str = "app-dialog";

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
    let win = match tauri::WebviewWindowBuilder::new(
        app,
        APP_DIALOG_WINDOW,
        WebviewUrl::App("dialog.html".into()),
    )
    .title("DeepSeek Harness Desktop")
    .inner_size(380.0, 320.0)
    .resizable(false)
    .decorations(false)
    .always_on_top(true)
    .skip_taskbar(true)
    // 屏外停车位：预绘制时在此显示，不会在启动瞬间闪到屏幕上
    .position(-32000.0, -32000.0)
    .visible(false)
    .build()
    {
        Ok(win) => win,
        Err(e) => {
            crate::logging::log(&format!("app-dialog: 窗口预创建失败：{e}"));
            return;
        }
    };
    pre_paint(app, &win);
}

/// 预绘制（仅 Windows）：隐藏窗口不绘制，首次 show 会先闪一帧白底。
/// 启动时在屏外停车位显示一次，让页面完成首帧绘制，1.2s 后隐藏
/// （代次守卫：期间用户已打开弹窗则不隐藏）。其余平台窗口保持隐藏。
#[cfg(windows)]
fn pre_paint(app: &AppHandle, win: &tauri::WebviewWindow) {
    let gen = app.state::<AppState>().dialog_gen();
    let _ = win.show();
    let handle = app.clone();
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(1200));
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

#[cfg(not(windows))]
fn pre_paint(_app: &AppHandle, _win: &tauri::WebviewWindow) {}

/// 居中于主窗口并显示（调用方已在主线程）。
fn show(app: &AppHandle, title: &str, kind: &str, initial: serde_json::Value) {
    let Some(win) = app.get_webview_window(APP_DIALOG_WINDOW) else {
        crate::logging::log("app-dialog: 窗口不存在（预创建失败？）");
        return;
    };
    // 代次 +1：若上次关闭的延迟隐藏尚未执行，令其失效，避免误藏本次弹窗
    app.state::<AppState>().bump_dialog_gen();
    // 居中于主窗口（逻辑坐标）
    if let Some(main) = crate::main_window(app) {
        if main.is_visible().unwrap_or(false) {
            if let (Ok(mp), Ok(ms)) = (main.outer_position(), main.outer_size()) {
                let scale = main.scale_factor().unwrap_or(1.0);
                let mlx = mp.x as f64 / scale;
                let mly = mp.y as f64 / scale;
                let mlw = ms.width as f64 / scale;
                let mlh = ms.height as f64 / scale;
                let (ww, wh) = (380.0, 320.0);
                let _ = win.set_position(tauri::Position::Logical(tauri::LogicalPosition::new(
                    mlx + (mlw - ww) / 2.0,
                    mly + (mlh - wh) / 2.0,
                )));
            }
        }
    }
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
    if let Some(main) = crate::main_window(app) {
        if main.is_visible().unwrap_or(false) {
            let _ = main.set_enabled(false);
            app.state::<AppState>().set_main_disabled(true);
        }
    }
}

/// 隐藏弹窗（关闭按钮/动作完成后）：恢复主窗口可用状态。
///
/// 隐藏前先把内容清空并保持可见一帧：隐藏窗口不再绘制，此刻画下的空卡片
/// 会成为下次 show 的第一帧——否则下次打开会先闪出上一弹窗的残影。
pub fn close(app: &AppHandle) {
    // 代次 +1：令挂起的延迟隐藏失效（关闭后立刻重开不会被误藏）
    let gen = app.state::<AppState>().bump_dialog_gen();
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
            // 等 50ms（≥2 帧）确保清空后的空卡片已绘制，再隐藏
            std::thread::sleep(std::time::Duration::from_millis(50));
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
    show(app, "DeepSeek API 余额", "balance", serde_json::json!(null));
    let handle = app.clone();
    let config = app.state::<AppState>().config();
    // 清空旧结果：轮询期间显示“查询中…”而非上次的陈旧数据
    handle.state::<AppState>().set_last_balance(None);
    std::thread::spawn(move || {
        let payload = crate::balance::query_balance(&config);
        handle.state::<AppState>().set_last_balance(Some(payload));
    });
}

// ---------- 检查更新 ----------

/// 打开检查更新弹窗：立即出窗显示进度，检查在后台执行、结果写入状态，
/// 页面轮询拉取。
pub fn open_check(app: &AppHandle) {
    show(app, "检查更新", "check", serde_json::json!(null));
    let handle = app.clone();
    handle.state::<AppState>().set_last_check(None);
    handle.state::<AppState>().set_update_done(false, None);
    handle
        .state::<AppState>()
        .set_check_progress(Some("正在检查更新…".into()));
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
        let (ok, message) = match crate::updater::apply(&handle, &which) {
            Ok(()) => (true, "更新完成".to_string()),
            Err(e) => (false, format!("更新失败：{e}")),
        };
        handle
            .state::<AppState>()
            .set_update_done(ok, Some(message));
    });
}

// ---------- 关于 ----------

/// 打开关于弹窗。
pub fn open_about(app: &AppHandle) {
    let config = app.state::<AppState>().config();
    let dsh_version =
        crate::runtime::installed_dsh_version(&config).unwrap_or_else(|| "未知".into());
    let initial = serde_json::json!({
        "app_version": env!("CARGO_PKG_VERSION"),
        "dsh_version": dsh_version,
    });
    show(app, "关于", "about", initial);
}
