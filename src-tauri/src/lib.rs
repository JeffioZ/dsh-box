//! DSHBox —— DeepSeek Harness (dsh) 桌面端外壳。
//!
//! 职责：管理 Node/dsh 运行时（检测、安装、更新），以隐藏窗口方式启动
//! `dsh web` 服务，用 WebView 加载 http://127.0.0.1:<port> 的官方界面，
//! 提供托盘/标题栏菜单与自绘弹窗（打开 / 检查更新 / API 余额 / 关于 / 退出），
//! 退出时清理全部子进程。

mod app_state;
mod autostart;
mod balance;
mod bootstrap;
mod commands;
mod control_center;
mod credentials;
mod dev_ui;
mod dsh;
mod file_actions;
mod file_icons;
mod heartbeat;
pub mod locale;
mod logging;
mod model_config;
mod native_dialog;
mod notify;
mod onboarding;
mod plugins;
mod processes;
mod runtime;
mod session_log;
mod stats;
mod titlebar;
mod tray;
mod tray_menu;
mod updater;
mod versions;
mod webview;
mod window;

use dev_ui::ensure_dev_ui_server;
#[cfg(test)]
use webview::hide_stats_apply;
use webview::{
    app_dev_origin, handle_dshd_scheme, hide_stats_early, inject_dsh_page, is_allowed_navigation,
    is_dsh_url, is_local_app_url, PAGE_INIT_SCRIPT,
};
pub use webview::{apply_hide_stats, apply_hide_tools, navigate, navigate_to_splash};

use app_state::{AppState, BootPhase};
use tauri::{AppHandle, Emitter, Manager};

/// 主窗口 label。
pub const MAIN_WINDOW: &str = "main";

/// 按平台选择 ureq 的 TLS 配置：Windows/macOS 用系统原生实现（对应
/// Cargo.toml 的 native-tls feature），Linux 用 rustls。
/// 注意：ureq 的默认 TlsProvider 是 Rustls 且「不会随 feature 自动切换」——
/// 不显式设置时运行期握手会直接报错，所有 https 请求都会失败。
pub fn default_tls_config() -> ureq::tls::TlsConfig {
    let builder = ureq::tls::TlsConfig::builder();
    #[cfg(target_os = "linux")]
    // Linux（rustls）：用默认 WebPki 内置根；PlatformVerifier 在 rustls 后端
    // 需要额外 feature，直接 panic。
    let config = builder.provider(ureq::tls::TlsProvider::Rustls);
    #[cfg(not(target_os = "linux"))]
    // Windows/macOS（native-tls）：用系统信任库验证。附加 webpki 根会覆盖
    // schannel 的默认信任行为（实测 npm registry 证书链因此无法验证）。
    let config = builder
        .provider(ureq::tls::TlsProvider::NativeTls)
        .root_certs(ureq::tls::RootCerts::PlatformVerifier);
    config.build()
}

/// panic = "abort" 的兜底：panic 信息默认输出到 GUI 应用不可见的 stderr。
/// 由 main 里的 panic hook 调用，直接追加写入应用日志（logging 可能尚未
/// 初始化，不能走 logging::log）。
pub fn log_panic(line: &str) {
    // 与正常日志使用同一配置解析，便携模式和 DSH_BOX_ROOT 下也能找到崩溃记录。
    let path = app_state::Config::load().logs_dir().join("dshbox.log");
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    {
        use std::io::Write;
        let _ = writeln!(f, "panic: {line}");
    }
}

/// 对外产品名（窗口标题/托盘/exe 属性等统一显示名）。
pub const APP_TITLE: &str = "DSHBox";

/// 本地启动页（生产环境 Tauri 资源源）。
pub const SPLASH_ORIGIN: &str = "tauri://localhost";

/// 深色主题的统一底色（与 dsh 深色主题 body 背景 #151517 一致，衔接无缝）。
pub(crate) const DARK_BG: tauri::window::Color = tauri::window::Color(0x15, 0x15, 0x17, 0xFF);
/// 浅色主题的统一底色（与 dsh 浅色主题 body 背景纯白一致）。
pub(crate) const LIGHT_BG: tauri::window::Color = tauri::window::Color(0xFF, 0xFF, 0xFF, 0xFF);

/// 向启动页广播状态（不确定进度）。
pub fn emit_status(app: &AppHandle, phase: BootPhase, message: &str, detail: &str) {
    emit_status_progress(app, phase, message, detail, None);
}

