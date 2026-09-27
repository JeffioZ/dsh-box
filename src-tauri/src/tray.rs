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
    // 顺序与 tray_menu::items(true) 保持一致：打开/访问 → 管理与查询 →
    // 服务维护 → 关于/退出
    Menu::with_items(
        app,
        &[
            &open_item,
            &browser_item,
            &sep1,
            &usage_item,
            &plugins_item,
            &settings_item,
            &sep2,
            &check_item,
            &restart_item,
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

/// 后台跟随 dsh 的设置：每 3s 检查一次 dsh 设置里 locale 与 ui-theme 的
/// preference（新版 dsh ≥0.1.7 为两级 cordis.patch.yml，旧版为
/// settings.yaml；按文件 mtime 门控，未变化时跳过解析），用户在 dsh 界面
/// 里切换语言/主题后外壳自动跟随；托盘菜单每次打开时也会即时检查一次。
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
        let mut last_fingerprint: Vec<Option<std::time::SystemTime>> = Vec::new();
        loop {
            // 1s 轮询：无变化时每轮只是两次 metadata stat（零解析开销不变），
            // 把「dsh 界面切主题 → 外壳标题栏/窗口跟随」的最差延迟从 3s
            // 压到 1s——dsh 自身 HMR 即时生效，3s 轮询让壳层显得明显迟滞
            std::thread::sleep(std::time::Duration::from_secs(1));
            let state = app.state::<AppState>();
            if state.is_quitting() {
                return;
            }
            if state.service_ownership().is_external() {
                last_fingerprint.clear();
                continue;
            }
            let config = state.config();
            // mtime 门控：设置文件集合（按 dsh 版本选择）任一变化才读取解析。
            // 每轮重算路径，dsh 运行中升级（存储迁移）后无需重启外壳。
            // 指纹按「每路径各自 mtime」比对而非取 max：低 mtime 一侧的
            // 变化（时钟回拨、备份还原）不抬高 max，会漏跟一拍。
            let fingerprint = crate::dsh_settings::watch_paths(&config)
                .iter()
                .map(|path| {
                    std::fs::metadata(path)
                        .ok()
                        .and_then(|meta| meta.modified().ok())
                })
                .collect::<Vec<_>>();
            if last_fingerprint == fingerprint {
                continue;
            }
            last_fingerprint = fingerprint;
            let h = app.clone();
            let _ = app.run_on_main_thread(move || check_dsh_settings_now(&h));
        }
    });
}

/// 系统任务栏/菜单栏当前是否浅色：决定托盘单色图标用黑版（浅底）还是
/// 白版（深底）。品牌图标为透明底纯黑/纯白两版，颜色必须跟随**系统**
/// 主题而非 dsh 应用主题（任务栏底色与 dsh 明暗设定无关）。
/// - Windows：HKCU Themes\Personalize 的 SystemUsesLightTheme（任务栏跟
///   系统主题，AppsUseLightTheme 只管应用窗口）；
/// - macOS：`defaults read -g AppleInterfaceStyle`（键不存在 = 浅色）；
/// - Linux：gsettings color-scheme 含 dark 则深色。
///
/// 任何失败按浅色兜底（黑图标）。运行中的系统主题 / DPI 变化由
/// start_follow_icon_context 轮询热切，无需重启。
#[cfg(windows)]
fn system_taskbar_light() -> bool {
    use windows_sys::core::w;
    use windows_sys::Win32::Foundation::ERROR_SUCCESS;
    use windows_sys::Win32::System::Registry::{RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_DWORD};
    let mut data: u32 = 0;
    let mut size = std::mem::size_of::<u32>() as u32;
    let rc = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            w!("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize"),
            w!("SystemUsesLightTheme"),
            RRF_RT_REG_DWORD,
            std::ptr::null_mut(),
            &mut data as *mut u32 as *mut _,
            &mut size,
        )
    };
    // 读失败（< Win10 1809 无该键、注册表异常）按浅色兜底，与 macOS/Linux
    // 分支一致；旧版 Windows 本就只有浅色任务栏，黑标是正确选择
    rc != ERROR_SUCCESS || data == 1
}

