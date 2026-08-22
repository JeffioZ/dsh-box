//! 应用共享状态：运行时配置、引导阶段、子进程句柄。

mod config;
mod managed_file;
mod store;

pub use config::Config;
#[cfg(test)]
use managed_file::merge_section_field;
pub(crate) use managed_file::{atomic_write, update_text_file};
pub(crate) use store::{load_state_value, save_config_value, save_state_value};

use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, Mutex};

use crate::processes::TreeGuard;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BootPhase {
    Starting,
    SwitchingService,
    ServiceChoice,
    InstallingNode,
    InstallingDsh,
    StartingServer,
    Ready,
    Cancelled,
    Error,
}

impl BootPhase {
    pub fn as_str(&self) -> &'static str {
        match self {
            BootPhase::Starting => "starting",
            BootPhase::SwitchingService => "switching-service",
            BootPhase::ServiceChoice => "service-choice",
            BootPhase::InstallingNode => "installing-node",
            BootPhase::InstallingDsh => "installing-dsh",
            BootPhase::StartingServer => "starting-server",
            BootPhase::Ready => "ready",
            BootPhase::Cancelled => "cancelled",
            BootPhase::Error => "error",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ServiceOwnership {
    None,
    Managed,
    External,
    ExternalDisconnected,
}

impl ServiceOwnership {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Managed => "managed",
            Self::External => "external",
            Self::ExternalDisconnected => "external-disconnected",
        }
    }

