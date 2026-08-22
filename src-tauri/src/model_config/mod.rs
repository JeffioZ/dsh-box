//! 模型配置导入与导出：把 `llm-pi-ai:` YAML 段解析、校验并读写
//! dsh 的原生配置文件（`$DSH_HOME/settings.yaml` 的 `llm-pi-ai` 段 +
//! `$DSH_HOME/.credentials.yaml` 的 apiKeyEnv 引用），不修改 dsh 代码。
//!
//! 与 onboarding 的 DeepSeek 官方 key 不同，这里导入的是任意提供方路由
//! （pi-ai 多提供方适配器），格式通用、不绑定任何厂商：`providers` 字典键
//! 即路由名，`apiKeyEnv` 是凭据引用名。凭据本身绝不进入 settings 文件——
//! 用户单独粘贴 key，经行级合并写入 `.credentials.yaml`。
//!
//! 写入策略与 `save_dsh_theme`/`save_dsh_locale` 一致：行级操作 + 原子写，
//! 只动 `llm-pi-ai` 段与指定凭据行，不触碰文件其他内容。settings 由 dsh 的
//! 文件监视器热发布，凭据由适配器每次请求解析，均无需重启服务。
//!
//! 导入文本约定为**完整的 `llm-pi-ai:` 顶层段**（模板由外部维护，格式保证）：
//!
//! ```yaml
//! llm-pi-ai:
//!   providers:
//!     <route>:
//!       displayName: ...
//!       apiKeyEnv: <REF>
//!       api: anthropic-messages
//!       baseURL: ...
//!       models:
//!         - id: ...
//! ```

mod parser;

use parser::{normalize_section, parse_providers, valid_env_name};

use crate::credentials::upsert as upsert_credential;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager};

use crate::app_state::{self, AppState, Config};

/// 顶层段键：本模块只整体替换这一个顶层段。
const SECTION_KEY: &str = "llm-pi-ai";

/// 一个 provider 的摘要（预览用）。
#[derive(Serialize)]
pub struct ProviderPreview {
    /// 路由名（providers 字典键）。
    pub route: String,
    /// displayName（缺省回退为路由名）。
    pub display_name: String,
    /// 模型数量（models 列表条数）。
    pub model_count: usize,
    /// 凭据引用名（apiKeyEnv；未声明则为空）。
    pub api_key_env: Option<String>,
}

/// 导入文本的解析/校验结果。
#[derive(Serialize)]
pub struct ImportPreview {
    /// 识别到的 provider 摘要列表（有序）。
    pub providers: Vec<ProviderPreview>,
    /// 需要用户提供 key 的凭据引用名（去重、有序）。
    pub api_key_envs: Vec<String>,
    /// settings.yaml 当前是否已有 llm-pi-ai 段（导入将整体替换）。
    pub replaces_existing: bool,
}

/// 应用导入的载荷：校验通过的 YAML 段 + 各 apiKeyEnv 对应的 key。
#[derive(Deserialize)]
pub struct ImportApplyPayload {
    /// 完整的 `llm-pi-ai:` 顶层段（与 preview 校验的同一份文本）。
    pub yaml: String,
    /// apiKeyEnv 引用名 → 用户粘贴的 key（仅写入 `apiKeyEnv` 声明的引用）。
    pub keys: Vec<(String, String)>,
}

/// 校验导入文本并返回预览（只读，不写盘）。
pub fn preview(config: &Config, yaml: &str) -> Result<ImportPreview, String> {
    let providers = parse_providers(yaml)?;
    let mut api_key_envs = Vec::new();
    for p in &providers {
        if let Some(ref name) = p.api_key_env {
            if !api_key_envs.contains(name) {
                api_key_envs.push(name.clone());
            }
        }
    }
    let settings_text =
        std::fs::read_to_string(config.dsh_home().join("settings.yaml")).unwrap_or_default();
    Ok(ImportPreview {
        providers: providers
            .into_iter()
            .map(|p| ProviderPreview {
                route: p.route,
                display_name: p.display_name,
                model_count: p.model_count,
                api_key_env: p.api_key_env,
            })
            .collect(),
        api_key_envs,
        replaces_existing: settings_has_section(&settings_text),
    })
}

