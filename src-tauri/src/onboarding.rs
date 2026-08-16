//! 首次使用配置（onboarding）：启动页上的一次性引导。
//!
//! 提供 API Key、界面语言、主题与开机自启动四项配置。全部写入 dsh 的
//! 原生配置文件（`$DSH_HOME/.credentials.yaml`、`$DSH_HOME/settings.yaml`）
//! 与外壳 config.json——不修改 dsh 代码，dsh 界面即时可见。
//!
//! 触发规则：config.json 无 `onboarded: true` 标记（保存或跳过都会落标记，
//! 之后不再打扰）。跳过不写入任何用户配置。

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
    /// 是否已配置 API Key（外壳 config 或 dsh 凭据文件任一存在）。
    pub api_key_set: bool,
    /// 当前界面语言（zh-CN / en）。
    pub language: String,
    /// 当前主题偏好（light / dark / system）。
    pub theme: String,
    /// 开机自启动当前状态。
    pub autostart: bool,
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
}

/// 读取首次配置状态（启动页拉取用）。
pub fn state(app: &AppHandle) -> OnboardingState {
    let config = app.state::<AppState>().config();
    OnboardingState {
        needs_onboarding: needs_onboarding(&config),
        api_key_set: config.api_key.is_some() || dsh_credentials_has_api_key(&config),
        language: if crate::locale::is_chinese() {
            "zh-CN".to_string()
        } else {
            "en".to_string()
        },
        theme: config.load_dsh_theme().unwrap_or("system").to_string(),
        autostart: crate::autostart::is_enabled(),
    }
}

/// 保存首次配置（“开始使用”或“跳过”）。
pub fn save(app: &AppHandle, payload: OnboardingPayload) -> Result<(), String> {
    let config = app.state::<AppState>().config();
    // 先落标记：即使后续某一步失败，下次启动也不再重复引导（避免死循环）
    app_state::save_config_value(&config.root, "onboarded", serde_json::Value::Bool(true))?;
    if payload.skip {
        return Ok(());
    }

    let mut credentials_changed = false;
    if let Some(key) = payload
        .api_key
        .as_deref()
        .map(str::trim)
        .filter(|k| !k.is_empty())
    {
        save_credentials_api_key(&config, key)?;
        app_state::save_config_value(
            &config.root,
            "api_key",
            serde_json::Value::String(key.to_string()),
        )?;
        credentials_changed = true;
    }

    if let Some(language) = payload.language.as_deref() {
        if matches!(language, "zh-CN" | "en") {
            app.state::<AppState>().set_ui_language(language)?;
            // 同步 dsh 界面语言（dsh 语言 id 为 zh/en；文件监视器热发布）
            let dsh_locale = if language == "zh-CN" { "zh" } else { "en" };
            if let Err(e) = config.save_dsh_locale(dsh_locale) {
                crate::logging::log(&format!("onboarding: 同步 dsh 语言失败：{e}"));
            }
            crate::tray::apply_language(app, language);
        }
    }

    if let Some(theme) = payload.theme.as_deref() {
        if matches!(theme, "light" | "dark" | "system") {
            config.save_dsh_theme(theme)?;
            crate::tray::apply_theme(app, theme);
        }
    }

    if let Some(enabled) = payload.autostart {
        crate::autostart::set_enabled(enabled)?;
    }

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

/// 是否首次使用：config.json 无 `onboarded: true`。
fn needs_onboarding(config: &app_state::Config) -> bool {
    let text = std::fs::read_to_string(config.root.join("config.json")).unwrap_or_default();
    serde_json::from_str::<serde_json::Value>(&text)
        .ok()
        .and_then(|json| json.get("onboarded").and_then(|v| v.as_bool()))
        != Some(true)
}

/// dsh 凭据文件是否已配置 DEEPSEEK_API_KEY。
fn dsh_credentials_has_api_key(config: &app_state::Config) -> bool {
    std::fs::read_to_string(config.dsh_home().join(".credentials.yaml"))
        .ok()
        .is_some_and(|text| {
            text.lines().any(|line| {
                line.trim_start().starts_with(DEEPSEEK_API_KEY_NAME)
                    && line
                        .split_once(':')
                        .is_some_and(|(_, v)| !v.trim().is_empty())
            })
        })
}

/// 行级合并写入 dsh 凭据文件（`DEEPSEEK_API_KEY: <key>`），原子替换。
/// 不触碰文件中的其他凭据条目。
fn save_credentials_api_key(config: &app_state::Config, key: &str) -> Result<(), String> {
    let path = config.dsh_home().join(".credentials.yaml");
    let text = std::fs::read_to_string(&path).unwrap_or_default();
    let mut out = String::new();
    let mut wrote = false;
    for line in text.lines() {
        if line.trim_start().starts_with(DEEPSEEK_API_KEY_NAME) {
            if !wrote {
                out.push_str(&format!("{DEEPSEEK_API_KEY_NAME}: {key}\n"));
                wrote = true;
            }
            continue; // 跳过旧行（仅保留一份）
        }
        out.push_str(line);
        out.push('\n');
    }
    if !wrote {
        if !out.is_empty() && !out.ends_with('\n') {
            out.push('\n');
        }
        out.push_str(&format!("{DEEPSEEK_API_KEY_NAME}: {key}\n"));
    }
    app_state::atomic_write(&path, &out)
}

#[cfg(test)]
mod tests {
    use super::{dsh_credentials_has_api_key, needs_onboarding, save_credentials_api_key};
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
        crate::app_state::save_config_value(&dir, "onboarded", serde_json::Value::Bool(true))
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
        cfg.dsh_home = Some(dir.join("home"));
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
        cfg.dsh_home = Some(dir.join("home"));
        let home = dir.join("home");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::write(
            home.join(".credentials.yaml"),
            "IBRAIN_API_KEY: keep-me\nDEEPSEEK_API_KEY: old-key\n",
        )
        .unwrap();
        save_credentials_api_key(&cfg, "new-key").unwrap();
        let text = std::fs::read_to_string(home.join(".credentials.yaml")).unwrap();
        assert!(text.contains("DEEPSEEK_API_KEY: new-key"));
        assert!(text.contains("IBRAIN_API_KEY: keep-me"));
        assert!(!text.contains("old-key"));
        assert!(dsh_credentials_has_api_key(&cfg));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
