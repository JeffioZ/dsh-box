//! 供应商发现：从 dsh 的 `settings.yaml` 读取已配置的账户路由。
//!
//! 两个来源（与 dsh-usage-stats 的供应商枚举一致）：
//! - 官方 DeepSeek 路由：`llm-deepseek` 段（缺省时用 `api.deepseek.com`）；
//! - 自定义/目录路由：`llm-pi-ai.providers` 字典的每个键（`displayName`、
//!   `apiKeyEnv`、`baseURL`）。
//!
//! `providers` 值支持块式（缩进）与流式（花括号）两种 YAML 写法：dsh 的
//! settings 写盘器按注释保留式 patch 重写、原样保留既有节点风格，手写或
//! 编辑器格式化引入的流式块会长期合法存在。
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

/// 是否为 `providers:` 键行（允许行内注释与行内流式 `{` 值；content 已去
/// 行首空白）。借鉴 model_config 的同名判定；跨模块私用不可取，保留本地
/// 简化副本。
fn is_providers_key(content: &str) -> bool {
    content.strip_prefix("providers:").is_some_and(|rest| {
        let rest = rest.trim_start();
        // 空/注释 = 块式（花括号值可能在下一行）；`{` 起为行内流式值
        rest.is_empty() || rest.starts_with('#') || rest.starts_with('{')
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
    // 流式（花括号）值优先：行内 `providers: {` 或键行之后首行以 `{` 开头。
    // 收集不到（块式）则继续走下方行级块式解析。
    if let Some(flow) = collect_flow_map_text(&lines, p_pos, p_indent) {
        return parse_flow_providers(&flow);
    }
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

/// 从 `providers:` 键行起收集流式 map 的完整文本（含外层花括号）。起点为
/// 键行行内 `{`，或其后首条（缩进深于键行、非空非注释）以 `{` 开头的行；
/// 字符级扫描，引号内的括号不计深度，外层花括号闭合即止。块式或花括号
/// 在段内未闭合（畸形）返回 None。
fn collect_flow_map_text(lines: &[&str], key_pos: usize, key_indent: usize) -> Option<String> {
    let mut start: Option<(usize, usize)> = None; // (行号, `{` 的字节偏移)
    let inline_rest = lines[key_pos].trim_start().strip_prefix("providers:")?;
    if let Some(off) = inline_rest.find('{') {
        let prefix_len = lines[key_pos].len() - inline_rest.len();
        start = Some((key_pos, prefix_len + off));
    } else {
        for (i, line) in lines.iter().enumerate().skip(key_pos + 1) {
            let content = line.trim_start();
            if content.is_empty() || content.starts_with('#') {
                continue;
            }
            if content.starts_with('{') && line.len() - content.len() > key_indent {
                start = Some((i, line.len() - content.len()));
            }
            break; // 首条内容行不是 `{` 开头即为块式
        }
    }
    let (first_line, first_off) = start?;
    let mut depth = 0i32;
    let mut quote: Option<char> = None;
    let mut escaped = false;
    let mut out = String::new();
    for (i, line) in lines.iter().enumerate().skip(first_line) {
        for (byte, ch) in line.char_indices() {
            if i == first_line && byte < first_off {
                continue;
            }
            out.push(ch);
            if let Some(q) = quote {
                if escaped {
                    escaped = false;
                } else if q == '"' && ch == '\\' {
                    escaped = true;
                } else if ch == q {
                    quote = None;
                }
                continue;
            }
            match ch {
                '"' | '\'' => quote = Some(ch),
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        return Some(out);
                    }
                }
                _ => {}
            }
        }
        out.push('\n');
    }
    None
}

/// 解析 providers 的流式 map 文本（`{`…`}` 已含）：路由 → 字段 map。只取
/// `displayName` / `apiKeyEnv` / `baseURL` 三个标量字段；`models` 等嵌套
/// 集合由条目切分的括号深度感知自然跳过。路由值为 `null`/`~`/空时按
/// 「无字段路由」计入（与块式同语义）；非 map 非空的畸形值跳过该路由。
fn parse_flow_providers(
    text: &str,
) -> Option<std::collections::BTreeMap<String, std::collections::BTreeMap<String, String>>> {
    let inner = text.trim().strip_prefix('{')?.strip_suffix('}')?;
    let mut out = std::collections::BTreeMap::new();
    for entry in split_flow_entries(inner) {
        let (key, value) = entry.split_once(':')?;
        let value = value.trim();
        let fields_text = if value.starts_with('{') && value.ends_with('}') {
            &value[1..value.len() - 1]
        } else if value.is_empty() || value == "null" || value == "~" {
            ""
        } else {
            continue;
        };
        let key = unquote_flow_scalar(key.trim());
        if key.is_empty() {
            continue;
        }
        let mut fields = std::collections::BTreeMap::new();
        for field in split_flow_entries(fields_text) {
            let Some((name, raw)) = field.split_once(':') else {
                continue;
            };
            let name = name.trim();
            if matches!(name, "displayName" | "apiKeyEnv" | "baseURL") {
                if let Some(v) = scalar_value(raw) {
                    fields.insert(name.to_string(), v);
                }
            }
        }
        out.insert(key, fields);
    }
    (!out.is_empty()).then_some(out)
}

/// 按深度 0 的逗号切分流式集合条目；引号内与嵌套 `{}`/`[]` 内的逗号不切。
/// 返回各条目原文（保留内部空白，含换行）。
fn split_flow_entries(text: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut cur = String::new();
    let mut depth = 0i32;
    let mut quote: Option<char> = None;
    let mut escaped = false;
    for ch in text.chars() {
        if let Some(q) = quote {
            cur.push(ch);
            if escaped {
                escaped = false;
            } else if q == '"' && ch == '\\' {
                escaped = true;
            } else if ch == q {
                quote = None;
            }
            continue;
        }
        match ch {
            '"' | '\'' => {
                quote = Some(ch);
                cur.push(ch);
            }
            '{' | '[' => {
                depth += 1;
                cur.push(ch);
            }
            '}' | ']' => {
                depth -= 1;
                cur.push(ch);
            }
            ',' if depth == 0 => parts.push(std::mem::take(&mut cur)),
            _ => cur.push(ch),
        }
    }
    if !cur.trim().is_empty() {
        parts.push(cur);
    }
    parts.retain(|p| !p.trim().is_empty());
    parts
}

/// 流式键/标量去一层成对引号（块式 `scalar_value` 的键用简化版）。
fn unquote_flow_scalar(s: &str) -> String {
    let paired = |v: &str, q: char| v.len() >= 2 && v.starts_with(q) && v.ends_with(q);
    if paired(s, '"') || paired(s, '\'') {
        s[1..s.len() - 1].to_string()
    } else {
        s.to_string()
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
    fn extracts_flow_style_providers_multiline() {
        // 跨行流式形态：providers 值为流式 map（dsh 写盘器原样保留既有
        // 流式风格），含 CJK 显示名、未声明字段（api）与嵌套 models 数组
        let text = "\
llm-pi-ai:\n  providers:\n    {\n      gateway:\n        {\n          displayName: 示例网关,\n          apiKeyEnv: GATEWAY_KEY,\n          api: openai-responses,\n          baseURL: https://gateway.example.com/v1,\n          models:\n            [\n              { id: model-a },\n              { id: model-b }\n            ]\n        }\n    }\n";
        let providers = extract_providers_block(text, "llm-pi-ai").unwrap();
        assert_eq!(providers.len(), 1);
        let route = providers.get("gateway").unwrap();
        assert_eq!(route.get("displayName").unwrap(), "示例网关");
        assert_eq!(route.get("apiKeyEnv").unwrap(), "GATEWAY_KEY");
        assert_eq!(
            route.get("baseURL").unwrap(),
            "https://gateway.example.com/v1"
        );
        // models 与未声明字段不得混入
        assert!(!route.contains_key("models"));
        assert!(!route.contains_key("api"));
    }

    #[test]
    fn extracts_flow_style_providers_inline() {
        // 行内流式：多路由、引号值、无字段路由（null 与空 map 均按块式
        // 「无字段路由」语义计入）
        let text = "llm-pi-ai:\n  providers: { gw: { displayName: My GW, apiKeyEnv: K, baseURL: 'https://x.example.com' }, kimi-coding: null, bare: {} }\n";
        let providers = extract_providers_block(text, "llm-pi-ai").unwrap();
        assert_eq!(providers.len(), 3);
        let gw = providers.get("gw").unwrap();
        assert_eq!(gw.get("displayName").unwrap(), "My GW");
        assert_eq!(gw.get("baseURL").unwrap(), "https://x.example.com");
        assert!(!providers
            .get("kimi-coding")
            .unwrap()
            .contains_key("displayName"));
        assert!(providers.get("bare").unwrap().is_empty());
    }

    #[test]
    fn flow_style_unclosed_braces_fall_back_to_block_path() {
        // 花括号未闭合（畸形）：流式收集失败，块式同样提取不到 → 整体
        // 返回 None，由调用方落「未提取到自定义路由」日志
        let text = "llm-pi-ai:\n  providers:\n    { gw: { apiKeyEnv: K\n";
        assert!(extract_providers_block(text, "llm-pi-ai").is_none());
    }

    #[test]
    fn configured_routes_reads_flow_style_settings() {
        // 端到端：流式 providers 与官方 DeepSeek 路由并列产出
        let root = std::env::temp_dir().join(format!(
            "dshbox-usage-providers-flow-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let mut config = Config::load();
        config.dsh_home = root.clone();
        std::fs::write(
            root.join("settings.yaml"),
            "llm-deepseek:\n  apiKeyEnv: DEEPSEEK_API_KEY\nllm-pi-ai:\n  providers:\n    {\n      gateway:\n        {\n          displayName: 示例网关,\n          apiKeyEnv: GATEWAY_KEY,\n          baseURL: https://gateway.example.com/v1\n        }\n    }\n",
        )
        .unwrap();
        let routes = configured_routes(&config);
        assert!(routes.iter().any(|r| r.id == "deepseek-official"));
        let gateway = routes.iter().find(|r| r.id == "gateway").unwrap();
        assert_eq!(gateway.display_name, "示例网关");
        assert_eq!(gateway.api_key_env.as_deref(), Some("GATEWAY_KEY"));
        let _ = std::fs::remove_dir_all(&root);
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
