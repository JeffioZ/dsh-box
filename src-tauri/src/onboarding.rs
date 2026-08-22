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
    /// 跳过：只落 onboarded 标记，不写任何配置。
    #[serde(default)]
    pub skip: bool,
    /// 新的 API Key（空/缺省表示不改动凭据）。
    pub api_key: Option<String>,
    /// 语言（zh-CN / en）。
    pub language: Option<String>,
    /// 主题（light / dark / system）。
    pub theme: Option<String>,
    /// 开机自启动。
    pub autostart: Option<bool>,
    /// 用户是否同意自动安装并维护内置插件；缺失按未同意处理。
    pub install_builtin_plugins: Option<bool>,
}

/// 读取首次配置状态（启动页拉取用）。
pub fn state(app: &AppHandle) -> OnboardingState {
    let config = app.state::<AppState>().config();
    OnboardingState {
        needs_onboarding: needs_onboarding(&config),
        api_key_set: ["DSH_BOX_API_KEY", "DEEPSEEK_API_KEY"]
            .iter()
            .any(|name| std::env::var(name).is_ok_and(|value| !value.is_empty()))
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

/// 保存首次配置（“开始使用”或“跳过”）。
pub fn save(app: &AppHandle, payload: OnboardingPayload) -> Result<(), String> {
    let config = app.state::<AppState>().config();
    if payload.skip {
        app_state::save_state_value(
            &config.root,
            "builtin_plugins_enabled",
            serde_json::json!(false),
        )?;
        finish_onboarding(&config)?;
        return Ok(());
    }

    let api_key = payload
        .api_key
        .as_deref()
        .map(str::trim)
        .filter(|key| !key.is_empty());
    if api_key.is_some_and(|key| key.chars().any(char::is_control)) {
        return Err(crate::locale::text(
            "API Key 含有不允许的控制字符。",
            "The API key contains invalid control characters.",
        )
        .into());
    }
    if payload
        .language
        .as_deref()
        .is_some_and(|language| !matches!(language, "zh-CN" | "en"))
    {
        return Err(crate::locale::text("不支持的界面语言。", "Unsupported UI language.").into());
    }
    if payload
        .theme
        .as_deref()
        .is_some_and(|theme| !matches!(theme, "light" | "dark" | "system"))
    {
        return Err(crate::locale::text("不支持的主题。", "Unsupported theme.").into());
    }

    let mut credentials_changed = false;
    if let Some(key) = api_key {
        save_credentials_api_key(&config, key)?;
        credentials_changed = true;
    }

    if let Some(language) = payload.language.as_deref() {
        app.state::<AppState>().set_ui_language(language)?;
        // 同步 dsh 界面语言（dsh 语言 id 为 zh/en；文件监视器热发布）
        let dsh_locale = if language == "zh-CN" { "zh" } else { "en" };
        if let Err(e) = config.save_dsh_locale(dsh_locale) {
            crate::logging::log(&format!("onboarding: 同步 dsh 语言失败：{e}"));
        }
        crate::tray::apply_language(app, language);
    }

    if let Some(theme) = payload.theme.as_deref() {
        config.save_dsh_theme(theme)?;
        crate::tray::apply_theme(app, theme);
    }

    if let Some(enabled) = payload.autostart {
        crate::autostart::set_enabled(enabled)?;
    }

    app_state::save_state_value(
        &config.root,
        "builtin_plugins_enabled",
        serde_json::json!(payload.install_builtin_plugins.unwrap_or(false)),
    )?;

    // 所有用户选择均已验证并成功落盘后才提交完成标记。前面的写入都是幂等的；
    // 任一步失败时保留引导，避免下次启动跳过尚未完成的配置。
    finish_onboarding(&config)?;

    // API Key 变化且服务已就绪：凭据在 dsh 启动时读取，重启服务使其生效
    // （restart_service 完成后自带 navigate，无需在此补跳转）
    if credentials_changed && app.state::<AppState>().phase() == BootPhase::Ready {
        let handle = app.clone();
        std::thread::spawn(move || {
            crate::logging::log("onboarding: API Key 已保存，重启服务生效");
            let _ = crate::updater::restart_service(&handle);
        });
        return Ok(());
    }
    // 服务已就绪但 boot 因首次配置停留（未跳转）：此处补一次跳转。
    // 尚未就绪则留给 boot_once 正常流程（onboarded 标记已落，不再停留）。
    if app.state::<AppState>().phase() == BootPhase::Ready {
        let config = app.state::<AppState>().config();
        let already_on_dsh = crate::main_webview(app)
            .and_then(|w| w.url().ok())
            .is_some_and(|url| crate::is_dsh_url(&url, &config));
        if !already_on_dsh {
            crate::navigate(app, &config.web_url());
        }
    }
    Ok(())
}

fn finish_onboarding(config: &app_state::Config) -> Result<(), String> {
    app_state::save_state_value(&config.root, "onboarded", serde_json::Value::Bool(true))?;
    app_state::mark_onboarding_done();
    Ok(())
}

/// 与启动状态机共用同一首次引导判定。
fn needs_onboarding(config: &app_state::Config) -> bool {
    crate::app_state::onboarding_required(&config.root)
}

/// 行级合并写入 dsh 凭据文件（`DEEPSEEK_API_KEY: <key>`），原子替换。
/// 不触碰文件中的其他凭据条目。
fn save_credentials_api_key(config: &app_state::Config, key: &str) -> Result<(), String> {
    crate::credentials::save(config, DEEPSEEK_API_KEY_NAME, key)
}

#[cfg(test)]
mod tests {
    use super::{needs_onboarding, save_credentials_api_key, DEEPSEEK_API_KEY_NAME};
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
            "IBRAIN_API_KEY: keep-me\nDEEPSEEK_API_KEY: old-key\n",
        )
        .unwrap();
        save_credentials_api_key(&cfg, "new-key").unwrap();
        let text = std::fs::read_to_string(home.join(".credentials.yaml")).unwrap();
        assert!(text.contains("DEEPSEEK_API_KEY: 'new-key'"));
        assert!(text.contains("IBRAIN_API_KEY: keep-me"));
        assert!(!text.contains("old-key"));
        assert!(crate::credentials::has(&cfg, DEEPSEEK_API_KEY_NAME));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
