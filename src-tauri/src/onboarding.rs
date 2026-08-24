//! 首次使用配置（onboarding）：启动页上的一次性引导。
//!
//! 提供 API Key、界面语言、主题与开机自启动四项配置。全部写入 dsh 的
//! 原生配置文件（`$DSH_HOME/.credentials.yaml`、`$DSH_HOME/settings.yaml`）
//! 与外壳用户配置——不修改 dsh 代码，dsh 界面即时可见。
//!
//! 完成标记写入内部 `state.json`。

use serde::{Deserialize, Serialize};
use tauri::AppHandle;
use tauri::Manager;

use crate::app_state::{self, AppState, BootPhase};

/// dsh 官方 DeepSeek provider 的凭据键名（dsh 凭据文件是 环境变量名: 值 的映射）。
const DEEPSEEK_API_KEY_NAME: &str = "DEEPSEEK_API_KEY";

#[derive(Serialize)]
pub struct OnboardingState {
    /// 是否显示首次配置。
    pub needs_onboarding: bool,
    /// 是否已通过环境变量或 dsh 凭据文件配置 API Key。
    pub api_key_set: bool,
    /// 当前界面语言（zh-CN / en）。
    pub language: String,
    /// 当前主题偏好（light / dark / system）。
    pub theme: String,
    /// 开机自启动当前状态。
    pub autostart: bool,
    /// 是否默认勾选内置插件安装（仅首次引导展示；新安装默认开启）。
    pub install_builtin_plugins: bool,
}

#[derive(Deserialize)]
pub struct OnboardingPayload {
    /// 新的 API Key（空/缺省表示不改动凭据）。
    pub api_key: Option<String>,
    /// 语言（zh-CN / en）。
    pub language: String,
    /// 主题（light / dark / system）。
    pub theme: String,
    /// 开机自启动。
    pub autostart: bool,
    /// 用户是否同意自动安装并维护内置插件。
    pub install_builtin_plugins: bool,
}

/// 读取首次配置状态（启动页拉取用）。
pub fn state(app: &AppHandle) -> OnboardingState {
    let config = app.state::<AppState>().config();
    OnboardingState {
        needs_onboarding: needs_onboarding(&config),
        api_key_set: ["DSH_BOX_API_KEY", "DEEPSEEK_API_KEY"]
            .iter()
            .any(|name| std::env::var(name).is_ok_and(|value| !value.trim().is_empty()))
            || crate::credentials::has(&config, DEEPSEEK_API_KEY_NAME),
        language: if crate::locale::is_chinese() {
            "zh-CN".to_string()
        } else {
            "en".to_string()
        },
        theme: config.load_dsh_theme().unwrap_or("system").to_string(),
        autostart: crate::autostart::is_enabled(),
        install_builtin_plugins: true,
    }
}

