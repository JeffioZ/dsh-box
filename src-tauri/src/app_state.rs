//! 应用共享状态：运行时配置、引导阶段、子进程句柄。

use serde::Serialize;
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, Mutex};

use crate::processes::TreeGuard;

/// 默认端口（与 dsh web 默认一致）。
pub const DEFAULT_PORT: u16 = 3080;
/// 应用数据根目录名（位于 %LOCALAPPDATA% 下）。
pub const APP_DIR_NAME: &str = "DSHDesktop";
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BootPhase {
    Starting,
    InstallingNode,
    InstallingDsh,
    StartingServer,
    Ready,
    Error,
}

impl BootPhase {
    pub fn as_str(&self) -> &'static str {
        match self {
            BootPhase::Starting => "starting",
            BootPhase::InstallingNode => "installing-node",
            BootPhase::InstallingDsh => "installing-dsh",
            BootPhase::StartingServer => "starting-server",
            BootPhase::Ready => "ready",
            BootPhase::Error => "error",
        }
    }
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
    /// 实际监听端口。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
}

/// 运行时配置。
#[derive(Clone)]
pub struct Config {
    /// dsh web 监听端口。
    pub port: u16,
    /// 应用数据根目录（node/、dsh/、logs/、config.json 所在处）。
    pub root: PathBuf,
    /// 给 dsh 子进程的 DSH_HOME（默认不设置，沿用系统默认 ~/.dsh）。
    pub dsh_home: Option<PathBuf>,
    /// 手动指定的 DeepSeek API Key（未指定时从 dsh 凭据/环境变量读取）。
    pub api_key: Option<String>,
    /// DeepSeek API 基地址。
    pub api_base: String,
}

impl Config {
    pub fn load() -> Config {
        let root = std::env::var("DSH_DESKTOP_ROOT")
            .map(PathBuf::from)
            .unwrap_or_else(|_| {
                let base = std::env::var("LOCALAPPDATA").unwrap_or_else(|_| ".".into());
                PathBuf::from(base).join(APP_DIR_NAME)
            });
        let port = std::env::var("DSH_DESKTOP_PORT")
            .ok()
            .and_then(|v| v.parse::<u16>().ok())
            .unwrap_or(DEFAULT_PORT);
        let dsh_home = std::env::var("DSH_DESKTOP_DSH_HOME")
            .ok()
            .map(PathBuf::from);
        let api_base = std::env::var("DSH_DESKTOP_API_BASE")
            .unwrap_or_else(|_| "https://api.deepseek.com".into());

        // 可选的 config.json 覆盖（环境变量优先）。
        let mut cfg = Config {
            port,
            root,
            dsh_home,
            api_key: None,
            api_base,
        };
        let cfg_file = cfg.root.join("config.json");
        if let Ok(text) = std::fs::read_to_string(&cfg_file) {
            if let Ok(json) = serde_json::from_str::<serde_json::Value>(&text) {
                if let Some(p) = json.get("port").and_then(|v| v.as_u64()) {
                    cfg.port = p as u16;
                }
                if let Some(k) = json.get("api_key").and_then(|v| v.as_str()) {
                    if !k.is_empty() {
                        cfg.api_key = Some(k.to_string());
                    }
                }
                if let Some(b) = json.get("api_base").and_then(|v| v.as_str()) {
                    if !b.is_empty() {
                        cfg.api_base = b.to_string();
                    }
                }
            }
        }
        // 环境变量永远覆盖 config.json。
        if let Ok(k) = std::env::var("DSH_DESKTOP_API_KEY") {
            if !k.is_empty() {
                cfg.api_key = Some(k);
            }
        }
        if let Ok(p) = std::env::var("DSH_DESKTOP_PORT") {
            if let Ok(p) = p.parse::<u16>() {
                cfg.port = p;
            }
        }
        cfg
    }

    pub fn node_dir(&self) -> PathBuf {
        self.root.join("node")
    }
    pub fn node_exe(&self) -> PathBuf {
        #[cfg(windows)]
        {
            self.node_dir().join("node.exe")
        }
        #[cfg(not(windows))]
        {
            self.node_dir().join("node")
        }
    }
    pub fn dsh_dir(&self) -> PathBuf {
        self.root.join("dsh")
    }
    /// dsh CLI 入口：优先按已安装包的 package.json#bin.dsh 解析，兜底 lib/bin.js。
    pub fn dsh_entry(&self) -> PathBuf {
        let pkg_dir = self.dsh_dir().join("node_modules/@deepseek-ai/dsh");
        let pkg_file = pkg_dir.join("package.json");
        if let Ok(text) = std::fs::read_to_string(&pkg_file) {
            if let Ok(json) = serde_json::from_str::<serde_json::Value>(&text) {
                let rel = json.get("bin").and_then(|bin| match bin {
                    serde_json::Value::String(s) => Some(s.clone()),
                    serde_json::Value::Object(map) => {
                        map.get("dsh").and_then(|v| v.as_str()).map(String::from)
                    }
                    _ => None,
                });
                if let Some(rel) = rel {
                    let p = pkg_dir.join(rel);
                    if p.exists() {
                        return p;
                    }
                }
            }
        }
        pkg_dir.join("lib/bin.js")
    }
    pub fn logs_dir(&self) -> PathBuf {
        self.root.join("logs")
    }
    pub fn dsh_log(&self) -> PathBuf {
        self.logs_dir().join("dsh.log")
    }
    pub fn web_url(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }

