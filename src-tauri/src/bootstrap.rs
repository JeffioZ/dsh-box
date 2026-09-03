//! Tauri 应用装配与生命周期入口。

use crate::*;
use tauri::Manager;

const MAIN_MIN_WIDTH: f64 = 720.0;
const MAIN_MIN_HEIGHT: f64 = 460.0;
const MAIN_PREFERRED_MIN_WIDTH: f64 = 820.0;
const MAIN_PREFERRED_MIN_HEIGHT: f64 = 520.0;
const MAIN_MAX_WIDTH: f64 = 1280.0;
const MAIN_MAX_HEIGHT: f64 = 820.0;

fn preferred_main_size(work_width: f64, work_height: f64) -> (f64, f64) {
    let width = (work_width * 0.8)
        .clamp(MAIN_PREFERRED_MIN_WIDTH, MAIN_MAX_WIDTH)
        .min(work_width.max(MAIN_MIN_WIDTH));
    let height = (work_height * 0.82)
        .clamp(MAIN_PREFERRED_MIN_HEIGHT, MAIN_MAX_HEIGHT)
        .min(work_height.max(MAIN_MIN_HEIGHT));
    (width, height)
}

/// 子 WebView 加载自愈看门狗：每 3s 检查一次就绪握手，未就绪则重载页面，
/// 最多重试 3 次；第 4 次仍失败则记错误日志后放弃（连续失败基本是环境性
/// 问题，重载解决不了）。检查间隔按 1s 粒度睡眠，退出标志能及时打断。
fn spawn_load_watchdog(
    handle: tauri::AppHandle,
    label: &'static str,
    is_ready: fn() -> bool,
    reload: fn(&tauri::AppHandle),
) {
    std::thread::spawn(move || {
        let mut remaining_reloads = 3u8;
        loop {
            for _ in 0..3 {
                std::thread::sleep(std::time::Duration::from_secs(1));
                if handle.state::<AppState>().is_quitting() {
                    return;
                }
            }
            if is_ready() {
                return;
            }
            if remaining_reloads == 0 {
                logging::log(&format!(
                    "{label}: 多次重载后仍未就绪，放弃自愈（疑似环境性问题，详见日志）"
                ));
                return;
            }
            remaining_reloads -= 1;
            logging::log(&format!(
                "{label}: 页面未就绪，重试加载（剩余 {remaining_reloads} 次）"
            ));
            reload(&handle);
        }
    });
}

/// 启动致命错误的最后兜底：记日志 + 用户可见提示，然后以退出码 1 优雅退出。
/// 此时 Tauri 事件循环尚未运行，tauri 对话框与自绘 UI 都不可用——Windows
/// 只能直接调 MessageBoxW（宽字符、无父窗口），非 Windows 退回 stderr。
/// 不能用 expect/panic：release 构建 panic = "abort"，GUI 应用没有可见
/// stderr，进程会无声消失，用户完全无从得知失败原因。
fn fatal_boot_exit(stage: &str, error: &dyn std::fmt::Display) -> ! {
    // 日志正常时已由 setup 初始化；若失败发生在更早阶段本条会静默丢弃，
    // 但提示框仍携带完整错误文本与日志路径指引
    let log_path = app_state::Config::load().logs_dir().join("dshbox.log");
    logging::log(&format!("启动: 致命错误（{stage}）：{error}"));
    let text = if locale::is_chinese() {
        format!(
            "{APP_TITLE} 启动失败（{stage}）。\n\n错误：{error}\n\n详细原因见日志文件：\n{}\n\n请重试；若反复出现，请将日志提交给支持人员排查。",
            log_path.display()
        )
    } else {
        format!(
            "{APP_TITLE} failed to start ({stage}).\n\nError: {error}\n\nSee the log file for details:\n{}\n\nPlease try again; if it keeps failing, send the log to support.",
            log_path.display()
        )
    };
    #[cfg(windows)]
    {
        use windows_sys::Win32::UI::WindowsAndMessaging::{MessageBoxW, MB_ICONERROR, MB_OK};
        let to_wide = |s: &str| {
            s.encode_utf16()
                .chain(std::iter::once(0))
                .collect::<Vec<u16>>()
        };
        let text_w = to_wide(&text);
        let title_w = to_wide(APP_TITLE);
        // 无父窗口：事件循环未起，主窗口可能根本未创建成功
        unsafe {
            MessageBoxW(
                std::ptr::null_mut(),
                text_w.as_ptr(),
                title_w.as_ptr(),
                MB_OK | MB_ICONERROR,
            );
        }
    }
    #[cfg(not(windows))]
    eprintln!("{text}");
    std::process::exit(1);
}

