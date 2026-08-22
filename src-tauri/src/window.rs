//! 主窗口：窗口位置/大小记忆（每次事件立即落盘）与按 DPI 设置图标。

use std::sync::Mutex;
use std::time::Instant;

use tauri::{AppHandle, Manager};

use crate::app_state::AppState;
use crate::{logging, main_window};

/// 启动后一段时间内的落盘静默期：恢复/系统协商产生的 Resized/Moved 事件
/// 不写入 config，避免系统微调后的尺寸被持久化、逐次启动累积变大。
static SAVE_SETTLE_UNTIL: Mutex<Option<Instant>> = Mutex::new(None);

/// 设置落盘静默期（启动恢复完成后调用；期间 save_now 直接跳过）。
pub fn start_save_settle(millis: u64) {
    *SAVE_SETTLE_UNTIL.lock().unwrap_or_else(|e| e.into_inner()) =
        Some(Instant::now() + std::time::Duration::from_millis(millis));
}

/// 保存主窗口位置/大小到 state.json（Resized/Moved 事件）。
/// 不做节流：节流窗口内进程被强制结束会丢失最后一次调整。
/// state.json 仅几百字节，拖动/缩放期间的写入频率完全可接受。
pub fn save_window_state(app: &AppHandle) {
    if let Some(until) = *SAVE_SETTLE_UNTIL.lock().unwrap_or_else(|e| e.into_inner()) {
        if Instant::now() < until {
            return; // 启动静默期：不把系统微调后的几何持久化
        }
    }
    save_now(app);
}

/// 保存主窗口位置/大小到 state.json（强制版，供关闭/退出事件，确保最后位置落盘）。
pub fn save_window_state_now(app: &AppHandle) {
    save_now(app);
}

/// 上次落盘的窗口几何（物理整数坐标）；仅值变化时记录日志，
/// 避免拖动/缩放期间日志刷屏，同时留下可对比的保存轨迹。
static LAST_SAVED: Mutex<Option<(i32, i32, u32, u32)>> = Mutex::new(None);

/// 系统几何协商增量（逻辑像素）：恢复时程序设置值 A 与系统终态 B 的差。
/// Windows 对无边框窗口的 SetWindowPos 会做几何协商（本机实测宽 +15、
/// 高 +37 逻辑像素），保存时扣除该增量，避免协商量逐次累积导致
/// “每次启动窗口大一圈”。
static NEGOTIATION_DELTA: Mutex<(f64, f64)> = Mutex::new((0.0, 0.0));

/// 记录一次系统协商增量（启动恢复/自适应设置后由 lib.rs 终态线程调用）。
pub(crate) fn record_negotiation_delta(dw: f64, dh: f64) {
    let mut delta = NEGOTIATION_DELTA.lock().unwrap_or_else(|e| e.into_inner());
    // 仅接受合理范围的正增量；异常值（超过 200 逻辑像素）视为真实
    // 用户调整或显示器切换，不参与补偿
    *delta = (dw.clamp(0.0, 200.0), dh.clamp(0.0, 200.0));
}

fn save_now(app: &AppHandle) {
    let config = app.state::<AppState>().config();
    if let Some(win) = main_window(app) {
        // 最大化/最小化状态不保存：否则会把“全屏尺寸”当作普通尺寸记忆，
        // 下次恢复时窗口突然变大甚至超出屏幕
        let maximized = win.is_maximized().unwrap_or(false);
        let minimized = win.is_minimized().unwrap_or(false);
        if maximized || minimized {
            return;
        }
        if let (Ok(pos), Ok(size)) = (win.outer_position(), win.outer_size()) {
            // 兜底：窗口尺寸几乎占满工作区也视为最大化残留（最大化状态翻转前的
            // Resized 事件可能抢在 is_maximized 生效前落盘），不保存
            if let Ok(Some(monitor)) = win.current_monitor() {
                let scale = monitor.scale_factor();
                let wa = monitor.work_area();
                let (w, h) = (size.width as f64 / scale, size.height as f64 / scale);
                let (ww, wh) = (wa.size.width as f64 / scale, wa.size.height as f64 / scale);
                if w >= ww - 8.0 && h >= wh - 8.0 {
                    return;
                }
            }
            let key = (pos.x, pos.y, size.width, size.height);
            let changed = {
                let mut last = LAST_SAVED.lock().unwrap_or_else(|e| e.into_inner());
                let changed = *last != Some(key);
                *last = Some(key);
                changed
            };
            if changed {
                logging::log(&format!(
                    "窗口: 保存 物理=({},{},{}x{})",
                    pos.x, pos.y, size.width, size.height
                ));
            }
            // 存逻辑坐标：除以当前缩放，跨不同 DPI 显示器切换时观感尺寸一致；
            // 宽高扣除本次会话测得的系统协商增量（见 NEGOTIATION_DELTA 说明），
            // 否则每次启动协商量叠加、窗口一圈圈变大
            let scale = win.scale_factor().unwrap_or(1.0);
            let (dw, dh) = *NEGOTIATION_DELTA.lock().unwrap_or_else(|e| e.into_inner());
            config.save_window_rect(
                pos.x as f64 / scale,
                pos.y as f64 / scale,
                (size.width as f64 / scale - dw).max(400.0),
                (size.height as f64 / scale - dh).max(300.0),
            );
        }
    }
}

/// 禁用系统圆角裁剪（DWMWCP_DONOTROUND）：透明窗口自绘圆角时必须关闭，
/// 否则系统 8px 圆角会叠加在内容圆角四角上（视觉错位）。
#[cfg(windows)]
pub(crate) fn disable_system_rounded_corners(win: &tauri::WebviewWindow) {
    use windows_sys::Win32::Graphics::Dwm::{
        DwmSetWindowAttribute, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_DONOTROUND,
    };
    let Ok(hwnd) = win.hwnd() else { return };
    let preference = DWMWCP_DONOTROUND;
    unsafe {
        let _ = DwmSetWindowAttribute(
            hwnd.0,
            DWMWA_WINDOW_CORNER_PREFERENCE as u32,
            &preference as *const _ as *const core::ffi::c_void,
            std::mem::size_of::<i32>() as u32,
        );
    }
}
/// 按显示器 DPI 设置窗口图标：标题栏/任务栏使用 1:1 物理像素的源图，
/// 避免从小图放大或系统二次缩放导致的模糊。
pub fn set_window_icon(app: &AppHandle) {
    let Some(win) = main_window(app) else {
        return;
    };
    let scale = win.scale_factor().unwrap_or(1.0);
    let bytes: &'static [u8] = if scale >= 2.0 {
        include_bytes!("../icons/64x64.png")
    } else if scale >= 1.5 {
        include_bytes!("../icons/48x48.png")
    } else if scale >= 1.25 {
        include_bytes!("../icons/40x40.png")
    } else {
        include_bytes!("../icons/32x32.png")
    };
    match tauri::image::Image::from_bytes(bytes) {
        Ok(img) => match win.set_icon(img) {
            Ok(()) => logging::log(&format!("窗口: 图标已设置（scale={scale:.2}）")),
            Err(e) => logging::log(&format!("窗口: 设置图标失败：{e}")),
        },
        Err(e) => logging::log(&format!("窗口: 解析图标失败：{e}")),
    }
}