    /// 读取 config.json 中持久化的窗口矩形（逻辑坐标 lx/ly/lw/lh：与 DPI 无关，
    /// 跨不同缩放显示器切换时观感尺寸一致）。旧格式（物理 x/y/w/h）读不到即返回 None。
    pub fn load_window_rect(&self) -> Option<(f64, f64, f64, f64)> {
        let text = std::fs::read_to_string(self.root.join("config.json")).ok()?;
        let json: serde_json::Value = serde_json::from_str(&text).ok()?;
        let w = json.get("window")?;
        Some((
            w.get("lx")?.as_f64()?,
            w.get("ly")?.as_f64()?,
            w.get("lw")?.as_f64()?,
            w.get("lh")?.as_f64()?,
        ))
    }

    /// 把窗口矩形（逻辑坐标）写入 config.json（保留其他字段）。
    pub fn save_window_rect(&self, x: f64, y: f64, w: f64, h: f64) {
        let path = self.root.join("config.json");
        let mut json = std::fs::read_to_string(&path)
            .ok()
            .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
            .unwrap_or_else(|| serde_json::json!({}));
        if let Some(obj) = json.as_object_mut() {
            obj.insert(
                "window".into(),
                serde_json::json!({ "lx": x, "ly": y, "lw": w, "lh": h }),
            );
        }
        let _ = std::fs::write(
            path,
            serde_json::to_string_pretty(&json).unwrap_or_default(),
        );
    }
}

pub(crate) struct Inner {
    config: Config,
    phase: BootPhase,
    message: String,
    detail: String,
    /// dsh 子进程树守卫（关闭即回收：Windows Job / Unix 进程组）。
    job: Option<TreeGuard>,
    dsh_pid: Option<u32>,
    quitting: bool,
    /// 更新进行中（看门狗跳过自动重启）。
    updating: bool,
    retry_tx: Sender<()>,
    /// 当前使用 Node 的版本（boot 时检测一次缓存，get_status 免 spawn）。
    node_version: Option<String>,
    /// 自绘弹窗最近一次打开载荷（app_dialog_get 拉取用）。
    last_dialog: Option<crate::app_dialog::AppDialogOpen>,
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
}

/// 全局状态（跨线程共享）。
pub struct AppState {
    inner: Arc<Mutex<Inner>>,
    /// 生命周期锁：引导/重启/更新互斥，杜绝双服务并发。
    lifecycle: Mutex<()>,
}

/// boot_loop 的“重试”信号接收端（启动时存入，仅取一次）。
pub static RETRY_RX: std::sync::Mutex<Option<Receiver<()>>> = std::sync::Mutex::new(None);

impl AppState {
    pub fn new() -> AppState {
        let (retry_tx, retry_rx) = std::sync::mpsc::channel::<()>();
        *RETRY_RX.lock().unwrap_or_else(|e| e.into_inner()) = Some(retry_rx);
        let config = Config::load();
        let inner = Inner {
            config,
            phase: BootPhase::Starting,
            message: String::new(),
            detail: String::new(),
            job: None,
            dsh_pid: None,
            quitting: false,
            updating: false,
            retry_tx,
            node_version: None,
            last_dialog: None,
            last_balance: None,
            last_check: None,
            check_progress: None,
            update_done_ok: false,
            update_done: None,
            main_disabled: false,
            dialog_gen: 0,
        };
        AppState {
            inner: Arc::new(Mutex::new(inner)),
            lifecycle: Mutex::new(()),
        }
    }

    pub(crate) fn lock_inner(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// 获取生命周期锁（引导/重启/更新串行化）。
    pub(crate) fn lifecycle_guard(&self) -> std::sync::MutexGuard<'_, ()> {
        self.lifecycle.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn config(&self) -> Config {
        self.lock_inner().config.clone()
    }

    pub fn snapshot(&self) -> StatusPayload {
        let (phase, message, detail, port, config, node_version) = {
            let g = self.lock_inner();
            (
                g.phase,
                g.message.clone(),
                g.detail.clone(),
                g.config.port,
                g.config.clone(),
                g.node_version.clone(),
            )
        };
        StatusPayload {
            phase: phase.as_str().to_string(),
            message,
            detail,
            progress: None,
            dsh_version: crate::runtime::installed_dsh_version(&config),
            node_version,
            port: Some(port),
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
                BootPhase::InstallingNode | BootPhase::InstallingDsh | BootPhase::StartingServer
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

    pub fn set_running(&self, pid: u32, job: Option<TreeGuard>) {
        let mut g = self.lock_inner();
        g.dsh_pid = Some(pid);
        g.job = job;
    }

    /// 缓存当前 Node 版本（boot 时检测一次，snapshot 直接读取，避免高频 spawn）。
    pub fn set_node_version(&self, version: Option<String>) {
        self.lock_inner().node_version = version;
    }

    /// 自绘弹窗：记录最近一次打开载荷。
    pub fn set_last_dialog(&self, payload: crate::app_dialog::AppDialogOpen) {
        self.lock_inner().last_dialog = Some(payload);
    }

    /// 自绘弹窗：读取最近一次打开载荷。
    pub fn last_dialog(&self) -> Option<crate::app_dialog::AppDialogOpen> {
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

    pub fn take_running(&self) -> (Option<u32>, Option<TreeGuard>) {
        let mut g = self.lock_inner();
        (g.dsh_pid.take(), g.job.take())
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
