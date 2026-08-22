//! 服务生命周期：引导主循环、看门狗自愈、健康检查、退出清理。

use std::time::{Duration, Instant};

use tauri::{AppHandle, Manager};

use crate::app_state::{AppState, BootPhase, RETRY_RX};
use crate::processes;
use crate::runtime::{self, ensure_dsh, ensure_node, start_server};
use crate::{emit_status, navigate, SPLASH_ORIGIN};

const READY_TIMEOUT: Duration = Duration::from_secs(120);
const WATCH_INTERVAL: Duration = Duration::from_secs(5);
const STARTUP_TRANSITION_TIMEOUT: Duration = Duration::from_millis(400);
/// 端口扫描窗口：Windows 的 Hyper-V/WSL 动态保留端口（excluded port range）
/// 可能连续圈定数百个端口（如 2914-3713 共 800 个），50 个远不够跳出保留块。
const PORT_SCAN_WINDOW: u16 = 2048;

// ---------- 引导主循环 ----------

pub fn boot_loop(app: AppHandle) {
    let rx = RETRY_RX.lock().unwrap_or_else(|e| e.into_inner()).take();
    loop {
        match boot_once(&app) {
            Ok(()) => {
                crate::logging::log("boot: 已就绪，进入看门狗模式");
                watchdog(&app);
                // 看门狗退出只可能因为：应用退出 / 服务停止（已置 Starting
                // 待自动重启）/ 阶段被更新或重启流程置为 Error（错误页已展示）。
                // Error 与引导失败路径一致：等待用户点击“重试”再引导——
                // 否则 retry 信号无人消费（看门狗阶段永远不会 recv）。
                if app.state::<AppState>().phase() == BootPhase::Error {
                    wait_retry(&app, rx.as_ref());
                }
            }
            Err(msg) => {
                crate::logging::log(&format!("boot: 失败：{msg}"));
                let state = app.state::<AppState>();
                state.set_phase(BootPhase::Error, &msg, "");
                emit_status(&app, BootPhase::Error, &msg, "");
                // 等待用户点击“重试”。
                wait_retry(&app, rx.as_ref());
            }
        }
        if app.state::<AppState>().is_quitting() {
            return;
        }
    }
}

/// 阻塞等待“重试”信号，随后把阶段切回 Starting（交回外层循环重新引导）。
/// 信号通道缺失时（不应发生）退化为延时自动重试，避免空转热循环。
fn wait_retry(app: &AppHandle, rx: Option<&std::sync::mpsc::Receiver<()>>) {
    if let Some(rx) = rx {
        let _ = rx.recv();
    } else {
        std::thread::sleep(Duration::from_secs(3));
    }
    app.state::<AppState>().set_phase(
        BootPhase::Starting,
        crate::locale::text("正在重新启动…", "Restarting…"),
        "",
    );
}

/// 首次使用配置未完成时停留启动页等待确认（save_onboarding 完成后补跳转）。
/// 返回 true 表示已停留：调用方应直接返回，不再 navigate。
/// 防御：等待上限 60 秒——若启动页因任何原因未显示配置面板（如 IPC 时序），
/// 自动落 onboarded 标记并放行，避免永久卡在启动页。
/// 等待解除条件：用户已保存/跳过（onboarding_done）——dev 构建下
/// 首次引导判定恒为 true，必须以用户动作为准。
fn wait_onboarding(state: &AppState) -> bool {
    if !state.onboarding_pending() {
        return false;
    }
    crate::logging::log("boot: 首次使用配置未完成，停留启动页等待确认");
    // 面板已显示：无限等待用户操作（有跳过按钮，随时可退出，无需兜底）；
    // 面板未显示（启动页 IPC 异常等）：60 秒兜底自动放行，避免卡死
    let deadline = std::time::Instant::now() + Duration::from_secs(60);
    loop {
        std::thread::sleep(Duration::from_secs(1));
        if crate::app_state::onboarding_done() {
            crate::logging::log("boot: 用户已完成首次配置，继续启动");
            return false;
        }
        if !crate::app_state::onboarding_shown() && std::time::Instant::now() >= deadline {
            // 超时兜底：自动落标记并继续，避免启动页卡死
            let config = state.config();
            let _ = crate::app_state::save_state_value(
                &config.root,
                "builtin_plugins_enabled",
                serde_json::Value::Bool(false),
            );
            let _ = crate::app_state::save_state_value(
                &config.root,
                "onboarded",
                serde_json::Value::Bool(true),
            );
            crate::logging::log("boot: 首次配置面板未显示，60 秒兜底放行");
            return false;
        }
    }
}

