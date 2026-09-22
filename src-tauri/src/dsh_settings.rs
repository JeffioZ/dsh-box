//! dsh 设置存储的定位与读写。
//!
//! dsh ≥0.1.7 废除 `$DSH_HOME/settings.yaml`：首启把它改名为
//! `settings.yaml.imported` 并把各段导入 Cordis profile patch 文档
//! （上游 packages/settings/settings 的 `importLegacyDocument`）。`dsh web`
//! 运行在 `web` profile 下（`dsh web` ≡ `dsh --profile web`），设置表单写入
//! profile 级 `$DSH_HOME/profiles/web/cordis.patch.yml`，组合时再叠加 home
//! 级 `$DSH_HOME/cordis.patch.yml`（后者优先级更高）。patch 文档是顶层
//! YAML 数组，每行元素 `- id: <entry>` 携带 `config:` 覆盖子树；dsh 的
//! HMR 对两级 patch 文档有文件监视，外部写入会触发热重组——与旧版
//! settings.yaml 监视器等价。
//!
//! 本模块按已安装 dsh 的版本选择存储位置（旧版仍读写 settings.yaml，
//! 向后累积兼容）；写入只落 profile 级（与 dsh 设置表单同层，不钉死用户
//! 后续在 dsh 界面里的修改）。DSHBox 在 dsh 首启前预写 profile patch 是
//! 安全的：上游 `initProfile` 幂等，`cordis.patch.yml` 已存在时不覆盖。

use crate::app_state::Config;
use serde::de::{DeserializeSeed, IgnoredAny, MapAccess, SeqAccess, Visitor};
use serde_json::Value;
use serde_saphyr::Spanned;
use std::collections::BTreeMap;
use std::path::PathBuf;

/// profile patch 文档所在的 profile 目录（`dsh web` 固定运行于 web profile）。
const WEB_PROFILE_DIR: &str = "profiles/web";
/// patch 文档文件名（上游 `PROFILE_PATCH_FILENAME`）。
const PATCH_FILENAME: &str = "cordis.patch.yml";

/// 已安装的 dsh 是否以 profile patch 文档存设置（≥0.1.7）。
///
/// 以本地 dsh 包自述版本为准：读不到包（未安装本地 dsh，如外部服务
/// 模式）时按旧版处理，与既有「外部模式不写本地配置冒充」的口径一致。
/// 不缓存结果，跟随循环按轮重新判定，dsh 升级后无需重启外壳。
/// 比较含预发布（`0.1.7-alpha.1` 即算新版）：用户 alpha/next 渠道先于
/// 正式版拿到新存储，按旧版写入会落到已废除的 settings.yaml。
pub(crate) fn uses_patch_settings(config: &Config) -> bool {
    let pkg = config
        .dsh_dir()
        .join("node_modules/@deepseek-ai/dsh/package.json");
    let version = std::fs::read_to_string(pkg)
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .and_then(|json| {
            json.get("version")
                .and_then(|v| v.as_str())
                .map(str::to_string)
        })
        .and_then(|v| semver::Version::parse(&v).ok());
    let requirement = semver::VersionReq::parse(">=0.1.7-0").expect("static version requirement");
    version.is_some_and(|v| requirement.matches(&v))
}

/// profile 级 patch 文档路径（dsh 设置表单的写入位置）。
pub(crate) fn profile_patch_path(config: &Config) -> PathBuf {
    config.dsh_home().join(WEB_PROFILE_DIR).join(PATCH_FILENAME)
}

/// 把 `entry.config.field = value` 写入 profile 级 patch 文档（目录不存在
/// 则创建；与 dsh 设置表单同层写入，不覆盖用户后续在 dsh 界面的修改）。
/// dsh 的 HMR 监视两级 patch 文档，写入即热重组生效。
pub(crate) fn save_profile_patch_field(
    config: &Config,
    entry: &str,
    field: &str,
    value: &Value,
) -> Result<(), String> {
    let path = profile_patch_path(config);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    crate::app_state::update_text_file(&path, |text| {
        patch_set_entry_field(&text, entry, field, value)
    })
}

/// home 级 patch 文档路径（组合优先级高于 profile 级）。
pub(crate) fn home_patch_path(config: &Config) -> PathBuf {
    config.dsh_home().join(PATCH_FILENAME)
}

