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

/// settings.yaml 当前是否已有自定义模型路由（`llm-pi-ai` 段含 `providers:` 键）。
/// 用于设置页在「无任何模型配置」时把模型配置板块置顶引导。
pub fn has_custom_providers(config: &Config) -> bool {
    let Ok(text) = std::fs::read_to_string(config.dsh_home().join("settings.yaml")) else {
        return false;
    };
    // 独立扫描 llm-pi-ai 顶层段，在段内查找 providers 键。段起始判定与
    // upsert/extract 共用 is_section_start_line（同一口径，见其注释）。
    let mut in_section = false;
    for line in text.lines() {
        let stripped = line.trim_start();
        if !in_section {
            if is_section_start_line(line) {
                in_section = true;
            }
            continue;
        }
        // 段内：遇到下一个非空非注释顶层键即结束。
        let indent = line.len() - line.trim_start().len();
        let is_comment = stripped.starts_with('#');
        if indent == 0 && !stripped.is_empty() && !is_comment {
            break;
        }
        if is_providers_key(stripped) {
            return true;
        }
    }
    false
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
    // 脱敏：分享文本绝不携带密钥样字段（与前端"不含 API Key"提示一致）
    Ok(Some(parser::strip_secret_lines(&custom_only)))
}

/// 从 settings.yaml 文本中提取 llm-pi-ai 顶层段（纯逻辑，供单测）。
fn extract_section_text(text: &str) -> String {
    let mut out = String::new();
    let mut in_section = false;
    for line in text.lines() {
        if !in_section {
            if is_section_start_line(line) {
                in_section = true;
                out.push_str(line);
                out.push('\n');
            }
            continue;
        }
        // 已进入段：遇到下一个顶层键即段结束。列 0 注释行不是顶层键，
        // 照常保留在段内（模板可能把说明注释插在段中间）。
        let is_comment = line.trim_start().starts_with('#');
        if !line.starts_with(' ') && !line.trim().is_empty() && !is_comment {
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
/// ⚠️ 上游 pi-ai 升级新增官方 provider 时，须对照上游目录同步此名单，
/// 否则新的官方路由会被误当作自定义路由随导出带出。
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

/// 是否为 `providers:` 键行（允许行内注释；content 已去行首空白）。
fn is_providers_key(content: &str) -> bool {
    content.strip_prefix("providers:").is_some_and(|rest| {
        let rest = rest.trim_start();
        rest.is_empty() || rest.starts_with('#')
    })
}

/// 从 llm-pi-ai 段文本中移除官方目录路由，仅保留自定义路由。
/// 无自定义路由时返回空字符串（调用方据此返回 None）。
/// 不假设缩进宽度：providers 块取缩进最小的 `providers:` 行，路由键是
/// 块内第一个更深层级的首层键；缩进的注释行只是注释，不会被当成路由键。
fn filter_builtin_routes(section: &str) -> String {
    let line_indent = |line: &str| line.len() - line.trim_start().len();
    let providers_indent = section
        .lines()
        .filter(|line| is_providers_key(line.trim_start()))
        .map(line_indent)
        .min();
    // 没有 providers 块就没有可导出的路由（与「全是官方路由」同等处理）
    let Some(providers_indent) = providers_indent else {
        return String::new();
    };
    // 路由键缩进 = providers 块内首个更深的非空非注释行（空行与注释不算键）
    let mut route_indent = None;
    let mut in_providers = false;
    for line in section.lines() {
        let content = line.trim_start();
        if !in_providers {
            if is_providers_key(content) && line_indent(line) == providers_indent {
                in_providers = true;
            }
            continue;
        }
        if content.is_empty() || content.starts_with('#') {
            continue;
        }
        if line_indent(line) <= providers_indent {
            break; // providers 块结束，没有任何路由键
        }
        route_indent = Some(line_indent(line));
        break;
    }
    let Some(route_indent) = route_indent else {
        return String::new();
    };
    let mut out = String::new();
    let mut in_providers = false;
    // 当前 provider 是否为自定义路由（决定其整块去留）
    let mut current_is_custom = false;
    // 是否保留过至少一个自定义 provider（否则整段视为空）
    let mut kept_any = false;
    // 是否已遇到首个路由键（之前的块级前言——注释/空行——总是保留）
    let mut seen_any_route = false;
    for line in section.lines() {
        let indent = line_indent(line);
        let content = line.trim_start();
        let is_blank_or_comment = content.is_empty() || content.starts_with('#');
        if !in_providers {
            if is_providers_key(content) && indent == providers_indent {
                in_providers = true;
            }
            out.push_str(line);
            out.push('\n');
            continue;
        }
        if !is_blank_or_comment && indent <= providers_indent {
            // providers 块结束：段级字段等其他内容总是保留
            in_providers = false;
            out.push_str(line);
            out.push('\n');
            continue;
        }
        if !is_blank_or_comment && indent == route_indent {
            // provider 路由键：剥离尾部冒号（及冒号前空白）与首尾引号后匹配
            let route = content
                .trim_end()
                .trim_end_matches(':')
                .trim_end()
                .trim_matches(['"', '\'']);
            current_is_custom = !BUILTIN_ROUTES.contains(&route);
            seen_any_route = true;
            if current_is_custom {
                kept_any = true;
                out.push_str(line);
                out.push('\n');
            }
            continue;
        }
        // 块内前言（首个路由键之前）总是保留；其余空行/注释/更深层级内容
        // 跟随当前 provider 的去留
        if !seen_any_route || current_is_custom {
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
///
/// 失败（含跨进程瞬时写入冲突重试后仍失败）会记录日志，便于定位偶发的一次性
/// 导入失败——此前失败只返回给前端弹窗提示，日志无痕。
pub fn apply(app: &AppHandle, payload: ImportApplyPayload) -> Result<(), String> {
    let result = apply_inner(app, payload);
    if let Err(e) = &result {
        crate::logging::log(&format!("model-import: 导入失败：{e}"));
    }
    result
}

fn apply_inner(app: &AppHandle, payload: ImportApplyPayload) -> Result<(), String> {
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
        if !valid_env_name(name) {
            return Err(crate::locale::text(
                "凭据名称含有不允许的字符。",
                "The credential name contains invalid characters.",
            )
            .into());
        }
        // 与设置页、首次引导同一口径（控制字符 + 4096 上限）
        crate::onboarding::validate_api_key(key.trim())?;
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
    app_state::update_text_file(&settings_path, |text| upsert_section(&text, &normalized))?;

    crate::logging::log(&format!(
        "model-import: 已导入 {} 个提供方路由（写 settings.yaml + credentials.yaml）",
        providers.len()
    ));
    Ok(())
}

/// 顶层 `llm-pi-ai` 段起始行判定：无缩进且键后紧跟 `:`——值可为空、行内
/// 注释或流式空表（`llm-pi-ai: {}`）。段识别三处（has/upsert/extract）必须
/// 共用同一口径：此前 upsert/extract 只认精确 `llm-pi-ai:` 行，遇 dsh 序列化
/// 或手写产出的 `llm-pi-ai: {}` 段行会漏判，导入时在文件末尾追加第二个
/// `llm-pi-ai:` 顶层键，造成重复键的损坏文件且后续导入无法自愈。
fn is_section_start_line(line: &str) -> bool {
    !line.starts_with(' ')
        && line
            .strip_prefix(SECTION_KEY)
            .is_some_and(|rest| rest.trim_start().starts_with(':'))
}

/// settings.yaml 文本是否已含 llm-pi-ai 顶层段。
fn settings_has_section(text: &str) -> bool {
    text.lines().any(is_section_start_line)
}

/// 行级替换（或追加）一个顶层段。`new_section` 必须是完整顶层段（含键行）。
/// 只替换同名顶层段，绝不触碰其他顶层段。原文件已含多个同名顶层段时返回
/// 错误：重复键的 YAML 语义未定义，静默选边会让结果取决于解析器。
fn upsert_section(text: &str, new_section: &str) -> Result<String, String> {
    let mut out = String::new();
    let mut replaced = false;
    let mut lines = text.lines().peekable();
    while let Some(line) = lines.next() {
        let is_top = !line.starts_with(' ') && !line.trim().is_empty();
        if is_top && is_section_start_line(line) {
            if replaced {
                return Err(crate::locale::text(
                    "settings.yaml 中存在重复的 llm-pi-ai 段，请先手动修复后再导入。",
                    "settings.yaml contains duplicate llm-pi-ai sections; fix the file manually before importing.",
                )
                .into());
            }
            // 跳过整个旧 llm-pi-ai 段（直到下一个顶层键；列 0 注释行属于
            // 段内容，一并跳过，否则其后的旧段内容会残留为孤儿行）
            while let Some(&next) = lines.peek() {
                let next_is_comment = next.trim_start().starts_with('#');
                if !next.starts_with(' ') && !next.trim().is_empty() && !next_is_comment {
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
    // 防御：合并结果必须恰好含一个顶层 llm-pi-ai 段。段行形态识别若再有
    // 遗漏（上游新序列化形态），在这里拦截为 Err——update_text_file 的
    // 约定是 transform 出错即不写盘，宁可导入失败也不产出重复键文件。
    if out.lines().filter(|l| is_section_start_line(l)).count() != 1 {
        return Err(crate::locale::text(
            "合并后的 settings.yaml 校验失败（llm-pi-ai 段数量异常），已取消写入。",
            "Merged settings.yaml failed validation (unexpected llm-pi-ai section count); write cancelled.",
        )
        .into());
    }
    Ok(out)
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
    fn import_rejects_inline_secret_fields() {
        let text = "llm-pi-ai:
  providers:
    gw:
      apiKey: sk-12345
      models:
        - id: x
";
        let err = parse_providers(text).unwrap_err();
        assert!(err.contains("apiKey"), "unexpected: {err}");
        // apiKeyEnv 引用不受影响
        let ok = "llm-pi-ai:
  providers:
    gw:
      apiKeyEnv: K
      models:
        - id: x
";
        assert!(parse_providers(ok).is_ok());
    }

    #[test]
    fn export_strips_secret_lines() {
        let section = "llm-pi-ai:
  providers:
    acme-gw:
      apiKey: sk-1
      api_key: sk-2
      Bearer-Token: t
      displayName: Acme
      models:
        - id: x
";
        let out = parser::strip_secret_lines(section);
        assert!(!out.contains("sk-1") && !out.contains("sk-2") && !out.contains("Bearer"));
        assert!(out.contains("displayName: Acme") && out.contains("id: x"));
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
        let merged = upsert_section(old, new).unwrap();
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
        let merged = upsert_section(old, new).unwrap();
        assert!(merged.contains("locale:\n  preference: zh"));
        assert!(merged.contains("id: y"));
    }

    #[test]
    fn credential_upsert_merges_by_name() {
        // 凭据以 v1 布局（version + refs）写入，与 dsh 读取格式一致
        let text = "version: 1\nrefs:\n  DEEPSEEK_API_KEY: keep-me\n  CORP_GATEWAY_KEY: old-key\n";
        let out = upsert_credential(text, "CORP_GATEWAY_KEY", "new-key");
        assert!(out.starts_with("version: 1"));
        assert!(out.contains("  CORP_GATEWAY_KEY: 'new-key'"));
        assert!(out.contains("  DEEPSEEK_API_KEY: keep-me"));
        assert!(!out.contains("old-key"));
    }

    #[test]
    fn credential_upsert_appends_when_missing() {
        let text = "version: 1\nrefs:\n  DEEPSEEK_API_KEY: keep-me\n";
        let out = upsert_credential(text, "CORP_GATEWAY_KEY", "k");
        assert!(out.contains("  DEEPSEEK_API_KEY: keep-me"));
        assert!(out.contains("  CORP_GATEWAY_KEY: 'k'"));
        assert!(out.starts_with("version: 1"));
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

    #[test]
    fn extract_section_keeps_content_after_column_zero_comment() {
        // 段中间的列 0 注释是段内容，不是下一个顶层键
        let text = "llm-pi-ai:\n  providers:\n    gw:\n      models:\n        - id: x\n# 段中间的说明\n  extra: keep\nlocale:\n  preference: zh\n";
        let out = extract_section_text(text);
        assert!(out.contains("# 段中间的说明"));
        assert!(out.contains("extra: keep"));
        assert!(!out.contains("locale:"));
    }

    #[test]
    fn extract_section_keeps_leading_column_zero_comment() {
        // 段首的列 0 注释同样属于段内容，不会让段提前结束而丢成空段
        let text =
            "llm-pi-ai:\n# 模板说明\n  providers:\n    gw:\n      models:\n        - id: x\n";
        let out = extract_section_text(text);
        assert!(out.contains("# 模板说明"));
        assert!(out.contains("providers:"));
        assert!(out.contains("id: x"));
    }

    #[test]
    fn upsert_rejects_duplicate_top_level_sections() {
        // 原文件已含多个同名顶层段：行为明确为报错，绝不重复插入
        let old = "llm-pi-ai:\n  providers:\n    a:\n      models:\n        - id: x\nllm-pi-ai:\n  providers:\n    b:\n      models:\n        - id: y\n";
        let new = "llm-pi-ai:\n  providers:\n    c:\n      models:\n        - id: z\n";
        assert!(upsert_section(old, new).is_err());
    }

    #[test]
    fn upsert_rejects_duplicate_sections_across_forms() {
        // 精确段行与流式空段行属于同一顶层键：混用同样是重复，必须报错
        let old = "llm-pi-ai:\n  providers:\n    a:\n      models:\n        - id: x\nllm-pi-ai: {}\nlocale:\n  preference: zh\n";
        let new = "llm-pi-ai:\n  providers:\n    c:\n      models:\n        - id: z\n";
        assert!(upsert_section(old, new).is_err());
    }

    #[test]
    fn upsert_replaces_flow_style_empty_section() {
        // dsh 序列化/手写可能产出 `llm-pi-ai: {}`：必须识别为既有段并整体
        // 替换，而不是在末尾追加第二个段（重复顶层键会损坏文件）
        let old = "locale:\n  preference: zh\nllm-pi-ai: {}\nui-theme:\n  preference: dark\n";
        let new = "llm-pi-ai:\n  providers:\n    b:\n      models:\n        - id: y\n";
        let merged = upsert_section(old, new).unwrap();
        assert!(!merged.contains("{}"));
        assert!(merged.contains("id: y"));
        assert!(merged.contains("locale:\n  preference: zh"));
        assert!(merged.contains("ui-theme:\n  preference: dark"));
        assert_eq!(merged.matches("llm-pi-ai").count(), 1);
    }

    #[test]
    fn upsert_replaces_section_line_with_trailing_comment() {
        let old = "locale:\n  preference: zh\nllm-pi-ai: # 模型路由\n  providers:\n    a:\n      models:\n        - id: x\n";
        let new = "llm-pi-ai:\n  providers:\n    b:\n      models:\n        - id: y\n";
        let merged = upsert_section(old, new).unwrap();
        assert!(!merged.contains("id: x"));
        assert!(merged.contains("id: y"));
        assert_eq!(merged.matches("llm-pi-ai").count(), 1);
    }

    #[test]
    fn section_detection_accepts_variant_forms() {
        // settings_has_section 与段起始判定接受空值/注释/流式空表三种段行
        assert!(settings_has_section("llm-pi-ai:\n  providers:\n"));
        assert!(settings_has_section("llm-pi-ai: {}\n"));
        assert!(settings_has_section("llm-pi-ai: # note\n  providers:\n"));
        assert!(!settings_has_section("llm-pi-ai-extra:\n  providers:\n"));
        assert!(!settings_has_section("  llm-pi-ai:\n"));
    }

    #[test]
    fn extract_section_accepts_flow_style_section_line() {
        let out = extract_section_text("locale:\n  preference: zh\nllm-pi-ai: {}\n");
        assert_eq!(out, "llm-pi-ai: {}\n");
    }

    #[test]
    fn upsert_skips_column_zero_comments_inside_old_section() {
        // 旧段内的列 0 注释及其后的旧段内容都属于旧段，不得残留为孤儿行
        let old = "llm-pi-ai:\n  providers:\n    a:\n      models:\n        - id: x\n# 旧段内的说明\n  stale: drop\nlocale:\n  preference: zh\n";
        let new = "llm-pi-ai:\n  providers:\n    b:\n      models:\n        - id: y\n";
        let merged = upsert_section(old, new).unwrap();
        assert!(!merged.contains("# 旧段内的说明"));
        assert!(!merged.contains("stale: drop"));
        assert!(merged.contains("id: y"));
        assert!(merged.contains("locale:\n  preference: zh"));
        assert_eq!(merged.matches("llm-pi-ai:").count(), 1);
    }

    #[test]
    fn filter_handles_four_space_indentation() {
        let section = "llm-pi-ai:\n    providers:\n        openai:\n            apiKeyEnv: OPENAI_API_KEY\n        acme-gw:\n            models:\n                - id: x\n";
        let out = filter_builtin_routes(section);
        assert!(!out.contains("openai:"));
        assert!(out.contains("acme-gw:"));
        assert!(out.contains("id: x"));
        assert!(out.contains("    providers:"));
    }

    #[test]
    fn filter_ignores_indented_comments_as_route_keys() {
        // 缩进的注释行不是路由键；首个路由键之前的块级注释照常保留
        let section = "llm-pi-ai:\n  providers:\n    # 网关说明\n    acme-gw:\n      models:\n        - id: x\n";
        let out = filter_builtin_routes(section);
        assert!(out.contains("# 网关说明"));
        assert!(out.contains("acme-gw:"));
        assert!(out.contains("id: x"));
    }

    #[test]
    fn filter_matches_quoted_route_keys() {
        // 路由键匹配前剥离首尾引号与尾部冒号空白
        let section = "llm-pi-ai:\n  providers:\n    \"openai\":\n      apiKeyEnv: K\n    'acme-gw':\n      models:\n        - id: x\n";
        let out = filter_builtin_routes(section);
        assert!(!out.contains("openai"));
        assert!(out.contains("acme-gw"));
        assert!(out.contains("id: x"));
    }

    #[test]
    fn has_custom_providers_detects_providers_in_llm_pi_ai_section() {
        let root = std::env::temp_dir().join(format!(
            "dshbox-mc-hasprov-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let mut config = crate::app_state::Config::load();
        config.dsh_home = root.clone();
        // 标准块式：有 providers。
        std::fs::write(
            root.join("settings.yaml"),
            "locale:\n  preference: zh\nllm-pi-ai:\n  providers:\n    gw:\n      apiKeyEnv: K\n",
        )
        .unwrap();
        assert!(has_custom_providers(&config));
        // 无 llm-pi-ai 段：false。
        std::fs::write(root.join("settings.yaml"), "locale:\n  preference: zh\n").unwrap();
        assert!(!has_custom_providers(&config));
        // llm-pi-ai 段存在但无 providers：false。
        std::fs::write(
            root.join("settings.yaml"),
            "llm-pi-ai:\n  other: 1\nlocale:\n  preference: zh\n",
        )
        .unwrap();
        assert!(!has_custom_providers(&config));
        // 段行带注释：应仍进入段并找到 providers。
        std::fs::write(
            root.join("settings.yaml"),
            "llm-pi-ai: # 模型\n  providers:\n    gw:\n      apiKeyEnv: K\n",
        )
        .unwrap();
        assert!(has_custom_providers(&config));
        // llm-pi-ai 子串（llm-pi-ai-extra）不应误判为段。
        std::fs::write(
            root.join("settings.yaml"),
            "llm-pi-ai-extra:\n  providers:\n    gw:\n      apiKeyEnv: K\n",
        )
        .unwrap();
        assert!(!has_custom_providers(&config));
        let _ = std::fs::remove_dir_all(&root);
    }
}
