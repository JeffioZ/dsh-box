//! 托盘图标。
//!
//! - Windows：左键打开主窗口，右键弹自绘菜单窗口（tray_menu 模块）——
//!   原生 TrackPopupMenu 弹出在本环境不稳定。
//! - macOS/Linux：使用系统原生托盘菜单（macOS 菜单栏点击即弹菜单、
//!   Linux AppIndicator 亦是左键菜单），稳定可靠，无需自绘替代。

use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager};

use crate::app_state::AppState;
#[cfg(windows)]
use crate::processes;
use crate::{dsh, show_main, APP_TITLE};

#[cfg(windows)]
pub fn create(app: &AppHandle) -> tauri::Result<()> {
    let Some(icon) = pick_tray_image(app) else {
        return Ok(());
    };
    TrayIconBuilder::with_id("main-tray")
        .icon(icon)
        .tooltip(APP_TITLE)
        .show_menu_on_left_click(false)
        .on_tray_icon_event(|tray, event| match event {
            TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } => {
                show_main(tray.app_handle());
            }
            TrayIconEvent::Click {
                button: MouseButton::Right,
                button_state: MouseButtonState::Up,
                position,
                ..
            } => {
                let app = tray.app_handle().clone();
                let app2 = app.clone();
                let at = (position.x, position.y);
                if let Err(e) = app.run_on_main_thread(move || {
                    crate::tray_menu::open_menu(&app2, at);
                }) {
                    crate::logging::log(&format!("tray-menu: 调度失败：{e}"));
                }
            }
            _ => {}
        })
        .build(app)?;
    Ok(())
}

#[cfg(not(windows))]
pub fn create(app: &AppHandle) -> tauri::Result<()> {
    use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};

    let open_item =
        MenuItem::with_id(app, "open", format!("打开 {APP_TITLE}"), true, None::<&str>)?;
    let balance_item = MenuItem::with_id(app, "balance", "查询 API 余额…", true, None::<&str>)?;
    let browser_item =
        MenuItem::with_id(app, "open_browser", "在浏览器中打开", true, None::<&str>)?;
    let restart_item = MenuItem::with_id(app, "restart", "重启服务", true, None::<&str>)?;
    let check_item = MenuItem::with_id(app, "check_update", "检查更新…", true, None::<&str>)?;
    let auto_item = MenuItem::with_id(
        app,
        "autostart",
        format!(
            "开机自启动：{}",
            if crate::autostart::is_enabled() {
                "已开启"
            } else {
                "已关闭"
            }
        ),
        true,
        None::<&str>,
    )?;
    let quit_item = MenuItem::with_id(app, "quit", "退出", true, None::<&str>)?;
    let sep = PredefinedMenuItem::separator(app)?;
    let about_item = MenuItem::with_id(app, "about", "关于", true, None::<&str>)?;
    let menu = Menu::with_items(
        app,
        &[
            &open_item,
            &balance_item,
            &browser_item,
            &sep,
            &restart_item,
            &check_item,
            &sep,
            &auto_item,
            &sep,
            &about_item,
            &quit_item,
        ],
    )?;

    let Some(icon) = pick_tray_image(app) else {
        return Ok(());
    };
    TrayIconBuilder::with_id("main-tray")
        .icon(icon)
        .tooltip(APP_TITLE)
        .menu(&menu)
        .show_menu_on_left_click(true)
        .on_menu_event(|app, event| run_action(app, event.id().as_ref()))
        .build(app)?;
    Ok(())
}

/// 托盘菜单项动作分发（自绘菜单 tray_menu_choose 与 macOS/Linux 原生菜单共用）。
pub(crate) fn run_action(app: &AppHandle, id: &str) {
    match id {
        "open" => show_main(app),
        "balance" => crate::app_dialog::open_balance(app),
        "open_browser" => open_browser(app),
        "restart" => restart_from_tray(app),
        "check_update" => crate::app_dialog::open_check(app),
        "autostart" => toggle_autostart(app),
        "about" => crate::app_dialog::open_about(app),
        "quit" => quit(app),
        _ => {}
    }
}