/// 跟随循环要监视的设置文件集合（按 dsh 版本选择；任一变化即触发跟随）。
pub(crate) fn watch_paths(config: &Config) -> Vec<PathBuf> {
    if uses_patch_settings(config) {
        vec![profile_patch_path(config), home_patch_path(config)]
    } else {
        vec![config.dsh_home().join("settings.yaml")]
    }
}

/// 组合读取某 entry 的 `config` 子树：home 级覆盖 profile 级（与上游
/// `readProfilePatches` 的叠加顺序一致；patch 行整体替换目标行的 config，
/// 因此两级都存在时取 home 级即可）。两级都没有该 entry 时返回 None。
pub(crate) fn entry_config(config: &Config, entry: &str) -> Option<Value> {
    for path in [home_patch_path(config), profile_patch_path(config)] {
        let Ok(text) = std::fs::read_to_string(path) else {
            continue;
        };
        if let Some(found) = patch_entry_config(&text, entry) {
            return Some(found);
        }
    }
    None
}

/// 从 patch 文档文本提取 `- id: <entry>` 元素的 `config` 子树。
/// 顶层不是数组、entry 缺失或 config 不是映射时返回 None。
pub(crate) fn patch_entry_config(text: &str, entry: &str) -> Option<Value> {
    if text.trim().is_empty() {
        return None;
    }
    let value: Value = serde_saphyr::from_str(text).ok()?;
    value
        .as_array()?
        .iter()
        .find(|element| element.get("id").and_then(|id| id.as_str()) == Some(entry))?
        .get("config")
        .filter(|config| config.is_object())
        .cloned()
}

/// 把 `entry.config.field = value` 合并进 patch 文档文本，其余内容（注释、
/// 键序、其他 entry）保持原样。entry / config / field 缺失时按标准缩进
/// 补写；输出整体重新解析并比对期望值后才返回（与 yaml_fields 同一
/// 自校验口径，解析异常时返回错误、不落盘）。
pub(crate) fn patch_set_entry_field(
    text: &str,
    entry: &str,
    field: &str,
    value: &Value,
) -> Result<String, String> {
    if text.trim().is_empty() {
        return append_entry("", entry, field, value);
    }
    // 先整体解析：坏文档不做局部修改（避免半写破坏 dsh 的 profile 组合）。
    let parsed: Value = serde_saphyr::from_str(text).map_err(|_| invalid())?;
    if !parsed.is_array() {
        return Err(invalid());
    }
    let scalar = serde_json::to_string(value).map_err(|_| invalid())?;
    let found =
        serde_saphyr::with_deserializer_from_str(text, |d| TargetEntry(entry).deserialize(d))
            .map_err(|_| invalid())?;

    let mut next = text.to_string();
    match found {
        Some(element) => {
            let config = element
                .value
                .config
                .as_ref()
                .filter(|fields| fields.value.contains_key(field));
            match config {
                Some(fields) => {
                    let range = field_range(fields, field, text)?;
                    next.replace_range(range, &scalar);
                }
                None if element.value.config.is_some() => {
                    insert_field(
                        &mut next,
                        element.value.config.as_ref().expect("checked above"),
                        text,
                        field,
                        &scalar,
                    )?;
                }
                None => insert_config(&mut next, &element, text, field, &scalar)?,
            }
        }
        // 顶层是 flow `[]`（初始模板）：把 `[]` 行替换为块式首元素；
        // 块式数组则整体追加到文件尾（顶层序列追加元素始终合法）。
        None if parsed.as_array().is_some_and(|rows| rows.is_empty()) => {
            let flow_line = text
                .lines()
                .position(|line| line.trim() == "[]")
                .ok_or_else(invalid)?;
            let mut replaced = String::with_capacity(text.len() + 64);
            for (index, line) in text.lines().enumerate() {
                if index == flow_line {
                    replaced.push_str(&entry_block(entry, field, &scalar));
                } else {
                    replaced.push_str(line);
                }
                replaced.push('\n');
            }
            return validate(replaced, entry, field, value, text);
        }
        None => return append_entry(text, entry, field, value),
    }
    validate(next, entry, field, value, text)
}

fn invalid() -> String {
    crate::locale::text(
        "无法安全修改该配置文件结构，原文件未更改。",
        "This configuration structure cannot be edited safely; the original file was kept.",
    )
    .into()
}

