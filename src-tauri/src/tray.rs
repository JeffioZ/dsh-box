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
fn native_menu(app: &AppHandle) -> tauri::Result<tauri::menu::Menu<tauri::Wry>> {
    use tauri::menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem, Submenu};

    let model = crate::tray_menu::items(true);
    let item = |id: &str| {
        model
            .iter()
            .find(|entry| entry.id == id)
            .expect("shared tray menu item must exist")
    };

    let open_item = MenuItem::with_id(app, "open", &item("open").label, true, None::<&str>)?;
    let balance_item =
        MenuItem::with_id(app, "balance", &item("balance").label, true, None::<&str>)?;
    let browser_item = MenuItem::with_id(
        app,
        "open_browser",
        &item("open_browser").label,
        true,
        None::<&str>,
    )?;
    let restart_item =
        MenuItem::with_id(app, "restart", &item("restart").label, true, None::<&str>)?;
    let check_item = MenuItem::with_id(
        app,
        "check_update",
        &item("check_update").label,
        true,
        None::<&str>,
    )?;
    let auto_item = MenuItem::with_id(
        app,
        "autostart",
        &item("autostart").label,
        true,
        None::<&str>,
    )?;
    let quit_item = MenuItem::with_id(app, "quit", &item("quit").label, true, None::<&str>)?;
    let about_item = MenuItem::with_id(app, "about", &item("about").label, true, None::<&str>)?;
    let language_model = item("language");
    let zh_model = &language_model.children[0];
    let en_model = &language_model.children[1];
    let zh_item = CheckMenuItem::with_id(
        app,
        zh_model.id.as_str(),
        &zh_model.label,
        true,
        zh_model.checked.unwrap_or(false),
        None::<&str>,
    )?;
    let en_item = CheckMenuItem::with_id(
        app,
        en_model.id.as_str(),
        &en_model.label,
        true,
        en_model.checked.unwrap_or(false),
        None::<&str>,
    )?;
    let language_menu = Submenu::with_id_and_items(
        app,
        language_model.id.as_str(),
        &language_model.label,
        true,
        &[&zh_item, &en_item],
    )?;
    let sep1 = PredefinedMenuItem::separator(app)?;
    let sep2 = PredefinedMenuItem::separator(app)?;
    let sep3 = PredefinedMenuItem::separator(app)?;
    let sep4 = PredefinedMenuItem::separator(app)?;
    Menu::with_items(
        app,
        &[
            &open_item,
            &balance_item,
            &browser_item,
            &sep1,
            &restart_item,
            &check_item,
            &sep2,
            &auto_item,
            &sep3,
            &language_menu,
            &sep4,
            &about_item,
            &quit_item,
        ],
    )
}

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

/// 托盘菜单项动作分发（自绘菜单 tray_menu_choose 与 macOS/Linux 原生菜单共用）。
pub(crate) fn run_action(app: &AppHandle, id: &str) {
    match id {
        "open" => show_main(app),
        "balance" => crate::app_dialog::open_balance(app),
        "open_browser" => open_browser(app),
        "restart" => restart_from_tray(app),
        "check_update" => crate::app_dialog::open_check(app),
        "autostart" => toggle_autostart(app),
        "hide_tool_calls" => toggle_hide_tool_calls(app),
        "language_zh" => change_language(app, "zh-CN"),
        "language_en" => change_language(app, "en"),
        "about" => crate::app_dialog::open_about(app),
        "quit" => quit(app),
        _ => {}
    }
}

fn change_language(app: &AppHandle, language: &str) {
    if let Err(error) = app.state::<AppState>().set_ui_language(language) {
        crate::logging::log(&format!("language: 保存失败：{error}"));
        return;
    }
    // 同步 dsh 界面的语言：写入 $DSH_HOME/settings.yaml 的 locale.preference
    // （dsh 语言 id 为 zh/en）。dsh 的 settings-file 有文件监视器，外部编辑
    // 会被热发布，界面无需重载即切换
    let dsh_locale = if language == "zh-CN" { "zh" } else { "en" };
    let config = app.state::<AppState>().config();
    match config.save_dsh_locale(dsh_locale) {
        Ok(()) => crate::logging::log(&format!("language: 已同步 dsh 语言 {dsh_locale}")),
        Err(e) => crate::logging::log(&format!("language: 同步 dsh 语言失败：{e}")),
    }
    apply_language(app, language);
}

/// 把语言应用到外壳（页面、注入脚本、原生托盘菜单）。
fn apply_language(app: &AppHandle, language: &str) {
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
        crate::app_dialog::APP_DIALOG_WINDOW,
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
}