    pub fn is_external(self) -> bool {
        matches!(self, Self::External | Self::ExternalDisconnected)
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ExternalServiceCandidate {
    pub port: u16,
    pub version: String,
    pub cwd: String,
    pub home: String,
}

#[derive(Clone, Serialize)]
pub struct StatusPayload {
    pub phase: String,
    pub message: String,
    pub detail: String,
    /// 确定进度 0-100；None = 不确定（动画）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub progress: Option<f64>,
    /// 已安装 dsh 版本（未安装为 None）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dsh_version: Option<String>,
    /// 当前使用的 Node 版本（如 v24.19.0）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub node_version: Option<String>,
    /// 当前 Node 自带的 npm 版本（如 12.0.2）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub npm_version: Option<String>,
    /// 实际监听端口。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
    pub download_source: String,
    /// 当前引导轮次。安装操作必须携带该值，过期页面不能影响新一轮安装。
    pub install_generation: u64,
    /// 当前阶段是否仍接受取消或切换下载源。
    pub can_cancel: bool,
    /// 服务归属决定哪些操作可由 DSHBox 执行（外部服务绝不停止或更新）。
    pub service_mode: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub external_service: Option<ExternalServiceCandidate>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum InstallAction {
    None,
    Cancel,
    SwitchSource,
}

#[derive(Clone, Copy, Debug)]
struct InstallControl {
    generation: u64,
    action: InstallAction,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct ServiceChoice {
    pub generation: u64,
    pub reuse: bool,
}

/// dev 构建标记（setup 时按 bake 的 devUrl 判定一次）。dev 构建下
/// onboarding 每次启动都引导（便于测试），正式构建不受影响。
static DEV_BUILD: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub(crate) fn mark_dev_build() {
    DEV_BUILD.store(true, std::sync::atomic::Ordering::Relaxed);
}

pub(crate) fn dev_build() -> bool {
    DEV_BUILD.load(std::sync::atomic::Ordering::Relaxed)
}

/// 首次配置是否已由用户在本进程完成。开发版借此保持
/// “每次启动展示一次”，同时避免服务重启返回启动页时重复展示。
static ONBOARDING_DONE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub(crate) fn mark_onboarding_done() {
    ONBOARDING_DONE.store(true, std::sync::atomic::Ordering::Relaxed);
}

pub(crate) fn mark_onboarding_pending() {
    ONBOARDING_DONE.store(false, std::sync::atomic::Ordering::Relaxed);
    ONBOARDING_SHOWN.store(false, std::sync::atomic::Ordering::Relaxed);
}

pub(crate) fn onboarding_done() -> bool {
    ONBOARDING_DONE.load(std::sync::atomic::Ordering::Relaxed)
}

/// 首次配置面板是否已在启动页显示（前端显示后回报）。
/// boot 等待据此区分：面板已显示则无限等待用户完成配置；未显示
/// （启动页异常）60 秒后自动放行防卡死。
static ONBOARDING_SHOWN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub(crate) fn mark_onboarding_shown() {
    ONBOARDING_SHOWN.store(true, std::sync::atomic::Ordering::Relaxed);
}

pub(crate) fn onboarding_shown() -> bool {
    ONBOARDING_SHOWN.load(std::sync::atomic::Ordering::Relaxed)
}

fn onboarding_required_for(
    is_dev_build: bool,
    completed_in_process: bool,
    persisted_onboarded: bool,
) -> bool {
    if is_dev_build {
        return !completed_in_process;
    }
    !persisted_onboarded
}

pub(crate) fn onboarding_required(root: &Path) -> bool {
    let persisted_onboarded =
        load_state_value(root, "onboarded").and_then(|value| value.as_bool()) == Some(true);
    onboarding_required_for(dev_build(), onboarding_done(), persisted_onboarded)
}

pub(crate) struct Inner {
    config: Config,
    /// 用户配置的托管服务首选端口；运行时接入外部/自动端口时不覆盖。
    managed_port_preference: u16,
    phase: BootPhase,
    message: String,
    detail: String,
    /// dsh 子进程树守卫（关闭即回收：Windows Job / Unix 进程组）。
    job: Option<TreeGuard>,
    /// 开发模式 UI 静态服务器守卫；正式版始终为 None。
    dev_ui_job: Option<TreeGuard>,
    /// 必须保留 Child 才能及时发现 EADDRINUSE 等启动失败，避免假等完整超时。
    dsh_child: Option<std::process::Child>,
    service_ownership: ServiceOwnership,
    external_service: Option<ExternalServiceCandidate>,
    quitting: bool,
    /// 更新进行中（看门狗跳过自动重启）。
    updating: bool,
    retry_tx: Sender<()>,
    service_choice_tx: Sender<ServiceChoice>,
    /// 启动页淡出完成通知；每次 Ready 前替换，旧页面的迟到通知不会影响下一轮。
    startup_transition_tx: Option<Sender<()>>,
    /// 当前使用 Node 的版本（boot 时检测一次缓存，get_status 免 spawn）。
    node_version: Option<String>,
    /// 便携 Node 自带 npm 版本（boot 时检测缓存）。
    npm_version: Option<String>,
    /// 自绘弹窗最近一次打开载荷（app_dialog_get 拉取用）。
    last_dialog: Option<crate::control_center::AppDialogOpen>,
    /// 最近一次余额查询结果（余额弹窗轮询拉取；事件通道对该窗口不可靠）。
    last_balance: Option<crate::balance::BalancePayload>,
    /// 最近一次更新检查结果 + 进度文案 + 更新完成结果（检查更新弹窗轮询拉取）。
    last_check: Option<crate::updater::CheckResult>,
    check_progress: Option<String>,
    update_done_ok: bool,
    update_done: Option<String>,
    /// 弹窗打开时是否禁用了主窗口（关闭时恢复）。
    main_disabled: bool,
    /// 弹窗生命周期代次：打开/关闭时 +1，挂起的延迟动作据此判断是否过期。
    dialog_gen: u64,
    /// PowerShell 更新的 UAC 预告在弹窗内等待确认；点击“继续”后置位。
    pwsh_pending: bool,
    pwsh_confirmed: bool,
    /// 最近一次 dsh 页面心跳（页面主线程存活标记）。
    last_heartbeat: Option<std::time::Instant>,
    /// 连续页面重载次数（指数退避）。
    heartbeat_failures: u32,
    /// 已后台预下载的应用更新（版本 + GitHub 资产 SHA-256，Windows 专属）。
    #[cfg(windows)]
    app_update_ready: Option<(String, String)>,
    /// 安装控制与引导阶段共用同一把锁，避免“阶段已变但取消仍落到下一轮”的竞态。
    install_control: InstallControl,
}

/// 全局状态（跨线程共享）。
pub struct AppState {
    inner: Arc<Mutex<Inner>>,
    /// 生命周期锁：引导/重启/更新互斥，杜绝双服务并发。
    lifecycle: Mutex<()>,
    /// 仅注入 dsh 主页面的自定义协议随机令牌。
    protocol_token: String,
    service_choice_rx: Mutex<Receiver<ServiceChoice>>,
}

/// boot_loop 的“重试”信号接收端（启动时存入，仅取一次）。
pub static RETRY_RX: std::sync::Mutex<Option<Receiver<()>>> = std::sync::Mutex::new(None);

impl AppState {
    pub fn new() -> AppState {
        let mut token_bytes = [0u8; 16];
        getrandom::fill(&mut token_bytes).expect("无法读取系统随机数");
        const HEX: &[u8; 16] = b"0123456789abcdef";
        let protocol_token: String = token_bytes
            .iter()
            .flat_map(|b| {
                [
                    HEX[(b >> 4) as usize] as char,
                    HEX[(b & 0x0f) as usize] as char,
                ]
            })
            .collect();

        let (retry_tx, retry_rx) = std::sync::mpsc::channel::<()>();
        let (service_choice_tx, service_choice_rx) = std::sync::mpsc::channel::<ServiceChoice>();
        *RETRY_RX.lock().unwrap_or_else(|e| e.into_inner()) = Some(retry_rx);
        let config = Config::load();
        let language_override = std::env::var("DSHD_LANG").ok();
        // 语言解析优先级：DSHD_LANG 环境变量 > dsh settings.yaml（locale.preference）
        // > config.json 的 language > 系统界面语言。dsh 偏好放在 config 之前，
        // 保证加载页第一帧就与 dsh 的界面语言一致。
        let preference = language_override
            .as_deref()
            .or(config.load_dsh_locale())
            .or(config.ui_language.as_deref());
        crate::locale::set_preference(preference);
        let managed_port_preference = config.port;
        let inner = Inner {
            config,
            managed_port_preference,
            phase: BootPhase::Starting,
            message: String::new(),
            detail: String::new(),
            job: None,
            dev_ui_job: None,
            dsh_child: None,
            service_ownership: ServiceOwnership::None,
            external_service: None,
            quitting: false,
            updating: false,
            retry_tx,
            service_choice_tx,
            startup_transition_tx: None,
            node_version: None,
            npm_version: None,
            last_dialog: None,
            last_balance: None,
            last_check: None,
            check_progress: None,
            update_done_ok: false,
            update_done: None,
            main_disabled: false,
            dialog_gen: 0,
            pwsh_pending: false,
            pwsh_confirmed: false,
            last_heartbeat: None,
            heartbeat_failures: 0,
            #[cfg(windows)]
            app_update_ready: None,
            install_control: InstallControl {
                generation: 0,
                action: InstallAction::None,
            },
        };
        AppState {
            inner: Arc::new(Mutex::new(inner)),
            lifecycle: Mutex::new(()),
            protocol_token,
            service_choice_rx: Mutex::new(service_choice_rx),
        }
    }

    pub(crate) fn lock_inner(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// 获取生命周期锁（引导/重启/更新串行化）。
    pub(crate) fn lifecycle_guard(&self) -> std::sync::MutexGuard<'_, ()> {
        self.lifecycle.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// 读取页面心跳状态：(最近心跳时刻, 连续重载次数)。
    pub(crate) fn heartbeat_state(&self) -> (Option<std::time::Instant>, u32) {
        let inner = self.lock_inner();
        (inner.last_heartbeat, inner.heartbeat_failures)
    }

    /// 记录一次页面心跳（页面注入脚本调用；同时清零连续重载计数）。
    pub(crate) fn set_heartbeat(&self) {
        let mut inner = self.lock_inner();
        inner.last_heartbeat = Some(std::time::Instant::now());
        inner.heartbeat_failures = 0;
    }

    /// 主页面当前不应被判死时顺延观察窗口，但不把它伪装成一次成功心跳，
    /// 也不清除真实失败累积的退避次数。
    pub(crate) fn defer_heartbeat(&self) {
        let mut inner = self.lock_inner();
        if inner.last_heartbeat.is_some() {
            inner.last_heartbeat = Some(std::time::Instant::now());
        }
    }

    /// 连续重载计数 +1，返回新值（指数退避用）。
    pub(crate) fn bump_heartbeat_failures(&self) -> u32 {
        let mut inner = self.lock_inner();
        inner.heartbeat_failures = inner.heartbeat_failures.saturating_add(1);
        inner.heartbeat_failures
    }

    /// 已后台预下载的应用更新（版本 + SHA-256；无则为 None）。
    #[cfg(windows)]
    pub(crate) fn app_update_ready(&self) -> Option<(String, String)> {
        self.lock_inner().app_update_ready.clone()
    }

    /// 记录/清除已预下载的应用更新版本与摘要。
    #[cfg(windows)]
    pub(crate) fn set_app_update_ready(&self, ready: Option<(String, String)>) {
        self.lock_inner().app_update_ready = ready;
    }

    pub(crate) fn protocol_token(&self) -> &str {
        &self.protocol_token
    }

    /// 开始新一轮引导并使旧页面持有的安装操作失效。
    pub(crate) fn begin_boot_attempt(&self) -> u64 {
        let mut inner = self.lock_inner();
        inner.install_control.generation = inner.install_control.generation.wrapping_add(1).max(1);
        inner.install_control.action = InstallAction::None;
        inner.install_control.generation
    }

    /// 请求终止当前安装。返回 false 表示页面轮次已过期或安装已结束；该情况
    /// 属于幂等完成，不应再向用户显示“没有可取消的安装”。
    pub(crate) fn request_install_action(&self, generation: u64, action: InstallAction) -> bool {
        let mut inner = self.lock_inner();
        if inner.install_control.generation != generation
            || !matches!(
                inner.phase,
                BootPhase::InstallingNode | BootPhase::InstallingDsh
            )
        {
            return false;
        }
        // 切源包含“取消当前下载并立即重启”，优先级高于普通取消。
        if inner.install_control.action != InstallAction::SwitchSource {
            inner.install_control.action = action;
        }
        true
    }

    pub(crate) fn install_cancelled(&self) -> bool {
        self.install_action() != InstallAction::None
    }

    pub(crate) fn install_action(&self) -> InstallAction {
        self.lock_inner().install_control.action
    }

    pub fn config(&self) -> Config {
        self.lock_inner().config.clone()
    }

    pub(crate) fn arm_startup_transition(&self) -> Receiver<()> {
        let (tx, rx) = std::sync::mpsc::channel();
        self.lock_inner().startup_transition_tx = Some(tx);
        rx
    }

    pub(crate) fn finish_startup_transition(&self) {
        if let Some(tx) = self.lock_inner().startup_transition_tx.take() {
            let _ = tx.send(());
        }
    }

    /// 把 config.json 与内存镜像作为一次串行提交处理，避免两个并发设置调用
    /// 交错成“磁盘是后一次、内存是前一次”。磁盘写失败时内存保持不变。
    fn persist_config_change(
        &self,
        key: &str,
        value: serde_json::Value,
        apply: impl FnOnce(&mut Config),
    ) -> Result<(), String> {
        let mut inner = self.lock_inner();
        save_config_value(&inner.config.root, key, value)?;
        apply(&mut inner.config);
        Ok(())
    }

    pub fn set_ui_language(&self, language: &str) -> Result<(), String> {
        if !matches!(language, "zh-CN" | "en") {
            return Err("Unsupported UI language".into());
        }
        self.persist_config_change(
            "language",
            serde_json::Value::String(language.to_string()),
            |config| config.ui_language = Some(language.to_string()),
        )?;
        crate::locale::set_preference(Some(language));
        Ok(())
    }

    /// 精确设置“隐藏工具调用”，避免两个并发调用通过 toggle 把值反转回去。
    pub fn set_hide_tool_calls(&self, value: bool) -> Result<(), String> {
        self.persist_config_change(
            "hide_tool_calls",
            serde_json::Value::Bool(value),
            |config| config.hide_tool_calls = value,
        )
    }

    pub fn set_hide_stats_line(&self, value: bool) -> Result<(), String> {
        self.persist_config_change(
            "hide_stats_line",
            serde_json::Value::Bool(value),
            |config| config.hide_stats_line = value,
        )
    }

    pub fn set_hide_statusbar(&self, value: bool) -> Result<(), String> {
        self.persist_config_change("hide_statusbar", serde_json::Value::Bool(value), |config| {
            config.hide_statusbar = value
        })
    }

    pub fn set_hide_balance(&self, value: bool) -> Result<(), String> {
        self.persist_config_change("hide_balance", serde_json::Value::Bool(value), |config| {
            config.hide_balance = value
        })
    }

    pub fn set_auto_update_plugins(&self, value: bool) -> Result<(), String> {
        self.persist_config_change(
            "auto_update_plugins",
            serde_json::Value::Bool(value),
            |config| config.auto_update_plugins = value,
        )
    }

    /// 设置 dsh 内核更新通道（latest/next），持久化到 config.json。
    pub fn set_dsh_update_channel(&self, channel: &str) -> Result<(), String> {
        if !matches!(channel, "latest" | "next") {
            return Err(
                crate::locale::text("未知更新通道。", "Unknown update channel.").to_string(),
            );
        }
        self.persist_config_change("dsh_update_channel", serde_json::json!(channel), |config| {
            config.dsh_update_channel = channel.to_string()
        })
    }

    pub fn set_close_behavior(&self, value: &str) -> Result<(), String> {
        if !matches!(value, "tray" | "quit") {
            return Err(crate::locale::text("未知关闭行为。", "Unknown close behavior.").into());
        }
        self.persist_config_change("close_behavior", serde_json::json!(value), |config| {
            config.close_behavior = value.to_string()
        })
    }

    pub fn set_launch_behavior(&self, value: &str) -> Result<(), String> {
        if !matches!(value, "window" | "tray") {
            return Err(crate::locale::text("未知启动行为。", "Unknown launch behavior.").into());
        }
        self.persist_config_change("launch_behavior", serde_json::json!(value), |config| {
            config.launch_behavior = value.to_string()
        })
    }

    pub(crate) fn request_install_source(
        &self,
        generation: u64,
        value: &str,
    ) -> Result<bool, String> {
        if !matches!(value, "auto" | "official" | "mirror") {
            return Err(crate::locale::text("未知下载源。", "Unknown download source.").into());
        }
        let mut inner = self.lock_inner();
        if inner.install_control.generation != generation {
            return Ok(false);
        }
        let restart_install = matches!(
            inner.phase,
            BootPhase::InstallingNode | BootPhase::InstallingDsh
        );
        if !restart_install && inner.phase != BootPhase::Cancelled {
            return Ok(false);
        }
        // 校验轮次、落盘和登记切源必须在同一临界区内完成：过期页面不能
        // 只改配置不重启，落盘失败也不能误取消当前安装。
        save_config_value(
            &inner.config.root,
            "download_source",
            serde_json::json!(value),
        )?;
        inner.config.download_source = value.to_string();
        if restart_install {
            inner.install_control.action = InstallAction::SwitchSource;
        }
        Ok(restart_install)
    }

    /// 首次使用配置是否尚未完成；开发构建每次进程启动展示一次。
    pub(crate) fn onboarding_pending(&self) -> bool {
        onboarding_required(&self.config().root)
    }

    pub fn snapshot(&self) -> StatusPayload {
        let (
            phase,
            message,
            detail,
            port,
            config,
            node_version,
            npm_version,
            install_generation,
            install_action,
            service_ownership,
            external_service,
        ) = {
            let g = self.lock_inner();
            (
                g.phase,
                g.message.clone(),
                g.detail.clone(),
                g.config.port,
                g.config.clone(),
                g.node_version.clone(),
                g.npm_version.clone(),
                g.install_control.generation,
                g.install_control.action,
                g.service_ownership,
                g.external_service.clone(),
            )
        };
        // 缓存缺失时即时检测一次：启动页首帧就显示完整的版本信息
        // （Node 版本由 boot 线程稍后检测，直接等会导致信息出现太晚、
        // 启动快时刚显示就随页面导航消失）
        let node_version = if external_service.is_some() {
            None
        } else if node_version.is_some() {
            node_version
        } else {
            let version = crate::runtime::current_node_version(&config);
            if version.is_some() {
                self.set_node_version(version.clone());
            }
            version
        };
        let npm_version = if external_service.is_some() {
            None
        } else if npm_version.is_some() {
            npm_version
        } else {
            let version = crate::runtime::npm_version(&config);
            if version.is_some() {
                self.set_npm_version(version.clone());
            }
            version
        };
        StatusPayload {
            phase: phase.as_str().to_string(),
            message,
            detail,
            progress: None,
            dsh_version: external_service
                .as_ref()
                .map(|service| service.version.clone())
                .filter(|version| !version.is_empty())
                .or_else(|| crate::runtime::installed_dsh_version(&config)),
            node_version,
            npm_version,
            port: Some(port),
            download_source: config.download_source,
            install_generation,
            can_cancel: matches!(phase, BootPhase::InstallingNode | BootPhase::InstallingDsh)
                && install_action == InstallAction::None,
            service_mode: service_ownership.as_str().to_string(),
            external_service,
        }
    }

    pub fn set_phase(&self, phase: BootPhase, message: &str, detail: &str) {
        let mut g = self.lock_inner();
        g.phase = phase;
        g.message = message.to_string();
        g.detail = detail.to_string();
    }

    /// 端口回退后更新（供 watchdog/重启使用最新端口）。
    pub fn set_port(&self, port: u16) {
        self.lock_inner().config.port = port;
    }

    pub fn managed_port_preference(&self) -> u16 {
        self.lock_inner().managed_port_preference
    }

    pub(crate) fn phase(&self) -> BootPhase {
        self.lock_inner().phase
    }

    pub fn is_updating(&self) -> bool {
        self.lock_inner().updating
    }

    /// 原子进入更新状态，避免 UI/托盘重复触发并发更新。
    pub fn try_begin_update(&self) -> bool {
        let mut g = self.lock_inner();
        if g.updating
            || matches!(
                g.phase,
                BootPhase::ServiceChoice
                    | BootPhase::SwitchingService
                    | BootPhase::InstallingNode
                    | BootPhase::InstallingDsh
                    | BootPhase::StartingServer
            )
        {
            return false;
        }
        g.updating = true;
        true
    }

    pub fn set_updating(&self, v: bool) {
        self.lock_inner().updating = v;
    }

    pub fn set_running(&self, child: std::process::Child, job: Option<TreeGuard>) {
        let mut g = self.lock_inner();
        g.dsh_child = Some(child);
        g.job = job;
        g.service_ownership = ServiceOwnership::Managed;
        g.external_service = None;
    }

    pub fn set_external_service(&self, service: ExternalServiceCandidate) {
        let mut g = self.lock_inner();
        g.config.port = service.port;
        g.service_ownership = ServiceOwnership::External;
        g.external_service = Some(service);
    }

    pub fn mark_external_disconnected(&self) {
        let mut g = self.lock_inner();
        if g.service_ownership == ServiceOwnership::External {
            g.service_ownership = ServiceOwnership::ExternalDisconnected;
        }
    }

    pub fn set_external_disconnected(&self, service: ExternalServiceCandidate) {
        let mut g = self.lock_inner();
        g.config.port = service.port;
        g.service_ownership = ServiceOwnership::ExternalDisconnected;
        g.external_service = Some(service);
    }

    pub fn clear_service_ownership(&self) {
        let mut g = self.lock_inner();
        g.service_ownership = ServiceOwnership::None;
        g.external_service = None;
    }

    pub fn service_ownership(&self) -> ServiceOwnership {
        self.lock_inner().service_ownership
    }

    pub fn external_service(&self) -> Option<ExternalServiceCandidate> {
        self.lock_inner().external_service.clone()
    }

    pub fn set_service_candidate(&self, candidate: Option<ExternalServiceCandidate>) {
        self.lock_inner().external_service = candidate;
    }

    pub fn request_service_choice(&self, generation: u64, reuse: bool) -> bool {
        let inner = self.lock_inner();
        if inner.install_control.generation != generation
            || inner.phase != BootPhase::ServiceChoice
            || inner.external_service.is_none()
        {
            return false;
        }
        inner
            .service_choice_tx
            .send(ServiceChoice { generation, reuse })
            .is_ok()
    }

    pub(crate) fn wait_service_choice(&self, generation: u64) -> Result<bool, String> {
        loop {
            if self.is_quitting() {
                return Err(crate::locale::text("应用已退出", "The app has quit").into());
            }
            let received = self
                .service_choice_rx
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .recv_timeout(std::time::Duration::from_millis(250));
            match received {
                Ok(choice) if choice.generation == generation => return Ok(choice.reuse),
                Ok(_) | Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                    return Err(crate::locale::text(
                        "服务选择通道已关闭。",
                        "The service selection channel was closed.",
                    )
                    .into());
                }
            }
        }
    }

    pub(crate) fn set_dev_ui_job(&self, job: Option<TreeGuard>) {
        self.lock_inner().dev_ui_job = job;
    }

    /// 是否持有由本应用启动的 dsh 进程。
    pub fn has_running_process(&self) -> bool {
        self.lock_inner().dsh_child.is_some()
    }

    /// 返回已退出的状态；仍在运行或没有托管进程时为 None。
    pub fn managed_process_exit(&self) -> Result<Option<std::process::ExitStatus>, String> {
        let mut inner = self.lock_inner();
        let Some(child) = inner.dsh_child.as_mut() else {
            return Ok(None);
        };
        child.try_wait().map_err(|e| e.to_string())
    }

    /// 缓存当前 Node 版本（boot 时检测一次，snapshot 直接读取，避免高频 spawn）。
    pub fn set_node_version(&self, version: Option<String>) {
        self.lock_inner().node_version = version;
    }

    /// 缓存便携 Node 自带 npm 版本（boot 时检测一次）。
    pub fn set_npm_version(&self, version: Option<String>) {
        self.lock_inner().npm_version = version;
    }

    /// 读取缓存的 Node 版本（None 表示尚未检测）。
    pub fn node_version(&self) -> Option<String> {
        self.lock_inner().node_version.clone()
    }

    /// 自绘弹窗：记录最近一次打开载荷。
    pub fn set_last_dialog(&self, payload: crate::control_center::AppDialogOpen) {
        self.lock_inner().last_dialog = Some(payload);
    }

    /// 自绘弹窗：读取最近一次打开载荷。
    pub fn last_dialog(&self) -> Option<crate::control_center::AppDialogOpen> {
        self.lock_inner().last_dialog.clone()
    }

    // ---------- 弹窗轮询数据（事件通道对该窗口不可靠，页面轮询拉取） ----------

    pub fn set_last_balance(&self, payload: Option<crate::balance::BalancePayload>) {
        self.lock_inner().last_balance = payload;
    }
    pub fn last_balance(&self) -> Option<crate::balance::BalancePayload> {
        self.lock_inner().last_balance.clone()
    }
    pub fn set_last_check(&self, result: Option<crate::updater::CheckResult>) {
        self.lock_inner().last_check = result;
    }
    pub fn last_check(&self) -> Option<crate::updater::CheckResult> {
        self.lock_inner().last_check.clone()
    }
    pub fn set_check_progress(&self, message: Option<String>) {
        self.lock_inner().check_progress = message;
    }
    pub fn check_progress(&self) -> Option<String> {
        self.lock_inner().check_progress.clone()
    }
    pub fn set_update_done(&self, ok: bool, message: Option<String>) {
        let mut g = self.lock_inner();
        g.update_done_ok = ok;
        g.update_done = message;
    }
    pub fn update_done(&self) -> Option<(bool, String)> {
        let g = self.lock_inner();
        g.update_done.clone().map(|m| (g.update_done_ok, m))
    }

    /// 弹窗禁用/恢复主窗口的标记读写。
    pub fn set_main_disabled(&self, v: bool) {
        self.lock_inner().main_disabled = v;
    }
    pub fn main_disabled(&self) -> bool {
        self.lock_inner().main_disabled
    }

    /// 弹窗生命周期代次：打开/关闭时 +1，返回新值；
    /// 挂起的延迟动作（如关闭后的延迟隐藏）拿旧值比对，不符即失效。
    pub fn bump_dialog_gen(&self) -> u64 {
        let mut g = self.lock_inner();
        g.dialog_gen += 1;
        g.dialog_gen
    }
    pub fn dialog_gen(&self) -> u64 {
        self.lock_inner().dialog_gen
    }

    /// PowerShell 更新的弹窗内确认状态（UAC 预告）。
    pub fn set_pwsh_pending(&self, v: bool) {
        self.lock_inner().pwsh_pending = v;
    }
    pub fn pwsh_pending(&self) -> bool {
        self.lock_inner().pwsh_pending
    }
    pub fn set_pwsh_confirmed(&self, v: bool) {
        self.lock_inner().pwsh_confirmed = v;
    }
    /// PowerShell 更新的弹窗内确认状态读取（仅 Windows 的 winget 更新流程使用）。
    #[cfg_attr(not(windows), allow(dead_code))]
    pub fn pwsh_confirmed(&self) -> bool {
        self.lock_inner().pwsh_confirmed
    }

    pub fn take_running(&self) -> (Option<std::process::Child>, Option<TreeGuard>) {
        let mut g = self.lock_inner();
        let running = (g.dsh_child.take(), g.job.take());
        if g.service_ownership == ServiceOwnership::Managed {
            g.service_ownership = ServiceOwnership::None;
        }
        running
    }

    pub fn is_quitting(&self) -> bool {
        self.lock_inner().quitting
    }

    pub fn set_quitting(&self, v: bool) {
        self.lock_inner().quitting = v;
    }

    pub fn signal_retry(&self) {
        let _ = self.lock_inner().retry_tx.send(());
    }
}

#[cfg(test)]
mod tests {
    use super::{merge_section_field, onboarding_required_for};

    #[test]
    fn onboarding_gate_distinguishes_dev_processes_from_internal_navigation() {
        // 开发版无视持久化完成标记，因此每次新进程仍可测试首次设置。
        assert!(onboarding_required_for(true, false, true));
        // 同一进程已完成后，服务重启返回启动页不得再次展示。
        assert!(!onboarding_required_for(true, true, false));
    }

    #[test]
    fn onboarding_gate_keeps_production_persistence_semantics() {
        assert!(onboarding_required_for(false, false, false));
        assert!(!onboarding_required_for(false, false, true));
    }

    #[test]
    fn merge_section_field_replaces_only_the_target() {
        let source = "locale:\n  preference: zh\n  extra: keep\nui-theme:\n  preference: dark\n";
        let merged = merge_section_field(source, "locale", "preference", "en");
        assert_eq!(
            merged,
            "locale:\n  preference: en\n  extra: keep\nui-theme:\n  preference: dark\n"
        );
    }

    #[test]
    fn merge_section_field_appends_missing_field_or_section() {
        assert_eq!(
            merge_section_field("locale:\n  extra: keep\n", "locale", "preference", "zh"),
            "locale:\n  extra: keep\n  preference: zh\n"
        );
        assert_eq!(
            merge_section_field("other:\n  value: keep\n", "locale", "preference", "en"),
            "other:\n  value: keep\nlocale:\n  preference: en\n"
        );
    }

    #[test]
    fn merge_section_field_collapses_duplicate_target_fields() {
        let source = "locale:\n  preference: zh\n  preference: en\n";
        let merged = merge_section_field(source, "locale", "preference", "zh");
        assert_eq!(merged.matches("preference:").count(), 1);
    }
}
