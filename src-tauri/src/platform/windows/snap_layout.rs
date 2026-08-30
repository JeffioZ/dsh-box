//! Win11 贴边浮层（Snap Layouts）支持。
//!
//! 机制：在自绘最大化按钮的位置创建一个不可见的原生子窗口，其
//! WM_NCHITTEST 返回 HTMAXBUTTON——系统据此在悬停时弹出贴边布局浮层
//! （官方唯一路径）；WebView 内容本身收不到 WM_NCHITTEST（Tauri#4531），
//! 这是所有自绘标题栏 WebView 应用的通用解法。点击由覆盖层直接走
//! SC_MAXIMIZE/SC_RESTORE 原生路径；悬停视觉经事件镜像回标题栏
//! （覆盖层挡住了 webview 的真实 hover）。
//!
//! 未采用 tauri-plugin-snap-layout：其命令参数为 WebviewWindow，只支持
//! webview 与窗口同级的常规结构；本应用标题栏是主窗口内的子 webview
//! （multiwebview），命令解析直接失败（"current webview is not a
//! WebviewWindow"），故按同一机制自实现——参数用 Webview，坐标由
//! 前端按钮矩形（webview 视口 CSS 像素）叠加 webview 在窗口内的偏移。
//!
//! 版本门控：仅 Win11（build ≥ 22000）创建覆盖层；更老的系统完全不启用
//! （回到纯 HTML 按钮路径，与功能加入前行为一致），不依赖“HTMAXBUTTON
//! 在旧系统无副作用”的假设。

use std::sync::{Arc, LazyLock};

use tauri::{Manager as _, Webview};
use windows_sys::Win32::System::SystemInformation::OSVERSIONINFOW;
use windows_sys::Win32::UI::HiDpi::GetDpiForWindow;

// RtlGetVersion 在 windows-sys 里被归入 Wdk feature，为一个函数引整个 WDK
// 不值——直接声明 ntdll 绑定。不用 GetVersionExW：它受兼容性清单欺骗，
// 无清单进程只会拿到 6.2.9200
#[link(name = "ntdll")]
unsafe extern "system" {
    fn RtlGetVersion(info: *mut OSVERSIONINFOW) -> i32;
}

use windows_sys::Win32::Foundation::{HANDLE, HWND, LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    TrackMouseEvent, TME_LEAVE, TME_NONCLIENT, TRACKMOUSEEVENT,
};
use windows_sys::Win32::UI::Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DestroyWindow, GetPropW, IsZoomed, LoadCursorW, RemovePropW, SendMessageW,
    SetCursor, SetPropW, SetWindowPos, HTMAXBUTTON, HWND_TOP, IDC_HAND, SC_MAXIMIZE, SC_RESTORE,
    SWP_NOACTIVATE, WM_NCDESTROY, WM_NCHITTEST, WM_NCLBUTTONDOWN, WM_NCLBUTTONUP, WM_NCMOUSELEAVE,
    WM_SETCURSOR, WM_SYSCOMMAND, WS_CHILD, WS_VISIBLE,
};

/// 覆盖层 HWND 挂在主窗口上的属性名（避免自注册窗口类，复用 STATIC）。
static CHILD_PROP: LazyLock<Vec<u16>> = LazyLock::new(|| prop_wide("TauriDshdSnapChild"));
/// 覆盖层窗口过程的状态指针属性名。
static STATE_PROP: LazyLock<Vec<u16>> = LazyLock::new(|| prop_wide("TauriDshdSnapState"));
/// 子类化标识（任意稳定值即可）。
const SUBCLASS_ID: usize = 0x6473_6864_536e_6170;

/// 悬停镜像事件（payload: { on: bool }）：标题栏页面据此切换 .is-hovered。
const HOVER_EVENT: &str = "dshd-snap-hover";
/// 按压镜像事件（payload: { on: bool }）：标题栏页面据此切换 .is-pressed。
/// 覆盖层吃掉了 webview 的真实 hover/active/手型光标，全部经事件与
/// WM_SETCURSOR 还原，保持与相邻窗控按钮一致的多态反馈。
const PRESS_EVENT: &str = "dshd-snap-press";

