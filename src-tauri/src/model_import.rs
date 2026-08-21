//! 模型配置导入：把用户粘贴的一段 `llm-pi-ai:` YAML 段解析、校验后写入
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

/// 从文本中逐行识别到的 provider 信息（内部表示）。
#[derive(Debug)]
struct ProviderInfo {
    route: String,
    display_name: String,
    model_count: usize,
    api_key_env: Option<String>,
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
    for (name, key) in &payload.keys {
        if !declared.contains(name) {
            return Err(crate::locale::text(
                "导入的配置中没有声明该凭据引用。",
                "The imported config does not declare this credential reference.",
            )
            .into());
        }
        if key.trim().is_empty() {
            return Err(
                crate::locale::text("API Key 不能为空。", "API key cannot be empty.").into(),
            );
        }
    }

    // 3) 写 settings.yaml：整体替换或追加 llm-pi-ai 段。
    let settings_path = config.dsh_home().join("settings.yaml");
    let settings_text = std::fs::read_to_string(&settings_path).unwrap_or_default();
    let normalized = normalize_section(&payload.yaml)?;
    let merged = upsert_section(&settings_text, &normalized);
    app_state::atomic_write(&settings_path, &merged)?;

    // 4) 写 .credentials.yaml：行级合并每个声明的凭据。
    let credentials_path = config.dsh_home().join(".credentials.yaml");
    let credentials_text = std::fs::read_to_string(&credentials_path).unwrap_or_default();
    let mut credentials_out = credentials_text;
    for (name, key) in &payload.keys {
        credentials_out = upsert_credential(&credentials_out, name, key.trim());
    }
    app_state::atomic_write(&credentials_path, &credentials_out)?;

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

/// 解析并校验导入文本中的 provider 路由。状态机按缩进层级推进：
/// 0 = 顶层键（只认 llm-pi-ai）、2 = providers / 段内扩展字段、
/// 4 = provider 路由键、>=6 = provider 内字段（apiKeyEnv/displayName/models）。
/// 只做行级结构识别，不要求完整 YAML 语义（模板格式由外部约定并保证）。
fn parse_providers(yaml: &str) -> Result<Vec<ProviderInfo>, String> {
    if yaml.trim().is_empty() {
        return Err(crate::locale::text("导入内容为空。", "The imported content is empty.").into());
    }

    let mut in_section = false;
    let mut in_providers = false;
    let mut in_models = false;
    let mut current: Option<ProviderInfo> = None;
    let mut providers: Vec<ProviderInfo> = Vec::new();
    let section = format!("{SECTION_KEY}:");

    for line in yaml.lines() {
        let trimmed = line.trim_end();
        if trimmed.is_empty() {
            continue;
        }
        let indent = line.len() - line.trim_start().len();
        let content = line.trim_start();
        if content.starts_with('#') {
            continue;
        }

        if indent == 0 {
            // 顶层键：llm-pi-ai 是唯一合法开头；段内再遇顶层键即段结束。
            if content == section {
                if !in_section {
                    in_section = true;
                    in_providers = false;
                    in_models = false;
                }
                continue;
            }
            if in_section {
                break;
            }
            return Err(crate::locale::text(
                "导入内容不是模型配置：缺少 llm-pi-ai 顶层段。",
                "The imported content is not a model config: missing the llm-pi-ai top-level section.",
            )
            .into());
        }

        if !in_section {
            return Err(crate::locale::text(
                "导入内容不是模型配置：llm-pi-ai 段结构不合法。",
                "The imported content is not a valid model config.",
            )
            .into());
        }

        if indent == 2 {
            // 结束当前 provider（回到段内其他字段或 providers 键）。
            if let Some(p) = current.take() {
                providers.push(p);
            }
            in_providers = content == "providers:";
            in_models = false;
            continue;
        }
        if indent == 4 {
            // provider 路由键（如 `internal-gateway:`）。
            if in_providers {
                if let Some(route) = content.strip_suffix(':') {
                    if !route.is_empty() && !route.contains(' ') {
                        if let Some(p) = current.take() {
                            providers.push(p);
                        }
                        current = Some(ProviderInfo {
                            route: route.to_string(),
                            display_name: route.to_string(),
                            model_count: 0,
                            api_key_env: None,
                        });
                        in_models = false;
                        continue;
                    }
                }
                return Err(crate::locale::text(
                    "模型配置 provider 路由名不合法。",
                    "Invalid provider route name in model config.",
                )
                .into());
            }
            continue;
        }
        if indent < 4 {
            continue;
        }

        // provider 内（indent >= 6）
        let Some(p) = current.as_mut() else { continue };
        if in_models {
            if content.starts_with("- ") {
                p.model_count += 1;
                continue;
            }
            if indent == 6 {
                // 退出 models 块，回到 provider 级字段继续解析这一行。
                in_models = false;
            } else {
                continue; // 模型条目字段（id:/name:/...），忽略。
            }
        }
        if let Some(rest) = content.strip_prefix("apiKeyEnv:") {
            let value = rest.trim().trim_matches(['"', '\'']);
            if value.is_empty() {
                return Err(crate::locale::text(
                    "模型配置中 apiKeyEnv 不能为空。",
                    "apiKeyEnv cannot be empty in model config.",
                )
                .into());
            }
            p.api_key_env = Some(value.to_string());
            continue;
        }
        if let Some(rest) = content.strip_prefix("displayName:") {
            let value = rest.trim().trim_matches(['"', '\'']);
            if !value.is_empty() {
                p.display_name = value.to_string();
            }
            continue;
        }
        if content == "models:" {
            in_models = true;
            continue;
        }
    }
    if let Some(p) = current.take() {
        providers.push(p);
    }

    if providers.is_empty() {
        return Err(crate::locale::text(
            "模型配置中没有识别到任何提供方路由（providers）。",
            "No provider routes found in the model config.",
        )
        .into());
    }
    for p in &providers {
        if p.model_count == 0 {
            return Err(crate::locale::text(
                "提供方 {route} 没有声明任何模型（models）。",
                "Provider {route} declares no models.",
            )
            .replace("{route}", &p.route));
        }
    }
    Ok(providers)
}

/// 把导入文本规范化为可写入的顶层段：去掉前导空行，确保末尾换行。
fn normalize_section(yaml: &str) -> Result<String, String> {
    // 先经 parse_providers 校验（保证结构合法）。
    parse_providers(yaml)?;
    let mut out = String::new();
    for line in yaml.lines() {
        if out.is_empty() && line.trim().is_empty() {
            continue; // 去掉前导空行
        }
        out.push_str(line);
        out.push('\n');
    }
    if !out.ends_with('\n') {
        out.push('\n');
    }
    Ok(out)
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

/// 行级合并写入一个凭据条目（`NAME: value`）：同名行替换、无则追加，
/// 不触碰其他凭据条目。与 onboarding 的 DEEPSEEK_API_KEY 写入同规格。
fn upsert_credential(text: &str, name: &str, value: &str) -> String {
    let mut out = String::new();
    let mut wrote = false;
    for line in text.lines() {
        if line.trim_start().starts_with(&format!("{name}:")) {
            if !wrote {
                out.push_str(&format!("{name}: {value}\n"));
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
        out.push_str(&format!("{name}: {value}\n"));
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
        assert!(out.contains("CORP_GATEWAY_KEY: new-key"));
        assert!(out.contains("DEEPSEEK_API_KEY: keep-me"));
        assert!(!out.contains("old-key"));
    }

    #[test]
    fn credential_upsert_appends_when_missing() {
        let text = "DEEPSEEK_API_KEY: keep-me\n";
        let out = upsert_credential(text, "CORP_GATEWAY_KEY", "k");
        assert!(out.contains("DEEPSEEK_API_KEY: keep-me"));
        assert!(out.contains("CORP_GATEWAY_KEY: k"));
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
}