/// 按显示器 DPI 选择托盘图标：物理尺寸 1:1 映射
/// （100%→16px、125%→20px、150%→24px、200%→32px），避免系统缩放糊化。
/// 图标风格与应用图标一致（蓝底圆角方块+白鲸），深浅任务栏均清晰。
fn pick_tray_image(app: &AppHandle) -> Option<tauri::image::Image<'static>> {
    let scale = app
        .get_window(crate::MAIN_WINDOW)
        .and_then(|w| w.scale_factor().ok())
        .unwrap_or(1.0);
    let bytes: &'static [u8] = if scale >= 2.0 {
        include_bytes!("../icons/tray-32.png")
    } else if scale >= 1.5 {
        include_bytes!("../icons/tray-24.png")
    } else if scale >= 1.25 {
        include_bytes!("../icons/tray-20.png")
    } else {
        include_bytes!("../icons/tray-16.png")
    };
    let size = if scale >= 2.0 {
        "32"
    } else if scale >= 1.5 {
        "24"
    } else if scale >= 1.25 {
        "20"
    } else {
        "16"
    };
    crate::logging::log(&format!("托盘: 图标 {size}px（scale={scale:.2}）"));
    tauri::image::Image::from_bytes(bytes).ok()
}

fn open_browser(app: &AppHandle) {
    let config = app.state::<AppState>().config();
    if !crate::dsh::health_check(config.port) {
        use tauri_plugin_dialog::MessageDialogKind;
        crate::dialog::show_message(
            app,
            "dsh 服务当前未运行，无法在浏览器中打开。".into(),
            "在浏览器中打开",
            MessageDialogKind::Warning,
        );
        return;
    }
    let url = config.web_url();
    #[cfg(windows)]
    {
        let mut cmd = std::process::Command::new("cmd");
        cmd.args(["/c", "start", "", &url]);
        processes::hide_console(&mut cmd);
        let _ = cmd.spawn();
    }
    #[cfg(target_os = "macos")]
    {
        let mut cmd = std::process::Command::new("open");
        cmd.arg(&url);
        let _ = cmd.spawn();
    }
    #[cfg(target_os = "linux")]
    {
        let mut cmd = std::process::Command::new("xdg-open");
        cmd.arg(&url);
        let _ = cmd.spawn();
    }
}

/// 托盘“重启服务”：启动/安装进行中拒绝，并反馈结果。
fn restart_from_tray(app: &AppHandle) {
    use tauri_plugin_dialog::MessageDialogKind;
    let state = app.state::<AppState>();
    if state.is_updating() {
        crate::dialog::show_message(
            app,
            "更新流程正在进行，请稍后再重启。".into(),
            "重启服务",
            MessageDialogKind::Warning,
        );
        return;
    }
    let phase = state.phase();
    if matches!(
        phase,
        crate::app_state::BootPhase::InstallingNode
            | crate::app_state::BootPhase::InstallingDsh
            | crate::app_state::BootPhase::StartingServer
    ) {
        crate::dialog::show_message(
            app,
            "启动流程进行中，请稍后再试。".into(),
            "重启服务",
            MessageDialogKind::Warning,
        );
        return;
    }
    let handle = app.clone();
    std::thread::spawn(move || {
        // 重启本身在内部处理成功/失败状态（失败会进错误页）；失败额外弹窗告知原因。
        if let Err(e) = crate::updater::restart_service(&handle) {
            use tauri_plugin_dialog::MessageDialogKind;
            crate::dialog::show_message(
                &handle,
                format!("重启服务失败：{e}"),
                "重启服务",
                MessageDialogKind::Warning,
            );
        }
    });
}

fn toggle_autostart(app: &AppHandle) {
    use tauri_plugin_dialog::MessageDialogKind;
    let cur = crate::autostart::is_enabled();
    match crate::autostart::set_enabled(!cur) {
        Ok(()) => {
            let msg = if cur {
                "已关闭开机自启动。"
            } else {
                "已开启开机自启动：下次开机将自动静默运行到托盘。"
            };
            crate::dialog::show_message(app, msg.into(), "开机自启动", MessageDialogKind::Info);
        }
        Err(e) => {
            crate::dialog::show_message(
                app,
                format!("设置失败：{e}"),
                "开机自启动",
                MessageDialogKind::Warning,
            );
        }
    }
}

fn quit(app: &AppHandle) {
    let state = app.state::<AppState>();
    state.set_quitting(true);
    dsh::shutdown(app);
    // 给进程树一点收尾时间
    std::thread::sleep(std::time::Duration::from_millis(300));
    app.exit(0);
}