#[cfg(target_os = "macos")]
fn system_taskbar_light() -> bool {
    // 浅色模式无 AppleInterfaceStyle 键（命令失败），深色输出 "Dark"
    std::process::Command::new("defaults")
        .args(["read", "-g", "AppleInterfaceStyle"])
        .output()
        .map(|o| {
            !String::from_utf8_lossy(&o.stdout)
                .trim()
                .eq_ignore_ascii_case("dark")
        })
        .unwrap_or(true)
}

#[cfg(all(unix, not(target_os = "macos")))]
fn system_taskbar_light() -> bool {
    // GNOME/兼容 gsettings 的桌面可探明；其余桌面按浅色兜底（黑图标）
    std::process::Command::new("gsettings")
        .args(["get", "org.gnome.desktop.interface", "color-scheme"])
        .output()
        .map(|o| {
            !String::from_utf8_lossy(&o.stdout)
                .to_ascii_lowercase()
                .contains("dark")
        })
        .unwrap_or(true)
}

/// DPI → 托盘物理像素档（100%→16px、125%→20px、150%→24px、200%→32px），
/// 1:1 映射避免系统缩放导致模糊
fn tray_size_for_scale(scale: f64) -> u32 {
    if scale >= 2.0 {
        32
    } else if scale >= 1.5 {
        24
    } else if scale >= 1.25 {
        20
    } else {
        16
    }
}

/// 「任务栏明暗 × 物理尺寸」→ 内嵌图标字节（浅底黑版 / 深底白版，
/// 透明底单色 SVG 直出，见 scripts/gen-icons.mjs）
fn tray_bytes(light: bool, size: u32) -> &'static [u8] {
    match (light, size) {
        (true, 32) => include_bytes!("../icons/tray-black-32.png"),
        (true, 24) => include_bytes!("../icons/tray-black-24.png"),
        (true, 20) => include_bytes!("../icons/tray-black-20.png"),
        (true, 16) => include_bytes!("../icons/tray-black-16.png"),
        (false, 32) => include_bytes!("../icons/tray-white-32.png"),
        (false, 24) => include_bytes!("../icons/tray-white-24.png"),
        (false, 20) => include_bytes!("../icons/tray-white-20.png"),
        (false, 16) => include_bytes!("../icons/tray-white-16.png"),
        _ => unreachable!("size 由 tray_size_for_scale 产生，恒为 16/20/24/32"),
    }
}

/// 按显示器 DPI 与系统任务栏明暗选择托盘图标（create() 装配时用一次；
/// 后续变化由 start_follow_icon_context 热切）。
fn pick_tray_image(app: &AppHandle) -> Option<tauri::image::Image<'static>> {
    let light = system_taskbar_light();
    let scale = app
        .get_window(crate::MAIN_WINDOW)
        .and_then(|w| w.scale_factor().ok())
        .unwrap_or(1.0);
    let size = tray_size_for_scale(scale);
    let tone = if light { "black" } else { "white" };
    crate::logging::log(&format!("托盘: 图标 {tone} {size}px（scale={scale:.2}）"));
    match tauri::image::Image::from_bytes(tray_bytes(light, size)) {
        Ok(image) => Some(image),
        Err(e) => {
            crate::logging::log(&format!("托盘: 图标 {tone} {size}px 解码失败：{e}"));
            None
        }
    }
}

