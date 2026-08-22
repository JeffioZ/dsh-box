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