type Fields = Spanned<BTreeMap<String, Spanned<Value>>>;

/// 一个 patch 元素（映射）里本模块关心的字段。
struct EntryFields {
    id: Option<String>,
    config: Option<Fields>,
}

impl<'de> serde::Deserialize<'de> for EntryFields {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        d.deserialize_map(EntryFieldsVisitor)
    }
}

struct EntryFieldsVisitor;

impl<'de> Visitor<'de> for EntryFieldsVisitor {
    type Value = EntryFields;
    fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.write_str("a patch entry mapping")
    }
    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
        let mut id = None;
        let mut config = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "id" => id = map.next_value()?,
                "config" => config = map.next_value()?,
                _ => {
                    let _: IgnoredAny = map.next_value()?;
                }
            }
        }
        Ok(EntryFields { id, config })
    }
}

/// 在顶层 patch 数组中定位 `id == 目标` 的元素（携带元素 span 与 config）。
struct TargetEntry<'a>(&'a str);

impl<'de> DeserializeSeed<'de> for TargetEntry<'_> {
    type Value = Option<Spanned<EntryFields>>;
    fn deserialize<D: serde::Deserializer<'de>>(self, d: D) -> Result<Self::Value, D::Error> {
        d.deserialize_seq(self)
    }
}

impl<'de> Visitor<'de> for TargetEntry<'_> {
    type Value = Option<Spanned<EntryFields>>;
    fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.write_str("a patch entry array")
    }
    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
        while let Some(element) = seq.next_element::<Spanned<EntryFields>>()? {
            if element.value.id.as_deref() == Some(self.0) {
                return Ok(Some(element));
            }
        }
        Ok(None)
    }
}

/// 节点 span（字节区间）；别名/锚点节点不参与局部修改。
fn range<T>(node: &Spanned<T>, text: &str) -> Result<std::ops::Range<usize>, String> {
    if node.referenced != node.defined {
        return Err(invalid());
    }
    let span = node.referenced.span();
    let start = usize::try_from(span.byte_offset().ok_or_else(invalid)?).map_err(|_| invalid())?;
    let len = usize::try_from(span.byte_len().ok_or_else(invalid)?).map_err(|_| invalid())?;
    let end = start.checked_add(len).ok_or_else(invalid)?;
    if text.get(start..end).is_none() {
        return Err(invalid());
    }
    Ok(start..end)
}

fn field_range(fields: &Fields, field: &str, text: &str) -> Result<std::ops::Range<usize>, String> {
    let node = fields.value.get(field).ok_or_else(invalid)?;
    range(node, text)
}

/// config 值映射内插入缺失字段：块式映射插到首行行首（与 config 既有
/// 子键同缩进）；flow 映射在收口 `}` 前追加（yaml_fields::set 同款）。
fn insert_field(
    next: &mut String,
    fields: &Fields,
    text: &str,
    field: &str,
    scalar: &str,
) -> Result<(), String> {
    let config_range = range(fields, text)?;
    if text[config_range.start..].starts_with('{') {
        let end = flow_end(text, config_range.start)?;
        let separator = if fields.value.is_empty() { "" } else { ", " };
        next.insert_str(end, &format!("{separator}{field}: {scalar}"));
    } else {
        let indent = fields.referenced.column().saturating_sub(1) as usize;
        let start = text[..config_range.start].rfind('\n').map_or(0, |i| i + 1);
        next.insert_str(start, &format!("{}{field}: {scalar}\n", " ".repeat(indent)));
    }
    Ok(())
}

/// entry 元素内补写缺失的 `config:` 映射（插在元素首行之后，元素其他
/// 子键之前——映射键序无关，解析等价即可）。
fn insert_config(
    next: &mut String,
    element: &Spanned<EntryFields>,
    text: &str,
    field: &str,
    scalar: &str,
) -> Result<(), String> {
    let element_range = range(element, text)?;
    let line_end = next[element_range.start..]
        .find('\n')
        .map_or(next.len(), |offset| element_range.start + offset + 1);
    next.insert_str(line_end, &format!("  config:\n    {field}: {scalar}\n"));
    Ok(())
}