/// 导出模型配置：读 settings.yaml 的 llm-pi-ai 段原文，供复制给同事导入。
/// 只导出自定义路由（pi-ai 官方目录之外的），官方目录路由（openai/anthropic
/// 等）同事自己也能配置，不含在分享里。返回 None 表示没有可导出的自定义
/// 路由（与文件读取失败区分，前端据此展示不同的提示）。
pub fn export_yaml(config: &Config) -> Result<Option<String>, String> {
    let settings_path = config.dsh_home().join("settings.yaml");
    let text = std::fs::read_to_string(&settings_path).map_err(|e| {
        crate::locale::error("读取 settings.yaml 失败", "Failed to read settings.yaml", e)
    })?;
    let section = extract_section_text(&text);
    if section.trim().is_empty() {
        return Ok(None);
    }
    let custom_only = filter_builtin_routes(&section);
    if custom_only.trim().is_empty() {
        return Ok(None);
    }
    Ok(Some(custom_only))
}

/// 从 settings.yaml 文本中提取 llm-pi-ai 顶层段（纯逻辑，供单测）。
fn extract_section_text(text: &str) -> String {
    let section = format!("{SECTION_KEY}:");
    let mut out = String::new();
    let mut in_section = false;
    for line in text.lines() {
        if !in_section {
            if line.trim_end() == section && !line.starts_with(' ') {
                in_section = true;
                out.push_str(line);
                out.push('\n');
            }
            continue;
        }
        // 已进入段：遇到下一个顶层键即段结束
        if !line.starts_with(' ') && !line.trim().is_empty() {
            break;
        }
        out.push_str(line);
        out.push('\n');
    }
    out
}

/// pi-ai 官方目录路由名单（`@earendil-works/pi-ai/dist/providers/*.models.js`
/// 的文件名列表，37 个）。导出时过滤掉这些——它们是官方目录路由，同事
/// 自己也能配置，没必要随分享带走。
/// ⚠️ 上游 pi-ai 升级新增官方 provider 时需同步此名单（维护点记录于 AGENTS.md）。
const BUILTIN_ROUTES: &[&str] = &[
    "amazon-bedrock",
    "ant-ling",
    "anthropic",
    "azure-openai-responses",
    "cerebras",
    "cloudflare-ai-gateway",
    "cloudflare-workers-ai",
    "deepseek",
    "fireworks",
    "github-copilot",
    "google",
    "google-vertex",
    "groq",
    "huggingface",
    "kimi-coding",
    "minimax",
    "minimax-cn",
    "mistral",
    "moonshotai",
    "moonshotai-cn",
    "nvidia",
    "openai",
    "openai-codex",
    "opencode",
    "opencode-go",
    "openrouter",
    "qwen-token-plan",
    "qwen-token-plan-cn",
    "together",
    "vercel-ai-gateway",
    "xai",
    "xiaomi",
    "xiaomi-token-plan-ams",
    "xiaomi-token-plan-cn",
    "xiaomi-token-plan-sgp",
    "zai",
    "zai-coding-cn",
];

/// 从 llm-pi-ai 段文本中移除官方目录路由，仅保留自定义路由。
/// 无自定义路由时返回空字符串（调用方据此返回 None）。
fn filter_builtin_routes(section: &str) -> String {
    let mut out = String::new();
    // 当前 provider 是否为自定义路由（决定其整块去留）
    let mut current_is_custom = false;
    // 是否保留过至少一个自定义 provider（否则整段视为空）
    let mut kept_any = false;
    for line in section.lines() {
        let indent = line.len() - line.trim_start().len();
        let content = line.trim_start();
        // llm-pi-ai:（indent 0）与 providers:（indent 2）等结构行总是保留
        if indent == 0 {
            out.push_str(line);
            out.push('\n');
            continue;
        }
        if indent == 2 {
            // providers: 或其他段级字段，始终保留
            out.push_str(line);
            out.push('\n');
            continue;
        }
        if indent == 4 {
            // provider 路由键
            let route = content.trim_end().trim_end_matches(':');
            current_is_custom = !BUILTIN_ROUTES.contains(&route);
            if current_is_custom {
                kept_any = true;
                out.push_str(line);
                out.push('\n');
            }
            continue;
        }
        // provider 内字段（indent >= 6）或更深的嵌套，跟随当前 provider 的去留
        if current_is_custom {
            out.push_str(line);
            out.push('\n');
        }
    }
    if !kept_any {
        return String::new();
    }
    out
}