/// 向启动页广播状态（可带 0-100 确定进度）。
pub fn emit_status_progress(
    app: &AppHandle,
    phase: BootPhase,
    message: &str,
    detail: &str,
    progress: Option<f64>,
) {
    // 共享状态是 IPC 查询、安装取消/切源和托盘可用性的权威来源；必须先提交
    // 再构造事件。此前只广播事件会出现“页面显示正在安装、后端仍是 Starting”，
    // 从而把本应可取消的安装判断为已结束。
    app.state::<AppState>().set_phase(phase, message, detail);
    // 事件载荷带完整版本信息：此前这里固定 None，前端每次收到事件都会
    // 重算 footer（版本/端口行）并将其清空——启动过程中 footer 短暂出现
    // 后即“消失”。snapshot 的版本检测有缓存，高频事件无额外开销。
    let snapshot = app.state::<AppState>().snapshot();
    let payload = app_state::StatusPayload {
        phase: phase.as_str().to_string(),
        message: message.to_string(),
        detail: detail.to_string(),
        progress,
        dsh_version: snapshot.dsh_version,
        node_version: snapshot.node_version,
        npm_version: snapshot.npm_version,
        port: snapshot.port,
        download_source: snapshot.download_source,
        install_generation: snapshot.install_generation,
        can_cancel: snapshot.can_cancel,
        service_mode: snapshot.service_mode,
        external_service: snapshot.external_service,
    };
    let _ = app.emit("dsh-status", payload);
    crate::tray::sync_menu_state(app);
}

/// 主窗口（Window 级操作：show/icon/title/scale 等）。
/// 不能用 get_webview_window：窗口存在子 webview（自绘标题栏）时它会返回 None。
pub fn main_window(app: &AppHandle) -> Option<tauri::Window> {
    app.get_window(MAIN_WINDOW)
}

/// 主 webview（Webview 级操作：navigate/eval）。
pub fn main_webview(app: &AppHandle) -> Option<tauri::Webview> {
    main_window(app)?
        .webviews()
        .into_iter()
        .find(|w| w.label() == MAIN_WINDOW)
}

pub(crate) fn main_is_visible(app: &AppHandle) -> bool {
    main_window(app)
        .and_then(|window| window.is_visible().ok())
        .unwrap_or(false)
}

/// 主窗口当前所在显示器的逻辑工作区 (x, y, w, h)。
pub(crate) fn logical_work_area(app: &AppHandle) -> Option<(f64, f64, f64, f64)> {
    let win = main_window(app)?;
    let mon = win.current_monitor().ok()??;
    let scale = mon.scale_factor();
    let wa = mon.work_area();
    Some((
        wa.position.x as f64 / scale,
        wa.position.y as f64 / scale,
        wa.size.width as f64 / scale,
        wa.size.height as f64 / scale,
    ))
}

/// 显示主窗口并聚焦（托盘“打开”）。
pub fn show_main(app: &AppHandle) {
    if let Some(w) = main_window(app) {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
    }
    stats::refresh_once(app.clone());
    balance::refresh_once(app.clone());
}

pub fn run() {
    bootstrap::run();
}

#[cfg(test)]
mod url_tests {
    use super::{hide_stats_apply, hide_stats_early, is_local_app_url};

    /// 注入脚本以 JS 单引号字符串承载 CSS：CSS 内再出现单引号会破坏整段
    /// 注入脚本语法（曾致 hide-stats/右键菜单/心跳一并失效的回归）。
    #[test]
    fn hide_stats_scripts_keep_js_quoting_valid() {
        for script in [hide_stats_early(), hide_stats_apply()] {
            assert!(!script.contains("[data-slot='"), "CSS 不得使用单引号");
            assert!(script.contains("__dshd_hide_stats"));
        }
        assert!(hide_stats_apply().contains("sweepStats"));
        assert!(hide_stats_early().contains("FJxK0a_root"));
    }

    #[test]
    fn local_app_origin_is_an_exact_pair() {
        assert!(is_local_app_url(
            &"tauri://localhost/index.html".parse().unwrap(),
            None
        ));
        assert!(is_local_app_url(
            &"http://tauri.localhost/control-center.html"
                .parse()
                .unwrap(),
            None
        ));
        assert!(!is_local_app_url(
            &"http://localhost/index.html".parse().unwrap(),
            None
        ));
        assert!(!is_local_app_url(
            &"http://tauri.localhost:18080/index.html".parse().unwrap(),
            None
        ));
    }

    #[test]
    fn dev_origin_allows_only_the_exact_dev_url() {
        let dev: url::Url = "http://localhost:4321".parse().unwrap();
        assert!(is_local_app_url(
            &"http://localhost:4321/titlebar.html".parse().unwrap(),
            Some(&dev)
        ));
        assert!(!is_local_app_url(
            &"http://localhost:9999/titlebar.html".parse().unwrap(),
            Some(&dev)
        ));
        // 用户名伪装不构成同一来源
        assert!(!is_local_app_url(
            &"http://localhost:4321@evil.com/titlebar.html"
                .parse()
                .unwrap(),
            Some(&dev)
        ));
        // 生产构建（无 devUrl）不放行 dev 来源
        assert!(!is_local_app_url(
            &"http://localhost:4321/titlebar.html".parse().unwrap(),
            None
        ));
    }
}
