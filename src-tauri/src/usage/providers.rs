//! 供应商发现：从 dsh 的 `settings.yaml` 读取已配置的账户路由。
//!
//! 两个来源（与 dsh-usage-stats 的供应商枚举一致）：
//! - 官方 DeepSeek 路由：`llm-deepseek` 段（缺省时用 `api.deepseek.com`）；
//! - 自定义/目录路由：`llm-pi-ai.providers` 字典的每个键（`displayName`、
//!   `apiKeyEnv`、`baseURL`）。
//!
//! 只读文件，不写回。凭据引用只是环境变量名（或 `.credentials.yaml` 键），
//! 绝不在此落盘任何密钥。

use crate::app_state::Config;

/// 一条可查询账户的供应商路由。
#[derive(Clone, Debug)]
pub struct ProviderRoute {
    /// 路由 id（pi-ai providers 字典键；官方 DeepSeek 为 `deepseek-official`）。
    pub id: String,
    /// 展示名（缺省为 id）。
    pub display_name: String,
    /// 凭据引用（环境变量名 / `.credentials.yaml` 键）。
    pub api_key_env: Option<String>,
    /// 上游 base URL。
    pub base_url: Option<String>,
}

/// 默认 DeepSeek 连接事实。
const DEEPSEEK_DEFAULTS: (&str, &str) = ("DEEPSEEK_API_KEY", "https://api.deepseek.com");

/// 读取 settings.yaml 中某顶层段的单个标量字段（行级，不整体解析）。
fn section_field(text: &str, section: &str, field: &str) -> Option<String> {
    let mut in_section = false;
    for line in text.lines() {
        if !in_section {
            if !line.starts_with(' ') && line.trim_end() == format!("{section}:") {
                in_section = true;
            }
            continue;
        }
        if !line.starts_with(' ') && !line.trim().is_empty() {
            return None; // 段结束
        }
        if let Some(rest) = line.trim_start().strip_prefix(field) {
            if rest.trim_start().starts_with(':') {
                return line
                    .split_once(':')
                    .map(|(_, v)| v.trim().trim_matches(['"', '\'']).to_string())
                    .filter(|v| !v.is_empty());
            }
        }
    }
    None
}

/// 读取 `llm-deepseek` 段的官方路由与 `llm-pi-ai.providers` 的全部路由。
pub fn configured_routes(config: &Config) -> Vec<ProviderRoute> {
    let Ok(text) = std::fs::read_to_string(config.dsh_home().join("settings.yaml")) else {
        return vec![ProviderRoute {
            id: "deepseek-official".to_string(),
            display_name: "DeepSeek".to_string(),
            api_key_env: Some(DEEPSEEK_DEFAULTS.0.to_string()),
            base_url: Some(DEEPSEEK_DEFAULTS.1.to_string()),
        }];
    };
    let mut out = Vec::new();

    // 官方 DeepSeek 路由。
    let ds_key = section_field(&text, "llm-deepseek", "apiKeyEnv");
    let ds_base = section_field(&text, "llm-deepseek", "baseURL");
    out.push(ProviderRoute {
        id: "deepseek-official".to_string(),
        display_name: "DeepSeek".to_string(),
        api_key_env: Some(ds_key.unwrap_or_else(|| DEEPSEEK_DEFAULTS.0.to_string())),
        base_url: Some(ds_base.unwrap_or_else(|| DEEPSEEK_DEFAULTS.1.to_string())),
    });

    // llm-pi-ai.providers 的每个路由。
    if let Some(providers) = extract_providers_block(&text, "llm-pi-ai") {
        for (route, fields) in providers {
            out.push(ProviderRoute {
                display_name: fields
                    .get("displayName")
                    .filter(|v| !v.is_empty())
                    .cloned()
                    .unwrap_or_else(|| route.clone()),
                api_key_env: fields.get("apiKeyEnv").cloned(),
                base_url: fields.get("baseURL").cloned(),
                id: route,
            });
        }
    }
    out
}

