//! 托盘图标。
//!
//! - Windows：左键打开主窗口，右键弹自绘菜单窗口（tray_menu 模块）——
//!   原生 TrackPopupMenu 弹出在本环境不稳定。
//! - macOS/Linux：使用系统原生托盘菜单（macOS 菜单栏点击即弹菜单、
//!   Linux AppIndicator 亦是左键菜单），稳定可靠，无需自绘替代。

use tauri::tray::TrayIconBuilder;
#[cfg(windows)]
use tauri::tray::{MouseButton, MouseButtonState, TrayIconEvent};
use tauri::{AppHandle, Manager};

use crate::app_state::AppState;
#[cfg(windows)]
use crate::processes;
use crate::{show_main, APP_TITLE};

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
fn native_menu(app: &AppHandle) -> tauri::Result<tauri::menu::Menu<tauri::Wry>> {
    use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};

    let model = crate::tray_menu::contextual_items(app, true);
    let item = |id: &str| -> tauri::Result<&crate::tray_menu::TrayMenuItem> {
        model.iter().find(|entry| entry.id == id).ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("shared tray menu item is missing: {id}"),
            )
            .into()
        })
    };

    let open_item = MenuItem::with_id(
        app,
        "open",
        &item("open")?.label,
        item("open")?.enabled,
        None::<&str>,
    )?;
    let balance_item = MenuItem::with_id(
        app,
        "balance",
        &item("balance")?.label,
        item("balance")?.enabled,
        None::<&str>,
    )?;
    let usage_item = MenuItem::with_id(
        app,
        "usage",
        &item("usage")?.label,
        item("usage")?.enabled,
        None::<&str>,
    )?;
    let browser_item = MenuItem::with_id(
        app,
        "open_browser",
        &item("open_browser")?.label,
        item("open_browser")?.enabled,
        None::<&str>,
    )?;
    let restart_item = MenuItem::with_id(
        app,
        "restart",
        &item("restart")?.label,
        item("restart")?.enabled,
        None::<&str>,
    )?;
    let check_item = MenuItem::with_id(
        app,
        "check_update",
        &item("check_update")?.label,
        true,
        None::<&str>,
    )?;
    let plugins_item = MenuItem::with_id(
        app,
        "plugins",
        &item("plugins")?.label,
        item("plugins")?.enabled,
        None::<&str>,
    )?;
    let settings_item = MenuItem::with_id(
        app,
        "settings",
        &item("settings")?.label,
        true,
        None::<&str>,
    )?;
    let quit_item = MenuItem::with_id(app, "quit", &item("quit")?.label, true, None::<&str>)?;
    let about_item = MenuItem::with_id(app, "about", &item("about")?.label, true, None::<&str>)?;
    let sep1 = PredefinedMenuItem::separator(app)?;
    let sep2 = PredefinedMenuItem::separator(app)?;
    let sep3 = PredefinedMenuItem::separator(app)?;
    // 顺序与 tray_menu::items(true) 保持一致：打开/访问 → 服务维护 →
    // 管理与查询 → 关于/退出
    Menu::with_items(
        app,
        &[
            &open_item,
            &browser_item,
            &sep1,
            &restart_item,
            &check_item,
            &sep2,
            &usage_item,
            &balance_item,
            &plugins_item,
            &settings_item,
            &sep3,
            &about_item,
            &quit_item,
        ],
    )
}

#[cfg(not(windows))]
static NATIVE_MENU_SIGNATURE: std::sync::atomic::AtomicU8 =
    std::sync::atomic::AtomicU8::new(u8::MAX);

/// dsh 状态事件可能包含高频下载进度；只有菜单能力位变化时才重建原生菜单。
#[cfg(not(windows))]
pub(crate) fn sync_menu_state(app: &AppHandle) {
    use std::sync::atomic::Ordering;
    let signature = crate::tray_menu::capability_signature(app);
    if NATIVE_MENU_SIGNATURE.swap(signature, Ordering::AcqRel) == signature {
        return;
    }
    if let (Some(tray), Ok(menu)) = (app.tray_by_id("main-tray"), native_menu(app)) {
        let _ = tray.set_menu(Some(menu));
    }
}

