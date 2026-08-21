//! 应用共享状态：运行时配置、引导阶段、子进程句柄。

use serde::Serialize;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, Mutex};

use crate::processes::TreeGuard;

/// 默认端口。高位端口：避开 Windows Hyper-V/WSL 动态保留段（常见于 2914~3713
/// 一带），保证开箱即用不冲突；被占用时启动流程仍会自动顺延（见 dsh.rs）。
pub const DEFAULT_PORT: u16 = 18080;
/// 应用数据根目录名；与 README 公布的各平台路径保持一致。
#[cfg(windows)]
pub const APP_DIR_NAME: &str = "DSHBox";
#[cfg(not(windows))]
pub const APP_DIR_NAME: &str = "com.deepseek.dsh-box";
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
    /// 给 dsh 子进程的 DSH_HOME（默认官方 ~/.dsh，与官方 dsh CLI 互通；
    /// 尊重官方 DSH_HOME 环境变量覆盖）。
    pub dsh_home: Option<PathBuf>,
    /// 手动指定的 DeepSeek API Key（未指定时从 dsh 凭据/环境变量读取）。
    pub api_key: Option<String>,
    /// DeepSeek API 基地址。
    pub api_base: String,
    /// Desktop shell UI language (zh-CN / en); None follows the OS.
    pub ui_language: Option<String>,
    /// 隐藏 dsh 对话中的工具调用卡片（仅保留文本消息与最终输出）。
    pub hide_tool_calls: bool,
    /// 隐藏 dsh 输入区上方的会话统计行（统计迁移到自绘状态栏，默认隐藏）。
    pub hide_stats_line: bool,
    /// 隐藏窗口底部自绘状态栏（会话统计与余额一并隐藏，默认显示）。
    pub hide_statusbar: bool,
    /// 隐藏状态栏右侧的余额 chip（统计保留，默认显示余额）。
    pub hide_balance: bool,
    /// 是否自动升级内置插件（dshmarket/dsh-file-drop，默认开启；
    /// 首次预装引导不受此开关影响）。
    pub auto_update_plugins: bool,
    /// dsh 内核更新通道："latest"（稳定推荐，默认）或 "next"（预览尝鲜）。
    pub dsh_update_channel: String,
}

