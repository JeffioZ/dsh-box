//! 用户配置解析、路径约定与 dsh 设置读写。

use std::path::PathBuf;

use super::managed_file::merge_section_field;
use super::{load_state_value, save_state_value, update_text_file};

/// 默认端口。高位端口：避开 Windows Hyper-V/WSL 动态保留段（常见于 2914~3713
/// 一带），保证开箱即用不冲突；被占用时启动流程仍会自动顺延（见 dsh.rs）。
pub const DEFAULT_PORT: u16 = 18080;
/// 应用数据根目录名；与 README 公布的各平台路径保持一致。
#[cfg(windows)]
pub const APP_DIR_NAME: &str = "DSHBox";
#[cfg(not(windows))]
pub const APP_DIR_NAME: &str = "com.deepseek.dsh-box";
/// 运行时配置。
#[derive(Clone)]
pub struct Config {
    /// dsh web 监听端口。
    pub port: u16,
    /// 应用数据根目录（node/、dsh/、logs/、config.json、state.json 所在处）。
    pub root: PathBuf,
    /// 给 dsh 子进程的 DSH_HOME（默认官方 ~/.dsh，与官方 dsh CLI 互通；
    /// 尊重官方 DSH_HOME 环境变量覆盖）。
    pub dsh_home: PathBuf,
    /// DeepSeek API 基地址。
    pub api_base: String,
    /// 桌面外壳界面语言（zh-CN / en）；None 跟随系统。
    pub ui_language: Option<String>,
    /// 隐藏 dsh 对话中的工具调用卡片（仅保留文本消息与最终输出）。
    pub hide_tool_calls: bool,
    /// 隐藏 dsh 输入区上方的会话统计行（统计迁移到自绘状态栏，默认隐藏）。
    pub hide_stats_line: bool,
    /// 隐藏窗口底部自绘状态栏（会话统计与余额一并隐藏，默认显示）。
    pub hide_statusbar: bool,
    /// 隐藏状态栏右侧的余额 chip（统计保留，默认显示余额）。
    pub hide_balance: bool,
    /// 是否自动升级内置插件清单中的包（默认开启；
    /// 首次预装引导不受此开关影响）。
    pub auto_update_plugins: bool,
    /// 主窗口不可见时，任务完成后是否发系统通知。
    pub task_notifications: bool,
    /// 每日用量提醒阈值（单位：百万 token；None/0 = 关闭）。
    /// 预计今日用量越过该值时发一次系统通知（每天至多一次）。
    pub usage_token_limit_m: Option<u64>,
    /// dsh 内核更新通道："latest"（稳定推荐，默认）或 "next"（预览尝鲜）。
    pub dsh_update_channel: String,
    /// 主窗口关闭按钮行为：tray（隐藏到托盘）或 quit（退出应用）。
    pub close_behavior: String,
    /// 普通启动后的呈现方式：window（显示窗口）或 tray（仅驻留托盘）。
    pub launch_behavior: String,
    /// 托管运行时下载策略：auto（官方失败切镜像）/ official / mirror。
    pub download_source: String,
}

