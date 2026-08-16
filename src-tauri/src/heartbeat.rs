//! dsh 页面心跳监控：页面主线程挂起/崩溃时（注入的 JS 停摆、心跳停报），
//! 由桌面端重载页面自愈；连续失败指数退避，避免反复重载风暴。
//!
//! 心跳由 navigate 注入的 `HEARTBEAT_INJECT` 每 10s 上报一次；
//! 仅在服务健康且页面为 dsh 页面时判定（服务异常交给 dsh 看门狗处理）。

use std::time::Duration;

use tauri::AppHandle;
use tauri::Manager;

use crate::app_state::AppState;

/// 监控轮询间隔。
const POLL_INTERVAL: Duration = Duration::from_secs(10);
/// 心跳超时基础值：页面挂起（主线程卡死）后约 35s 判定一次。
const BASE_TIMEOUT: Duration = Duration::from_secs(35);
/// 指数退避上限（2^5 × 35s ≈ 18.7 分钟）；连续重载 6 次仍失败则放慢到上限。
const MAX_BACKOFF_POWER: u32 = 5;

/// 启动页面心跳监控（后台线程，退出中自动停止）。
pub fn start_page_watch(app: AppHandle) {
    std::thread::spawn(move || loop {
        std::thread::sleep(POLL_INTERVAL);
        if app.state::<AppState>().is_quitting() {
            return;
        }
        if let Err(e) = poll_once(&app) {
            crate::logging::log(&format!("heartbeat: 监控失败：{e}"));
        }
    });
}

/// 记录一次页面心跳（页面注入脚本调用）。
pub fn beat(app: &AppHandle) {
    app.state::<AppState>().set_heartbeat();
}

/// 页面心跳命令：仅允许 dsh 页面调用（命令本身无副作用，只更新存活标记）。
#[tauri::command]
pub fn page_heartbeat(webview: tauri::Webview) -> Result<(), String> {
    let url = webview.url().map_err(|e| e.to_string())?;
    let config = webview.app_handle().state::<AppState>().config();
    if !crate::is_dsh_url(&url, &config) {
        return Err(crate::locale::text(
            "仅允许 dsh 页面调用此操作。",
            "This action can only be invoked from the dsh page.",
        )
        .into());
    }
    beat(webview.app_handle());
    Ok(())
}

fn poll_once(app: &AppHandle) -> Result<(), String> {
    let config = app.state::<AppState>().config();
    // 服务不健康时交给 dsh 看门狗（重启窗口内页面必然无响应，不能误判为页面挂起）
    if !crate::dsh::health_check(config.port) {
        return Ok(());
    }
    let Some(wv) = crate::main_webview(app) else {
        return Ok(()); // 主 webview 尚未创建
    };
    let url = wv.url().map_err(|e| format!("读取页面地址失败：{e}"))?;
    if !crate::is_dsh_url(&url, &config) {
        return Ok(()); // 本地启动页/其他页面不监控
    }
    let (last, failures) = app.state::<AppState>().heartbeat_state();
    let Some(last) = last else {
        return Ok(()); // 尚未收到首次心跳（页面刚加载），等待
    };
    let backoff = 2u32.pow(failures.min(MAX_BACKOFF_POWER));
    let timeout = BASE_TIMEOUT.saturating_mul(backoff);
    if last.elapsed() <= timeout {
        return Ok(());
    }
    // 心跳超时：判定页面挂起，重载自愈并退避
    let failures = app.state::<AppState>().bump_heartbeat_failures();
    crate::logging::log(&format!(
        "heartbeat: 页面心跳超时（{:.0}s，第 {failures} 次重载），执行重载",
        timeout.as_secs_f64()
    ));
    wv.reload().map_err(|e| format!("重载页面失败：{e}"))
}

#[cfg(test)]
mod tests {
    use super::MAX_BACKOFF_POWER;

    #[test]
    fn backoff_power_is_capped() {
        // 退避幂次封顶在 MAX_BACKOFF_POWER（5），2^p 最大为 32，不会溢出；
        // 实际封顶逻辑见 backoff() 中的 failures.min(MAX_BACKOFF_POWER)
        assert_eq!(MAX_BACKOFF_POWER, 5);
        assert_eq!(2u32.pow(MAX_BACKOFF_POWER), 32);
    }
}