/// 保存首次配置并开始使用。
pub fn save(app: &AppHandle, payload: OnboardingPayload) -> Result<(), String> {
    if !can_finish_onboarding(app.state::<AppState>().phase()) {
        return Err(crate::locale::text(
            "运行环境尚未准备好，请等待安装和启动完成。",
            "The runtime is not ready yet. Wait for installation and startup to finish.",
        )
        .into());
    }
    let config = app.state::<AppState>().config();

    let api_key = payload
        .api_key
        .as_deref()
        .map(str::trim)
        .filter(|key| !key.is_empty());
    if let Some(key) = api_key {
        validate_api_key(key)?;
    }
    if !matches!(payload.language.as_str(), "zh-CN" | "en") {
        return Err(crate::locale::text("不支持的界面语言。", "Unsupported UI language.").into());
    }
    if !matches!(payload.theme.as_str(), "light" | "dark" | "system") {
        return Err(crate::locale::text("不支持的主题。", "Unsupported theme.").into());
    }

    let mut credentials_changed = false;
    if let Some(key) = api_key {
        save_credentials_api_key(&config, key)?;
        credentials_changed = true;
    }

    app.state::<AppState>().set_ui_language(&payload.language)?;
    // 同步 dsh 界面语言（dsh 语言 id 为 zh/en；文件监视器热发布）
    let dsh_locale = if payload.language == "zh-CN" {
        "zh"
    } else {
        "en"
    };
    if let Err(e) = config.save_dsh_locale(dsh_locale) {
        crate::logging::log(&format!("onboarding: 同步 dsh 语言失败：{e}"));
    }
    crate::tray::apply_language(app, &payload.language);

    // 与语言同步同一口径：外壳主题已由 apply_theme 即时生效，写入 dsh 配置
    // 失败不阻断引导收尾（用户稍后可在外壳设置中重新调整并再次同步）。
    if let Err(e) = config.save_dsh_theme(&payload.theme) {
        crate::logging::log(&format!("onboarding: 同步 dsh 主题失败：{e}"));
    }
    crate::tray::apply_theme(app, &payload.theme);

    crate::autostart::set_enabled(payload.autostart)?;

    app_state::save_state_value(
        &config.root,
        "builtin_plugins_enabled",
        serde_json::json!(payload.install_builtin_plugins),
    )?;

    // 所有用户选择均已验证并成功落盘后才准备交接。完成原子标记必须最后
    // 发布：boot 正在等待该标记，若先发布再设置 Starting，会有机会先进入
    // dsh、随后又立刻重启。
    persist_onboarding_completion(&config)?;
    let managed_service =
        app.state::<AppState>().service_ownership() == app_state::ServiceOwnership::Managed;
    if credentials_changed && managed_service {
        // 由正在等待 onboarding 的 boot 线程在内置插件处理结束后统一重启，
        // 避免凭据与插件各自触发一次服务中断。
        app.state::<AppState>().require_onboarding_restart();
    }
    app_state::mark_onboarding_done();
    // 不在此直接导航或重启：mark_onboarding_done 会唤醒 boot_inner，后者
    // 统一处理内置插件、凭据重启和 enter_web_app。
    Ok(())
}

fn can_finish_onboarding(phase: BootPhase) -> bool {
    phase == BootPhase::Ready
}

fn persist_onboarding_completion(config: &app_state::Config) -> Result<(), String> {
    app_state::save_state_value(
        &config.root,
        "local_onboarding_deferred",
        serde_json::Value::Bool(false),
    )?;
    app_state::save_state_value(&config.root, "onboarded", serde_json::Value::Bool(true))
}

/// 与启动状态机共用同一首次引导判定。
fn needs_onboarding(config: &app_state::Config) -> bool {
    crate::app_state::onboarding_required(&config.root)
}

/// 用户输入 API Key 的统一格式校验（调用方已 trim 并剔除空值）：拒绝控制
/// 字符与异常长度（4096 上限）。设置页与首次引导共用同一口径，避免一处
/// 收紧一处放行。
pub(crate) fn validate_api_key(key: &str) -> Result<(), String> {
    if key.len() > 4096 || key.chars().any(char::is_control) {
        return Err(crate::locale::text(
            "API Key 含有无效字符或长度异常。",
            "The API key contains invalid characters or is too long.",
        )
        .into());
    }
    Ok(())
}

/// 行级合并写入 dsh 凭据文件（`DEEPSEEK_API_KEY: <key>`），原子替换。
/// 不触碰文件中的其他凭据条目。
fn save_credentials_api_key(config: &app_state::Config, key: &str) -> Result<(), String> {
    crate::credentials::save(config, DEEPSEEK_API_KEY_NAME, key)
}

#[cfg(test)]
mod tests {
    use super::{
        can_finish_onboarding, needs_onboarding, persist_onboarding_completion,
        save_credentials_api_key, DEEPSEEK_API_KEY_NAME,
    };
    use crate::app_state::Config;
    use std::path::PathBuf;

    fn config_with_root(root: PathBuf) -> Config {
        // 构造最小 Config：其余字段不影响被测函数
        let mut cfg = Config::load();
        cfg.root = root;
        cfg
    }

