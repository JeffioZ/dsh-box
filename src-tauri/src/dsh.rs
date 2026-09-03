//! 服务生命周期：引导主循环、看门狗自愈、健康检查、退出清理。

use std::io::{Read, Seek, SeekFrom};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use tauri::{AppHandle, Manager};

use crate::app_state::{
    AppState, BootPhase, ExternalServiceCandidate, InstallAction, ServiceOwnership, RETRY_RX,
};
use crate::processes;
use crate::runtime::{self, ensure_dsh, ensure_node, start_server};
use crate::{emit_status, navigate, navigate_to_splash};

const READY_TIMEOUT: Duration = Duration::from_secs(120);
const WATCH_INTERVAL: Duration = Duration::from_secs(5);
const STARTUP_TRANSITION_TIMEOUT: Duration = Duration::from_millis(400);
const DSH_OFFICIAL_PORT: u16 = 3080;
const LAST_MANAGED_PORT_KEY: &str = "last_managed_port";
const EXTERNAL_SERVICE_PREFERENCE_KEY: &str = "external_service_preference";
static SERVICE_PROBE_AGENT: OnceLock<ureq::Agent> = OnceLock::new();

#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
struct ExternalServicePreference {
    reuse: bool,
    service: ExternalServiceCandidate,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PortAvailability {
    Free,
    Listening,
    Unbindable,
}

#[derive(Debug, PartialEq, Eq)]
enum BootOutcome {
    Ready,
    Cancelled,
    Failed(String),
}

// ---------- 引导主循环 ----------

pub fn boot_loop(app: AppHandle) {
    let rx = RETRY_RX.lock().unwrap_or_else(|e| e.into_inner()).take();
    loop {
        match boot_once(&app) {
            BootOutcome::Ready => {
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
            BootOutcome::Cancelled => {
                let message = crate::locale::text("安装已取消", "Installation cancelled");
                crate::logging::log("boot: 用户取消安装，等待重新安装");
                let state = app.state::<AppState>();
                state.set_phase(BootPhase::Cancelled, message, "");
                emit_status(&app, BootPhase::Cancelled, message, "");
                wait_retry(&app, rx.as_ref());
            }
            BootOutcome::Failed(msg) => {
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

/// 首次使用配置未完成时停留启动页等待确认。保存返回后，调用方必须
/// 重新检查阶段：配置保存可能已把后续导航交给服务重启流程。
/// 防御：等待上限 60 秒——若启动页因任何原因未显示配置面板（如 IPC 时序），
/// 自动落 onboarded 标记并放行，避免永久卡在启动页。
/// 等待解除条件：用户已保存（onboarding_done）。开发构建每个进程仍展示
/// 一次，但同一进程内的服务重启不会再次进入引导。
/// 到期且面板未显示时的兜底放行期限。
const ONBOARDING_FALLBACK_SECS: Duration = Duration::from_secs(60);

/// `wait_onboarding` 单次轮询的决策（纯逻辑，供表驱动测试）。
#[derive(Debug, PartialEq, Eq)]
enum OnboardingWaitAction {
    /// 用户已完成配置，继续启动。
    Proceed,
    /// 继续等待（面板已显示则无限期，未到期则等下一轮）。
    Wait,
    /// 兜底放行：到期、面板未显示且探活也不可见。
    ForceOnboard,
}

/// `done`=用户已保存；`shown`=启动页已回报面板渲染；`deadline_passed`=已过
/// 兜底期限；`probe_visible`=到期探活确认面板可见（未探活传 false）。
/// 面板已显示时永不放行——用户可能正在填写。
fn onboarding_wait_decision(
    done: bool,
    shown: bool,
    deadline_passed: bool,
    probe_visible: bool,
) -> OnboardingWaitAction {
    if done {
        OnboardingWaitAction::Proceed
    } else if shown || !deadline_passed || probe_visible {
        OnboardingWaitAction::Wait
    } else {
        OnboardingWaitAction::ForceOnboard
    }
}

fn wait_onboarding(app: &AppHandle, state: &AppState) {
    if !state.onboarding_pending() {
        return;
    }
    crate::logging::log("boot: 首次使用配置未完成，停留启动页等待确认");
    // 面板已显示：无限等待用户明确完成；
    // 面板未显示（启动页 IPC 异常等）：60 秒后先主动探活一次，确认面板
    // 确实未渲染才兜底放行，避免把「回报迟到但面板已显示」误判为异常而
    // 跳过用户正在填写的首次设置。
    let deadline = std::time::Instant::now() + ONBOARDING_FALLBACK_SECS;
    let mut probed = false;
    let mut probe_visible = false;
    loop {
        std::thread::sleep(Duration::from_secs(1));
        if !probed && !crate::app_state::onboarding_shown() && std::time::Instant::now() >= deadline
        {
            // 到期且尚未收到面板显示回报：先探查主 WebView 里面板是否可见。
            // Tauri 的 eval 不返回脚本值，探活函数通过带代次的 IPC 显式回报；
            // 失败或页面不在启动页时按「未显示」处理，不引入新的卡死路径。
            probed = true;
            probe_visible = probe_onboarding_visibility(app);
            if probe_visible {
                crate::logging::log("boot: 首次配置面板探活可见，转为无限等待");
            }
        }
        match onboarding_wait_decision(
            crate::app_state::onboarding_done(),
            crate::app_state::onboarding_shown(),
            std::time::Instant::now() >= deadline,
            probe_visible,
        ) {
            OnboardingWaitAction::Proceed => {
                crate::logging::log("boot: 用户已完成首次配置，继续启动");
                return;
            }
            OnboardingWaitAction::Wait => continue,
            OnboardingWaitAction::ForceOnboard => {
                // 超时兜底：自动落标记并继续，避免启动页卡死
                let config = state.config();
                let _ = crate::app_state::save_state_value(
                    &config.root,
                    "builtin_plugins_enabled",
                    serde_json::Value::Bool(false),
                );
                let _ = crate::app_state::save_state_value(
                    &config.root,
                    "local_onboarding_deferred",
                    serde_json::Value::Bool(false),
                );
                let _ = crate::app_state::save_state_value(
                    &config.root,
                    "onboarded",
                    serde_json::Value::Bool(true),
                );
                crate::app_state::mark_onboarding_done();
                crate::logging::log("boot: 首次配置面板未显示，60 秒兜底放行");
                return;
            }
        }
    }
}

/// 到期主动探活：仅当主 WebView 当前停在本地启动页、且页内探活函数确认
/// 面板可见（并返回当前探测代次的 ACK）时返回 true。eval 或查询失败、页面
/// 已离开启动页等一律返回 false，交回放行兜底处理。
fn probe_onboarding_visibility(app: &AppHandle) -> bool {
    let Some(webview) = crate::main_webview(app) else {
        crate::logging::log("boot: 探活失败（主 WebView 不存在），按未显示放行");
        return false;
    };
    let on_startup_page = webview.url().ok().is_some_and(|url| {
        let dev = crate::app_dev_origin(app);
        crate::is_local_app_url(&url, dev.as_ref())
    });
    if !on_startup_page {
        crate::logging::log("boot: 探活跳过（主 WebView 不在本地启动页），按未显示放行");
        return false;
    }
    let generation = crate::app_state::begin_onboarding_probe();
    let script =
        format!("window.__dshdOnboardingVisible && window.__dshdOnboardingVisible({generation})");
    if webview.eval(&script).is_err() {
        crate::logging::log("boot: 探活 eval 失败，按未显示放行");
        return false;
    }
    match crate::app_state::wait_onboarding_probe(generation, Duration::from_secs(3)) {
        Some(visible) => visible,
        None => {
            crate::logging::log("boot: 首次配置面板探活 3 秒内未确认，按未显示放行");
            false
        }
    }
}

/// 首次配置可能触发凭据或插件重启。此时加载页已显示对应忙碌状态，当前
/// boot 必须立即释放生命周期锁，让统一重启协调器继续，不能先进入 dsh。
fn onboarding_handoff_pending(state: &AppState) -> bool {
    let phase = state.phase();
    let ownership = state.service_ownership();
    let deferred = crate::plugins::deferred_restart_pending();
    let handoff = onboarding_handoff_decision(phase, ownership, deferred);
    if handoff {
        if phase != BootPhase::Ready {
            crate::logging::log("boot: 首次配置后的界面切换已由其他生命周期流程接管");
        } else {
            crate::logging::log("boot: 预置插件待应用，保持启动页并交给统一重启流程");
        }
    }
    handoff
}

/// `onboarding_handoff_pending` 的纯决策（供表驱动测试）：
/// 阶段已非 Ready（其他生命周期流程接管）或托管服务有待应用的重启。
fn onboarding_handoff_decision(
    phase: BootPhase,
    ownership: ServiceOwnership,
    deferred_restart_pending: bool,
) -> bool {
    phase != BootPhase::Ready
        || (ownership == ServiceOwnership::Managed && deferred_restart_pending)
}

/// 首次本地配置的统一收尾。无论服务是本轮启动还是由并发重启路径复用，
/// 都必须在进入 dsh 前完成插件引导，并把设置与插件变更合并为一次重启。
fn finish_managed_onboarding(
    app: &AppHandle,
    state: &AppState,
    config: &crate::app_state::Config,
    was_onboarding: bool,
) -> Result<(), String> {
    if !was_onboarding || state.onboarding_pending() {
        return Ok(());
    }
    let ready = crate::locale::text("已就绪", "Ready");
    let plugin_work_pending = crate::plugins::bootstrap_work_pending(config);
    if plugin_work_pending {
        let message = crate::locale::text("正在安装内置插件…", "Installing built-in plugins…");
        state.set_phase(BootPhase::Starting, message, "");
        emit_status(app, BootPhase::Starting, message, "");
    }
    let plugins_changed = crate::plugins::bootstrap_once_blocking(app, config);
    let settings_restart = state.take_onboarding_restart_required();
    if plugins_changed || settings_restart {
        crate::logging::log("boot: 首次设置与插件变更将通过一次服务重启生效");
        if let Err(e) = crate::updater::restart_service_locked(app) {
            crate::logging::log(&format!("boot: 首次设置应用重启失败：{e}"));
            return Err(e);
        }
    } else if plugin_work_pending {
        // 安装尝试可能因退避或环境错误未产生变更；恢复 Ready 后照常进入，
        // 后台维护会按既有退避策略重试。
        state.set_phase(BootPhase::Ready, ready, "");
        emit_status(app, BootPhase::Ready, ready, "");
    }
    Ok(())
}

/// 让启动页完成淡出后立即导航；页面未响应时按短超时兜底，导航正确性不依赖动画事件。
pub(crate) fn enter_web_app(app: &AppHandle, url: &str) {
    enter_web_app_inner(app, url, false);
}

/// 服务重启后的页面必须重新导航，即使 WebView 的地址栏仍保留同源旧 URL；
/// 旧文档可能已经断线或停在浏览器错误状态，不能用 origin 判断其可复用。
pub(crate) fn reenter_web_app(app: &AppHandle, url: &str) {
    enter_web_app_inner(app, url, true);
}

fn enter_web_app_inner(app: &AppHandle, url: &str, force: bool) {
    // 幂等防护：已在目标 dsh 页面（scheme/host/port 一致，忽略路径差异）时
    // 跳过导航，避免重复 navigate 打断已加载的 dsh 造成白屏重载。
    // 调用方须自行保证 Ready 状态已就绪（本函数跳过后不再 emit）。
    let config = app.state::<AppState>().config();
    let already_on_dsh = crate::main_webview(app)
        .and_then(|webview| webview.url().ok())
        .is_some_and(|current| crate::is_dsh_url(&current, &config));
    if already_on_dsh && !force {
        return;
    }
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

fn boot_once(app: &AppHandle) -> BootOutcome {
    let state = app.state::<AppState>();
    state.begin_boot_attempt();
    let result = boot_inner(app);
    classify_boot_result(state.install_action(), result)
}

fn classify_boot_result(action: InstallAction, result: Result<(), String>) -> BootOutcome {
    match (action, result) {
        (InstallAction::Cancel, _) => BootOutcome::Cancelled,
        (InstallAction::None, Ok(())) => BootOutcome::Ready,
        (InstallAction::None, Err(message)) => BootOutcome::Failed(message),
    }
}

/// 一轮完整引导；安装取消只在边界处转成业务结果，内部下载函数仍可保留具体错误。
/// 全程持有生命周期锁，与托盘“重启服务”/更新流程互斥，杜绝双服务并发。
fn boot_inner(app: &AppHandle) -> Result<(), String> {
    let state = app.state::<AppState>();
    let _guard = state.lifecycle_guard();

    // 并发路径（更新/手动重启）可能刚把服务拉起：直接复用，避免双实例——
    // 否则端口回退会启动第二个实例，新守卫还会把刚重启好的服务及其进程树一并终止。
    let config = state.config();
    if state.is_updating() {
        crate::logging::log("boot: 更新流程进行中，跳过本轮引导（看门狗会持续监控）");
        return Ok(());
    }
    if state.has_running_process() && state.managed_process_exit()?.is_none() && {
        // 端口与 token 都取新鲜快照：并发路径（更新重启等）可能经
        // `--port 0` 换过端口，函数开头的 config 快照会指向旧端口
        let fresh = state.config();
        health_check(fresh.port, fresh.auth_token.as_deref())
    } {
        crate::logging::log("boot: 服务已由并发路径就绪，直接复用");
        let ready = crate::locale::text("已就绪", "Ready");
        state.set_phase(BootPhase::Ready, ready, "");
        // 首次设置的完成按钮以真实 Ready 为门控；面板可见时前端只更新
        // 卡片而不淡出，保存后 enter_web_app 再完成统一过渡。
        let was_onboarding = state.onboarding_pending();
        if was_onboarding {
            emit_status(app, BootPhase::Ready, ready, "");
        }
        wait_onboarding(app, &state);
        if onboarding_handoff_pending(&state) {
            return Ok(());
        }
        finish_managed_onboarding(app, &state, &config, was_onboarding)?;
        enter_web_app(app, &state.config().web_page_url());
        crate::plugins::start_market_bootstrap(app.clone());
        crate::updater::silent_check(app);
        return Ok(());
    }

    if state.has_running_process() {
        crate::logging::log("boot: 清理未就绪的托管服务进程");
        shutdown(app);
    }

    // 若上次更新被强杀/断电打断，先恢复到确定可用的旧目录。
    crate::updater::transaction::recover_interrupted_updates(&state.config())?;

    // 0) 发现官方部署或用户自行启动的 dsh。HTML 标记只证明页面外观，
    // host.describe 才证明它实现官方 RPC；上游目前不暴露 DSH_HOME/实例 ID，
    // 因此首次接入必须让用户明确选择，不能猜测为本应用自己的服务。
    let mut config = state.config();
    if let Some(external) = choose_external_service(app, &config)? {
        config.port = external.port;
        state.set_external_service(external.clone());
        crate::logging::log(&format!(
            "dsh: 接入外部服务 port={} cwd={}（退出、重启和更新均不接管）",
            external.port, external.cwd
        ));
        let ready = crate::locale::text("已就绪", "Ready");
        state.set_phase(BootPhase::Ready, ready, "");
        if state.onboarding_pending() {
            emit_status(app, BootPhase::Ready, ready, "");
        }
        wait_onboarding(app, &state);
        if onboarding_handoff_pending(&state) {
            return Ok(());
        }
        enter_web_app(app, &state.config().web_page_url());
        return Ok(());
    }
    state.clear_service_ownership();

    // 1) Node 运行时
    let checking_node =
        crate::locale::text("正在检查 Node.js 运行时…", "Checking the Node.js runtime…");
    state.set_phase(BootPhase::Starting, checking_node, "");
    emit_status(app, BootPhase::Starting, checking_node, "");
    let node = ensure_node(app, &config)?;
    if state.install_cancelled() {
        return Err("installation interrupted".into());
    }
    state.set_node_version(Some(node.version.clone()));
    state.set_npm_version(runtime::npm_version(&config));

    // 2) dsh 包
    ensure_dsh(app, &config, &node.executable)?;
    if state.install_cancelled() {
        return Err("installation interrupted".into());
    }

    // 3) 启动服务。只尝试“上次成功端口 → 用户首选端口”；两者都不可用时
    // 直接传 --port 0 让操作系统原子分配，避免大范围顺序扫描与 TOCTOU 竞态。
    let requested_port = choose_managed_port(&config, state.managed_port_preference());
    if requested_port == 0 {
        let message = crate::locale::text(
            "首选端口不可用，正在自动选择可用端口…",
            "The preferred port is unavailable. Choosing a free port…",
        );
        emit_status(app, BootPhase::Starting, message, "");
    } else {
        config.port = requested_port;
        state.set_port(requested_port);
    }
    let starting_server = crate::locale::text("正在启动 dsh 服务…", "Starting the dsh service…");
    state.set_phase(BootPhase::StartingServer, starting_server, "");
    emit_status(app, BootPhase::StartingServer, starting_server, "");
    config.port = requested_port;
    let actual_port = launch_managed(app, &mut config, &node.executable)?;
    config.port = actual_port;

    // 5) 就绪，进入界面
    let ready = crate::locale::text("已就绪", "Ready");
    state.set_phase(BootPhase::Ready, ready, "");
    crate::logging::log(&format!(
        "boot: 就绪 dsh={} node={} port={}",
        runtime::installed_dsh_version(&config).unwrap_or_default(),
        state.node_version().unwrap_or_default(),
        config.port
    ));
    if state.onboarding_pending() {
        emit_status(app, BootPhase::Ready, ready, "");
    }
    let was_onboarding = state.onboarding_pending();
    wait_onboarding(app, &state);
    if onboarding_handoff_pending(&state) {
        return Ok(());
    }
    finish_managed_onboarding(app, &state, &config, was_onboarding)?;
    enter_web_app(app, &state.config().web_page_url());
    // 初始托管启动时后台维护线程已在等待；从外部服务切回本地时，原线程
    // 已按归属边界退出，这里负责重新启动且内部有单实例门控。
    crate::plugins::start_market_bootstrap(app.clone());
    // 启动后静默检查 dsh 更新（后台线程，不阻塞；有新版才提示）
    crate::updater::silent_check(app);
    Ok(())
}

fn choose_managed_port(config: &crate::app_state::Config, preferred: u16) -> u16 {
    let last = crate::app_state::load_state_value(&config.root, LAST_MANAGED_PORT_KEY)
        .and_then(|value| value.as_u64())
        .and_then(|value| u16::try_from(value).ok())
        .filter(|port| *port != 0);
    select_managed_port(last, preferred, port_availability)
}

fn select_managed_port(
    last: Option<u16>,
    preferred: u16,
    mut availability: impl FnMut(u16) -> PortAvailability,
) -> u16 {
    let mut candidates = Vec::with_capacity(2);
    if let Some(port) = last {
        candidates.push(port);
    }
    if preferred != 0 && !candidates.contains(&preferred) {
        candidates.push(preferred);
    }
    candidates
        .into_iter()
        .find(|port| availability(*port) == PortAvailability::Free)
        .unwrap_or(0)
}

fn external_preference(config: &crate::app_state::Config) -> Option<ExternalServicePreference> {
    crate::app_state::load_state_value(&config.root, EXTERNAL_SERVICE_PREFERENCE_KEY)
        .and_then(|value| serde_json::from_value(value).ok())
}

fn save_external_preference(
    config: &crate::app_state::Config,
    reuse: bool,
    service: &ExternalServiceCandidate,
) -> Result<(), String> {
    crate::app_state::save_state_value(
        &config.root,
        EXTERNAL_SERVICE_PREFERENCE_KEY,
        serde_json::to_value(ExternalServicePreference {
            reuse,
            service: service.clone(),
        })
        .map_err(|e| e.to_string())?,
    )
}

fn choose_external_service(
    app: &AppHandle,
    config: &crate::app_state::Config,
) -> Result<Option<ExternalServiceCandidate>, String> {
    let state = app.state::<AppState>();
    let preference = external_preference(config);

    // 用户已明确选择接入时，服务消失不应静默切换到另一套数据目录；保留
    // 外部归属并给出可恢复入口，避免同一窗口悄然展示另一套会话。
    if let Some(saved) = preference.as_ref().filter(|item| item.reuse) {
        match describe_dsh(saved.service.port) {
            Some(current) if same_external_identity(&current, &saved.service) => {
                // 版本升级不改变实例身份；顺手刷新展示信息，避免仅因正常升级
                // 下次又被当成陌生服务。
                if current != saved.service {
                    save_external_preference(config, true, &current)?;
                }
                return Ok(Some(current));
            }
            None => {
                state.set_external_disconnected(saved.service.clone());
                return Err(crate::locale::owned(
                    format!(
                        "外部 dsh 服务（端口 {}）当前不可用。可重试连接，或改用 DSHBox 本地服务。",
                        saved.service.port
                    ),
                    format!(
                        "The external dsh service on port {} is unavailable. Try reconnecting, or switch to DSHBox's local service.",
                        saved.service.port
                    ),
                ));
            }
            Some(_) => {
                crate::logging::log("dsh: 已记住的外部服务身份发生变化，重新询问用户");
            }
        }
    }

    let preferred = state.managed_port_preference();
    let mut ports = Vec::with_capacity(3);
    if let Some(port) = preference
        .as_ref()
        .filter(|saved| saved.reuse)
        .map(|saved| saved.service.port)
    {
        ports.push(port);
    }
    for port in [preferred, DSH_OFFICIAL_PORT] {
        if !ports.contains(&port) {
            ports.push(port);
        }
    }
    for port in ports {
        let Some(candidate) = describe_dsh(port) else {
            continue;
        };
        if preference
            .as_ref()
            .is_some_and(|saved| !saved.reuse && same_external_identity(&saved.service, &candidate))
        {
            crate::logging::log(&format!("dsh: 用户已选择不接入端口 {port} 的同一外部服务"));
            continue;
        }

        state.set_service_candidate(Some(candidate.clone()));
        let message =
            crate::locale::text("发现正在运行的 dsh 服务", "A running dsh service was found");
        state.set_phase(BootPhase::ServiceChoice, message, "");
        emit_status(app, BootPhase::ServiceChoice, message, "");
        let generation = state.snapshot().install_generation;
        let reuse = state.wait_service_choice(generation)?;
        save_external_preference(config, reuse, &candidate)?;
        if reuse {
            if state.onboarding_pending() {
                // 首次设置里的凭据/插件只适用于 DSHBox 托管的 DSH_HOME。
                // 用户选择外部服务后直接完成引导，避免“保存成功但外部不生效”。
                crate::app_state::save_state_value(
                    &config.root,
                    "builtin_plugins_enabled",
                    serde_json::Value::Bool(false),
                )?;
                crate::app_state::save_state_value(
                    &config.root,
                    "local_onboarding_deferred",
                    serde_json::Value::Bool(true),
                )?;
                crate::app_state::save_state_value(
                    &config.root,
                    "onboarded",
                    serde_json::Value::Bool(true),
                )?;
                crate::app_state::mark_onboarding_done();
            }
            return Ok(Some(candidate));
        }
        state.set_service_candidate(None);
        state.clear_service_ownership();
        return Ok(None);
    }
    Ok(None)
}

/// 官方页面标记 + host.describe 双重校验。上游当前没有暴露 DSH_HOME 或
/// instanceId，cwd/home 只能用于“是否还是同一候选”的稳定指纹，不能据此
/// 宣称两套进程共享数据目录。
pub(crate) fn describe_dsh(port: u16) -> Option<ExternalServiceCandidate> {
    // 外部服务持有自己的 token（打印在它的控制台），DSHBox 无从获取，
    // 只能做无凭据探测；新版 dsh 外部服务会因此探测失败（已知限制）。
    if !health_check(port, None) {
        return None;
    }
    let url = format!("http://127.0.0.1:{port}/api/host.describe");
    let response = service_probe_agent()
        .post(&url)
        .send_json(serde_json::json!({
            "type": "client-request",
            "rpcId": "dshbox-service-probe",
            "method": "host.describe",
            "payload": {}
        }))
        .ok()?;
    let json: serde_json::Value = response.into_body().read_json().ok()?;
    parse_external_description(port, &json)
}

fn parse_external_description(
    port: u16,
    json: &serde_json::Value,
) -> Option<ExternalServiceCandidate> {
    let result = json.get("result")?;
    if !result.get("ok")?.as_bool()? {
        return None;
    }
    let value = result.get("value")?;
    Some(ExternalServiceCandidate {
        port,
        version: value.get("version")?.as_str()?.to_string(),
        cwd: value.get("cwd")?.as_str()?.to_string(),
        home: value.get("home")?.as_str()?.to_string(),
    })
}

fn same_external_identity(
    left: &ExternalServiceCandidate,
    right: &ExternalServiceCandidate,
) -> bool {
    left.port == right.port && left.cwd == right.cwd && left.home == right.home
}

fn service_probe_agent() -> &'static ureq::Agent {
    SERVICE_PROBE_AGENT.get_or_init(|| {
        ureq::Agent::config_builder()
            .timeout_connect(Some(Duration::from_millis(800)))
            .timeout_recv_response(Some(Duration::from_secs(2)))
            .timeout_recv_body(Some(Duration::from_secs(2)))
            .build()
            .new_agent()
    })
}

pub(crate) fn launch_managed(
    app: &AppHandle,
    config: &mut crate::app_state::Config,
    node_exe: &std::path::Path,
) -> Result<u16, String> {
    let requested_port = config.port;
    let actual_port = start_and_wait_managed(app, config, node_exe).or_else(|error| {
        if requested_port != 0
            && (is_bind_failure(&error)
                || port_availability(requested_port) != PortAvailability::Free)
        {
            crate::logging::log(&format!(
                "dsh: 固定端口 {requested_port} 启动时发生绑定竞争，改由系统分配"
            ));
            shutdown(app);
            config.port = 0;
            start_and_wait_managed(app, config, node_exe)
        } else {
            Err(error)
        }
    })?;
    config.port = actual_port;
    let state = app.state::<AppState>();
    state.set_port(actual_port);
    let _ = crate::app_state::save_state_value(
        &config.root,
        LAST_MANAGED_PORT_KEY,
        serde_json::json!(actual_port),
    );
    Ok(actual_port)
}

fn start_and_wait_managed(
    app: &AppHandle,
    config: &crate::app_state::Config,
    node_exe: &std::path::Path,
) -> Result<u16, String> {
    start_and_wait_managed_inner(app, config, node_exe, true)
}

fn start_and_wait_managed_inner(
    app: &AppHandle,
    config: &crate::app_state::Config,
    node_exe: &std::path::Path,
    allow_install_recovery: bool,
) -> Result<u16, String> {
    let state = app.state::<AppState>();
    let started = start_server(app, config, node_exe)?;
    let log_offset = started.log_offset;
    state.set_running(started.child, started.guard);
    let mut actual_port = (config.port != 0).then_some(config.port);
    let mut auth_token: Option<String> = None;
    let deadline = Instant::now() + READY_TIMEOUT;
    loop {
        // token 行可能晚于端口行打印：拿到 token 前每轮都读日志段；
        // token 是进程级常量，取到后不再读（读的是本次启动的输出段，
        // 不会混入上一进程的旧值）
        if actual_port.is_none() || auth_token.is_none() {
            let log = read_log_since(&config.dsh_log(), log_offset);
            if actual_port.is_none() {
                actual_port = parse_server_port(&log);
                if let Some(port) = actual_port {
                    state.set_port(port);
                    crate::logging::log(&format!("dsh: 操作系统分配端口 {port}"));
                }
            }
            if auth_token.is_none() {
                auth_token = parse_server_token(&log);
            }
        }
        if let Some(port) = actual_port {
            if health_check(port, auth_token.as_deref()) {
                // 供后续看门狗/心跳/重启与导航使用（与 set_port 同步的伴生状态）
                state.set_auth_token(auth_token.clone());
                crate::plugins::clear_resolved_install_marker(config);
                return Ok(port);
            }
        }
        if let Some(status) = state.managed_process_exit()? {
            let log = read_log_since(&config.dsh_log(), log_offset);
            // 仅对 DSHBox 记录的中断安装做一次定向恢复。相同上游错误也可能
            // 来自用户自行维护的 profile，未命中事务标记时绝不修改 manifest。
            if allow_install_recovery {
                // 两种可恢复形态：bundle 残留引用（半写）与刚变更插件加载崩溃
                // （装得上但起不来，如与新 dsh API 不兼容的 SyntaxError）。
                let recoverable =
                    unresolved_bundle_package(&log).or_else(|| plugin_load_error_package(&log));
                if let Some(stale) = recoverable {
                    crate::logging::log(&format!(
                        "dsh: 启动失败指向插件 {stale}，核对 DSHBox 插件事务"
                    ));
                    match crate::plugins::recover_interrupted_plugin_mutation(config, &stale) {
                        Ok(true) => {
                            crate::logging::log(&format!(
                                "dsh: 已回退 DSHBox 记录的 {stale} 插件变更，重试启动一次"
                            ));
                            shutdown(app);
                            return start_and_wait_managed_inner(app, config, node_exe, false);
                        }
                        Ok(false) => {
                            crate::logging::log(&format!(
                                "dsh: {stale} 不属于可恢复的 DSHBox 插件事务，保留用户配置"
                            ));
                        }
                        Err(e) => {
                            crate::logging::log(&format!("dsh: 回退插件变更 {stale} 失败：{e}"));
                        }
                    }
                }
            }
            shutdown(app);
            return Err(crate::locale::owned(
                format!("dsh 服务启动后立即退出（{status}）。{}", log_tail(&log)),
                format!(
                    "The dsh service exited during startup ({status}). {}",
                    log_tail(&log)
                ),
            ));
        }
        if state.is_quitting() {
            shutdown(app);
            return Err(crate::locale::text("应用已退出", "The app has quit").into());
        }
        if Instant::now() > deadline {
            shutdown(app);
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
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// 从 dsh 启动日志解析“无法解析 profile bundle”的包名。
/// 命中返回待清理的残留包名（如 dsh-pocket），否则 None。
fn unresolved_bundle_package(log: &str) -> Option<String> {
    // 日志形如：cannot resolve profile bundle "dsh-pocket" from ...
    let mut rest = log;
    while let Some(pos) = rest.find("cannot resolve profile bundle") {
        let after = &rest[pos + "cannot resolve profile bundle".len()..];
        let start = after.find('"')? + 1;
        let end = after[start..].find('"')? + start;
        if start < end {
            return Some(after[start..end].to_string());
        }
        rest = &after[end..];
    }
    None
}

/// 从 dsh 启动日志解析“插件加载崩溃”的肇事插件包名。
/// 形态：ESM 加载错误（SyntaxError 等）之前的最后一段代码位置指向
/// `node_modules/<pkg>/...`。栈里 `.pnpm` 嵌套路径是插件运行时的内部
/// 依赖，不构成可操作的目标，只有顶层的用户插件包名才有诊断价值。
/// 返回 None 表示看不出插件因素（按通用启动失败处理）。
pub(crate) fn plugin_load_error_package(log: &str) -> Option<String> {
    if !log.contains("SyntaxError") {
        return None;
    }
    // 逐行倒查：从每个 SyntaxError 行向上找最近的顶层 node_modules 路径。
    let lines: Vec<&str> = log.lines().collect();
    for (index, line) in lines.iter().enumerate() {
        if !line.contains("SyntaxError") {
            continue;
        }
        for prior in lines[..index].iter().rev() {
            if let Some(package) = top_level_node_modules_package(prior) {
                return Some(package);
            }
        }
    }
    None
}

/// 解析一行里的顶层 `node_modules/<pkg>` 包名；`.pnpm` 内部嵌套与
/// 非路径行返回 None。支持 `@scope/name` 与正反斜杠两种分隔符。
fn top_level_node_modules_package(line: &str) -> Option<String> {
    let pos = line.find("node_modules")? + "node_modules".len();
    let after = line[pos..].trim_start_matches(['/', '\\']);
    if after.is_empty() {
        return None;
    }
    let mut segments = after.split(['/', '\\']);
    let first = segments.next().unwrap_or_default();
    if first == ".pnpm" {
        // pnpm 虚拟存储内部路径（如 .pnpm/@scope+name@ver/node_modules/...），
        // 其后的 node_modules 段指向插件运行时的传递依赖而非用户插件
        return None;
    }
    if let Some(scoped) = first.strip_prefix('@') {
        return segments
            .next()
            .filter(|second| !second.is_empty())
            .map(|second| format!("@{scoped}/{second}"));
    }
    (!first.is_empty()).then(|| first.to_string())
}

fn read_log_since(path: &std::path::Path, offset: u64) -> String {
    let Ok(mut file) = std::fs::File::open(path) else {
        return String::new();
    };
    let len = file.metadata().map(|meta| meta.len()).unwrap_or(0);
    let _ = file.seek(SeekFrom::Start(offset.min(len)));
    let mut text = String::new();
    let _ = file.read_to_string(&mut text);
    text
}

/// dsh 启动日志中服务 URL 行的固定前缀（端口与 token 解析共用）。
const SERVER_URL_PREFIXES: [&str; 2] = ["dsh web: http://127.0.0.1:", "dsh web: http://localhost:"];

/// 只保留以换行结尾的完整行：日志由进程并发追加，末尾可能是尚未写完的
/// 半行，撕裂行里的残缺端口/token 不能采信。
fn complete_log_lines(log: &str) -> std::str::Lines<'_> {
    let complete = match log.rfind('\n') {
        Some(pos) => &log[..=pos],
        None => "",
    };
    complete.lines()
}

fn parse_server_port(log: &str) -> Option<u16> {
    complete_log_lines(log).find_map(|line| {
        SERVER_URL_PREFIXES.iter().find_map(|prefix| {
            let rest = line.trim().strip_prefix(prefix)?;
            let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
            digits.parse::<u16>().ok().filter(|port| *port != 0)
        })
    })
}

/// 从 dsh 启动日志解析当前进程的 web 鉴权 token。新版 dsh（≥0.1.2）的
/// URL 行携带 `?token=<base64url>`，token 为进程级随机值、每次启动变化；
/// 旧版无鉴权时不出现 token（偶发的浏览器打开 token 行也兼容）。取最后
/// 一条完整行，避免采到撕裂半行；解析范围由调用方的 log_offset 限定在
/// 当前进程输出内，不会混入上一次启动的旧 token。
pub(crate) fn parse_server_token(log: &str) -> Option<String> {
    complete_log_lines(log)
        .filter_map(|line| {
            SERVER_URL_PREFIXES.iter().find_map(|prefix| {
                let rest = line.trim().strip_prefix(prefix)?;
                let token = rest.split_once("?token=")?.1;
                let token = token
                    .split(['&', ' ', '\r', '\t'])
                    .next()
                    .unwrap_or_default();
                (!token.is_empty()).then(|| token.to_string())
            })
        })
        .next_back()
}

fn log_tail(log: &str) -> String {
    let mut chars: Vec<char> = log.chars().rev().take(2000).collect();
    chars.reverse();
    chars.into_iter().collect::<String>().trim().to_string()
}

fn is_bind_failure(error: &str) -> bool {
    let lower = error.to_ascii_lowercase();
    [
        "eaddrinuse",
        "eacces",
        "address already in use",
        "permission denied",
    ]
    .iter()
    .any(|marker| lower.contains(marker))
}

pub(crate) fn forget_external_service(app: &AppHandle) -> Result<(), String> {
    let state = app.state::<AppState>();
    if !state.service_ownership().is_external() {
        return Err(crate::locale::text(
            "当前没有已连接或已记住的外部服务。",
            "No external dsh service is connected or available to reconnect.",
        )
        .into());
    }
    let previous_phase = state.phase();
    let config = state.config();
    if let Some(service) = state.external_service() {
        // “改用本地”是对当前外部实例的明确选择。记住其指纹，避免下一轮
        // 立即再次弹出同一个候选；实例身份变化后仍会重新询问。
        save_external_preference(&config, false, &service)?;
    } else {
        crate::app_state::save_state_value(
            &config.root,
            EXTERNAL_SERVICE_PREFERENCE_KEY,
            serde_json::Value::Null,
        )?;
    }
    if crate::app_state::load_state_value(&config.root, "local_onboarding_deferred")
        .and_then(|value| value.as_bool())
        == Some(true)
    {
        // 首次启动因接入外部服务而跳过的配置，只在首次改用本地时恢复。
        // 已经用过本地服务的用户不存在该标记，不会重复显示引导。
        crate::app_state::save_state_value(
            &config.root,
            "onboarded",
            serde_json::Value::Bool(false),
        )?;
        crate::app_state::save_state_value(
            &config.root,
            "local_onboarding_deferred",
            serde_json::Value::Bool(false),
        )?;
        crate::app_state::mark_onboarding_pending();
    }
    state.clear_service_ownership();
    state.set_port(state.managed_port_preference());
    let message = crate::locale::text(
        "正在切换到 DSHBox 本地服务…",
        "Switching to DSHBox's local service…",
    );
    state.set_phase(BootPhase::SwitchingService, message, "");
    navigate_to_splash(app);
    emit_status(app, BootPhase::SwitchingService, message, "");
    if previous_phase == BootPhase::Error {
        state.signal_retry();
    }
    Ok(())
}

// ---------- 看门狗 ----------

/// 看门狗单个 tick 的决策（纯逻辑，供表驱动测试）。
/// `service_failed` 为「托管进程已退出或健康检查失败」的综合判定；
/// `None` 表示尚未探测——决策返回 `CheckService` 交由 IO 层探测后带
/// 结果重新决策（避免在 updating/非 Ready 时做 800ms 的健康探测）。
#[derive(Debug, PartialEq, Eq)]
enum WatchdogAction {
    /// 应用退出中：结束看门狗。
    Exit,
    /// 正常或暂时性状态：重置失败计数继续监控。
    ContinueReset,
    /// Ready 且尚未探测：IO 层做进程/健康探测后重新决策。
    CheckService,
    /// 健康检查未通过但未达阈值：保持计数继续监控。
    ContinueCounting,
    /// 连续两次失败且服务为外部归属：标记断开并交回错误页。
    MarkExternalDisconnected,
    /// 连续两次失败且服务为托管：清进程并触发自动重启。
    RestartManaged,
}

/// `failures` 为此前已累计的连续失败次数（0 起）。
fn watchdog_step(
    quitting: bool,
    updating: bool,
    phase: BootPhase,
    ownership: ServiceOwnership,
    service_failed: Option<bool>,
    failures: u32,
) -> WatchdogAction {
    if quitting {
        return WatchdogAction::Exit;
    }
    if updating {
        return WatchdogAction::ContinueReset;
    }
    if phase != BootPhase::Ready {
        // 阶段离开 Ready：更新/重启失败会把阶段置为 Error（错误页已展示），
        // 退出看门狗交回 boot_loop 等待“重试”——在此继续会让 retry 信号
        // 永远无人消费
        return if matches!(phase, BootPhase::Error | BootPhase::SwitchingService) {
            WatchdogAction::Exit
        } else {
            WatchdogAction::ContinueReset
        };
    }
    let Some(service_failed) = service_failed else {
        return WatchdogAction::CheckService;
    };
    if !service_failed {
        return WatchdogAction::ContinueReset;
    }
    // 连续两次失败才处理：dsh 思考高峰时响应可能短暂超时。
    if failures + 1 < 2 {
        return WatchdogAction::ContinueCounting;
    }
    if ownership == ServiceOwnership::External {
        WatchdogAction::MarkExternalDisconnected
    } else {
        WatchdogAction::RestartManaged
    }
}

/// 看门狗：服务掉线时回到启动页并自动重启（重启失败会走 Err 分支显示错误+手动重试）。
/// 更新流程进行中（updating=true）时跳过，避免打断运行时安装。
fn watchdog(app: &AppHandle) {
    let mut failures = 0u32;
    loop {
        std::thread::sleep(WATCH_INTERVAL);
        let state = app.state::<AppState>();
        // None = 尚未探测；决策要求探测时才真正做进程/健康检查
        let mut service_failed: Option<bool> = None;
        loop {
            let action = watchdog_step(
                state.is_quitting(),
                state.is_updating(),
                state.phase(),
                state.service_ownership(),
                service_failed,
                failures,
            );
            match action {
                WatchdogAction::Exit => return,
                WatchdogAction::ContinueReset => {
                    failures = 0;
                    break;
                }
                WatchdogAction::CheckService => {
                    let config = state.config();
                    let port = config.port;
                    // 外部服务没有子进程可查，只看健康检查；托管服务进程
                    // 退出即视为失败。token 只对托管服务有意义（外部服务
                    // 拿不到，config 中亦为 None，退化为无凭据探测）。
                    let token = (state.service_ownership() == ServiceOwnership::Managed)
                        .then_some(config.auth_token.as_deref())
                        .flatten();
                    service_failed =
                        Some(if state.service_ownership() == ServiceOwnership::Managed {
                            state
                                .managed_process_exit()
                                .map(|status| status.is_some())
                                .unwrap_or(true)
                                || !health_check(port, token)
                        } else {
                            !health_check(port, token)
                        });
                }
                WatchdogAction::ContinueCounting => {
                    failures += 1;
                    crate::logging::log(&format!("watchdog: 健康检查失败 {failures}/2"));
                    break;
                }
                // 达到阈值：复检更新/重启可能刚把服务拉起，避免误停刚就绪的服务
                WatchdogAction::MarkExternalDisconnected => {
                    if state.is_updating() || state.phase() != BootPhase::Ready {
                        failures = 0;
                        break;
                    }
                    // 外部进程不属于 DSHBox：不停止、不替换、不偷偷切换数据源。
                    crate::logging::log(
                        "watchdog: 外部 dsh 服务已断开，等待用户恢复或改用本地服务",
                    );
                    state.mark_external_disconnected();
                    navigate_to_splash(app);
                    let message = crate::locale::text(
                        "外部 dsh 服务已断开。可重试连接，或改用 DSHBox 本地服务。",
                        "The external dsh service disconnected. Try reconnecting, or switch to DSHBox's local service.",
                    );
                    state.set_phase(BootPhase::Error, message, "");
                    emit_status(app, BootPhase::Error, message, "");
                    return;
                }
                WatchdogAction::RestartManaged => {
                    if state.is_updating() || state.phase() != BootPhase::Ready {
                        failures = 0;
                        break;
                    }
                    // 托管服务已停止：清理残留进程，回启动页，由外层循环自动重启。
                    crate::logging::log("watchdog: dsh 服务已停止，准备自动重启");
                    shutdown(app);
                    navigate_to_splash(app);
                    let restarting = crate::locale::text(
                        "服务已停止，正在自动重启…",
                        "The service stopped. Restarting automatically…",
                    );
                    state.set_phase(BootPhase::Starting, restarting, "");
                    emit_status(app, BootPhase::Starting, restarting, "");
                    std::thread::sleep(Duration::from_secs(2));
                    return;
                }
            }
        }
    }
}

// ---------- 退出清理与健康检查 ----------

/// 退出清理：进程树守卫销毁（Windows Job / Unix 进程组）为主；仅当无
/// 守卫时（Job 创建失败的降级路径）才按 PID 树兜底——守卫回收后 PID 可能
/// 已被系统复用，再 taskkill 有误杀无关进程的风险。
pub fn shutdown(app: &AppHandle) {
    let state = app.state::<AppState>();
    let (mut child, job) = state.take_running();
    let pid = child.as_ref().map(std::process::Child::id);
    if job.is_some() {
        drop(job);
    } else if let Some(pid) = pid {
        processes::kill_tree(pid);
    }
    if let Some(child) = child.as_mut() {
        let _ = child.wait();
    }
}

/// 区分“已有监听者”和“无人监听但系统禁止绑定”。Windows 动态保留端口
/// 属于后者，不应触发 HTTP 重试或相邻端口扫描。
pub(crate) fn port_availability(port: u16) -> PortAvailability {
    use std::net::{SocketAddr, TcpListener, TcpStream};
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    if TcpStream::connect_timeout(&addr, Duration::from_millis(300)).is_ok() {
        return PortAvailability::Listening;
    }
    match TcpListener::bind(addr) {
        Ok(listener) => {
            drop(listener);
            PortAvailability::Free
        }
        Err(_) => PortAvailability::Unbindable,
    }
}

/// 验证端口上的服务确实是 dsh Web UI，而不只是任意 TCP 监听者。
/// `token`：新版 dsh（≥0.1.2）的进程级 web 鉴权 token；有值时请求
/// `/?token=`，token 交换成功（303 + 种下 dsh-auth cookie）即视为健康。
/// None（旧版无鉴权 / 外部服务无凭据）维持裸 `GET /` 的标题判定。
pub(crate) fn health_check(port: u16, token: Option<&str>) -> bool {
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
    let target = match token {
        Some(token) => format!("/?token={token}"),
        None => "/".to_string(),
    };
    let request = format!(
        "GET {target} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nAccept: text/html\r\nConnection: close\r\n\r\n"
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
    is_dsh_response(&response) || (token.is_some() && is_token_exchange_response(&response))
}

fn is_dsh_response(response: &[u8]) -> bool {
    let success = response.starts_with(b"HTTP/1.1 200 ") || response.starts_with(b"HTTP/1.0 200 ");
    const MARKER: &[u8] = b"<title>DeepSeek Harness</title>";
    success
        && response
            .windows(MARKER.len())
            .any(|window| window == MARKER)
}

/// token 交换成功的最小判定：303 重定向到 `/` 并种下 `dsh-auth-*` 会话
/// cookie（上游 browser-auth authorizeIndex 的既定行为）。401（token
/// 错误/过期）与其他状态一律不视为健康。
fn is_token_exchange_response(response: &[u8]) -> bool {
    if !(response.starts_with(b"HTTP/1.1 303 ") || response.starts_with(b"HTTP/1.0 303 ")) {
        return false;
    }
    let head = String::from_utf8_lossy(&response[..response.len().min(4096)]).to_ascii_lowercase();
    head.contains("set-cookie: dsh-auth-")
}

#[cfg(test)]
mod tests {
    use super::{
        classify_boot_result, is_bind_failure, is_dsh_response, onboarding_handoff_decision,
        onboarding_wait_decision, parse_external_description, parse_server_port,
        same_external_identity, select_managed_port, watchdog_step, BootOutcome, InstallAction,
        OnboardingWaitAction, PortAvailability, ServiceOwnership, WatchdogAction,
    };

    #[test]
    fn onboarding_wait_waits_indefinitely_while_panel_is_shown() {
        // 面板已回报显示：即使远超兜底期限也不放行
        assert_eq!(
            onboarding_wait_decision(false, true, true, false),
            OnboardingWaitAction::Wait
        );
        assert_eq!(
            onboarding_wait_decision(true, true, true, false),
            OnboardingWaitAction::Proceed
        );
    }

    #[test]
    fn parse_server_token_reads_last_complete_token_line() {
        // 新版 URL 行带 token；旧版裸 URL 行不含 token（跳过）
        assert_eq!(
            super::parse_server_token("dsh web: http://127.0.0.1:18080\nnoise\ndsh web: http://127.0.0.1:18080/?token=abc_DEF-123\n"),
            Some("abc_DEF-123".into())
        );
        // localhost 前缀与多 token 行取最后一条
        assert_eq!(
            super::parse_server_token(
                "dsh web: http://localhost:3080/?token=first\ndsh web: http://localhost:3080/?token=second\n"
            ),
            Some("second".into())
        );
        // 撕裂半行（无结尾换行）不采信；纯旧版日志返回 None
        assert_eq!(
            super::parse_server_token("dsh web: http://127.0.0.1:18080/?token=torn"),
            None
        );
        assert_eq!(
            super::parse_server_token("dsh web: http://127.0.0.1:18080\n"),
            None
        );
    }

    #[test]
    fn token_exchange_response_requires_303_and_auth_cookie() {
        let ok = b"HTTP/1.1 303 See Other\r\nset-cookie: dsh-auth-xyz=v1.sig; Path=/\r\nlocation: /\r\n\r\n";
        assert!(super::is_token_exchange_response(ok));
        // 401（token 错误/过期）不能视为健康
        let unauthorized = b"HTTP/1.1 401 Unauthorized\r\ncontent-type: text/plain\r\n\r\ndsh web authentication required";
        assert!(!super::is_token_exchange_response(unauthorized));
        // 303 但没有 dsh-auth cookie（非 dsh 服务）不视为健康
        let bare_redirect = b"HTTP/1.1 303 See Other\r\nlocation: /\r\n\r\n";
        assert!(!super::is_token_exchange_response(bare_redirect));
    }

    #[test]
    fn onboarding_wait_forces_onboard_only_after_deadline_and_failed_probe() {
        // 未到期：等待
        assert_eq!(
            onboarding_wait_decision(false, false, false, false),
            OnboardingWaitAction::Wait
        );
        // 到期但探活确认面板可见：继续等待
        assert_eq!(
            onboarding_wait_decision(false, false, true, true),
            OnboardingWaitAction::Wait
        );
        // 到期、面板未显示、探活不可见：兜底放行
        assert_eq!(
            onboarding_wait_decision(false, false, true, false),
            OnboardingWaitAction::ForceOnboard
        );
    }

    #[test]
    fn handoff_delegates_when_phase_left_ready_or_restart_pending() {
        use crate::app_state::BootPhase;
        assert!(onboarding_handoff_decision(
            BootPhase::Starting,
            ServiceOwnership::Managed,
            false
        ));
        assert!(onboarding_handoff_decision(
            BootPhase::Ready,
            ServiceOwnership::Managed,
            true
        ));
        // 外部服务不参与插件重启协调
        assert!(!onboarding_handoff_decision(
            BootPhase::Ready,
            ServiceOwnership::External,
            true
        ));
        assert!(!onboarding_handoff_decision(
            BootPhase::Ready,
            ServiceOwnership::Managed,
            false
        ));
    }

    #[test]
    fn watchdog_exits_on_terminal_phases_and_quitting() {
        use crate::app_state::BootPhase;
        assert_eq!(
            watchdog_step(
                true,
                false,
                BootPhase::Ready,
                ServiceOwnership::Managed,
                None,
                0
            ),
            WatchdogAction::Exit
        );
        for phase in [BootPhase::Error, BootPhase::SwitchingService] {
            assert_eq!(
                watchdog_step(false, false, phase, ServiceOwnership::Managed, None, 0),
                WatchdogAction::Exit
            );
        }
    }

    #[test]
    fn watchdog_skips_probe_until_ready_and_not_updating() {
        use crate::app_state::BootPhase;
        // Ready 且未探测：要求 IO 层做健康判定
        assert_eq!(
            watchdog_step(
                false,
                false,
                BootPhase::Ready,
                ServiceOwnership::Managed,
                None,
                0
            ),
            WatchdogAction::CheckService
        );
        // updating / 非 Ready 阶段：不做探测直接重置计数
        assert_eq!(
            watchdog_step(
                false,
                true,
                BootPhase::Ready,
                ServiceOwnership::Managed,
                None,
                0
            ),
            WatchdogAction::ContinueReset
        );
        assert_eq!(
            watchdog_step(
                false,
                false,
                BootPhase::Starting,
                ServiceOwnership::Managed,
                None,
                0
            ),
            WatchdogAction::ContinueReset
        );
    }

    #[test]
    fn watchdog_requires_two_consecutive_failures_before_acting() {
        use crate::app_state::BootPhase;
        // 首次失败：只计数
        assert_eq!(
            watchdog_step(
                false,
                false,
                BootPhase::Ready,
                ServiceOwnership::Managed,
                Some(true),
                0
            ),
            WatchdogAction::ContinueCounting
        );
        // 第二次失败：托管重启 / 外部标记断开
        assert_eq!(
            watchdog_step(
                false,
                false,
                BootPhase::Ready,
                ServiceOwnership::Managed,
                Some(true),
                1
            ),
            WatchdogAction::RestartManaged
        );
        assert_eq!(
            watchdog_step(
                false,
                false,
                BootPhase::Ready,
                ServiceOwnership::External,
                Some(true),
                1
            ),
            WatchdogAction::MarkExternalDisconnected
        );
        // 健康恢复：重置
        assert_eq!(
            watchdog_step(
                false,
                false,
                BootPhase::Ready,
                ServiceOwnership::Managed,
                Some(false),
                1
            ),
            WatchdogAction::ContinueReset
        );
    }

    #[test]
    fn cancellation_is_not_reported_as_startup_failure() {
        let result = classify_boot_result(InstallAction::Cancel, Err("internal sentinel".into()));
        assert_eq!(result, BootOutcome::Cancelled);
    }

    #[test]
    fn ordinary_failure_keeps_its_detail() {
        let result = classify_boot_result(InstallAction::None, Err("network error".into()));
        assert_eq!(result, BootOutcome::Failed("network error".into()));
    }

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

    #[test]
    fn parses_os_assigned_port_from_dsh_output() {
        assert_eq!(
            parse_server_port("loading\ndsh web: http://127.0.0.1:49152\nready"),
            Some(49152)
        );
        assert_eq!(
            parse_server_port("dsh web: http://localhost:3080 (copied)\n"),
            Some(3080)
        );
        assert_eq!(parse_server_port("dsh web: http://127.0.0.1:0\n"), None);
        assert_eq!(parse_server_port("other server: 49152\n"), None);
    }

    #[test]
    fn torn_log_tail_is_not_parsed_as_port() {
        // 末尾未写完的半行必须丢弃：否则会把撕裂的残缺端口当成实际端口
        assert_eq!(parse_server_port("dsh web: http://127.0.0.1:491"), None);
        assert_eq!(parse_server_port("dsh web: http://127.0.0.1:49"), None);
        // 完整行之后的半行不影响已写完的端口行
        assert_eq!(
            parse_server_port("dsh web: http://127.0.0.1:49152\ndsh web: http://127.0.0.1:9"),
            Some(49152)
        );
    }

    #[test]
    fn only_address_binding_failures_trigger_port_zero_retry() {
        assert!(is_bind_failure("listen EADDRINUSE: address already in use"));
        assert!(is_bind_failure("listen EACCES: permission denied"));
        assert!(!is_bind_failure("Cannot find module '@deepseek-ai/dsh'"));
    }

    #[test]
    fn managed_port_prefers_last_then_config_and_never_scans_neighbors() {
        assert_eq!(
            select_managed_port(Some(49152), 18080, |port| match port {
                49152 => PortAvailability::Free,
                18080 => PortAvailability::Unbindable,
                _ => panic!("unexpected adjacent port probe: {port}"),
            }),
            49152
        );
        assert_eq!(
            select_managed_port(Some(49152), 18080, |port| match port {
                49152 => PortAvailability::Listening,
                18080 => PortAvailability::Free,
                _ => panic!("unexpected adjacent port probe: {port}"),
            }),
            18080
        );
        assert_eq!(
            select_managed_port(None, 18080, |_| PortAvailability::Unbindable),
            0
        );
    }

    #[test]
    fn parses_official_host_description_identity() {
        let json = serde_json::json!({
            "result": {
                "ok": true,
                "value": {
                    "version": "0.0.1",
                    "cwd": "C:/workspace",
                    "home": "C:/Users/test"
                }
            }
        });
        let candidate = parse_external_description(3080, &json).unwrap();
        assert_eq!(candidate.port, 3080);
        assert_eq!(candidate.cwd, "C:/workspace");
        assert_eq!(candidate.home, "C:/Users/test");
        assert!(parse_external_description(
            3080,
            &serde_json::json!({ "result": { "ok": false } })
        )
        .is_none());
        assert!(parse_external_description(
            3080,
            &serde_json::json!({
                "result": { "ok": true, "value": { "version": "0.0.1", "cwd": "x" } }
            })
        )
        .is_none());
    }

    #[test]
    fn external_identity_survives_version_updates_but_not_workspace_changes() {
        let original = crate::app_state::ExternalServiceCandidate {
            port: 3080,
            version: "0.1.0".into(),
            cwd: "C:/workspace".into(),
            home: "C:/Users/test".into(),
        };
        let mut updated = original.clone();
        updated.version = "0.2.0".into();
        assert!(same_external_identity(&original, &updated));
        updated.cwd = "C:/another-workspace".into();
        assert!(!same_external_identity(&original, &updated));
    }

    #[test]
    fn plugin_load_error_extracts_culprit_package_from_esm_syntax_error() {
        // 真实故障形态：代码帧路径指向顶层用户插件，其后才是 pnpm 栈
        let log = "\
dsh web: starting
file:///C:/dsh/node_modules/dsh-better-sidebar/lib/index.js:11\n\
      import { settingsNamespace } from \"@deepseek-ai/dsh-settings\";\n\
      SyntaxError: The requested module '@deepseek-ai/dsh-settings' does not provide an export named 'settingsNamespace'\n\
    at #asyncInstantiate (node:internal/modules/esm/module_job:327:21)\n\
    at async file:///C:/dsh/node_modules/.pnpm/@deepseek-ai+cordis-plugin-loader_x/node_modules/@deepseek-ai/cordis-plugin-loader/lib/index.js:274:41\n\
";
        assert_eq!(
            super::plugin_load_error_package(log).as_deref(),
            Some("dsh-better-sidebar")
        );
    }

    #[test]
    fn plugin_load_error_resolves_scoped_packages_and_backslashes() {
        let log = "C:\\dsh\\node_modules\\@deepseek-ai\\dsh-pocket\\lib\\index.js:1\nSyntaxError: Unexpected token 'export'\n";
        assert_eq!(
            super::plugin_load_error_package(log).as_deref(),
            Some("@deepseek-ai/dsh-pocket")
        );
    }

    #[test]
    fn plugin_load_error_ignores_pnpm_internal_only_logs_and_plain_failures() {
        // 栈里只有 .pnpm 内部路径：看不出用户插件，不猜测
        let internal_only = "SyntaxError: The requested module does not provide an export\n    at async file:///C:/dsh/node_modules/.pnpm/x@1/node_modules/x/lib/index.js:1:1\n";
        assert_eq!(super::plugin_load_error_package(internal_only), None);
        // 没有任何 SyntaxError 的普通启动失败
        assert_eq!(
            super::plugin_load_error_package("Error: listen EADDRINUSE"),
            None
        );
    }
}
