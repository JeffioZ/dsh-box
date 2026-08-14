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

/// 保存主窗口位置/大小到 config.json（Resized/Moved 事件）。
/// 不做节流：节流窗口内进程被强制结束会丢失最后一次调整。
/// config.json 仅几百字节，拖动/缩放期间的写入频率完全可接受。
pub fn save_window_state(app: &AppHandle) {
    if let Some(until) = *SAVE_SETTLE_UNTIL.lock().unwrap_or_else(|e| e.into_inner()) {
        if Instant::now() < until {
            return; // 启动静默期：不把系统微调后的几何持久化
        }
    }
    save_now(app);
}

/// 保存主窗口位置/大小到 config.json（强制版，供关闭/退出事件，确保最后位置落盘）。
pub fn save_window_state_now(app: &AppHandle) {
    save_now(app);
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
            // 存逻辑坐标：除以当前缩放，跨不同 DPI 显示器切换时观感尺寸一致
            let scale = win.scale_factor().unwrap_or(1.0);
            config.save_window_rect(
                pos.x as f64 / scale,
                pos.y as f64 / scale,
                size.width as f64 / scale,
                size.height as f64 / scale,
            );
        }
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