/// 让启动页完成淡出后立即导航；页面未响应时按短超时兜底，导航正确性不依赖动画事件。
pub(crate) fn enter_web_app(app: &AppHandle, url: &str) {
    let on_startup_page = crate::main_webview(app)
        .and_then(|webview| webview.url().ok())
        .is_some_and(|current| {
            let dev = crate::app_dev_origin(app);
            crate::is_local_app_url(&current, dev.as_ref())
        });
    let transition = on_startup_page.then(|| app.state::<AppState>().arm_startup_transition());
    let ready = crate::locale::text("已就绪", "Ready");
    emit_status(app, BootPhase::Ready, ready, "");
    if let Some(transition) = transition {
        let _ = transition.recv_timeout(STARTUP_TRANSITION_TIMEOUT);
    }
    navigate(app, url);
}

/// 一轮完整引导；成功返回 Ok(())，失败返回错误信息。
/// 全程持有生命周期锁，与托盘“重启服务”/更新流程互斥，杜绝双服务并发。
fn boot_once(app: &AppHandle) -> Result<(), String> {
    let state = app.state::<AppState>();
    let _guard = state.lifecycle_guard();

    // 并发路径（更新/手动重启）可能刚把服务拉起：直接复用，避免双实例——
    // 否则端口回退会启动第二个实例，新守卫还会把刚重启好的服务及其进程树一并终止。
    let config = state.config();
    if state.is_updating() {
        crate::logging::log("boot: 更新流程进行中，跳过本轮引导（看门狗会持续监控）");
        return Ok(());
    }
    if state.has_running_process() && health_check(config.port) {
        crate::logging::log("boot: 服务已由并发路径就绪，直接复用");
        let ready = crate::locale::text("已就绪", "Ready");
        state.set_phase(BootPhase::Ready, ready, "");
        // 先等首次配置完成，再 emit Ready：前端收到 ready 会整体淡出
        // 启动页（含首次配置面板）——若先 emit 后等待，面板会视觉消失
        // 而 boot 仍在等待用户操作
        if wait_onboarding(&state) {
            return Ok(());
        }
        enter_web_app(app, &config.web_url());
        crate::updater::silent_check(app);
        return Ok(());
    }

    // 若上次更新被强杀/断电打断，先恢复到确定可用的旧目录。
    crate::updater::transaction::recover_interrupted_updates(&state.config())?;

    // 0) 端口复用：配置端口已被一个健康的 dsh 服务占用时（同一 DSH_HOME 的
    //    另一实例——如用户自己终端里跑的 dsh）直接接入，不再改用后续端口
    //    另起第二个实例。两个实例共写同一会话日志会交错追加，造成 seq gap
    //    损坏（历史加载失败：corrupt session log）；复用即单实例单写者，
    //    且两个界面看到同一份实时数据。健康检查不通过（繁忙/非 dsh）才走
    //    后续端口。忙时重试 3 次：误判的代价是再起一个实例共写日志。
    let mut config = state.config();
    let base = config.port;
    let mut reusable = false;
    if port_is_occupied(base) {
        for attempt in 1..=3 {
            if health_check(base) {
                reusable = true;
                break;
            }
            crate::logging::log(&format!(
                "dsh: 端口 {base} 被占用但健康检查未通过（{attempt}/3），稍后重试"
            ));
            std::thread::sleep(Duration::from_millis(1000));
        }
    }
    if reusable {
        crate::logging::log(&format!(
            "dsh: 端口 {base} 已有健康的 dsh 服务，直接复用（不启动第二个实例）"
        ));
        let ready = crate::locale::text("已就绪", "Ready");
        state.set_phase(BootPhase::Ready, ready, "");
        // 先等首次配置完成再 emit Ready（同并发路径：ready 会触发启动页淡出）
        if wait_onboarding(&state) {
            return Ok(());
        }
        enter_web_app(app, &config.web_url());
        crate::updater::silent_check(app);
        return Ok(());
    }
    let mut chosen = None;
    for off in 0..=PORT_SCAN_WINDOW {
        let Some(port) = base.checked_add(off) else {
            break;
        };
        if !port_is_occupied(port) {
            chosen = Some(port);
            break;
        }
    }
    let chosen = chosen.ok_or_else(|| {
        if crate::locale::is_chinese() {
            format!("端口 {base} 及其后 {PORT_SCAN_WINDOW} 个端口均被占用或被系统保留")
        } else {
            format!(
                "Ports {base} through {} are all in use or reserved by the system",
                base.saturating_add(PORT_SCAN_WINDOW)
            )
        }
    })?;
    if chosen != base {
        state.set_port(chosen);
        config.port = chosen;
        crate::logging::log(&format!("dsh: 端口 {base} 被占用，改用 {chosen}"));
        let message = if crate::locale::is_chinese() {
            format!("端口 {base} 被占用，改用 {chosen}…")
        } else {
            format!("Port {base} is in use. Using {chosen} instead…")
        };
        emit_status(app, BootPhase::Starting, &message, "");
    }

    // 1) Node 运行时
    let checking_node =
        crate::locale::text("正在检查 Node.js 运行时…", "Checking the Node.js runtime…");
    state.set_phase(BootPhase::Starting, checking_node, "");
    emit_status(app, BootPhase::Starting, checking_node, "");
    let node = ensure_node(app, &config)?;
    state.set_node_version(Some(node.version.clone()));
    state.set_npm_version(runtime::npm_version(&config));

    // 2) dsh 包
    ensure_dsh(app, &config, &node.executable)?;

    // 首装可能耗时数分钟，启动前复检端口，避免绑定冲突被误报为“启动超时”
    if port_is_occupied(config.port) {
        let base = config.port;
        let mut next = None;
        for off in 1..=PORT_SCAN_WINDOW {
            let Some(p) = base.checked_add(off) else {
                break;
            };
            if !port_is_occupied(p) {
                next = Some(p);
                break;
            }
        }
        let next = next.ok_or_else(|| {
            if crate::locale::is_chinese() {
                format!("端口 {base} 被占用，后 {PORT_SCAN_WINDOW} 个端口均不可用")
            } else {
                format!("Port {base} and the next {PORT_SCAN_WINDOW} ports are unavailable")
            }
        })?;
        state.set_port(next);
        config.port = next;
        crate::logging::log(&format!("dsh: 启动前端口 {base} 被占用，改用 {next}"));
        let message = if crate::locale::is_chinese() {
            format!("端口 {base} 被占用，改用 {next}…")
        } else {
            format!("Port {base} is in use. Using {next} instead…")
        };
        emit_status(app, BootPhase::Starting, &message, "");
    }

    // 3) 启动服务
    let starting_server = crate::locale::text("正在启动 dsh 服务…", "Starting the dsh service…");
    state.set_phase(BootPhase::StartingServer, starting_server, "");
    emit_status(app, BootPhase::StartingServer, starting_server, "");
    let (pid, job) = start_server(app, &config, &node.executable)?;
    state.set_running(pid, job);

    // 4) 轮询就绪
    let deadline = Instant::now() + READY_TIMEOUT;
    loop {
        if health_check(config.port) {
            break;
        }
        if app.state::<AppState>().is_quitting() {
            return Err(crate::locale::text("应用已退出", "The app has quit").into());
        }
        if Instant::now() > deadline {
            processes::kill_tree(pid);
            return Err(if crate::locale::is_chinese() {
                format!(
                    "dsh 服务启动超时（{} 秒内未就绪），请查看日志：{}",
                    READY_TIMEOUT.as_secs(),
                    config.dsh_log().display()
                )
            } else {
                format!(
                    "The dsh service did not become ready within {} seconds. See the log: {}",
                    READY_TIMEOUT.as_secs(),
                    config.dsh_log().display()
                )
            });
        }
        std::thread::sleep(Duration::from_millis(50));
    }

    // 5) 就绪，进入界面
    let ready = crate::locale::text("已就绪", "Ready");
    state.set_phase(BootPhase::Ready, ready, "");
    crate::logging::log(&format!(
        "boot: 就绪 dsh={} node={} port={}",
        runtime::installed_dsh_version(&config).unwrap_or_default(),
        state.node_version().unwrap_or_default(),
        config.port
    ));
    // 先等首次配置完成再 emit Ready：ready 会触发启动页整体淡出（含面板），
    // 若先 emit 后等待，面板会视觉消失而 boot 仍在等待用户操作
    if wait_onboarding(&state) {
        return Ok(());
    }
    enter_web_app(app, &config.web_url());
    // 启动后静默检查 dsh 更新（后台线程，不阻塞；有新版才提示）
    crate::updater::silent_check(app);
    Ok(())
}