impl Config {
    pub fn load() -> Config {
        // 空字符串按未设置处理（与空环境变量的常见惯例一致）
        let root = std::env::var("DSH_BOX_ROOT")
            .ok()
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| portable_root().unwrap_or_else(default_app_root));
        let dsh_home = std::env::var("DSH_HOME")
            .ok()
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                dirs::home_dir()
                    .map(|home| home.join(".dsh"))
                    .unwrap_or_else(|| std::env::temp_dir().join("dsh"))
            });
        let api_base = "https://api.deepseek.com".into();

        // 可选的 config.json 覆盖（环境变量优先）。
        let mut cfg = Config {
            port: DEFAULT_PORT,
            root,
            dsh_home,
            api_base,
            ui_language: None,
            hide_tool_calls: false,
            hide_stats_line: true,
            hide_statusbar: false,
            hide_balance: false,
            auto_update_plugins: true,
            task_notifications: true,
            usage_token_limit_m: None,
            dsh_update_channel: "latest".to_string(),
            close_behavior: "tray".to_string(),
            launch_behavior: "window".to_string(),
            download_source: "auto".to_string(),
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
                if let Some(v) = json.get("task_notifications").and_then(|v| v.as_bool()) {
                    cfg.task_notifications = v;
                }
                // 阈值单位：百万 token。负数/0 无意义，读取时一并归一为关闭。
                if let Some(limit) = json
                    .get("usage_token_limit_m")
                    .and_then(|v| v.as_u64().or_else(|| v.as_f64().map(|f| f.max(0.0) as u64)))
                {
                    cfg.usage_token_limit_m = (limit > 0).then_some(limit);
                }
                if let Some(ch) = json.get("dsh_update_channel").and_then(|v| v.as_str()) {
                    if matches!(ch, "latest" | "next" | "alpha") {
                        cfg.dsh_update_channel = ch.to_string();
                    }
                }
                if let Some(value) = json.get("close_behavior").and_then(|v| v.as_str()) {
                    if matches!(value, "tray" | "quit") {
                        cfg.close_behavior = value.to_string();
                    }
                }
                if let Some(value) = json.get("launch_behavior").and_then(|v| v.as_str()) {
                    if matches!(value, "window" | "tray") {
                        cfg.launch_behavior = value.to_string();
                    }
                }
                if let Some(value) = json.get("download_source").and_then(|v| v.as_str()) {
                    if matches!(value, "auto" | "official" | "mirror") {
                        cfg.download_source = value.to_string();
                    }
                }
            } else {
                crate::logging::log(&format!(
                    "config: config.json 解析失败，按默认值运行（{}）",
                    cfg_file.display()
                ));
            }
        }
        // 环境变量永远覆盖 config.json。
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
    /// DSHBox 自管的包管理器目录。与 dsh 安装目录分离，更新/回滚 dsh 时
    /// pnpm 仍然可用，也不会依赖用户 PATH 中恰好安装了哪个版本。
    pub fn package_manager_dir(&self) -> PathBuf {
        self.root.join("package-manager")
    }
    pub fn package_manager_bin_dir(&self) -> PathBuf {
        self.package_manager_dir().join("node_modules/.bin")
    }
    pub fn pnpm_cli(&self) -> PathBuf {
        self.package_manager_dir()
            .join("node_modules/pnpm/bin/pnpm.mjs")
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
    pub fn dsh_home(&self) -> &std::path::Path {
        &self.dsh_home
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
        if !matches!(language, "zh" | "en") {
            return Err("Unsupported locale".into());
        }
        let path = self.dsh_home().join("settings.yaml");
        update_text_file(&path, |text| {
            Ok(merge_section_field(&text, "locale", "preference", language))
        })
    }

    /// 把主题偏好写入 dsh 的 settings.yaml（`ui-theme.preference: light|dark|system`）。
    /// 与 save_dsh_locale 同规格：行级合并、原子写；dsh 的文件监视器会热发布。
    pub fn save_dsh_theme(&self, theme: &str) -> Result<(), String> {
        if !matches!(theme, "light" | "dark" | "system") {
            return Err("Unsupported theme".into());
        }
        let path = self.dsh_home().join("settings.yaml");
        update_text_file(&path, |text| {
            Ok(merge_section_field(&text, "ui-theme", "preference", theme))
        })
    }

    /// 读取内部状态中的窗口矩形（逻辑坐标 lx/ly/lw/lh：与 DPI 无关，
    /// 跨不同缩放显示器切换时观感尺寸一致）。
    pub fn load_window_rect(&self) -> Option<(f64, f64, f64, f64)> {
        let w = load_state_value(&self.root, "window")?;
        Some((
            w.get("lx")?.as_f64()?,
            w.get("ly")?.as_f64()?,
            w.get("lw")?.as_f64()?,
            w.get("lh")?.as_f64()?,
        ))
    }

    /// 把窗口矩形（逻辑坐标）写入 state.json（保留其他字段）。
    pub fn save_window_rect(&self, x: f64, y: f64, w: f64, h: f64) {
        if let Err(e) = save_state_value(
            &self.root,
            "window",
            serde_json::json!({ "lx": x, "ly": y, "lw": w, "lh": h }),
        ) {
            crate::logging::log(&format!("state: 保存窗口状态失败：{e}"));
        }
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

#[cfg(test)]
mod tests {
    use super::Config;

    #[test]
    fn task_notifications_defaults_on_and_reads_config_override() {
        // 空目录：无 config.json，默认开启。
        let root = std::env::temp_dir().join(format!(
            "dshbox-config-default-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let prev = std::env::var("DSH_BOX_ROOT").ok();
        std::env::set_var("DSH_BOX_ROOT", &root);
        let default = Config::load();
        assert!(default.task_notifications);

        // 显式关闭：解析后尊重写入值。
        std::fs::write(root.join("config.json"), r#"{"task_notifications":false}"#).unwrap();
        let overridden = Config::load();
        assert!(!overridden.task_notifications);

        match prev {
            Some(value) => std::env::set_var("DSH_BOX_ROOT", value),
            None => std::env::remove_var("DSH_BOX_ROOT"),
        }
        let _ = std::fs::remove_dir_all(root);
    }
}