/// 是否 Win11+（贴边浮层存在的最低版本，build 22000）。
static WIN11: LazyLock<bool> = LazyLock::new(|| unsafe {
    let mut info = OSVERSIONINFOW {
        dwOSVersionInfoSize: std::mem::size_of::<OSVERSIONINFOW>() as u32,
        ..Default::default()
    };
    if RtlGetVersion(&mut info) != 0 {
        return false;
    }
    info.dwBuildNumber >= 22000
});

/// Win32 窗口属性名（GetPropW/SetPropW 需要 0 结尾宽字符）。
fn prop_wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

struct SnapState {
    hovering: bool,
    pressed: bool,
    parent_hwnd: HWND,
    emit: Arc<dyn Fn(&'static str, bool) + Send + Sync>,
}

/// 更新/创建覆盖层。x/y/width/height 为按钮在前端视口内的 CSS 逻辑像素。
pub fn update(webview: &Webview, window: &tauri::Window, x: i32, y: i32, width: i32, height: i32) {
    if !*WIN11 {
        return;
    }
    let Ok(hwnd) = window.hwnd() else {
        return;
    };
    // HWND 裸指针不是 Send：先转 isize 搬进主线程闭包再还原
    let parent_raw = hwnd.0 as isize;
    let Ok(webview_pos) = webview.position() else {
        return;
    };
    let webview = webview.clone();
    let _ = window.run_on_main_thread(move || unsafe {
        let parent_hwnd = parent_raw as HWND;
        if windows_sys::Win32::UI::WindowsAndMessaging::IsWindow(parent_hwnd) == 0 {
            return;
        }
        let dpi = GetDpiForWindow(parent_hwnd);
        let x = webview_pos.x + scaled(x, dpi);
        let y = webview_pos.y + scaled(y, dpi);
        let width = scaled(width, dpi);
        let height = scaled(height, dpi);

        let child = GetPropW(parent_hwnd, CHILD_PROP.as_ptr()) as HWND;
        if !child.is_null() {
            SetWindowPos(child, HWND_TOP, x, y, width, height, SWP_NOACTIVATE);
            return;
        }
        // STATIC 空壳窗口：消息全部由子类化过程接管，无需注册自定义类
        let class_name: [u16; 7] = [
            b'S' as u16,
            b'T' as u16,
            b'A' as u16,
            b'T' as u16,
            b'I' as u16,
            b'C' as u16,
            0,
        ];
        let child = CreateWindowExW(
            0,
            class_name.as_ptr(),
            std::ptr::null(),
            WS_CHILD | WS_VISIBLE,
            x,
            y,
            width,
            height,
            parent_hwnd,
            0 as _,
            0 as _,
            std::ptr::null(),
        );
        if child.is_null() {
            return;
        }
        SetPropW(parent_hwnd, CHILD_PROP.as_ptr(), child as HANDLE);

        // 悬停/按压镜像走签名事件广播（emit_signed + dshdListen 的 nonce
        // 校验）：裸 emit 会被标题栏的事件过滤器整包丢弃，且广播+签名
        // 顺带继承防伪造语义
        let app = webview.app_handle().clone();
        let state = Box::new(SnapState {
            hovering: false,
            pressed: false,
            parent_hwnd,
            emit: Arc::new(move |event, on| {
                // 载荷必须是对象：sign_payload 只向对象注入 nonce，
                // 标题栏 dshdListen 的过滤器会丢弃无签名的标量载荷
                crate::emit_signed(&app, event, &serde_json::json!({ "on": on }));
            }),
        });
        let raw = Box::into_raw(state);
        SetPropW(child, STATE_PROP.as_ptr(), raw as HANDLE);
        SetWindowSubclass(child, Some(subclass_proc), SUBCLASS_ID, raw as usize);
    });
}

/// 移除覆盖层（标题栏页面卸载时由前端调用）。
pub fn detach(window: &tauri::Window) {
    let Ok(hwnd) = window.hwnd() else {
        return;
    };
    let parent_raw = hwnd.0 as isize;
    let _ = window.run_on_main_thread(move || unsafe {
        let parent_hwnd = parent_raw as HWND;
        let child = RemovePropW(parent_hwnd, CHILD_PROP.as_ptr()) as HWND;
        if !child.is_null() {
            DestroyWindow(child);
        }
    });
}

/// CSS 逻辑像素 → 物理像素（四舍五入；与窗口 DPI 一致）。
fn scaled(value: i32, dpi: u32) -> i32 {
    let sign = if value < 0 { -1i64 } else { 1i64 };
    (sign * (value.unsigned_abs() as i64 * dpi as i64 + 48) / 96) as i32
}

unsafe extern "system" fn subclass_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    subclass_id: usize,
    ref_data: usize,
) -> LRESULT {
    let state_ptr = ref_data as *mut SnapState;
    match msg {
        WM_NCDESTROY => {
            RemoveWindowSubclass(hwnd, Some(subclass_proc), subclass_id);
            let owned = RemovePropW(hwnd, STATE_PROP.as_ptr()) as *mut SnapState;
            if !owned.is_null() {
                let _ = Box::from_raw(owned);
            }
            DefSubclassProc(hwnd, msg, wparam, lparam)
        }
        WM_NCHITTEST => {
            // 覆盖层整个区域都是“最大化按钮”：系统在此悬停弹出贴边浮层
            if !state_ptr.is_null() {
                let state = &mut *state_ptr;
                if !state.hovering {
                    state.hovering = true;
                    (state.emit)(HOVER_EVENT, true);
                    let mut tme = TRACKMOUSEEVENT {
                        cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
                        dwFlags: TME_LEAVE | TME_NONCLIENT,
                        hwndTrack: hwnd,
                        dwHoverTime: 0,
                    };
                    TrackMouseEvent(&mut tme);
                }
                return HTMAXBUTTON as LRESULT;
            }
            DefSubclassProc(hwnd, msg, wparam, lparam)
        }
        WM_NCMOUSELEAVE => {
            if !state_ptr.is_null() {
                let state = &mut *state_ptr;
                if state.hovering {
                    state.hovering = false;
                    (state.emit)(HOVER_EVENT, false);
                }
                if state.pressed {
                    state.pressed = false;
                    (state.emit)(PRESS_EVENT, false);
                }
            }
            DefSubclassProc(hwnd, msg, wparam, lparam)
        }
        WM_NCLBUTTONDOWN => {
            // 按下不交给默认处理（避免触发系统拖动等 NC 行为），只等抬起
            if wparam == HTMAXBUTTON as usize && !state_ptr.is_null() {
                let state = &mut *state_ptr;
                if !state.pressed {
                    state.pressed = true;
                    (state.emit)(PRESS_EVENT, true);
                }
                return 0;
            }
            DefSubclassProc(hwnd, msg, wparam, lparam)
        }
        WM_NCLBUTTONUP => {
            if wparam == HTMAXBUTTON as usize && !state_ptr.is_null() {
                let state = &mut *state_ptr;
                if state.pressed {
                    state.pressed = false;
                    (state.emit)(PRESS_EVENT, false);
                }
                let parent = state.parent_hwnd;
                if IsZoomed(parent) != 0 {
                    SendMessageW(parent, WM_SYSCOMMAND, SC_RESTORE as WPARAM, 0);
                } else {
                    SendMessageW(parent, WM_SYSCOMMAND, SC_MAXIMIZE as WPARAM, 0);
                }
                return 0;
            }
            DefSubclassProc(hwnd, msg, wparam, lparam)
        }
        WM_SETCURSOR => {
            // 还原 .tb-btn 的手型光标（覆盖层默认箭头）
            if !state_ptr.is_null() {
                SetCursor(LoadCursorW(0 as _, IDC_HAND));
                return 1;
            }
            DefSubclassProc(hwnd, msg, wparam, lparam)
        }
        _ => DefSubclassProc(hwnd, msg, wparam, lparam),
    }
}