/// 把 dsh 的主题偏好（light|dark|system）应用到外壳各窗口：
/// 显式 light/dark 覆盖 WebView 的配色（CSS prefers-color-scheme 跟随），
/// system 恢复跟随系统。set_theme 后各窗口背景色同步对齐——主窗口用导航
/// 底色，弹窗/托盘菜单用卡片底色（窗口边距与淡出中性帧必须与卡片同色，
/// 否则露出旧主题的色环）。
fn apply_theme(app: &AppHandle, theme: &str) {
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
    }
    // 弹窗与托盘菜单：不透明窗口，窗口底色随主题同步（卡片同色，
    // 圆角由 Win11 系统裁剪，圆角外区域显示底色）
    for label in [
        crate::app_dialog::APP_DIALOG_WINDOW,
        crate::tray_menu::TRAY_MENU_WINDOW,
    ] {
        if let Some(window) = app.get_webview_window(label) {
            let _ = window.set_theme(resolved);
            let light = window.theme().ok() == Some(tauri::Theme::Light);
            let color = if light {
                crate::CARD_BG_LIGHT
            } else {
                crate::CARD_BG_DARK
            };
            let _ = window.set_background_color(Some(color));
        }
    }
}

/// 最近应用的语言/主题（供跟随线程比对）。
static LAST_LANGUAGE: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);
static LAST_THEME: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);

/// 单次比对 dsh 设置并应用变化（语言/主题）。需在主线程调用。
pub fn check_dsh_settings_now(app: &AppHandle) {
    let config = app.state::<AppState>().config();
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
            if app.state::<AppState>().is_quitting() {
                return;
            }
            let config = app.state::<AppState>().config();
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
    tauri::image::Image::from_bytes(bytes).ok()
}

fn open_browser(app: &AppHandle) {
    let config = app.state::<AppState>().config();
    if !crate::dsh::health_check(config.port) {
        use tauri_plugin_dialog::MessageDialogKind;
        crate::dialog::show_message(
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
            crate::locale::text(
                "更新流程正在进行，请稍后再重启。",
                "An update is in progress. Please restart the service later.",
            )
            .into(),
            crate::locale::text("重启服务", "Restart service"),
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
            crate::locale::text(
                "启动流程进行中，请稍后再试。",
                "Startup is in progress. Please try again later.",
            )
            .into(),
            crate::locale::text("重启服务", "Restart service"),
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
                format!(
                    "{}: {e}",
                    crate::locale::text("重启服务失败", "Failed to restart the service")
                ),
                crate::locale::text("重启服务", "Restart service"),
                MessageDialogKind::Warning,
            );
        }
    });
}

fn toggle_hide_tool_calls(app: &AppHandle) {
    match app.state::<AppState>().toggle_hide_tool_calls() {
        Ok(on) => {
            crate::logging::log(&format!(
                "hide-tools: 已{}隐藏工具调用",
                if on { "开启" } else { "关闭" }
            ));
            crate::apply_hide_tools(app);
        }
        Err(e) => crate::logging::log(&format!("hide-tools: 保存失败：{e}")),
    }
}

fn toggle_autostart(app: &AppHandle) {
    // 消息框必须挪到工作线程：blocking_show 在主线程会冻结整个事件循环
    // （插件文档明确禁止）。本函数可能从托盘菜单动作触发——macOS/Linux
    // 的原生托盘回调在主线程，Windows 的托盘点击事件同样在主线程
    let handle = app.clone();
    std::thread::spawn(move || {
        use tauri_plugin_dialog::MessageDialogKind;
        // 已有切换在处理中时忽略新点击：连续快速点击不会排队弹多个结果框
        static TOGGLE_PENDING: std::sync::atomic::AtomicBool =
            std::sync::atomic::AtomicBool::new(false);
        if TOGGLE_PENDING.swap(true, std::sync::atomic::Ordering::AcqRel) {
            return;
        }
        // 本函数此后无提前返回，直接复位即可；若将来增加分支请改用 RAII 守卫
        let result = {
            // 串行化读-改-写：连续快速触发时，避免两个线程读到相同的旧状态、
            // 执行相同方向的切换（结果状态未变但提示已切换）
            static TOGGLE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
            let _guard = TOGGLE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
            let cur = crate::autostart::is_enabled();
            crate::autostart::set_enabled(!cur).map(|()| cur)
        };
        TOGGLE_PENDING.store(false, std::sync::atomic::Ordering::Release);
        match result {
            Ok(cur) => {
                let msg = if cur {
                    crate::locale::text(
                        "已关闭开机自启动。",
                        "Launch at startup has been turned off.",
                    )
                } else {
                    crate::locale::text(
                        "已开启开机自启动：下次开机将静默启动并驻留托盘。",
                        "Launch at startup is on. The app will start silently in the tray next time.",
                    )
                };
                crate::dialog::show_message(
                    &handle,
                    msg.into(),
                    crate::locale::text("开机自启动", "Launch at startup"),
                    MessageDialogKind::Info,
                );
            }
            Err(e) => {
                crate::dialog::show_message(
                    &handle,
                    format!(
                        "{}: {e}",
                        crate::locale::text("设置失败", "Could not change the setting")
                    ),
                    crate::locale::text("开机自启动", "Launch at startup"),
                    MessageDialogKind::Warning,
                );
            }
        }
    });
}

fn quit(app: &AppHandle) {
    let state = app.state::<AppState>();
    state.set_quitting(true);
    dsh::shutdown(app);
    // 给进程树一点收尾时间
    std::thread::sleep(std::time::Duration::from_millis(300));
    app.exit(0);
}