/// 文件尾追加新 entry 块；空文本直接生成块式数组。
fn append_entry(text: &str, entry: &str, field: &str, value: &Value) -> Result<String, String> {
    let scalar = serde_json::to_string(value).map_err(|_| invalid())?;
    let mut next = text.to_string();
    if !next.is_empty() && !next.ends_with('\n') {
        next.push('\n');
    }
    next.push_str(&entry_block(entry, field, &scalar));
    validate(next, entry, field, value, text)
}

fn entry_block(entry: &str, field: &str, scalar: &str) -> String {
    format!("- id: {entry}\n  config:\n    {field}: {scalar}\n")
}

/// 输出自校验：整体重新解析后，除目标字段按期望变化外与原文等价。
/// 期望值按写入路径同步构造：entry/config/field 缺失时对应补写完整块。
fn validate(
    next: String,
    entry: &str,
    field: &str,
    value: &Value,
    original: &str,
) -> Result<String, String> {
    let mut expected: Value = if original.trim().is_empty() {
        serde_json::json!([])
    } else {
        serde_saphyr::from_str(original).map_err(|_| invalid())?
    };
    let rows = expected.as_array_mut().ok_or_else(invalid)?;
    match rows
        .iter_mut()
        .find(|element| element.get("id").and_then(|id| id.as_str()) == Some(entry))
    {
        Some(element) => {
            let object = element.as_object_mut().ok_or_else(invalid)?;
            match object
                .get_mut("config")
                .and_then(|config| config.as_object_mut())
            {
                Some(config) => {
                    config.insert(field.to_string(), value.clone());
                }
                None => {
                    object.insert("config".to_string(), serde_json::json!({ field: value }));
                }
            }
        }
        None => {
            rows.push(serde_json::json!({ "id": entry, "config": { field: value } }));
        }
    }
    let parsed: Value = serde_saphyr::from_str(&next).map_err(|_| invalid())?;
    if parsed != expected {
        return Err(invalid());
    }
    Ok(next)
}