impl Config {
    pub fn load() -> Config {
        let root = std::env::var("DSH_BOX_ROOT")
            .map(PathBuf::from)
            .unwrap_or_else(|_| portable_root().unwrap_or_else(default_app_root));
        let port = std::env::var("DSH_BOX_PORT")
            .ok()
            .and_then(|v| v.parse::<u16>().ok())
            .filter(|p| *p > 0)
            .unwrap_or(DEFAULT_PORT);
        let dsh_home = Some(
            std::env::var("DSH_HOME")
                .ok()
                .map(PathBuf::from)
                .unwrap_or_else(|| {
                    dirs::home_dir()
                        .map(|home| home.join(".dsh"))
                        .unwrap_or_else(|| std::env::temp_dir().join("dsh"))
                }),
        );
        let api_base = "https://api.deepseek.com".into();

        // 可选的 config.json 覆盖（环境变量优先）。
        let mut cfg = Config {
            port,
            root,
            dsh_home,
            api_key: None,
            api_base,
            ui_language: None,
            hide_tool_calls: false,
            hide_stats_line: true,
            hide_statusbar: false,
            hide_balance: false,
            auto_update_plugins: true,
            dsh_update_channel: "latest".to_string(),
        };
        let cfg_file = cfg.root.join("config.json");
        if let Ok(text) = std::fs::read_to_string(&cfg_file) {
            if let Ok(json) = serde_json::from_str::<serde_json::Value>(&text) {
                if let Some(p) = json
                    .get("port")
                    .and_then(|v| v.as_u64())
                    .and_then(|p| u16::try_from(p).ok())
                    .filter(|p| *p > 0)
                {
                    cfg.port = p;
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
                if let Some(language) = json.get("language").and_then(|v| v.as_str()) {
                    if matches!(language, "zh-CN" | "en") {
                        cfg.ui_language = Some(language.to_string());
                    }
                }
                if let Some(hide) = json.get("hide_tool_calls").and_then(|v| v.as_bool()) {
                    cfg.hide_tool_calls = hide;
                }
                if let Some(hide) = json.get("hide_stats_line").and_then(|v| v.as_bool()) {
                    cfg.hide_stats_line = hide;
                }
                if let Some(hide) = json.get("hide_statusbar").and_then(|v| v.as_bool()) {
                    cfg.hide_statusbar = hide;
                }
                if let Some(hide) = json.get("hide_balance").and_then(|v| v.as_bool()) {
                    cfg.hide_balance = hide;
                }
                if let Some(upd) = json.get("auto_update_plugins").and_then(|v| v.as_bool()) {
                    cfg.auto_update_plugins = upd;
                }
                if let Some(ch) = json.get("dsh_update_channel").and_then(|v| v.as_str()) {
                    if matches!(ch, "latest" | "next") {
                        cfg.dsh_update_channel = ch.to_string();
                    }
                }
            }
        }
        // 环境变量永远覆盖 config.json。
        if let Ok(k) = std::env::var("DSH_BOX_API_KEY") {
            if !k.is_empty() {
                cfg.api_key = Some(k);
            }
        }
        if let Ok(p) = std::env::var("DSH_BOX_PORT") {
            if let Ok(p @ 1..=u16::MAX) = p.parse::<u16>() {
                cfg.port = p;
            }
        }
        if let Ok(base) = std::env::var("DSH_BOX_API_BASE") {
            if !base.is_empty() {
                cfg.api_base = base;
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

    /// dsh 主目录（DSH_HOME）：默认官方 ~/.dsh，与官方 dsh CLI 互通；
    /// 尊重 DSH_HOME 环境变量覆盖。
    pub fn dsh_home(&self) -> PathBuf {
        self.dsh_home.clone().unwrap_or_else(|| {
            dirs::home_dir()
                .map(|home| home.join(".dsh"))
                .unwrap_or_else(std::env::temp_dir)
        })
    }

    /// 读取 dsh settings.yaml 中指定段落的字段值（顶层 `section:` 块内的
    /// `field:` 行）。行级解析，供语言/主题跟随使用。
    fn dsh_settings_value(&self, section: &str, field: &str) -> Option<String> {
        let text = std::fs::read_to_string(self.dsh_home().join("settings.yaml")).ok()?;
        let mut in_section = false;
        for line in text.lines() {
            if !line.starts_with(' ') && line.trim_end() == format!("{section}:") {
                in_section = true;
                continue;
            }
            if in_section {
                // 只认“field:”开头的行：strip_prefix 后必须以冒号开始，
                // 避免误命中 `preferences:` 等同前缀字段
                if let Some(rest) = line.trim_start().strip_prefix(field) {
                    if rest.trim_start().starts_with(':') {
                        let value = line
                            .split_once(':')
                            .map(|(_, v)| v.trim().trim_matches(['"', '\'']))?;
                        return Some(value.to_string());
                    }
                }
                if !line.starts_with(' ') && !line.is_empty() {
                    break; // 段落结束
                }
            }
        }
        None
    }

    /// 读取 dsh 的语言偏好（`locale.preference`：zh|en）→ 应用语言 id。
    pub fn load_dsh_locale(&self) -> Option<&'static str> {
        match self.dsh_settings_value("locale", "preference")?.as_str() {
            "zh" => Some("zh-CN"),
            "en" => Some("en"),
            _ => None,
        }
    }

    /// 读取 dsh 的主题偏好（`ui-theme.preference`：light|dark|system）。
    pub fn load_dsh_theme(&self) -> Option<&'static str> {
        match self.dsh_settings_value("ui-theme", "preference")?.as_str() {
            "light" => Some("light"),
            "dark" => Some("dark"),
            "system" => Some("system"),
            _ => None,
        }
    }

    /// dsh 主题偏好 → 窗口主题：light/dark 固定主题（WebView 的
    /// prefers-color-scheme 随之固定），system 或缺失 → None 跟随系统。
    /// 所有窗口（主窗口/弹窗/托盘菜单）首次创建与主题切换共用同一解析。
    pub fn resolve_dsh_theme(&self) -> Option<tauri::Theme> {
        match self.load_dsh_theme() {
            Some("light") => Some(tauri::Theme::Light),
            Some("dark") => Some(tauri::Theme::Dark),
            _ => None,
        }
    }

    /// 把语言偏好写入 dsh 的 settings.yaml（`locale.preference: zh|en`）。
    /// dsh 的 settings-file 提供者有文件监视器，外部编辑会被热发布，
    /// 界面语言无需重载即切换。仅做行级合并，不触碰其他段落。
    pub fn save_dsh_locale(&self, language: &str) -> Result<(), String> {
        let path = self.dsh_home().join("settings.yaml");
        let new_line = format!("  preference: {language}");
        let text = std::fs::read_to_string(&path).unwrap_or_default();
        let mut out = String::new();
        let mut in_locale = false;
        let mut wrote = false;
        for line in text.lines() {
            if line.starts_with("locale:") && !line.starts_with(' ') {
                in_locale = true;
                out.push_str(line);
                out.push('\n');
                continue;
            }
            if in_locale {
                if let Some(rest) = line.trim_start().strip_prefix("preference") {
                    // 替换 locale 段内的 preference 行（只认 `preference:`，
                    // 不误伤 `preferences:` 等同前缀字段）
                    if rest.trim_start().starts_with(':') {
                        if !wrote {
                            out.push_str(&new_line);
                            out.push('\n');
                            wrote = true;
                        }
                        continue;
                    }
                }
                if !line.starts_with(' ') && !line.is_empty() {
                    // locale 段结束：在其末尾补 preference 行
                    if !wrote {
                        out.push_str(&new_line);
                        out.push('\n');
                        wrote = true;
                    }
                    in_locale = false;
                }
            }
            out.push_str(line);
            out.push('\n');
        }
        if !wrote {
            if in_locale {
                // locale 是最后一个段落且没有 preference 行
                out.push_str(&new_line);
                out.push('\n');
            } else {
                // 追加新的 locale 段落
                if !out.is_empty() && !out.ends_with('\n') {
                    out.push('\n');
                }
                out.push_str("locale:\n");
                out.push_str(&new_line);
                out.push('\n');
            }
        }
        // 复用 config.json 的原子写助手（临时文件 + fsync + Windows
        // ReplaceFileW 原子替换）：崩溃/断电不会把用户 settings.yaml 截断
        // 成不完整文件，dsh 的监视器与跟随线程也不会读到中间态
        atomic_write(&path, &out)
    }

    /// 把主题偏好写入 dsh 的 settings.yaml（`ui-theme.preference: light|dark|system`）。
    /// 与 save_dsh_locale 同规格：行级合并、原子写；dsh 的文件监视器会热发布。
    pub fn save_dsh_theme(&self, theme: &str) -> Result<(), String> {
        if !matches!(theme, "light" | "dark" | "system") {
            return Err("Unsupported theme".into());
        }
        let path = self.dsh_home().join("settings.yaml");
        let new_line = format!("  preference: {theme}");
        let text = std::fs::read_to_string(&path).unwrap_or_default();
        let mut out = String::new();
        let mut in_theme = false;
        let mut wrote = false;
        for line in text.lines() {
            if line.starts_with("ui-theme:") && !line.starts_with(' ') {
                in_theme = true;
                out.push_str(line);
                out.push('\n');
                continue;
            }
            if in_theme {
                if let Some(rest) = line.trim_start().strip_prefix("preference") {
                    if rest.trim_start().starts_with(':') {
                        if !wrote {
                            out.push_str(&new_line);
                            out.push('\n');
                            wrote = true;
                        }
                        continue;
                    }
                }
                if !line.starts_with(' ') && !line.is_empty() {
                    if !wrote {
                        out.push_str(&new_line);
                        out.push('\n');
                        wrote = true;
                    }
                    in_theme = false;
                }
            }
            out.push_str(line);
            out.push('\n');
        }
        if !wrote {
            if in_theme {
                out.push_str(&new_line);
                out.push('\n');
            } else {
                if !out.is_empty() && !out.ends_with('\n') {
                    out.push('\n');
                }
                out.push_str("ui-theme:\n");
                out.push_str(&new_line);
                out.push('\n');
            }
        }
        atomic_write(&path, &out)
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
        if let Err(e) = save_config_value(
            &self.root,
            "window",
            serde_json::json!({ "lx": x, "ly": y, "lw": w, "lh": h }),
        ) {
            crate::logging::log(&format!("config: 保存窗口状态失败：{e}"));
        }
    }

    fn save_language(&self, language: &str) -> Result<(), String> {
        save_config_value(
            &self.root,
            "language",
            serde_json::Value::String(language.to_string()),
        )
    }
}

pub(crate) fn default_app_root() -> PathBuf {
    dirs::data_local_dir()
        .or_else(dirs::home_dir)
        .unwrap_or_else(std::env::temp_dir)
        .join(APP_DIR_NAME)
}

/// 便携模式：exe 同级存在 `portable.txt` 时，数据目录跟随 exe（exe 旁 `data/`）。
/// 显式标记避免误判；删除标记即恢复常规模式。环境变量 `DSH_BOX_ROOT` 优先。
fn portable_root() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?;
    if dir.join("portable.txt").is_file() {
        Some(dir.join("data"))
    } else {
        None
    }
}

static CONFIG_WRITE_LOCK: Mutex<()> = Mutex::new(());

pub(crate) fn save_config_value(
    root: &Path,
    key: &str,
    value: serde_json::Value,
) -> Result<(), String> {
    let _guard = CONFIG_WRITE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let path = root.join("config.json");
    let mut json = match std::fs::read_to_string(&path) {
        Ok(text) => serde_json::from_str::<serde_json::Value>(&text)
            .map_err(|e| format!("config.json 解析失败，已保留原文件：{e}"))?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => serde_json::json!({}),
        Err(e) => return Err(format!("读取 config.json 失败：{e}")),
    };
    let object = json
        .as_object_mut()
        .ok_or_else(|| "config.json 顶层不是对象，已保留原文件".to_string())?;
    object.insert(key.to_string(), value);
    let text = serde_json::to_string_pretty(&json).map_err(|e| e.to_string())?;
    atomic_write(&path, &text)
}

pub(crate) fn atomic_write(path: &Path, text: &str) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    // 临时文件统一用 .json.tmp 后缀：即使目标是 settings.yaml，也刻意不
    // 使用 .yaml 后缀，避免 dsh 的文件监视器在原子替换前读到半成品
    let temp = path.with_extension("json.tmp");
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(&temp)
        .map_err(|e| e.to_string())?;
    file.write_all(text.as_bytes()).map_err(|e| e.to_string())?;
    file.sync_all().map_err(|e| e.to_string())?;
    drop(file);
    if let Err(e) = replace_file(&temp, path) {
        let _ = std::fs::remove_file(&temp);
        return Err(e.to_string());
    }
    Ok(())
}

