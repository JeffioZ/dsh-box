//! 开发构建的本地 UI 静态服务器。

use crate::*;
use tauri::Manager;

/// dev 构建：内置页面经 devUrl 从 UI 静态服务器（4321）加载，
/// 未监听时自动拉起 node scripts/serve-ui.mjs 并等待就绪（最多 5s）。
/// 仅 dev 构建启用；返回 true 表示调用后服务器已就绪（原本在运行或本次
/// 成功拉起），false 表示非 dev 构建或服务器未就绪（调用方据此延迟 reload
/// 兜底）。
/// 必须在主窗口创建前调用：保证 webview 首次加载即成功，无需 reload
/// （reload 会重置页面状态、导致启动面板重复显示/闪烁）。
pub(crate) fn ensure_dev_ui_server(app: &AppHandle) -> bool {
    use std::net::TcpStream;
    use std::time::Duration;

    if app.config().build.dev_url.is_none() {
        return false;
    }
    if TcpStream::connect(("127.0.0.1", 4321)).is_ok() {
        // 服务器已在运行（如 dev-run.ps1 先行拉起）：直接就绪，无需重启或兜底
        return true;
    }
    // serve-ui.mjs 位于仓库 scripts/，开发版 exe 在 dist-dev/ 下
    let Ok(exe_dir) = std::env::current_exe().and_then(|p| {
        p.parent()
            .map(|d| d.to_path_buf())
            .ok_or_else(|| std::io::Error::other("no parent"))
    }) else {
        return false;
    };
    let script = exe_dir.join("..").join("scripts").join("serve-ui.mjs");
    if !script.is_file() {
        logging::log(&format!(
            "dev-ui: 未找到 {}，无法自动拉起 UI 服务器",
            script.display()
        ));
        return false;
    }
    let args = vec![script.to_string_lossy().into_owned()];
    let spawn = processes::spawn_process(
        std::path::Path::new("node"),
        &args,
        &[],
        script.parent(),
        None,
    );
    match spawn {
        Ok(child) => {
            // 复用统一进程树守卫：应用退出时回收开发服务器，不留孤儿。
            let guard = processes::TreeGuard::from_child(&child);
            app.state::<AppState>().set_dev_ui_job(guard);
            // 等待监听就绪（最多 5s）；期间阻塞 setup——dev 工具路径，
            // 服务器冷启动通常 <1s，可接受
            for _ in 0..50 {
                if TcpStream::connect(("127.0.0.1", 4321)).is_ok() {
                    logging::log("dev-ui: 已自动启动 UI 静态服务器（4321）");
                    return true;
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            logging::log("dev-ui: UI 服务器 5s 内未就绪，自绘界面可能空白");
            false
        }
        Err(e) => {
            logging::log(&format!(
                "dev-ui: 拉起 UI 服务器失败（node 不在 PATH？）：{e}"
            ));
            false
        }
    }
}