fn flow_end(text: &str, start: usize) -> Result<usize, String> {
    let mut depth = 0;
    let mut quote = None;
    let mut escaped = false;
    let mut comment = false;
    let mut chars = text[start..].char_indices().peekable();
    while let Some((offset, ch)) = chars.next() {
        if comment {
            if ch == '\n' {
                comment = false;
            }
            continue;
        }
        if let Some(q) = quote {
            if escaped {
                escaped = false;
                continue;
            }
            if q == '"' && ch == '\\' {
                escaped = true;
                continue;
            }
            if ch == q {
                if q == '\'' && chars.peek().is_some_and(|(_, c)| *c == '\'') {
                    chars.next();
                } else {
                    quote = None;
                }
            }
            continue;
        }
        match ch {
            '"' | '\'' => quote = Some(ch),
            '#' => comment = true,
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return Ok(start + offset);
                }
            }
            _ => {}
        }
    }
    Err(invalid())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn save_profile_patch_field_writes_isolated_home() {
        let root = std::env::temp_dir().join(format!(
            "dshbox-dsh-settings-save-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let mut config = crate::app_state::Config::load();
        config.root = root.join("box");
        config.dsh_home = root.join("home");
        std::fs::create_dir_all(&config.root).unwrap();
        std::fs::create_dir_all(&config.dsh_home).unwrap();
        // 伪造已安装 dsh ≥0.1.7（dsh_dir 基于 root）。
        let pkg = config
            .dsh_dir()
            .join("node_modules/@deepseek-ai/dsh/package.json");
        std::fs::create_dir_all(pkg.parent().unwrap()).unwrap();
        std::fs::write(
            &pkg,
            serde_json::json!({ "version": "0.1.7-alpha.1" }).to_string(),
        )
        .unwrap();
        assert!(uses_patch_settings(&config));
        // 首次写入：目录与文档一并创建。
        save_profile_patch_field(&config, "ui-theme", "preference", &json!("dark")).unwrap();
        let path = profile_patch_path(&config);
        let text = std::fs::read_to_string(&path).unwrap();
        assert_eq!(
            patch_entry_config(&text, "ui-theme").unwrap()["preference"],
            json!("dark")
        );
        // 第二个 entry 追加，首个保持不动。
        save_profile_patch_field(&config, "locale", "preference", &json!("zh")).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert_eq!(
            patch_entry_config(&text, "ui-theme").unwrap()["preference"],
            json!("dark")
        );
        assert_eq!(
            patch_entry_config(&text, "locale").unwrap()["preference"],
            json!("zh")
        );
        // 组合读取同源（无 home 级覆盖时取 profile 级）。
        assert_eq!(
            entry_config(&config, "ui-theme").unwrap()["preference"],
            json!("dark")
        );
        // home 级存在时覆盖 profile 级。
        std::fs::write(
            home_patch_path(&config),
            "- id: ui-theme\n  config:\n    preference: system\n",
        )
        .unwrap();
        assert_eq!(
            entry_config(&config, "ui-theme").unwrap()["preference"],
            json!("system")
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn reads_entry_config_from_patch_document() {
        let text = "\
# 模板注释
[]
";
        assert!(patch_entry_config(text, "ui-theme").is_none());
        let text = "\
- id: llm-pi-ai
  config:
    providers:
      corp:
        displayName: Corp
- id: ui-theme
  config:
    preference: dark
    fontSize: 15
- id: locale
  config:
    preference: zh
";
        assert_eq!(
            patch_entry_config(text, "ui-theme").and_then(|c| c.get("preference").cloned()),
            Some(json!("dark"))
        );
        assert!(patch_entry_config(text, "missing").is_none());
        // config 不是映射（标量/数组）不参与
        let bad = "- id: ui-theme\n  config: 3\n";
        assert!(patch_entry_config(bad, "ui-theme").is_none());
    }

    #[test]
    fn replaces_existing_field_preserving_neighbors() {
        let text = "\
# 顶部注释
- id: ui-theme
  config:
    preference: light # 行内注释
    fontSize: 15
- id: other
  config:
    keep: yes
";
        let next = patch_set_entry_field(text, "ui-theme", "preference", &json!("dark")).unwrap();
        assert!(next.contains("# 顶部注释"));
        assert!(next.contains("preference: \"dark\" # 行内注释"));
        assert!(next.contains("fontSize: 15"));
        assert!(next.contains("keep: yes"));
        assert_eq!(
            patch_entry_config(&next, "ui-theme").unwrap()["preference"],
            json!("dark")
        );
    }

    #[test]
    fn inserts_missing_field_into_existing_config() {
        let text = "\
- id: ui-theme
  config:
    fontSize: 15
";
        let next = patch_set_entry_field(text, "ui-theme", "preference", &json!("system")).unwrap();
        assert_eq!(
            patch_entry_config(&next, "ui-theme").unwrap()["preference"],
            json!("system")
        );
        assert_eq!(
            patch_entry_config(&next, "ui-theme").unwrap()["fontSize"],
            json!(15)
        );
    }

    #[test]
    fn appends_config_and_entry_blocks() {
        // entry 存在但无 config
        let text = "- id: ui-theme\n";
        let next = patch_set_entry_field(text, "ui-theme", "preference", &json!("dark")).unwrap();
        assert_eq!(
            patch_entry_config(&next, "ui-theme").unwrap()["preference"],
            json!("dark")
        );
        // entry 不存在 → 追加
        let text = "- id: other\n  config:\n    a: 1\n";
        let next = patch_set_entry_field(text, "ui-theme", "preference", &json!("dark")).unwrap();
        assert_eq!(
            patch_entry_config(&next, "ui-theme").unwrap()["preference"],
            json!("dark")
        );
        assert_eq!(patch_entry_config(&next, "other").unwrap()["a"], json!(1));
        // 空文档与初始模板 `[]` → 生成块式数组
        let next = patch_set_entry_field("", "locale", "preference", &json!("en")).unwrap();
        assert_eq!(
            patch_entry_config(&next, "locale").unwrap()["preference"],
            json!("en")
        );
        let template = "# Your patch layer\n[]\n";
        let next = patch_set_entry_field(template, "locale", "preference", &json!("zh")).unwrap();
        assert!(next.contains("# Your patch layer"));
        assert_eq!(
            patch_entry_config(&next, "locale").unwrap()["preference"],
            json!("zh")
        );
    }

    #[test]
    fn rejects_structures_it_cannot_edit_safely() {
        // 顶层非数组
        assert!(
            patch_set_entry_field("locale: zh\n", "locale", "preference", &json!("zh")).is_err()
        );
        // YAML 语法错误
        assert!(patch_set_entry_field("- id: [broken\n", "x", "y", &json!(1)).is_err());
    }
}