/// 托盘图标热切：后台复核「系统任务栏明暗 × 主窗口 DPI」，任一变化即换
/// 图标。选轮询而非系统事件：Windows 的 WM_SETTINGCHANGE 需要自建窗口
/// 过程，macOS/Linux 无等价广播，而仓库已有 start_follow_dsh_settings 的
/// 「低频轮询 + 变化才动作」模式可循。Windows 间隔 2s（单次注册表读，
/// 微秒级），其余平台 5s（defaults/gsettings 子进程查询较贵）。首拍只记录
/// 不换图——图标已由 create() 按同一输入装好。
pub fn start_follow_icon_context(app: AppHandle) {
    std::thread::spawn(move || {
        let interval = if cfg!(windows) { 2 } else { 5 };
        let mut last: Option<(bool, u32)> = None;
        loop {
            std::thread::sleep(std::time::Duration::from_secs(interval));
            if app.state::<AppState>().is_quitting() {
                return;
            }
            let light = system_taskbar_light();
            let size = tray_size_for_scale(
                app.get_window(crate::MAIN_WINDOW)
                    .and_then(|w| w.scale_factor().ok())
                    .unwrap_or(1.0),
            );
            if last == Some((light, size)) {
                continue;
            }
            let is_initial = last.is_none();
            last = Some((light, size));
            if is_initial {
                continue;
            }
            crate::logging::log(&format!(
                "托盘: 环境变化（任务栏{}、{size}px）→ 换图标",
                if light { "浅色" } else { "深色" }
            ));
            if let Ok(image) = tauri::image::Image::from_bytes(tray_bytes(light, size)) {
                let handle = app.clone();
                let _ = app.run_on_main_thread(move || {
                    if let Some(tray) = handle.tray_by_id("main-tray") {
                        let _ = tray.set_icon(Some(image));
                    }
                });
            }
        }
    });
}

fn open_browser(app: &AppHandle) {
    let config = app.state::<AppState>().config();
    if !crate::dsh::health_check(config.port, config.auth_token.as_deref()) {
        crate::control_center::open_notice(
            app,
            crate::locale::text("在浏览器中打开", "Open in browser"),
            crate::locale::text(
                "dsh 服务当前未运行，无法在浏览器中打开。",
                "The dsh service is not running, so it cannot be opened in a browser.",
            )
            .into(),
            "warn",
        );
        return;
    }
    // 带 token 打开：浏览器首个请求完成交换后由会话 cookie（30 天）接管
    let url = config.web_page_url();
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
    let state = app.state::<AppState>();
    if state.service_ownership().is_external() {
        crate::control_center::open_notice(
            app,
            crate::locale::text("重启 dsh 服务", "Restart dsh service"),
            crate::locale::text(
                "当前连接的是外部 dsh 服务，请在原服务环境中重启。",
                "The current dsh service is external. Restart it in that service's environment.",
            )
            .into(),
            "info",
        );
        return;
    }
    if state.is_updating() {
        crate::control_center::open_notice(
            app,
            crate::locale::text("重启 dsh 服务", "Restart dsh service"),
            crate::locale::text(
                "更新流程正在进行，请稍后再重启。",
                "An update is in progress. Please restart the service later.",
            )
            .into(),
            "warn",
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
        crate::control_center::open_notice(
            app,
            crate::locale::text("重启 dsh 服务", "Restart dsh service"),
            crate::locale::text(
                "启动流程进行中，请稍后再试。",
                "Startup is in progress. Please try again later.",
            )
            .into(),
            "warn",
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
            crate::control_center::open_notice(
                &handle,
                crate::locale::text("重启 dsh 服务", "Restart dsh service"),
                format!(
                    "{}: {e}",
                    crate::locale::text("重启 dsh 服务失败", "Failed to restart the dsh service")
                ),
                "warn",
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

#[cfg(test)]
mod tests {
    use super::tray_size_for_scale;

    /// DPI 分档边界：档位值与阈值两侧的落档都必须稳定（热切与装配共用）
    #[test]
    fn tray_size_maps_dpi_tiers() {
        assert_eq!(tray_size_for_scale(1.0), 16);
        assert_eq!(tray_size_for_scale(1.24), 16);
        assert_eq!(tray_size_for_scale(1.25), 20);
        assert_eq!(tray_size_for_scale(1.49), 20);
        assert_eq!(tray_size_for_scale(1.5), 24);
        assert_eq!(tray_size_for_scale(1.99), 24);
        assert_eq!(tray_size_for_scale(2.0), 32);
        assert_eq!(tray_size_for_scale(2.5), 32);
    }
}
