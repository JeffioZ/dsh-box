//! 模型配置 YAML 的类型化解析与输入边界。

use serde::Deserialize;
use std::collections::BTreeMap;

use super::extract_section_text;

/// 从文本中逐行识别到的 provider 信息（内部表示）。
#[derive(Debug)]
pub(super) struct ProviderInfo {
    pub(super) route: String,
    pub(super) display_name: String,
    pub(super) model_count: usize,
    pub(super) api_key_env: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ImportedDocument {
    #[serde(rename = "llm-pi-ai")]
    llm_pi_ai: ImportedSection,
}

#[derive(Deserialize)]
struct ImportedSection {
    providers: BTreeMap<String, ImportedProvider>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ImportedProvider {
    #[serde(default)]
    display_name: Option<String>,
    #[serde(default)]
    api_key_env: Option<String>,
    models: Vec<ImportedModel>,
}

#[derive(Deserialize)]
struct ImportedModel {
    id: String,
}

/// 密钥样字段名集合（小写、去 -/_ 归一）。凭据只允许经 `apiKeyEnv` 引用
/// 写入 `.credentials.yaml`——settings 与分享文本不应出现内联密钥，
/// 导入侧遇到即报错（此前会随原文直写 settings）。
fn secret_like_field(key: &str) -> bool {
    let normalized: String = key
        .to_ascii_lowercase()
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .collect();
    matches!(
        normalized.as_str(),
        "apikey"
            | "apisecret"
            | "key"
            | "token"
            | "accesstoken"
            | "refreshtoken"
            | "bearertoken"
            | "secret"
            | "clientsecret"
            | "password"
            | "passwd"
            | "authorization"
            | "proxyauthorization"
            | "xapikey"
            | "cookie"
            | "setcookie"
    )
}

/// 扫描文本中的密钥样字段行，返回首个命中的字段名（导入拒绝用）。
pub(super) fn find_secret_field(text: &str) -> Option<String> {
    fn visit(value: &serde_json::Value) -> Option<String> {
        match value {
            serde_json::Value::Object(map) => {
                for (key, child) in map {
                    if secret_like_field(key) {
                        return Some(key.clone());
                    }
                    if let Some(found) = visit(child) {
                        return Some(found);
                    }
                }
                None
            }
            serde_json::Value::Array(items) => items.iter().find_map(visit),
            _ => None,
        }
    }
    let value: serde_json::Value = serde_saphyr::from_str(text).ok()?;
    visit(&value)
}

/// 剔除密钥样字段行（导出脱敏用：分享文本绝不携带明文凭据）。
pub(super) fn strip_secret_lines(text: &str) -> Result<String, String> {
    fn scrub(value: &mut serde_json::Value) {
        match value {
            serde_json::Value::Object(map) => {
                map.retain(|key, _| !secret_like_field(key));
                for child in map.values_mut() {
                    scrub(child);
                }
            }
            serde_json::Value::Array(items) => items.iter_mut().for_each(scrub),
            _ => {}
        }
    }
    let mut value: serde_json::Value = serde_saphyr::from_str(text).map_err(|_| {
        crate::locale::text(
            "配置无法安全解析，未导出任何内容。",
            "The configuration could not be parsed safely; nothing was exported.",
        )
        .to_string()
    })?;
    scrub(&mut value);
    // 重新生成分享副本，避免块标量残留、锚点或注释携带被删除的秘密。
    Ok(crate::yaml_fields::render(&value))
}

/// 用完整 YAML 解析器校验语义，再投影成预览所需的最小类型。写盘仍保留用户
/// 原文，避免序列化过程改写注释、字段顺序或上游扩展字段。
pub(super) fn parse_providers(yaml: &str) -> Result<Vec<ProviderInfo>, String> {
    if yaml.trim().is_empty() {
        return Err(crate::locale::text("导入内容为空。", "The imported content is empty.").into());
    }
    const MAX_IMPORT_BYTES: usize = 1024 * 1024;
    if yaml.len() > MAX_IMPORT_BYTES {
        return Err(crate::locale::text(
            "模型配置超过 1 MB 上限。",
            "The model configuration exceeds the 1 MB limit.",
        )
        .into());
    }
    if let Some(field) = find_secret_field(yaml) {
        return Err(crate::locale::owned(
            format!(
                "配置包含内联密钥字段 {field}：请删除该字段，改用 apiKeyEnv 引用并在导入时单独填写密钥（settings 与分享文本不保存明文密钥）。"
            ),
            format!(
                "The configuration contains an inline secret field {field}: remove it and use an apiKeyEnv reference instead, providing the key separately during import (settings and shared text never store plaintext secrets)."
            ),
        ));
    }
    let document: ImportedDocument = serde_saphyr::from_str(yaml).map_err(|_| {
        crate::locale::text(
            "模型配置 YAML 无效，请检查结构、字段类型和模型 id。",
            "Invalid model configuration YAML; check its structure, field types and model IDs.",
        )
        .to_string()
    })?;
    if extract_section_text(yaml).trim().is_empty() {
        return Err(crate::locale::text(
            "请粘贴以 llm-pi-ai: 开头的完整顶层配置段。",
            "Paste the complete top-level section beginning with llm-pi-ai:.",
        )
        .into());
    }
    if document.llm_pi_ai.providers.is_empty() {
        return Err(crate::locale::text(
            "模型配置中没有识别到任何提供方路由（providers）。",
            "No provider routes were found in the model configuration.",
        )
        .into());
    }
    let mut providers = Vec::with_capacity(document.llm_pi_ai.providers.len());
    for (route, provider) in document.llm_pi_ai.providers {
        if route.is_empty()
            || route
                .chars()
                .any(|char| char.is_whitespace() || char.is_control())
        {
            return Err(crate::locale::text(
                "模型配置 provider 路由名不合法。",
                "The model configuration contains an invalid provider route name.",
            )
            .into());
        }
        if provider.models.is_empty() {
            return Err(crate::locale::text(
                "提供方 {route} 没有声明任何模型（models）。",
                "Provider {route} declares no models.",
            )
            .replace("{route}", &route));
        }
        let mut model_ids = std::collections::HashSet::new();
        if provider.models.iter().any(|model| {
            model.id.trim().is_empty()
                || model.id.chars().any(char::is_control)
                || !model_ids.insert(model.id.as_str())
        }) {
            return Err(crate::locale::text(
                "模型 id 不能为空、重复或包含控制字符。",
                "Model IDs must be nonempty, unique and free of control characters.",
            )
            .into());
        }
        let api_key_env = provider
            .api_key_env
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty());
        if api_key_env
            .as_deref()
            .is_some_and(|name| !valid_env_name(name))
        {
            return Err(crate::locale::text(
                "模型配置中的 apiKeyEnv 不是合法的环境变量名。",
                "apiKeyEnv must be a valid environment variable name.",
            )
            .into());
        }
        let display_name = provider
            .display_name
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| route.clone());
        providers.push(ProviderInfo {
            route,
            display_name,
            model_count: provider.models.len(),
            api_key_env,
        });
    }
    Ok(providers)
}

/// 把导入文本规范化为可写入的顶层段：去掉前导空行，确保末尾换行。
pub(super) fn normalize_section(yaml: &str) -> Result<String, String> {
    // 先经 parse_providers 校验（保证结构合法）。
    parse_providers(yaml)?;
    Ok(extract_section_text(yaml))
}

pub(super) fn valid_env_name(name: &str) -> bool {
    let mut chars = name.chars();
    matches!(chars.next(), Some('_' | 'A'..='Z' | 'a'..='z'))
        && chars.all(|c| matches!(c, '_' | 'A'..='Z' | 'a'..='z' | '0'..='9'))
}