#[cfg(windows)]
fn replace_file(temp: &Path, target: &Path) -> std::io::Result<()> {
    if !target.exists() {
        return std::fs::rename(temp, target);
    }
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::ReplaceFileW;
    let target: Vec<u16> = target.as_os_str().encode_wide().chain(Some(0)).collect();
    let temp: Vec<u16> = temp.as_os_str().encode_wide().chain(Some(0)).collect();
    let ok = unsafe {
        ReplaceFileW(
            target.as_ptr(),
            temp.as_ptr(),
            std::ptr::null(),
            0,
            std::ptr::null(),
            std::ptr::null(),
        )
    };
    if ok == 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(not(windows))]
fn replace_file(temp: &Path, target: &Path) -> std::io::Result<()> {
    std::fs::rename(temp, target)
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

/// 首次配置是否已由用户完成（保存/跳过）。boot 等待用：
/// dev 构建下 onboarding_pending 恒 true，等待判定必须以用户动作为准，
/// 否则保存后仍等满 60 秒兜底超时才继续。
static ONBOARDING_DONE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub(crate) fn mark_onboarding_done() {
    ONBOARDING_DONE.store(true, std::sync::atomic::Ordering::Relaxed);
}

pub(crate) fn onboarding_done() -> bool {
    ONBOARDING_DONE.load(std::sync::atomic::Ordering::Relaxed)
}

/// 首次配置面板是否已在启动页显示（前端显示后回报）。
/// boot 等待据此区分：面板已显示则无限等待用户操作（有跳过按钮，
/// 无需兜底）；未显示（启动页异常）60 秒后自动放行防卡死。
static ONBOARDING_SHOWN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub(crate) fn mark_onboarding_shown() {
    ONBOARDING_SHOWN.store(true, std::sync::atomic::Ordering::Relaxed);
}

pub(crate) fn onboarding_shown() -> bool {
    ONBOARDING_SHOWN.load(std::sync::atomic::Ordering::Relaxed)
}

pub(crate) struct Inner {
    config: Config,
    phase: BootPhase,
    message: String,
    detail: String,
    /// dsh 子进程树守卫（关闭即回收：Windows Job / Unix 进程组）。
    job: Option<TreeGuard>,
    /// 开发模式 UI 静态服务器守卫；正式版始终为 None。
    dev_ui_job: Option<TreeGuard>,
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
    /// PowerShell 更新的 UAC 预告在弹窗内等待确认；点击“继续”后置位。
    pwsh_pending: bool,
    pwsh_confirmed: bool,
    /// 最近一次 dsh 页面心跳（页面主线程存活标记）。
    last_heartbeat: Option<std::time::Instant>,
    /// 连续页面重载次数（指数退避）。
    heartbeat_failures: u32,
    /// 已后台预下载的应用更新版本（exe 更新用，Windows 专属）。
    #[cfg(windows)]
    app_update_ready: Option<String>,
}

/// 全局状态（跨线程共享）。
pub struct AppState {
    inner: Arc<Mutex<Inner>>,
    /// 生命周期锁：引导/重启/更新互斥，杜绝双服务并发。
    lifecycle: Mutex<()>,
    /// 仅注入 dsh 主页面的自定义协议随机令牌。
    protocol_token: String,
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
        let inner = Inner {
            config,
            phase: BootPhase::Starting,
            message: String::new(),
            detail: String::new(),
            job: None,
            dev_ui_job: None,
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
            pwsh_pending: false,
            pwsh_confirmed: false,
            last_heartbeat: None,
            heartbeat_failures: 0,
            #[cfg(windows)]
            app_update_ready: None,
        };
        AppState {
            inner: Arc::new(Mutex::new(inner)),
            lifecycle: Mutex::new(()),
            protocol_token,
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

    /// 连续重载计数 +1，返回新值（指数退避用）。
    pub(crate) fn bump_heartbeat_failures(&self) -> u32 {
        let mut inner = self.lock_inner();
        inner.heartbeat_failures = inner.heartbeat_failures.saturating_add(1);
        inner.heartbeat_failures
    }

    /// 已后台预下载的应用更新版本（无则为 None）。
    #[cfg(windows)]
    pub(crate) fn app_update_ready(&self) -> Option<String> {
        self.lock_inner().app_update_ready.clone()
    }

    /// 记录/清除已预下载的应用更新版本。
    #[cfg(windows)]
    pub(crate) fn set_app_update_ready(&self, version: Option<String>) {
        self.lock_inner().app_update_ready = version;
    }

    pub(crate) fn protocol_token(&self) -> &str {
        &self.protocol_token
    }

    pub fn config(&self) -> Config {
        self.lock_inner().config.clone()
    }

    pub fn set_ui_language(&self, language: &str) -> Result<(), String> {
        if !matches!(language, "zh-CN" | "en") {
            return Err("Unsupported UI language".into());
        }
        let config = self.config();
        config.save_language(language)?;
        self.lock_inner().config.ui_language = Some(language.to_string());
        crate::locale::set_preference(Some(language));
        Ok(())
    }

    /// 切换“隐藏工具调用”开关，持久化到 config.json，返回新值。
    pub fn toggle_hide_tool_calls(&self) -> Result<bool, String> {
        let config = self.config();
        let next = !config.hide_tool_calls;
        save_config_value(
            &config.root,
            "hide_tool_calls",
            serde_json::Value::Bool(next),
        )?;
        self.lock_inner().config.hide_tool_calls = next;
        Ok(next)
    }

    /// 切换“隐藏会话统计行”开关，持久化到 config.json，返回新值。
    pub fn toggle_hide_stats_line(&self) -> Result<bool, String> {
        let config = self.config();
        let next = !config.hide_stats_line;
        save_config_value(
            &config.root,
            "hide_stats_line",
            serde_json::Value::Bool(next),
        )?;
        self.lock_inner().config.hide_stats_line = next;
        Ok(next)
    }

    /// 切换“隐藏状态栏”开关，持久化到 config.json，返回新值。
    pub fn toggle_hide_statusbar(&self) -> Result<bool, String> {
        let config = self.config();
        let next = !config.hide_statusbar;
        save_config_value(
            &config.root,
            "hide_statusbar",
            serde_json::Value::Bool(next),
        )?;
        self.lock_inner().config.hide_statusbar = next;
        Ok(next)
    }

    /// 切换“隐藏余额 chip”开关，持久化到 config.json，返回新值。
    pub fn toggle_hide_balance(&self) -> Result<bool, String> {
        let config = self.config();
        let next = !config.hide_balance;
        save_config_value(&config.root, "hide_balance", serde_json::Value::Bool(next))?;
        self.lock_inner().config.hide_balance = next;
        Ok(next)
    }

    /// 切换“自动升级内置插件”开关，持久化到 config.json，返回新值。
    pub fn toggle_auto_update_plugins(&self) -> Result<bool, String> {
        let config = self.config();
        let next = !config.auto_update_plugins;
        save_config_value(
            &config.root,
            "auto_update_plugins",
            serde_json::Value::Bool(next),
        )?;
        self.lock_inner().config.auto_update_plugins = next;
        Ok(next)
    }

    /// 设置 dsh 内核更新通道（latest/next），持久化到 config.json。
    pub fn set_dsh_update_channel(&self, channel: &str) -> Result<(), String> {
        if !matches!(channel, "latest" | "next") {
            return Err(
                crate::locale::text("未知更新通道。", "Unknown update channel.").to_string(),
            );
        }
        let config = self.config();
        save_config_value(
            &config.root,
            "dsh_update_channel",
            serde_json::json!(channel),
        )?;
        self.lock_inner().config.dsh_update_channel = channel.to_string();
        Ok(())
    }

    /// 首次使用配置是否尚未完成：仅当数据目录完全没有 config.json（全新安装）
    /// 时才需要引导。老用户升级（config.json 已存在，含窗口记忆/语言等）一律
    /// 跳过——此前按 onboarded 标记判断会把升级用户误判为首次，导致 boot
    /// 永久等待配置而无法进入 dsh 界面。dev 构建恒为 true（每次启动展示
    /// 首次配置界面，便于测试；正式构建不受影响）。
    pub(crate) fn onboarding_pending(&self) -> bool {
        dev_build() || !self.config().root.join("config.json").is_file()
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
        // 缓存缺失时即时检测一次：启动页首帧就显示完整的版本信息
        // （Node 版本由 boot 线程稍后检测，直接等会导致信息出现太晚、
        // 启动快时刚显示就随页面导航消失）
        let node_version = if node_version.is_some() {
            node_version
        } else {
            let version = crate::runtime::current_node_version(&config);
            if version.is_some() {
                self.set_node_version(version.clone());
            }
            version
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

    pub(crate) fn set_dev_ui_job(&self, job: Option<TreeGuard>) {
        self.lock_inner().dev_ui_job = job;
    }

    /// 是否持有由本应用启动的 dsh 进程。
    pub fn has_running_process(&self) -> bool {
        self.lock_inner().dsh_pid.is_some()
    }

    /// 缓存当前 Node 版本（boot 时检测一次，snapshot 直接读取，避免高频 spawn）。
    pub fn set_node_version(&self, version: Option<String>) {
        self.lock_inner().node_version = version;
    }

    /// 读取缓存的 Node 版本（None 表示尚未检测）。
    pub fn node_version(&self) -> Option<String> {
        self.lock_inner().node_version.clone()
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
