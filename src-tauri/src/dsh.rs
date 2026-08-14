//! 服务生命周期：引导主循环、看门狗自愈、健康检查、退出清理。

use std::time::{Duration, Instant};

use tauri::{AppHandle, Manager};

use crate::app_state::{AppState, BootPhase, RETRY_RX};
use crate::processes;
use crate::runtime::{self, ensure_dsh, ensure_node, start_server};
use crate::{emit_status, navigate, SPLASH_ORIGIN};

const READY_TIMEOUT: Duration = Duration::from_secs(120);
const WATCH_INTERVAL: Duration = Duration::from_secs(5);

// ---------- 引导主循环 ----------

pub fn boot_loop(app: AppHandle) {
    let rx = RETRY_RX.lock().unwrap_or_else(|e| e.into_inner()).take();
    loop {
        match boot_once(&app) {
            Ok(()) => {
                crate::logging::log("boot: ready，进入看门狗模式");
                watchdog(&app);
            }
            Err(msg) => {
                crate::logging::log(&format!("boot: 失败：{msg}"));
                let state = app.state::<AppState>();
                state.set_phase(BootPhase::Error, &msg, "");
                emit_status(&app, BootPhase::Error, &msg, "");
                // 等待用户点击“重试”。
                if let Some(rx) = &rx {
                    let _ = rx.recv();
                }
                state.set_phase(BootPhase::Starting, "正在重新启动…", "");
            }
        }
        if app.state::<AppState>().is_quitting() {
            return;
        }
    }
}

/// 一轮完整引导；成功返回 Ok(())，失败返回错误信息。
/// 全程持有生命周期锁，与托盘“重启服务”/更新流程互斥，杜绝双服务并发。
fn boot_once(app: &AppHandle) -> Result<(), String> {
    let state = app.state::<AppState>();
    let _guard = state.lifecycle_guard();

    // 并发路径（更新/手动重启）可能刚把服务拉起：直接复用，避免双实例——
    // 否则端口回退会启动第二个实例，新守卫还会把刚重启好的服务整树杀掉。
    let config = state.config();
    if state.is_updating() {
        crate::logging::log("boot: 更新流程进行中，跳过本轮引导（看门狗会持续监控）");
        return Ok(());
    }
    if health_check(config.port) {
        crate::logging::log("boot: 服务已由并发路径就绪，直接复用");
        state.set_phase(BootPhase::Ready, "已就绪", "");
        emit_status(
            app,
            BootPhase::Ready,
            "已就绪",
            &format!("http://127.0.0.1:{}", config.port),
        );
        std::thread::sleep(Duration::from_millis(320));
        navigate(app, &config.web_url());
        crate::updater::silent_check(app);
        return Ok(());
    }

    // 若上次更新被强杀/断电打断，先恢复到确定可用的旧目录。
    crate::update_txn::recover_interrupted_updates(&state.config())?;

    // 0) 端口回退：配置端口被占用时顺延到空闲端口（覆盖端口冲突场景）。
    let mut config = state.config();
    let base = config.port;
    let mut chosen = None;
    for off in 0..=50u16 {
        let Some(port) = base.checked_add(off) else {
            break;
        };
        if !health_check(port) {
            chosen = Some(port);
            break;
        }
    }
    let chosen = chosen.ok_or_else(|| format!("从端口 {base} 起连续 51 个端口均被占用"))?;
    if chosen != base {
        state.set_port(chosen);
        config.port = chosen;
        crate::logging::log(&format!("dsh: 端口 {base} 被占用，改用 {chosen}"));
        emit_status(
            app,
            BootPhase::Starting,
            &format!("端口 {base} 被占用，改用 {chosen}…"),
            "",
        );
    }

    // 1) Node 运行时
    state.set_phase(BootPhase::Starting, "正在检查 Node.js 运行时…", "");
    emit_status(app, BootPhase::Starting, "正在检查 Node.js 运行时…", "");
    let node_exe = ensure_node(app, &config)?;
    // 检测一次并缓存版本（snapshot/get_status 直接读取，避免每次 IPC spawn node）
    state.set_node_version(
        runtime::node_version(&node_exe).map(|(m, n, p)| format!("v{m}.{n}.{p}")),
    );

    // 2) dsh 包
    ensure_dsh(app, &config, &node_exe)?;

    // 首装可能耗时数分钟，启动前复检端口，避免绑定冲突被误报为“启动超时”
    if health_check(config.port) {
        let base = config.port;
        let mut next = None;
        for off in 1..=50u16 {
            let Some(p) = base.checked_add(off) else {
                break;
            };
            if !health_check(p) {
                next = Some(p);
                break;
            }
        }
        let next = next.ok_or_else(|| format!("端口 {base} 被占用，后 50 个端口均不可用"))?;
        state.set_port(next);
        config.port = next;
        crate::logging::log(&format!("dsh: 启动前端口 {base} 被占用，改用 {next}"));
        emit_status(
            app,
            BootPhase::Starting,
            &format!("端口 {base} 被占用，改用 {next}…"),
            "",
        );
    }

    // 3) 启动服务
    state.set_phase(BootPhase::StartingServer, "正在启动 dsh 服务…", "");
    emit_status(
        app,
        BootPhase::StartingServer,
        "正在启动 dsh 服务…",
        &format!("端口 {}", config.port),
    );
    let (pid, job) = start_server(app, &config, &node_exe)?;
    state.set_running(pid, job);

    // 4) 轮询就绪
    let deadline = Instant::now() + READY_TIMEOUT;
    loop {
        if health_check(config.port) {
            break;
        }
        if app.state::<AppState>().is_quitting() {
            return Err("用户退出".into());
        }
        if Instant::now() > deadline {
            processes::kill_tree(pid);
            return Err(format!(
                "dsh 服务启动超时（{} 秒内未就绪），请查看日志：{}",
                READY_TIMEOUT.as_secs(),
                config.dsh_log().display()
            ));
        }
        std::thread::sleep(Duration::from_millis(100));
    }

    // 5) 就绪，进入界面
    state.set_phase(BootPhase::Ready, "已就绪", "");
    emit_status(
        app,
        BootPhase::Ready,
        "已就绪",
        &format!("http://127.0.0.1:{}", config.port),
    );
    crate::logging::log(&format!(
        "boot: 就绪 dsh={} node={} port={}",
        runtime::installed_dsh_version(&config).unwrap_or_default(),
        runtime::current_node_version(&config).unwrap_or_default(),
        config.port
    ));
    // 给启动页 300ms 淡出动画留余量，再跳转 dsh 界面（配合 WebView 背景色，无白闪）
    std::thread::sleep(Duration::from_millis(320));
    navigate(app, &config.web_url());
    // 启动后静默检查 dsh 更新（后台线程，不阻塞；有新版才提示）
    crate::updater::silent_check(app);
    Ok(())
}