#[cfg(windows)]
pub(crate) fn sync_menu_state(_app: &AppHandle) {}

#[cfg(not(windows))]
pub fn create(app: &AppHandle) -> tauri::Result<()> {
    let menu = native_menu(app)?;

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

/// 托盘菜单项动作分发（自绘菜单 menu_choose 与 macOS/Linux 原生菜单共用）。
pub(crate) fn run_action(app: &AppHandle, id: &str) {
    if !crate::tray_menu::action_enabled(app, id) {
        crate::logging::log(&format!("menu: 已忽略当前不可用的动作 {id}"));
        return;
    }
    match id {
        "open" => show_main(app),
        "usage" => crate::control_center::open_usage(app),
        "balance" => crate::control_center::open_balance(app),
        "open_browser" => open_browser(app),
        "restart" => restart_from_tray(app),
        "check_update" => crate::control_center::open_check(app),
        "plugins" => crate::control_center::open_plugins(app),
        "settings" => crate::control_center::open_settings(app),
        "about" => crate::control_center::open_about(app),
        "quit" => quit(app),
        _ => {}
    }
}

/// 把语言应用到外壳（页面、注入脚本、原生托盘菜单）。
pub(crate) fn apply_language(app: &AppHandle, language: &str) {
    crate::locale::set_preference(Some(language));
    let encoded = serde_json::to_string(language).unwrap_or_else(|_| "\"en\"".into());
    let script = format!(
        "window.__DSHD_LANG={encoded};\
         window.dshdSetLanguage&&window.dshdSetLanguage({encoded});\
         window.__dshdSetInjectedLanguage&&window.__dshdSetInjectedLanguage({encoded});"
    );
    if let Some(main) = crate::main_window(app) {
        for webview in main.webviews() {
            let _ = webview.eval(&script);
        }
    }
    for label in [
        crate::control_center::APP_DIALOG_WINDOW,
        crate::tray_menu::TRAY_MENU_WINDOW,
    ] {
        if let Some(window) = app.get_webview_window(label) {
            let _ = window.eval(&script);
        }
    }
    #[cfg(not(windows))]
    if let (Some(tray), Ok(menu)) = (app.tray_by_id("main-tray"), native_menu(app)) {
        let _ = tray.set_menu(Some(menu));
    }
    // 状态栏统计文本由 Rust 按当前语言生成（含量词/单位），语言切换后立即
    // 重推一次，不等下一个 5s 轮询周期（statusbar 前端无法重译 Rust 快照）
    crate::usage::refresh_once(app.clone());
}

/// 把 dsh 的主题偏好（light|dark|system）应用到外壳各窗口：
/// 显式 light/dark 覆盖 WebView 的配色（CSS prefers-color-scheme 跟随），
/// system 恢复跟随系统。主窗口同步实体导航底色；弹窗/托盘菜单是透明宿主，
/// 只切换 theme 让内容层令牌更新，绝不在运行期重设窗口背景色（会触发
/// WebView2 透明合成层重建并产生闪烁）。
pub(crate) fn apply_theme(app: &AppHandle, theme: &str) {
    let resolved = match theme {
        "light" => Some(tauri::Theme::Light),
        "dark" => Some(tauri::Theme::Dark),
        _ => None,
    };
    // 主窗口背景色随主题切换（导航衔接用，与创建时逻辑一致）
    if let Some(main) = crate::main_window(app) {
        let _ = main.set_theme(resolved);
        let light = main.theme().ok() == Some(tauri::Theme::Light);
        let color = if light {
            crate::LIGHT_BG
        } else {
            crate::DARK_BG
        };
        let _ = main.set_background_color(Some(color));
        crate::titlebar::set_statusbar_theme_background(app, light);
    }
    // 弹窗与托盘菜单：透明宿主只更新 prefers-color-scheme。
    for label in [
        crate::control_center::APP_DIALOG_WINDOW,
        crate::tray_menu::TRAY_MENU_WINDOW,
    ] {
        if let Some(window) = app.get_webview_window(label) {
            let _ = window.set_theme(resolved);
        }
    }
}

/// 最近应用的语言/主题（供跟随线程比对）。
static LAST_LANGUAGE: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);
static LAST_THEME: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);