pub(crate) fn run() {
    let app = tauri::Builder::default()
        .register_uri_scheme_protocol("dshd", handle_dshd_scheme)
        .plugin(tauri_plugin_single_instance::init(|app, args, _cwd| {
            // 与启动路径一致：转发参数带 --minimized 时保持托盘静默，不唤出窗口
            if !args.iter().any(|a| a == "--minimized") {
                show_main(app);
            }
        }))
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_clipboard_manager::init())
        .manage(AppState::new())
        .invoke_handler(commands::invoke_handler())
        .setup(|app| {
            // dev 构建标记（devUrl bake）：onboarding 每次启动引导便于测试
            if app.config().build.dev_url.is_some() {
                app_state::mark_dev_build();
            }
            // 日志初始化必须最早：其后的 dev-ui 拉起、窗口创建等步骤都会
            // 记日志，晚初始化会把这些日志静默丢弃
            let cfg = app.state::<AppState>().config();
            logging::init(cfg.logs_dir().join("dshbox.log"));
            logging::log(&format!(
                "启动: port={} root={}",
                cfg.port,
                cfg.root.display()
            ));
            // dev 构建：先拉起 UI 静态服务器（同步等待就绪 ≤5s）再创建窗口——
            // 主 webview 首次加载即成功，避免“加载失败 → 服务器就绪后 reload”
            // 的白屏与页面闪烁（reload 会重置页面状态、启动面板重复显示）。
            // 返回值为“服务器是否已就绪”：仅 dev 构建且未就绪时需要延迟
            // reload 兜底；生产构建恒返回 false（由 dev_url 守卫排除）。
            let dev_ui_ready = ensure_dev_ui_server(app.handle());
            // 手建主窗口（conf windows 为空）：带初始化脚本预设 dsh 深色主题，
            // 背景色跟随系统主题，与 dsh/loading 底色统一，消除启动与导航的明暗闪烁
            let navigation_app = app.handle().clone();
            let page_load_app = app.handle().clone();
            let hide_stats_early = if app.state::<AppState>().config().hide_stats_line {
                hide_stats_early()
            } else {
                String::new()
            };
            let page_init_script = format!(
                "{}\n{}\n{}",
                locale::init_script(),
                PAGE_INIT_SCRIPT,
                hide_stats_early
            );
            let win = tauri::WebviewWindowBuilder::new(
                app,
                MAIN_WINDOW,
                tauri::WebviewUrl::App("index.html".into()),
            )
            .title(APP_TITLE)
            .inner_size(1280.0, 820.0)
            // 默认尺寸不变；只放宽高 DPI/小屏下的最小逻辑视口。dsh 官方
            // 在 560/680/720/760px 设有响应式断点，720×460 仍保留完整功能。
            .min_inner_size(MAIN_MIN_WIDTH, MAIN_MIN_HEIGHT)
            .resizable(true)
            .center()
            .visible(false)
            .background_color(DARK_BG)
            // 禁用后台节流：失焦时 WebView2 暂停渲染，loading 进度条动画
            // 会停摆（首次设置停留后"卡住不动、恢复时一闪而过"），
            // 状态栏等实时更新的子页面也会滞后
            .background_throttling(tauri::utils::config::BackgroundThrottlingPolicy::Disabled)
            // Windows 上默认的 drag-drop handler 会禁用页面 HTML5 拖放
            // （破坏 dsh 页面自身的拖放与文件上传插件），显式关闭
            .disable_drag_drop_handler()
            .initialization_script(page_init_script)
            .on_navigation(move |url| {
                let allowed = is_allowed_navigation(&navigation_app, url);
                if !allowed {
                    logging::log(&format!("navigation: 已拦截非白名单地址 {url}"));
                }
                allowed
            })
            // 完整注入必须跟随每次页面加载：reload 不经过 navigate()，若这里只
            // 补 hide-stats，右键菜单、标题修正与心跳都会在重载后永久丢失。
            // 注入脚本自身幂等，navigate() 的定时重试仅作为加载竞态兜底。
            .on_page_load(move |_window, _payload| {
                if let Some(webview) = crate::main_webview(&page_load_app) {
                    if let Err(error) = inject_dsh_page(&page_load_app, &webview) {
                        logging::log(&format!("navigation: 页面加载后注入失败：{error}"));
                    }
                }
            })
            .build()
            // 主窗口是应用存在的意义，创建失败即致命：可见化报错后退出
            .unwrap_or_else(|e| fatal_boot_exit("主窗口创建", &e));
            // 不使用 set_shadow：tao 的无边框阴影实现带隐藏 insets（窗口
            // 外矩形比可见区域大一圈），保存/恢复 outer_size 时 insets 逐次
            // 累积——正是“每次启动窗口大一圈”的来源；且它会附加 1px 白边。
            // 主窗口保持无装饰直角窗口，尺寸记忆由窗口状态逻辑独立负责。
            #[cfg(target_os = "macos")]
            let _ = win.set_title_bar_style(tauri::TitleBarStyle::Overlay);
            // dsh 主题优先：启动时即读取 settings.yaml 的 ui-theme.preference，
            // light/dark 直接固定窗口主题，system 或未设置则跟随系统。
            // 这样加载页从第一帧起就与 dsh 的主题一致，而不是先按系统主题
            // 显示、加载完成后再切换（win.theme() 随后取到的是固定后的主题，
            // 背景色也随之对齐，避免启动闪烁）。
            if let Some(theme) = app.state::<AppState>().config().resolve_dsh_theme() {
                let _ = win.set_theme(Some(theme));
            }
            if let Ok(theme) = win.theme() {
                let color = if theme == tauri::Theme::Dark {
                    DARK_BG
                } else {
                    LIGHT_BG
                };
                let _ = win.set_background_color(Some(color));
            }

            let cfg = app.state::<AppState>().config();

            // 记忆窗口位置/大小：全程逻辑坐标——保存的就是逻辑值，恢复也直接用
            // 逻辑坐标设置，交给系统做 DPI 换算（物理坐标设置在高 DPI 下会被
            // 系统二次协商撑大尺寸，导致底部再次越过任务栏）。
            // 目标显示器选择 + 裁剪都在逻辑空间完成，且必须“同一台显示器”与
            // 窗口相交；恢复时硬性收敛进该显示器工作区。
            // 本次启动实际应用的尺寸（恢复值或自适应值），供终态线程
            // 测量系统协商增量（见 window.rs 的 NEGOTIATION_DELTA 说明）
            let mut applied_size: (f64, f64) = (0.0, 0.0);
            let restored = cfg
                .load_window_rect()
                .map(|(lx, ly, lw, lh)| {
                    if lw < 400.0 || lh < 300.0 {
                        return false;
                    }
                    let target = app
                        .available_monitors()
                        .unwrap_or_default()
                        .iter()
                        .find_map(|m| {
                            let scale = m.scale_factor();
                            let wa = m.work_area();
                            // 逻辑工作区
                            let (px, py) = (wa.position.x as f64 / scale, wa.position.y as f64 / scale);
                            let (pw, ph) = (wa.size.width as f64 / scale, wa.size.height as f64 / scale);
                            // 仅要求与工作区相交（留最小可见区），尺寸不合则裁剪
                            let ok = lx < px + pw - 40.0
                                && lx + lw > px + 40.0
                                && ly < py + ph - 40.0
                                && ly + lh > py + 40.0;
                            ok.then_some((px, py, pw, ph))
                        });
                    if let Some((px, py, pw, ph)) = target {
                        // 硬性收敛进工作区：尺寸不超工作区，位置完整可见
                        let wc = lw.clamp(MAIN_MIN_WIDTH.min(pw), pw);
                        let hc = lh.clamp(MAIN_MIN_HEIGHT.min(ph), ph);
                        let xc = lx.clamp(px, px + pw - wc);
                        let yc = ly.clamp(py, py + ph - hc);
                        logging::log(&format!(
                            "窗口: 恢复 原始=({lx:.0},{ly:.0},{lw:.0}x{lh:.0}) 裁剪=({xc:.0},{yc:.0},{wc:.0}x{hc:.0}) 工作区=({pw:.0}x{ph:.0})"
                        ));
                        if let Some(win) = main_window(app.handle()) {
                            let _ = win.set_position(tauri::Position::Logical(
                                tauri::LogicalPosition::new(xc, yc),
                            ));
                            let _ = win.set_size(tauri::Size::Logical(
                                tauri::LogicalSize::new(wc, hc),
                            ));
                            applied_size = (wc, hc);
                        }
                        true
                    } else {
                        false
                    }
                })
                .unwrap_or(false);
            if !restored {
                if let Some(win) = main_window(app.handle()) {
                    // 无有效记忆：按当前显示器工作区自适应（约 80%，受最小尺寸与设计上限约束），
                    // 适配小屏/高 DPI 显示器
                    if let Ok(Some(monitor)) = win.current_monitor() {
                        let scale = monitor.scale_factor();
                        let wa = monitor.work_area();
                        let ww = wa.size.width as f64 / scale;
                        let wh = wa.size.height as f64 / scale;
                        let (w, h) = preferred_main_size(ww, wh);
                        let _ =
                            win.set_size(tauri::Size::Logical(tauri::LogicalSize::new(w, h)));
                        applied_size = (w, h);
                    }
                    // set_size 后重新居中（创建时的 center 基于初始尺寸）
                    let _ = win.center();
                }
            }
            // 主窗口几何恢复完成：此时创建自绘弹窗，初始尺寸/位置即最终值
            // （避免预创建于 setup 早期导致首次打开时的几何异步跳变）
            crate::logging::log("boot: 主窗口几何就绪，预创建弹窗");
            control_center::precreate(app.handle());
            // 启动后越界兜底收敛：show 时系统会对窗口几何做一次协商（本机观察
            // 约 +14w/+9h，随后稳定），协商后的尺寸即最终值——不再按保存值
            // “重新应用”：中途再 set 一次会被系统再次协商拉回，形成 loading
            // 期间肉眼可见的尺寸跳动（正是启动时窗口变一下的来源）。1.5s 后
            // 仅做越界收敛（阈值 4 逻辑像素，跳过无害的亚像素噪声）；启动
            // 静默期内不落盘，协商漂移不会被持久化，逐次启动大小保持稳定。
            {
                let handle = app.handle().clone();
                let applied = applied_size;
                std::thread::spawn(move || {
                    std::thread::sleep(std::time::Duration::from_millis(1500));
                    // 阶段二：终态兜底收敛（位置/尺寸硬性收进工作区）
                    let Some(win) = main_window(&handle) else {
                        return;
                    };
                    if win.is_maximized().unwrap_or(false) {
                        return;
                    }
                    let (Ok(pos), Ok(size)) = (win.outer_position(), win.outer_size()) else {
                        return;
                    };
                    let scale = win.scale_factor().unwrap_or(1.0);
                    let (lx, ly) = (pos.x as f64 / scale, pos.y as f64 / scale);
                    let (lw, lh) = (size.width as f64 / scale, size.height as f64 / scale);
                    // 测量本次设置的系统协商增量，供后续保存时扣除；
                    // applied 为 (0,0) 表示本次未设置尺寸（无有效记忆且读不到
                    // 显示器信息），此时差值是窗口实际尺寸而非协商量，不能记录
                    if applied != (0.0, 0.0) {
                        crate::window::record_negotiation_delta(lw - applied.0, lh - applied.1);
                    }
                    if let Some((px, py, pw, ph)) = logical_work_area(&handle) {
                        let wc = lw.min(pw);
                        let hc = lh.min(ph);
                        let xc = lx.clamp(px, px + pw - wc);
                        let yc = ly.clamp(py, py + ph - hc);
                        logging::log(&format!(
                            "窗口: 终态 逻辑=({lx:.0},{ly:.0},{lw:.0}x{lh:.0}) 工作区=({pw:.0}x{ph:.0})"
                        ));
                        if (xc - lx).abs() > 4.0
                            || (yc - ly).abs() > 4.0
                            || (wc - lw).abs() > 4.0
                            || (hc - lh).abs() > 4.0
                        {
                            logging::log(&format!(
                                "窗口: 二次收敛 ({lx:.0},{ly:.0},{lw:.0}x{lh:.0}) -> ({xc:.0},{yc:.0},{wc:.0}x{hc:.0})"
                            ));
                            let _ = win.set_position(tauri::Position::Logical(
                                tauri::LogicalPosition::new(xc, yc),
                            ));
                            let _ = win.set_size(tauri::Size::Logical(
                                tauri::LogicalSize::new(wc, hc),
                            ));
                        }
                    }
                });
            }
            // 按 DPI 设置窗口图标（标题栏/任务栏 1:1 像素，避免系统缩放糊化）
            window::set_window_icon(app.handle());
            // dev 兜底：仅 dev 构建且服务器未就绪（冷启动慢、被安全软件
            // 拦截后放行）时主 webview 可能白屏——延迟 reload 一次自愈；
            // 生产构建 dev_url 为 None（ensure 恒返回 false）与正常 dev
            // 路径（服务器已就绪）都不触发，不产生闪烁
            if app.config().build.dev_url.is_some() && !dev_ui_ready {
                let reload_handle = app.handle().clone();
                std::thread::spawn(move || {
                    std::thread::sleep(std::time::Duration::from_millis(3000));
                    logging::log("dev-ui: 服务器未就绪，延迟重载内置页面兜底");
                    for (_, window) in reload_handle.webview_windows() {
                        for (_, wv) in window.webviews() {
                            let _ = wv.reload();
                        }
                    }
                });
            }
            // 自绘标题栏：去掉系统标题栏（macOS 除外）、创建顶条子 webview、主 webview 让位
            if let Err(e) = titlebar::init(app.handle()) {
                logging::log(&format!("标题栏: 初始化失败：{e}"));
            }
            // 底部状态栏：会话统计 + 余额 + 设置入口（独立子 webview）
            if let Err(e) = titlebar::init_statusbar(app.handle()) {
                logging::log(&format!("状态栏: 初始化失败：{e}"));
            }
            // 标题栏/状态栏加载自愈：页面初始化完成回报就绪（titlebar_ready /
            // statusbar_ready）；未回报（加载失败/脚本初始化失败/被跳过）由
            // 看门狗重载重试——偶发的「启动后标题栏/状态栏空白」由此兜底
            spawn_load_watchdog(
                app.handle().clone(),
                "titlebar",
                titlebar::is_ready,
                titlebar::reload,
            );
            spawn_load_watchdog(
                app.handle().clone(),
                "statusbar",
                titlebar::statusbar_is_ready,
                titlebar::reload_statusbar,
            );
            // 标题栏渲染自愈：合成层失效（间歇空白、DOM 正常）无法探测，
            // 周期发送重绘脉冲兜底恢复
            {
                let handle = app.handle().clone();
                std::thread::spawn(move || loop {
                    std::thread::sleep(std::time::Duration::from_secs(30));
                    if handle.state::<AppState>().is_quitting() {
                        return;
                    }
                    titlebar::repaint_pulse(&handle);
                });
            }
            // 托盘菜单窗口：启动时预创建（隐藏）。自绘弹窗移至主窗口几何
            // 恢复完成之后创建（见窗口恢复段）——创建时主窗口几何已知，
            // 弹窗初始几何即正确，消除首次打开时"位置不对 + 闪烁"
            #[cfg(windows)]
            tray_menu::precreate(app.handle());
            // 标题栏余额常驻显示：后台每 5 分钟刷新一次
            crate::balance::start_periodic_refresh(app.handle().clone());
            // 运行期每 6 小时自动检查一次 dsh 更新（发现新版弹提示，不自动安装）
            updater::start_periodic_check(app.handle().clone());
            // 任务完成系统通知（主窗口不可见时；只读轮询 dsh 会话日志）
            notify::start_task_watch(app.handle().clone());
            // dsh 页面心跳监控：页面挂起/崩溃时重载自愈（指数退避）
            heartbeat::start_page_watch(app.handle().clone());
            // 跟随 dsh 的设置（语言/主题）：后台每 3s 检查 settings.yaml mtime
            tray::start_follow_dsh_settings(app.handle().clone());
            // 状态栏会话统计：每 5s 轮询 dsh 投影并广播（失败静默显示占位）
            crate::usage::start_periodic(app.handle().clone());
            // 状态栏实时生成速率：每 2s 尾帧解码会话日志估算流式 tok/s
            crate::usage::start_live_rate(app.handle().clone());
            // 账户后台监测：dsh 就绪后立即全量刷新余额/订阅缓存，此后每 5 分钟
            // 一轮并广播 usage-accounts-updated（控制中心用量页只读缓存）
            crate::usage::start_account_monitor(app.handle().clone());
            // 凭据文件跟随：每 3s 检查 .credentials.yaml 的 mtime，变化即触发
            // 一轮账户刷新——dsh 设置页填完 key 后状态栏无需等 5 分钟周期
            crate::usage::start_credentials_follow(app.handle().clone());
            // 用量预警：每 10 分钟聚合增量日志并线性外推今日全天用量，
            // 预计越过用户阈值时发一次系统通知（每天至多一次）
            crate::usage::start_usage_alerts(app.handle().clone());
            // 内置插件市场（dsh-market）：dsh 就绪后自动预装，此后每日同步最新版
            plugins::start_market_bootstrap(app.handle().clone());
            // 窗口以隐藏状态创建，图标就绪后再显示 —— 任务栏/标题栏第一帧即是清晰图标
            let config = app.state::<AppState>().config();
            let minimized = std::env::args().any(|a| a == "--minimized")
                || config.launch_behavior == "tray";
            if minimized {
                logging::log("启动: --minimized 静默进托盘");
            } else if let Some(win) = main_window(app.handle()) {
                let _ = win.show();
                // 状态栏首帧即数据：窗口显示后再推一次统计/余额。
                // 立即 emit 时状态栏子 webview 尚未首帧，事件丢失/迟到；
                // 延迟 150ms 覆盖子 webview 首帧渲染后再更新（消除 chip 闪烁）
                let handle = app.handle().clone();
                std::thread::spawn(move || {
                    std::thread::sleep(std::time::Duration::from_millis(150));
                    crate::usage::refresh_once(handle.clone());
                    crate::balance::refresh_once(handle);
                });
            }
            // 启动静默期：恢复/协商产生的几何事件不落盘（3s 内），
            // 避免系统微调后的尺寸被持久化、逐次启动累积变大
            window::start_save_settle(3000);

            match tray::create(app.handle()) {
                Ok(()) => logging::log("托盘: 已创建"),
                Err(e) => logging::log(&format!("托盘: 创建失败：{e}")),
            }
            let handle = app.handle().clone();
            std::thread::spawn(move || dsh::boot_loop(handle));
            Ok(())
        })
        .on_window_event(|window, event| match event {
            tauri::WindowEvent::CloseRequested { api, .. } => {
                // 辅助窗口有自己的隐藏/模态恢复生命周期，不能套用主窗口的
                // “驻留托盘/退出应用”策略。尤其控制中心打开时主窗口处于
                // disabled，直接隐藏会让主窗口永久无法交互。
                if window.label() != MAIN_WINDOW {
                    api.prevent_close();
                    if window.label() == crate::control_center::APP_DIALOG_WINDOW {
                        crate::control_center::close(window.app_handle());
                    } else if window.label() == crate::tray_menu::TRAY_MENU_WINDOW {
                        crate::tray_menu::hide_menu(window.app_handle());
                    } else {
                        let _ = window.hide();
                    }
                    return;
                }
                // 默认关窗隐藏到托盘；用户可改为直接退出。
                // 无论哪条路径，先保存一次窗口状态：退出路径（is_quitting）
                // 下窗口即将销毁，等 ExitRequested 再读时窗口句柄已不存在，
                // 最后一次位置会丢失。
                window::save_window_state_now(window.app_handle());
                let state = window.app_handle().state::<AppState>();
                if !state.inner().is_quitting() && state.config().close_behavior == "tray" {
                    api.prevent_close();
                    let _ = window.hide();
                } else if !state.inner().is_quitting() {
                    api.prevent_close();
                    state.set_quitting(true);
                    // 先藏窗给即时反馈；停服收尾挪到后台线程——shutdown 的
                    // 进程 wait 会阻塞事件循环，主线程同步执行时关窗会冻结
                    // 数秒（与托盘退出同一模式，见 tray::quit）
                    let _ = window.hide();
                    let handle = window.app_handle().clone();
                    std::thread::spawn(move || quit_sequence(&handle));
                }
            }
            tauri::WindowEvent::Focused(focused) => {
                // 标题栏/状态栏失焦样式跟随主窗口焦点：子 webview 的 window
                // focus/blur 事件与主窗口焦点并不同步（WebView2 行为），
                // 由 Rust 侧统一广播，页面侧按此切换样式
                for label in [
                    crate::titlebar::TITLEBAR_LABEL,
                    crate::titlebar::STATUSBAR_LABEL,
                ] {
                    if let Some(wv) = window.webviews().into_iter().find(|w| w.label() == label) {
                        let _ = wv.eval(format!(
                            "window.__dshdSetWindowActive && window.__dshdSetWindowActive({focused})"
                        ));
                    }
                }
                if *focused {
                    // 获焦时触发重绘脉冲：合成层失效导致的标题栏空白
                    // 在窗口重新激活时自愈
                    crate::titlebar::repaint_pulse(window.app_handle());
                }
            }
            tauri::WindowEvent::ThemeChanged(theme) if window.label() == MAIN_WINDOW => {
                let color = if *theme == tauri::Theme::Light {
                    LIGHT_BG
                } else {
                    DARK_BG
                };
                let _ = window.set_background_color(Some(color));
                titlebar::set_statusbar_theme_background(
                    window.app_handle(),
                    *theme == tauri::Theme::Light,
                );
            }
            tauri::WindowEvent::ScaleFactorChanged { new_inner_size, .. }
                if window.label() == MAIN_WINDOW =>
            {
                titlebar::sync_bounds_for_size(window.app_handle(), *new_inner_size);
            }
            tauri::WindowEvent::Resized(size) => {
                if window.label() != MAIN_WINDOW {
                    return;
                }
                // 标题栏/主 webview 边界跟随窗口尺寸（必须先于带 guard 的臂执行，
                // 否则 resize 时 sync_bounds 永不触发，标题栏被主 webview 覆盖）
                titlebar::sync_bounds_for_size(window.app_handle(), *size);
                if !window
                    .app_handle()
                    .state::<AppState>()
                    .inner()
                    .is_quitting()
                {
                    // 拖动/缩放停顿 250ms 后保存，退出时另行强制落盘。
                    window::save_window_state(window.app_handle());
                }
            }
            tauri::WindowEvent::Moved(_)
                if window.label() == MAIN_WINDOW
                    && !window
                    .app_handle()
                    .state::<AppState>()
                    .inner()
                    .is_quitting() =>
            {
                window::save_window_state(window.app_handle());
            }
            _ => {}
        })
        .build(tauri::generate_context!())
        // 事件循环/插件装配失败同样致命：可见化报错后退出，不再静默 abort
        .unwrap_or_else(|e| fatal_boot_exit("应用构建", &e));

    app.run(|app_handle, event| {
        match event {
            tauri::RunEvent::ExitRequested { api, .. } => {
                window::save_window_state_now(app_handle);
                // 退出已由某入口发起（关窗/托盘/quit 命令）：后台线程正在
                // 停服并会再次 exit。这里若同步再跑一次 shutdown，
                // take_running 的竞速可能让主线程拿到 Child 并阻塞在 wait
                // 上——正是本次修复要消除的退出冻结。
                if app_handle.state::<AppState>().inner().is_quitting() {
                    return;
                }
                // 未经发起的直接退出请求（如 macOS Cmd+Q / 系统会话结束）：
                // 同样不在主线程同步停服。先拦下本次退出，后台停服完成后
                // 再 exit（第二次 ExitRequested 走上面的 is_quitting 分支放行）。
                app_handle.state::<AppState>().set_quitting(true);
                let handle = app_handle.clone();
                std::thread::spawn(move || quit_sequence(&handle));
                api.prevent_exit();
            }
            // macOS：点击系统通知/从后台恢复时恢复隐藏窗口（Windows 的通知
            // 点击走系统激活 + 单实例回调 show_main，此处兜底 macOS/Linux）
            tauri::RunEvent::Resumed => show_main(app_handle),
            _ => {}
        }
    });
}

/// 退出的收尾序列：停服 → 给进程树收尾时间 → 退出。必须在后台线程执行
/// （shutdown 的进程 wait 会阻塞调用线程），三处退出入口共用。
pub(crate) fn quit_sequence(app: &AppHandle) {
    dsh::shutdown(app);
    std::thread::sleep(std::time::Duration::from_millis(300));
    app.exit(0);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preferred_main_size_keeps_default_caps_on_large_displays() {
        assert_eq!(preferred_main_size(1920.0, 1080.0), (1280.0, 820.0));
    }

    #[test]
    fn preferred_main_size_fits_high_dpi_work_area() {
        assert_eq!(preferred_main_size(1024.0, 600.0), (820.0, 520.0));
        assert_eq!(preferred_main_size(853.0, 493.0), (820.0, 493.0));
    }

    #[test]
    fn preferred_main_size_never_drops_below_window_minimum() {
        assert_eq!(preferred_main_size(640.0, 400.0), (720.0, 460.0));
    }
}