// ---------- 看门狗 ----------

/// 看门狗：服务掉线时回到启动页并自动重启（重启失败会走 Err 分支显示错误+手动重试）。
/// 更新流程进行中（updating=true）时跳过，避免打断 npm/node 安装。
fn watchdog(app: &AppHandle) {
    loop {
        std::thread::sleep(WATCH_INTERVAL);
        let state = app.state::<AppState>();
        if state.is_quitting() {
            return;
        }
        if state.is_updating() {
            continue;
        }
        let port = state.config().port;
        if !health_check(port) {
            // 复检：更新/重启可能刚把服务拉起，避免误停刚就绪的服务
            if app.state::<AppState>().is_updating() {
                continue;
            }
            // 服务已停止：清理残留进程，回启动页，短暂延迟后由外层循环自动重启。
            crate::logging::log("watchdog: dsh 服务已停止，准备自动重启");
            shutdown(app);
            navigate(app, SPLASH_ORIGIN);
            let state = app.state::<AppState>();
            state.set_phase(BootPhase::Starting, "服务已停止，正在自动重启…", "");
            emit_status(app, BootPhase::Starting, "服务已停止，正在自动重启…", "");
            std::thread::sleep(Duration::from_secs(2));
            return;
        }
    }
}

// ---------- 退出清理与健康检查 ----------

/// 退出清理：进程树守卫销毁（Windows Job / Unix 进程组）+ taskkill 兜底。
pub fn shutdown(app: &AppHandle) {
    let state = app.state::<AppState>();
    let (pid, job) = state.take_running();
    drop(job);
    if let Some(pid) = pid {
        processes::kill_tree(pid);
    }
}

/// 服务端口是否可连接（即服务是否就绪）。
pub(crate) fn health_check(port: u16) -> bool {
    use std::net::{SocketAddr, TcpStream};
    let addr: SocketAddr = format!("127.0.0.1:{port}").parse().unwrap();
    TcpStream::connect_timeout(&addr, Duration::from_millis(800)).is_ok()
}

/// 等待服务端口就绪。
pub(crate) fn wait_ready(port: u16, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if health_check(port) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    false
}