// ---------- 看门狗 ----------

/// 看门狗：服务掉线时回到启动页并自动重启（重启失败会走 Err 分支显示错误+手动重试）。
/// 更新流程进行中（updating=true）时跳过，避免打断 npm/node 安装。
fn watchdog(app: &AppHandle) {
    let mut failures = 0u32;
    loop {
        std::thread::sleep(WATCH_INTERVAL);
        let state = app.state::<AppState>();
        if state.is_quitting() {
            return;
        }
        if state.is_updating() {
            failures = 0;
            continue;
        }
        if state.phase() != BootPhase::Ready {
            // 阶段离开 Ready：更新/重启失败会把阶段置为 Error（错误页已展示），
            // 退出看门狗交回 boot_loop 等待“重试”——在此 continue 会让
            // retry 信号永远无人消费
            if state.phase() == BootPhase::Error {
                return;
            }
            failures = 0;
            continue;
        }
        let port = state.config().port;
        if !health_check(port) {
            // 连续两次失败才重启：dsh 思考高峰时响应可能短暂超时，
            // 单次失败不足以判定服务死亡，避免误杀正在思考的会话
            failures += 1;
            crate::logging::log(&format!("watchdog: 健康检查失败 {failures}/2"));
            if failures < 2 {
                continue;
            }
            // 复检：更新/重启可能刚把服务拉起，避免误停刚就绪的服务
            let state = app.state::<AppState>();
            if state.is_updating() || state.phase() != BootPhase::Ready {
                failures = 0;
                continue;
            }
            // 服务已停止：清理残留进程，回启动页，短暂延迟后由外层循环自动重启。
            crate::logging::log("watchdog: dsh 服务已停止，准备自动重启");
            shutdown(app);
            navigate(app, SPLASH_ORIGIN);
            let state = app.state::<AppState>();
            let restarting = crate::locale::text(
                "服务已停止，正在自动重启…",
                "The service stopped. Restarting automatically…",
            );
            state.set_phase(BootPhase::Starting, restarting, "");
            emit_status(app, BootPhase::Starting, restarting, "");
            std::thread::sleep(Duration::from_secs(2));
            return;
        }
        failures = 0;
    }
}