/// 从 settings.yaml 文本提取 `llm-pi-ai:` 段内 `providers:` 块里每个路由的
/// `displayName` / `apiKeyEnv` / `baseURL`。
///
/// 只行级解析相关的键，避免整体反序列化改动用户文件里的注释/顺序。
fn extract_providers_block(
    text: &str,
    section: &str,
) -> Option<std::collections::BTreeMap<String, std::collections::BTreeMap<String, String>>> {
    // 定位 `llm-pi-ai:` 顶层段 → `providers:` 子块。
    let mut in_section = false;
    let mut route: Option<String> = None;
    let mut route_fields: Option<std::collections::BTreeMap<String, String>> = None;
    let mut out = std::collections::BTreeMap::new();
    for line in text.lines() {
        if !in_section {
            if !line.starts_with(' ') && line.trim_end() == format!("{section}:") {
                in_section = true;
            }
            continue;
        }
        if !line.starts_with(' ') && !line.trim().is_empty() {
            break; // 下一顶层键，段结束
        }
        let indent = line.len() - line.trim_start().len();
        let trimmed = line.trim_start();
        if indent == 2 && trimmed == "providers:" {
            route = None;
            continue;
        }
        // 路由键：providers 下 4 空格缩进的 `<route>:`
        if indent == 4 {
            if let Some(key) = trimmed.strip_suffix(':') {
                if !key.trim().is_empty() {
                    if let Some((prev_route, prev_fields)) = route.take().zip(route_fields.take()) {
                        out.insert(prev_route, prev_fields);
                    }
                    route = Some(key.trim().to_string());
                    route_fields = Some(std::collections::BTreeMap::new());
                    continue;
                }
            }
        }
        // 字段行：路由块内 6 空格缩进的 `field: value`
        if indent == 6 {
            if let (Some(_cur), Some(fields)) = (route.as_ref(), route_fields.as_mut()) {
                if let Some((field, value)) = trimmed.split_once(':') {
                    let field = field.trim();
                    let value = value.trim().trim_matches(['"', '\'']).to_string();
                    if matches!(field, "displayName" | "apiKeyEnv" | "baseURL") && !value.is_empty()
                    {
                        fields.insert(field.to_string(), value);
                    }
                }
            }
        }
    }
    if let Some((prev_route, prev_fields)) = route.take().zip(route_fields.take()) {
        out.insert(prev_route, prev_fields);
    }
    if out.is_empty() {
        None
    } else {
        Some(out)
    }
}

#[cfg(test)]
mod tests {
    use super::{configured_routes, extract_providers_block, section_field};
    use crate::app_state::Config;

    const SETTINGS: &str = "\
locale:\n  preference: zh\n\
llm-deepseek:\n  apiKeyEnv: DEEPSEEK_API_KEY\n  baseURL: https://api.deepseek.com\n\
llm-pi-ai:\n  providers:\n    gateway:\n      displayName: My Gateway\n      apiKeyEnv: GATEWAY_KEY\n      baseURL: https://gateway.example.com/v1\n      models:\n        - id: gpt-x\n    kimi-coding:\n      apiKeyEnv: KIMI_API_KEY\n";

    #[test]
    fn reads_section_fields_linewise() {
        assert_eq!(
            section_field(SETTINGS, "llm-deepseek", "apiKeyEnv").as_deref(),
            Some("DEEPSEEK_API_KEY")
        );
        assert_eq!(
            section_field(SETTINGS, "llm-deepseek", "baseURL").as_deref(),
            Some("https://api.deepseek.com")
        );
        assert_eq!(section_field(SETTINGS, "llm-deepseek", "missing"), None);
    }

    #[test]
    fn extracts_provider_routes_with_base_url() {
        let providers = extract_providers_block(SETTINGS, "llm-pi-ai").unwrap();
        assert_eq!(providers.len(), 2);
        let gateway = providers.get("gateway").unwrap();
        assert_eq!(gateway.get("displayName").unwrap(), "My Gateway");
        assert_eq!(gateway.get("apiKeyEnv").unwrap(), "GATEWAY_KEY");
        assert_eq!(
            gateway.get("baseURL").unwrap(),
            "https://gateway.example.com/v1"
        );
        // models 块与未声明字段不得混入
        assert!(!gateway.contains_key("models"));
        assert!(!providers
            .get("kimi-coding")
            .unwrap()
            .contains_key("displayName"));
    }

    #[test]
    fn configured_routes_always_includes_deepseek() {
        let mut config = Config::load();
        config.dsh_home = std::env::temp_dir().join("dshbox-usage-providers-nonexistent");
        std::fs::create_dir_all(&config.dsh_home).unwrap();
        // 无 settings.yaml 时回落默认 DeepSeek。
        let routes = configured_routes(&config);
        assert!(routes.iter().any(|r| r.id == "deepseek-official"));
        let _ = std::fs::remove_dir_all(&config.dsh_home);
    }
}
