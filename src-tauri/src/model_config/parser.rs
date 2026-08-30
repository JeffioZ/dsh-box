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
    models: Vec<serde::de::IgnoredAny>,
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
    )
}

/// 扫描文本中的密钥样字段行，返回首个命中的字段名（导入拒绝用）。
pub(super) fn find_secret_field(text: &str) -> Option<String> {
    for line in text.lines() {
        let trimmed = line.trim_start();
        let Some((key, _)) = trimmed.split_once(':') else {
            continue;
        };
        // 跳过注释与路由键（缩进层级不区分：providers 内任意深度的
        // 密钥样键都算命中；顶层段键 llm-pi-ai 不在集合内）
        if trimmed.starts_with('#') {
            continue;
        }
        if secret_like_field(key.trim()) {
            return Some(key.trim().to_string());
        }
    }
    None
}

/// 剔除密钥样字段行（导出脱敏用：分享文本绝不携带明文凭据）。
pub(super) fn strip_secret_lines(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for line in text.lines() {
        let trimmed = line.trim_start();
        let is_secret = !trimmed.starts_with('#')
            && trimmed
                .split_once(':')
                .is_some_and(|(key, _)| secret_like_field(key.trim()));
        if !is_secret {
            out.push_str(line);
            out.push('\n');
        }
    }
    out
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
    let document: ImportedDocument = serde_saphyr::from_str(yaml).map_err(|error| {
        crate::locale::owned(
            format!("模型配置 YAML 无效：{error}"),
            format!("Invalid model configuration YAML: {error}"),
        )
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