// ---------- 退出清理与健康检查 ----------

/// 退出清理：进程树守卫销毁（Windows Job / Unix 进程组）为主；仅当无
/// 守卫时（Job 创建失败的降级路径）才按 PID 树兜底——守卫回收后 PID 可能
/// 已被系统复用，再 taskkill 有误杀无关进程的风险。
pub fn shutdown(app: &AppHandle) {
    let state = app.state::<AppState>();
    let (pid, job) = state.take_running();
    if job.is_some() {
        drop(job);
    } else if let Some(pid) = pid {
        processes::kill_tree(pid);
    }
}

/// 端口是否已被任意进程占用，仅用于选择可用监听端口。
pub(crate) fn port_is_occupied(port: u16) -> bool {
    use std::net::{SocketAddr, TcpListener, TcpStream};
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    // 1) 已有监听者：connect 成功即占用。
    if TcpStream::connect_timeout(&addr, Duration::from_millis(300)).is_ok() {
        return true;
    }
    // 2) Windows 保留端口（excluded port range，Hyper-V/WSL 动态圈定）等
    //    导致 bind EACCES 的端口：无人监听但无法监听，同样视为不可用，
    //    否则 dsh 启动会在此端口 listen EACCES 后超时（connect 探测看不到）。
    TcpListener::bind(addr).is_err()
}

/// 验证端口上的服务确实是 dsh Web UI，而不只是任意 TCP 监听者。
pub(crate) fn health_check(port: u16) -> bool {
    use std::io::{Read, Write};
    use std::net::{SocketAddr, TcpStream};

    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    let Ok(mut stream) = TcpStream::connect_timeout(&addr, Duration::from_millis(800)) else {
        return false;
    };
    // 读写超时放宽到 2s：dsh 思考高峰时响应可能短暂停滞，
    // 过早判定失败会被看门狗误杀正在进行的会话
    let timeout = Some(Duration::from_millis(2000));
    let _ = stream.set_read_timeout(timeout);
    let _ = stream.set_write_timeout(timeout);
    let request = format!(
        "GET / HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nAccept: text/html\r\nConnection: close\r\n\r\n"
    );
    if stream.write_all(request.as_bytes()).is_err() {
        return false;
    }

    let mut response = Vec::with_capacity(8192);
    let mut chunk = [0u8; 4096];
    while response.len() < 64 * 1024 {
        match stream.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => response.extend_from_slice(&chunk[..n]),
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) =>
            {
                break;
            }
            Err(_) => return false,
        }
    }
    is_dsh_response(&response)
}

fn is_dsh_response(response: &[u8]) -> bool {
    let success = response.starts_with(b"HTTP/1.1 200 ") || response.starts_with(b"HTTP/1.0 200 ");
    const MARKER: &[u8] = b"<title>DeepSeek Harness</title>";
    success
        && response
            .windows(MARKER.len())
            .any(|window| window == MARKER)
}

/// 等待服务端口就绪。
pub(crate) fn wait_ready(port: u16, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if health_check(port) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    false
}

#[cfg(test)]
mod tests {
    use super::is_dsh_response;

    #[test]
    fn health_response_requires_dsh_marker() {
        assert!(is_dsh_response(
            b"HTTP/1.1 200 OK\r\n\r\n<html><title>DeepSeek Harness</title></html>"
        ));
        assert!(!is_dsh_response(
            b"HTTP/1.1 200 OK\r\n\r\n<html><title>Another service</title></html>"
        ));
        assert!(!is_dsh_response(
            b"HTTP/1.1 500 Internal Server Error\r\n\r\n<html><title>DeepSeek Harness</title></html>"
        ));
    }
}