/// 单次比对 dsh 设置并应用变化（语言/主题）。需在主线程调用。
pub fn check_dsh_settings_now(app: &AppHandle) {
    let state = app.state::<AppState>();
    if state.service_ownership().is_external() {
        return;
    }
    let config = state.config();
    // 语言跟随（DSHD_LANG 显式覆盖时跳过）
    if std::env::var("DSHD_LANG").is_err() {
        if let Some(language) = config.load_dsh_locale() {
            let changed = {
                let last = LAST_LANGUAGE.lock().unwrap_or_else(|e| e.into_inner());
                last.as_deref() != Some(language)
            };
            if changed {
                crate::logging::log(&format!("language: 跟随 dsh 切换为 {language}"));
                apply_language(app, language);
                *LAST_LANGUAGE.lock().unwrap_or_else(|e| e.into_inner()) =
                    Some(language.to_string());
            }
        }
    }
    // 主题跟随
    if let Some(theme) = config.load_dsh_theme() {
        let changed = {
            let last = LAST_THEME.lock().unwrap_or_else(|e| e.into_inner());
            last.as_deref() != Some(theme)
        };
        if changed {
            crate::logging::log(&format!("theme: 跟随 dsh 切换为 {theme}"));
            apply_theme(app, theme);
            *LAST_THEME.lock().unwrap_or_else(|e| e.into_inner()) = Some(theme.to_string());
        }
    }
}

/// 后台跟随 dsh 的设置：每 3s 检查一次 settings.yaml 的 locale.preference 与
/// ui-theme.preference（按文件 mtime 门控，未变化时跳过解析），用户在 dsh
/// 界面里切换语言/主题后外壳自动跟随；托盘菜单每次打开时也会即时检查一次。
pub fn start_follow_dsh_settings(app: AppHandle) {
    *LAST_LANGUAGE.lock().unwrap_or_else(|e| e.into_inner()) = Some(
        if crate::locale::is_chinese() {
            "zh-CN"
        } else {
            "en"
        }
        .to_string(),
    );
    // 初始值取启动时实际应用的主题（主窗口/弹窗/托盘菜单在创建时已按
    // dsh 偏好解析），避免每轮启动都先误报一次“跟随 dsh 切换”
    *LAST_THEME.lock().unwrap_or_else(|e| e.into_inner()) = Some(
        app.state::<AppState>()
            .config()
            .load_dsh_theme()
            .unwrap_or("system")
            .to_string(),
    );
    std::thread::spawn(move || {
        let mut last_mtime = None;
        loop {
            std::thread::sleep(std::time::Duration::from_secs(3));
            let state = app.state::<AppState>();
            if state.is_quitting() {
                return;
            }
            if state.service_ownership().is_external() {
                last_mtime = None;
                continue;
            }
            let config = state.config();
            // mtime 门控：文件未变时跳过读取与解析
            let path = config.dsh_home().join("settings.yaml");
            let Ok(meta) = std::fs::metadata(&path) else {
                continue;
            };
            let Some(mtime) = meta.modified().ok() else {
                continue;
            };
            if last_mtime == Some(mtime) {
                continue;
            }
            last_mtime = Some(mtime);
            let h = app.clone();
            let _ = app.run_on_main_thread(move || check_dsh_settings_now(&h));
        }
    });
}

/// 按显示器 DPI 选择托盘图标：物理尺寸 1:1 映射
/// （100%→16px、125%→20px、150%→24px、200%→32px），避免系统缩放导致模糊。
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
    match tauri::image::Image::from_bytes(bytes) {
        Ok(image) => Some(image),
        Err(e) => {
            crate::logging::log(&format!("托盘: 图标 {size}px 解码失败：{e}"));
            None
        }
    }
}