/// 应用导入：写 settings.yaml 的 llm-pi-ai 段 + .credentials.yaml 的凭据。
pub fn apply(app: &AppHandle, payload: ImportApplyPayload) -> Result<(), String> {
    let config = app.state::<AppState>().config();

    // 1) 先校验结构（与 preview 同一解析路径，确保写入的文本合法）。
    let providers = parse_providers(&payload.yaml)?;

    // 2) 校验 key 与 apiKeyEnv 对应：只接受声明过的引用，且 key 非空。
    let mut declared = Vec::new();
    for p in &providers {
        if let Some(ref name) = p.api_key_env {
            if !declared.contains(name) {
                declared.push(name.clone());
            }
        }
    }
    let mut provided = Vec::new();
    for (name, key) in &payload.keys {
        if !declared.contains(name) {
            return Err(crate::locale::text(
                "导入的配置中没有声明该凭据引用。",
                "The imported configuration does not declare this credential reference.",
            )
            .into());
        }
        if key.trim().is_empty() {
            return Err(
                crate::locale::text("API Key 不能为空。", "API key cannot be empty.").into(),
            );
        }
        if !valid_env_name(name) || key.chars().any(char::is_control) {
            return Err(crate::locale::text(
                "凭据名称或 API Key 含有不允许的字符。",
                "The credential name or API key contains invalid characters.",
            )
            .into());
        }
        if provided.contains(name) {
            return Err(crate::locale::text(
                "同一凭据不能重复提供。",
                "The same credential cannot be provided more than once.",
            )
            .into());
        }
        provided.push(name.clone());
    }

    // 3) 先写凭据：若后续 settings 写入失败，最多留下未引用的凭据；反过来会让
    // 已热发布的路由短暂引用不存在的 key，影响正在进行的模型请求。
    let credentials_path = config.dsh_home().join(".credentials.yaml");
    if !payload.keys.is_empty() {
        app_state::update_text_file(&credentials_path, |mut text| {
            for (name, key) in &payload.keys {
                text = upsert_credential(&text, name, key.trim());
            }
            Ok(text)
        })?;
    }

    // 4) 整体替换或追加 llm-pi-ai 段；读—改—写在同一锁内完成。
    let settings_path = config.dsh_home().join("settings.yaml");
    let normalized = normalize_section(&payload.yaml)?;
    app_state::update_text_file(&settings_path, |text| {
        Ok(upsert_section(&text, &normalized))
    })?;

    crate::logging::log(&format!(
        "model-import: 已导入 {} 个提供方路由（写 settings.yaml + credentials.yaml）",
        providers.len()
    ));
    Ok(())
}

/// settings.yaml 文本是否已含 llm-pi-ai 顶层段。
fn settings_has_section(text: &str) -> bool {
    let section = format!("{SECTION_KEY}:");
    text.lines()
        .any(|line| !line.starts_with(' ') && line.trim_end() == section)
}