    #[test]
    fn onboarding_required_without_marker() {
        let dir = std::env::temp_dir().join(format!("dshd-onb-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let cfg = config_with_root(dir.clone());
        assert!(needs_onboarding(&cfg));
        // 写入标记后不再需要
        crate::app_state::save_state_value(&dir, "onboarded", serde_json::Value::Bool(true))
            .unwrap();
        assert!(!needs_onboarding(&cfg));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn onboarding_can_finish_only_after_the_service_is_ready() {
        assert!(can_finish_onboarding(crate::app_state::BootPhase::Ready));
        for phase in [
            crate::app_state::BootPhase::Starting,
            crate::app_state::BootPhase::SwitchingService,
            crate::app_state::BootPhase::ServiceChoice,
            crate::app_state::BootPhase::InstallingNode,
            crate::app_state::BootPhase::InstallingDsh,
            crate::app_state::BootPhase::StartingServer,
            crate::app_state::BootPhase::Cancelled,
            crate::app_state::BootPhase::Error,
        ] {
            assert!(!can_finish_onboarding(phase));
        }
    }

    #[test]
    fn api_key_validation_rejects_control_chars_and_excessive_length() {
        use super::validate_api_key;
        assert!(validate_api_key("sk-normal-key_123").is_ok());
        assert!(validate_api_key("line\nbreak").is_err());
        assert!(validate_api_key("tab\tchar").is_err());
        assert!(validate_api_key(&"x".repeat(4096)).is_ok());
        assert!(validate_api_key(&"x".repeat(4097)).is_err());
    }

    #[test]
    fn completing_local_onboarding_clears_external_deferral() {
        let dir = std::env::temp_dir().join(format!("dshd-onb-deferred-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let cfg = config_with_root(dir.clone());
        crate::app_state::save_state_value(
            &dir,
            "local_onboarding_deferred",
            serde_json::Value::Bool(true),
        )
        .unwrap();

        persist_onboarding_completion(&cfg).unwrap();

        assert_eq!(
            crate::app_state::load_state_value(&dir, "local_onboarding_deferred")
                .and_then(|value| value.as_bool()),
            Some(false)
        );
        assert_eq!(
            crate::app_state::load_state_value(&dir, "onboarded").and_then(|value| value.as_bool()),
            Some(true)
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn theme_write_is_line_merged() {
        let dir = std::env::temp_dir().join(format!("dshd-theme-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut cfg = config_with_root(dir.clone());
        cfg.dsh_home = dir.join("home");
        let home = dir.join("home");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::write(
            home.join("settings.yaml"),
            "locale:\n  preference: zh\nui-theme:\n  preference: dark\nother:\n  x: 1\n",
        )
        .unwrap();
        cfg.save_dsh_theme("light").unwrap();
        let text = std::fs::read_to_string(home.join("settings.yaml")).unwrap();
        assert!(text.contains("ui-theme:\n  preference: light"));
        assert!(
            text.contains("locale:\n  preference: zh"),
            "其他段落不得被触碰"
        );
        assert!(text.contains("other:\n  x: 1"));
        assert!(!text.contains("preference: dark"));
        // 无 ui-theme 段时追加
        std::fs::write(home.join("settings.yaml"), "locale:\n  preference: zh\n").unwrap();
        cfg.save_dsh_theme("dark").unwrap();
        let text = std::fs::read_to_string(home.join("settings.yaml")).unwrap();
        assert!(text.contains("ui-theme:\n  preference: dark"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn credentials_write_is_line_merged() {
        let dir = std::env::temp_dir().join(format!("dshd-cred-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut cfg = config_with_root(dir.clone());
        cfg.dsh_home = dir.join("home");
        let home = dir.join("home");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::write(
            home.join(".credentials.yaml"),
            "version: 1\nrefs:\n  IBRAIN_API_KEY: keep-me\n  DEEPSEEK_API_KEY: old-key\n",
        )
        .unwrap();
        save_credentials_api_key(&cfg, "new-key").unwrap();
        let text = std::fs::read_to_string(home.join(".credentials.yaml")).unwrap();
        assert!(text.contains("  DEEPSEEK_API_KEY: 'new-key'"));
        assert!(text.contains("  IBRAIN_API_KEY: keep-me"));
        assert!(!text.contains("old-key"));
        assert!(crate::credentials::has(&cfg, DEEPSEEK_API_KEY_NAME));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
