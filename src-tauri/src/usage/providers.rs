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

/// 行级标量取值：成对引号去一层；未加引号的 ` #` 起为行内注释须截断
/// （`abc#def` 的 # 前无空白，不截断——与 credentials.rs 的 plain
/// scalar 截断规则一致）。
fn scalar_value(raw: &str) -> Option<String> {
    let value = raw.trim();
    let paired = |v: &str, q: char| v.len() >= 2 && v.starts_with(q) && v.ends_with(q);
    let value = if paired(value, '"') || paired(value, '\'') {
        value[1..value.len() - 1].trim().to_string()
    } else {
        value
            .split(" #")
            .next()
            .unwrap_or_default()
            .trim_end()
            .to_string()
    };
    (!value.is_empty()).then_some(value)
}

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
                return line.split_once(':').and_then(|(_, v)| scalar_value(v));
            }
        }
    }
    None
}

/// 是否存在某顶层段（`section:` 行）。
fn has_section(text: &str, section: &str) -> bool {
    text.lines()
        .any(|line| !line.starts_with(' ') && line.trim_end() == format!("{section}:"))
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

    // llm-pi-ai.providers 的每个路由。段存在但提取不到任何路由时记日志：
    // providers 块缺失或结构无法识别会让全部自定义路由被静默丢弃。
    match extract_providers_block(&text, "llm-pi-ai") {
        Some(providers) => {
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
        None if has_section(&text, "llm-pi-ai") => {
            crate::logging::log(
                "usage: settings.yaml 的 llm-pi-ai 段未提取到自定义路由（providers 块缺失或结构无法识别）",
            );
        }
        None => {}
    }
    out
}

/// 是否为 `providers:` 键行（允许行内注释；content 已去行首空白）。
/// 借鉴 model_config 的同名判定；跨模块私用不可取，保留本地简化副本。
fn is_providers_key(content: &str) -> bool {
    content.strip_prefix("providers:").is_some_and(|rest| {
        let rest = rest.trim_start();
        rest.is_empty() || rest.starts_with('#')
    })
}

/// 从 settings.yaml 文本提取 `llm-pi-ai:` 段内 `providers:` 块里每个路由的
/// `displayName` / `apiKeyEnv` / `baseURL`。
///
/// 三级缩进（providers 键 / 路由键 / 字段行）动态探测而非硬编码 2/4/6
/// 空格（思路同 model_config 的动态 route_indent），兼容 4 空格等缩进
/// 风格。只行级解析相关的键，避免整体反序列化改动用户文件里的注释/顺序。
fn extract_providers_block(
    text: &str,
    section: &str,
) -> Option<std::collections::BTreeMap<String, std::collections::BTreeMap<String, String>>> {
    let indent_of = |line: &str| line.len() - line.trim_start().len();
    // 收集顶层段内的行（到下一顶层键为止）。
    let mut lines: Vec<&str> = Vec::new();
    let mut in_section = false;
    for line in text.lines() {
        if !in_section {
            if indent_of(line) == 0 && line.trim_end() == format!("{section}:") {
                in_section = true;
            }
            continue;
        }
        if indent_of(line) == 0 && !line.trim().is_empty() {
            break; // 下一顶层键，段结束
        }
        lines.push(line);
    }
    // providers: 键及其缩进。
    let p_pos = lines
        .iter()
        .position(|l| is_providers_key(l.trim_start()))?;
    let p_indent = indent_of(lines[p_pos]);
    // providers 块：键之后、缩进回到 providers 键层级之前（空行/注释不终结块）。
    let block: Vec<&str> = lines[p_pos + 1..]
        .iter()
        .take_while(|l| {
            let content = l.trim_start();
            content.is_empty() || content.starts_with('#') || indent_of(l) > p_indent
        })
        .copied()
        .collect();
    // 路由键缩进 = 块内首个非空非注释行的缩进。
    let route_indent = block.iter().find_map(|l| {
        let content = l.trim_start();
        (!content.is_empty() && !content.starts_with('#')).then(|| indent_of(l))
    })?;
    // 字段行缩进 = 块内比路由键更深的最小缩进（更深层嵌套块里的同名键
    // 不会被误当路由字段）。
    let field_indent = block
        .iter()
        .map(|l| indent_of(l))
        .filter(|&i| i > route_indent)
        .min();

    let mut route: Option<String> = None;
    let mut route_fields: Option<std::collections::BTreeMap<String, String>> = None;
    let mut out = std::collections::BTreeMap::new();
    for line in block {
        let content = line.trim_start();
        if content.is_empty() || content.starts_with('#') {
            continue;
        }
        let indent = indent_of(line);
        // 路由键：`<route>:`
        if indent == route_indent {
            if let Some(key) = content.trim_end().strip_suffix(':') {
                let key = key.trim().trim_matches(['"', '\'']);
                if !key.is_empty() {
                    if let Some((prev_route, prev_fields)) = route.take().zip(route_fields.take()) {
                        out.insert(prev_route, prev_fields);
                    }
                    route = Some(key.to_string());
                    route_fields = Some(std::collections::BTreeMap::new());
                }
            }
            continue;
        }
        // 字段行：`field: value`
        if Some(indent) == field_indent {
            if let (Some(_cur), Some(fields)) = (route.as_ref(), route_fields.as_mut()) {
                if let Some((field, value)) = content.split_once(':') {
                    let field = field.trim();
                    if matches!(field, "displayName" | "apiKeyEnv" | "baseURL") {
                        if let Some(value) = scalar_value(value) {
                            fields.insert(field.to_string(), value);
                        }
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
    use super::{configured_routes, extract_providers_block, scalar_value, section_field};
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
    fn extracts_routes_with_four_space_indentation() {
        // 缩进风格不写死：providers/路由键/字段行三级缩进动态探测。
        let text = "\
llm-pi-ai:\n    providers:\n        gateway:\n            displayName: My Gateway\n            apiKeyEnv: GATEWAY_KEY\n            baseURL: https://gateway.example.com/v1\n";
        let providers = extract_providers_block(text, "llm-pi-ai").unwrap();
        let gateway = providers.get("gateway").unwrap();
        assert_eq!(gateway.get("displayName").unwrap(), "My Gateway");
        assert_eq!(gateway.get("apiKeyEnv").unwrap(), "GATEWAY_KEY");
        assert_eq!(
            gateway.get("baseURL").unwrap(),
            "https://gateway.example.com/v1"
        );
    }

    #[test]
    fn strips_inline_comments_from_field_values() {
        // plain scalar 的 ` #` 起为行内注释（对齐 credentials.rs 规则）；
        // `abc#def` 的 # 前无空白不截断；引号内的 # 属于值本身。
        assert_eq!(
            scalar_value(" https://x # 备注").as_deref(),
            Some("https://x")
        );
        assert_eq!(scalar_value(" abc#def").as_deref(), Some("abc#def"));
        assert_eq!(
            scalar_value(" \"https://x # keep\"").as_deref(),
            Some("https://x # keep")
        );
        let text = "\
llm-pi-ai:\n  providers:\n    gateway:\n      baseURL: https://x.example.com # 备注\n      apiKeyEnv: abc#def\n";
        let providers = extract_providers_block(text, "llm-pi-ai").unwrap();
        let gateway = providers.get("gateway").unwrap();
        assert_eq!(gateway.get("baseURL").unwrap(), "https://x.example.com");
        assert_eq!(gateway.get("apiKeyEnv").unwrap(), "abc#def");
    }

    #[test]
    fn logs_when_llm_pi_ai_section_has_no_extractable_routes() {
        let root = std::env::temp_dir().join(format!(
            "dshbox-usage-providers-log-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let mut config = Config::load();
        config.dsh_home = root.clone();
        // llm-pi-ai 段存在但没有 providers 块：自定义路由被静默丢弃，必须留痕。
        std::fs::write(root.join("settings.yaml"), "llm-pi-ai:\n  other: 1\n").unwrap();
        let log_path = root.join("logs").join("dshbox.log");
        crate::logging::init(log_path.clone());
        let routes = configured_routes(&config);
        assert!(routes.iter().all(|r| r.id == "deepseek-official"));
        let logged = std::fs::read_to_string(&log_path).unwrap_or_default();
        assert!(
            logged.contains("llm-pi-ai"),
            "应记录块缺失日志，实际：{logged}"
        );
        let _ = std::fs::remove_dir_all(&root);
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