/// 行级替换（或追加）一个顶层段。`new_section` 必须是完整顶层段（含键行）。
/// 只替换同名顶层段，绝不触碰其他顶层段。
fn upsert_section(text: &str, new_section: &str) -> String {
    let mut out = String::new();
    let mut replaced = false;
    let section = format!("{SECTION_KEY}:");
    let mut lines = text.lines().peekable();
    while let Some(line) = lines.next() {
        let is_top = !line.starts_with(' ') && !line.trim().is_empty();
        if is_top && line.trim_end() == section {
            // 跳过整个旧 llm-pi-ai 段（直到下一个顶层键）
            while let Some(&next) = lines.peek() {
                if !next.starts_with(' ') && !next.trim().is_empty() {
                    break;
                }
                lines.next();
            }
            if !out.is_empty() && !out.ends_with('\n') {
                out.push('\n');
            }
            out.push_str(new_section);
            if !new_section.ends_with('\n') {
                out.push('\n');
            }
            replaced = true;
            continue;
        }
        out.push_str(line);
        out.push('\n');
    }
    if !replaced {
        if !out.is_empty() && !out.ends_with('\n') {
            out.push('\n');
        }
        out.push_str(new_section);
        if !new_section.ends_with('\n') {
            out.push('\n');
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "\
llm-pi-ai:
  providers:
    internal-gateway:
      displayName: Internal Gateway
      apiKeyEnv: CORP_GATEWAY_KEY
      api: anthropic-messages
      baseURL: https://gateway.example.com/v1
      models:
        - id: model-alpha
          name: Alpha
        - id: model-beta
  providers-note: keep
";

    #[test]
    fn parse_extracts_providers() {
        let providers = parse_providers(SAMPLE).unwrap();
        assert_eq!(providers.len(), 1);
        assert_eq!(providers[0].route, "internal-gateway");
        assert_eq!(providers[0].display_name, "Internal Gateway");
        assert_eq!(providers[0].model_count, 2);
        assert_eq!(
            providers[0].api_key_env.as_deref(),
            Some("CORP_GATEWAY_KEY")
        );
    }

    #[test]
    fn reject_non_model_config() {
        let err = parse_providers("locale:\n  preference: zh\n").unwrap_err();
        assert!(err.contains("llm-pi-ai"), "unexpected: {err}");
    }

    #[test]
    fn reject_additional_top_level_section() {
        let text = "llm-pi-ai:\n  providers:\n    gw:\n      models:\n        - id: x\nlocale:\n  preference: en\n";
        let err = normalize_section(text).unwrap_err();
        assert!(err.contains("llm-pi-ai"), "unexpected: {err}");
    }

    #[test]
    fn reject_duplicate_model_section() {
        let text = "llm-pi-ai:\n  providers:\n    one:\n      models:\n        - id: x\nllm-pi-ai:\n  providers:\n    two:\n      models:\n        - id: y\n";
        let err = parse_providers(text).unwrap_err();
        assert!(
            err.contains("重复") || err.contains("duplicate"),
            "unexpected: {err}"
        );
    }

    #[test]
    fn reject_malformed_yaml_and_duplicate_provider_keys() {
        let malformed = "llm-pi-ai:\n  providers:\n    gw: [\n";
        assert!(parse_providers(malformed).is_err());

        let duplicate = "llm-pi-ai:\n  providers:\n    gw:\n      models: [{ id: x }]\n    gw:\n      models: [{ id: y }]\n";
        assert!(parse_providers(duplicate).is_err());
    }

    #[test]
    fn parses_valid_flow_style_models_semantically() {
        let text = "llm-pi-ai:\n  providers:\n    gw:\n      displayName: \"Gateway: Primary\"\n      models: [{ id: x }, { id: y }]\n";
        let providers = parse_providers(text).unwrap();
        assert_eq!(providers[0].display_name, "Gateway: Primary");
        assert_eq!(providers[0].model_count, 2);
    }

    #[test]
    fn reject_invalid_api_key_env() {
        let text = "llm-pi-ai:\n  providers:\n    gw:\n      apiKeyEnv: BAD-NAME\n      models:\n        - id: x\n";
        let err = parse_providers(text).unwrap_err();
        assert!(err.contains("apiKeyEnv"), "unexpected: {err}");
    }

    #[test]
    fn accepts_portable_api_key_env_names() {
        for name in ["KEY", "_KEY", "Key_2"] {
            assert!(valid_env_name(name), "expected valid: {name}");
        }
        for name in ["", "2KEY", "BAD-NAME", "BAD NAME", "键"] {
            assert!(!valid_env_name(name), "expected invalid: {name}");
        }
    }

    #[test]
    fn reject_empty_providers() {
        let err = parse_providers("llm-pi-ai:\n  other: 1\n").unwrap_err();
        assert!(err.contains("providers"), "unexpected: {err}");
    }

    #[test]
    fn reject_provider_without_models() {
        let text = "llm-pi-ai:\n  providers:\n    gw:\n      apiKeyEnv: K\n";
        let err = parse_providers(text).unwrap_err();
        assert!(err.contains("models"), "unexpected: {err}");
    }

    #[test]
    fn upsert_replaces_existing_section() {
        let old = "locale:\n  preference: zh\nllm-pi-ai:\n  providers:\n    a:\n      models:\n        - id: x\nui-theme:\n  preference: dark\n";
        let new = "llm-pi-ai:\n  providers:\n    b:\n      models:\n        - id: y\n";
        let merged = upsert_section(old, new);
        assert!(merged.contains("locale:\n  preference: zh"));
        assert!(merged.contains("ui-theme:\n  preference: dark"));
        // 旧 llm-pi-ai 段被替换
        assert!(!merged.contains("id: x"));
        assert!(merged.contains("id: y"));
        // 只有一份 llm-pi-ai
        assert_eq!(merged.matches("llm-pi-ai:").count(), 1);
    }

    #[test]
    fn upsert_appends_when_missing() {
        let old = "locale:\n  preference: zh\n";
        let new = "llm-pi-ai:\n  providers:\n    b:\n      models:\n        - id: y\n";
        let merged = upsert_section(old, new);
        assert!(merged.contains("locale:\n  preference: zh"));
        assert!(merged.contains("id: y"));
    }

    #[test]
    fn credential_upsert_merges_by_name() {
        let text = "DEEPSEEK_API_KEY: keep-me\nCORP_GATEWAY_KEY: old-key\n";
        let out = upsert_credential(text, "CORP_GATEWAY_KEY", "new-key");
        assert!(out.contains("CORP_GATEWAY_KEY: 'new-key'"));
        assert!(out.contains("DEEPSEEK_API_KEY: keep-me"));
        assert!(!out.contains("old-key"));
    }

    #[test]
    fn credential_upsert_appends_when_missing() {
        let text = "DEEPSEEK_API_KEY: keep-me\n";
        let out = upsert_credential(text, "CORP_GATEWAY_KEY", "k");
        assert!(out.contains("DEEPSEEK_API_KEY: keep-me"));
        assert!(out.contains("CORP_GATEWAY_KEY: 'k'"));
    }

    #[test]
    fn parse_multiple_providers() {
        let text = "\
llm-pi-ai:
  providers:
    alpha:
      apiKeyEnv: ALPHA_KEY
      models:
        - id: a1
        - id: a2
    beta:
      apiKeyEnv: BETA_KEY
      models:
        - id: b1
";
        let providers = parse_providers(text).unwrap();
        assert_eq!(providers.len(), 2);
        assert_eq!(providers[0].route, "alpha");
        assert_eq!(providers[0].model_count, 2);
        assert_eq!(providers[0].api_key_env.as_deref(), Some("ALPHA_KEY"));
        assert_eq!(providers[1].route, "beta");
        assert_eq!(providers[1].model_count, 1);
        assert_eq!(providers[1].api_key_env.as_deref(), Some("BETA_KEY"));
    }

    #[test]
    fn parse_fields_after_models() {
        // apiKeyEnv 出现在 models 之后仍被识别（models 块结束后回到字段解析）
        let text = "\
llm-pi-ai:
  providers:
    gw:
      models:
        - id: x
      apiKeyEnv: LATE_KEY
";
        let providers = parse_providers(text).unwrap();
        assert_eq!(providers.len(), 1);
        assert_eq!(providers[0].model_count, 1);
        assert_eq!(providers[0].api_key_env.as_deref(), Some("LATE_KEY"));
    }

    #[test]
    fn normalize_strips_leading_blank_lines() {
        let text = "\n\nllm-pi-ai:\n  providers:\n    gw:\n      models:\n        - id: x\n";
        let normalized = normalize_section(text).unwrap();
        assert!(
            normalized.starts_with("llm-pi-ai:"),
            "unexpected: {normalized}"
        );
        assert!(normalized.ends_with('\n'));
        assert_eq!(normalized.matches("llm-pi-ai:").count(), 1);
    }

    #[test]
    fn parse_models_then_two_fields() {
        // models 之后同一 provider 连续两个字段（apiKeyEnv + displayName）
        let text = "llm-pi-ai:
  providers:
    alpha:
      models:
        - id: a1
      apiKeyEnv: ALPHA_KEY
      displayName: Alpha
";
        let providers = parse_providers(text).unwrap();
        assert_eq!(providers.len(), 1);
        assert_eq!(providers[0].api_key_env.as_deref(), Some("ALPHA_KEY"));
        assert_eq!(providers[0].display_name, "Alpha");
    }

    #[test]
    fn normalize_rejects_invalid() {
        let err = normalize_section("locale:\n  preference: zh\n").unwrap_err();
        assert!(err.contains("llm-pi-ai"), "unexpected: {err}");
    }

    #[test]
    fn extract_section_pulls_llm_pi_ai_block() {
        let text = "locale:\n  preference: zh\nllm-pi-ai:\n  providers:\n    gw:\n      models:\n        - id: x\nui-theme:\n  preference: dark\n";
        let out = extract_section_text(text);
        assert_eq!(
            out,
            "llm-pi-ai:\n  providers:\n    gw:\n      models:\n        - id: x\n"
        );
    }

    #[test]
    fn extract_section_empty_when_absent() {
        let out = extract_section_text("locale:\n  preference: zh\n");
        assert!(out.trim().is_empty());
    }

    #[test]
    fn filter_removes_builtin_keeps_custom() {
        let section = "llm-pi-ai:\n  providers:\n    openai:\n      apiKeyEnv: OPENAI_API_KEY\n    acme-gateway:\n      displayName: Acme\n      apiKeyEnv: ACME_KEY\n      models:\n        - id: x\n";
        let out = filter_builtin_routes(section);
        assert!(!out.contains("openai:"));
        assert!(out.contains("acme-gateway:"));
        assert!(out.contains("displayName: Acme"));
        // 结构行保留
        assert!(out.contains("llm-pi-ai:"));
        assert!(out.contains("providers:"));
    }

    #[test]
    fn filter_all_builtin_returns_empty() {
        let section = "llm-pi-ai:\n  providers:\n    openai:\n      apiKeyEnv: OPENAI_API_KEY\n    deepseek:\n      apiKeyEnv: DEEPSEEK_API_KEY\n";
        let out = filter_builtin_routes(section);
        // 全部是官方路由：应返回空字符串（而非只剩结构行）
        assert!(out.is_empty(), "expected empty, got: {out}");
    }
}