fn open_browser(app: &AppHandle) {
    let config = app.state::<AppState>().config();
    if !crate::dsh::health_check(config.port) {
        use tauri_plugin_dialog::MessageDialogKind;
        crate::native_dialog::show_message(
            app,
            crate::locale::text(
                "dsh 服务当前未运行，无法在浏览器中打开。",
                "The dsh service is not running, so it cannot be opened in a browser.",
            )
            .into(),
            crate::locale::text("在浏览器中打开", "Open in browser"),
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
        // spawn 后不 wait，子进程退出会留 zombie，起线程回收
        if let Ok(mut child) = cmd.spawn() {
            std::thread::spawn(move || {
                let _ = child.wait();
            });
        }
    }
    #[cfg(target_os = "linux")]
    {
        let mut cmd = std::process::Command::new("xdg-open");
        cmd.arg(&url);
        if let Ok(mut child) = cmd.spawn() {
            std::thread::spawn(move || {
                let _ = child.wait();
            });
        }
    }
}

/// 托盘“重启服务”：启动/安装进行中拒绝，并反馈结果。
fn restart_from_tray(app: &AppHandle) {
    use tauri_plugin_dialog::MessageDialogKind;
    let state = app.state::<AppState>();
    if state.service_ownership().is_external() {
        crate::native_dialog::show_message(
            app,
            crate::locale::text(
                "当前连接的是外部 dsh 服务，请在原服务环境中重启。",
                "The current dsh service is external. Restart it in that service's environment.",
            )
            .into(),
            crate::locale::text("重启 dsh 服务", "Restart dsh service"),
            MessageDialogKind::Info,
        );
        return;
    }
    if state.is_updating() {
        crate::native_dialog::show_message(
            app,
            crate::locale::text(
                "更新流程正在进行，请稍后再重启。",
                "An update is in progress. Please restart the service later.",
            )
            .into(),
            crate::locale::text("重启 dsh 服务", "Restart dsh service"),
            MessageDialogKind::Warning,
        );
        return;
    }
    let phase = state.phase();
    if matches!(
        phase,
        crate::app_state::BootPhase::SwitchingService
            | crate::app_state::BootPhase::ServiceChoice
            | crate::app_state::BootPhase::InstallingNode
            | crate::app_state::BootPhase::InstallingDsh
            | crate::app_state::BootPhase::StartingServer
    ) {
        crate::native_dialog::show_message(
            app,
            crate::locale::text(
                "启动流程进行中，请稍后再试。",
                "Startup is in progress. Please try again later.",
            )
            .into(),
            crate::locale::text("重启 dsh 服务", "Restart dsh service"),
            MessageDialogKind::Warning,
        );
        return;
    }
    let handle = app.clone();
    std::thread::spawn(move || {
        // 快速双击时两次点击都可能通过上面的检查：进入重启前复查，
        // 已有启动/更新流程在进行则放弃本次（前一次点击会继续执行）。
        // Starting 一并视为忙碌：restart_service_locked 一开始就置该相位，
        // 不含它复查挡不住紧跟着的第二次重启。
        let state = handle.state::<AppState>();
        if state.is_updating()
            || matches!(
                state.phase(),
                crate::app_state::BootPhase::Starting
                    | crate::app_state::BootPhase::SwitchingService
                    | crate::app_state::BootPhase::ServiceChoice
                    | crate::app_state::BootPhase::InstallingNode
                    | crate::app_state::BootPhase::InstallingDsh
                    | crate::app_state::BootPhase::StartingServer
            )
        {
            crate::logging::log("托盘: 已有启动/更新流程在进行，忽略本次重启请求");
            return;
        }
        // 重启本身在内部处理成功/失败状态（失败会进错误页）；失败额外弹窗告知原因。
        if let Err(e) = crate::updater::restart_service(&handle) {
            use tauri_plugin_dialog::MessageDialogKind;
            crate::native_dialog::show_message(
                &handle,
                format!(
                    "{}: {e}",
                    crate::locale::text("重启 dsh 服务失败", "Failed to restart the dsh service")
                ),
                crate::locale::text("重启 dsh 服务", "Restart dsh service"),
                MessageDialogKind::Warning,
            );
        }
    });
}

fn quit(app: &AppHandle) {
    let state = app.state::<AppState>();
    state.set_quitting(true);
    // 停服与收尾等待挪到后台线程：macOS/Linux 下本函数在托盘事件（主线程）
    // 里执行，shutdown 的进程 wait 与 sleep 会阻塞事件循环（与关窗/quit
    // 命令共用 bootstrap::quit_sequence）
    let handle = app.clone();
    std::thread::spawn(move || crate::bootstrap::quit_sequence(&handle));
}
