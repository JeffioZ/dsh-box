//! 对受管 YAML 字段做有解析位置依据的局部修改；输出再次校验后才交给文件层。

use serde::de::{DeserializeSeed, IgnoredAny, MapAccess, Visitor};
use serde_json::Value;
use serde_saphyr::Spanned;
use std::collections::BTreeMap;

type Fields = Spanned<BTreeMap<String, Spanned<Value>>>;

struct Section<'a>(&'a str);
impl<'de> DeserializeSeed<'de> for Section<'_> {
    type Value = Option<Fields>;
    fn deserialize<D: serde::Deserializer<'de>>(self, d: D) -> Result<Self::Value, D::Error> {
        d.deserialize_map(self)
    }
}
impl<'de> Visitor<'de> for Section<'_> {
    type Value = Option<Fields>;
    fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.write_str("a settings mapping")
    }
    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
        let mut result = None;
        while let Some(key) = map.next_key::<String>()? {
            if key == self.0 {
                result = Some(map.next_value()?);
            } else {
                let _: IgnoredAny = map.next_value()?;
            }
        }
        Ok(result)
    }
}

fn invalid() -> String {
    crate::locale::text(
        "无法安全修改该 YAML 结构，原文件未更改。",
        "This YAML structure cannot be edited safely; the original file was kept.",
    )
    .into()
}

pub(crate) fn parse(text: &str) -> Result<Value, String> {
    if text.trim().is_empty() {
        return Ok(serde_json::json!({}));
    }
    let value: Value = serde_saphyr::from_str(text).map_err(|_| invalid())?;
    if !value.is_object() {
        return Err(invalid());
    }
    Ok(value)
}

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

pub(crate) fn set(text: &str, section: &str, field: &str, value: Value) -> Result<String, String> {
    let mut expected = parse(text)?;
    let map = expected.as_object_mut().ok_or_else(invalid)?;
    let section_value = map.entry(section).or_insert_with(|| serde_json::json!({}));
    section_value
        .as_object_mut()
        .ok_or_else(invalid)?
        .insert(field.into(), value.clone());
    let existing = if text.trim().is_empty() {
        None
    } else {
        serde_saphyr::with_deserializer_from_str(text, |d| Section(section).deserialize(d))
            .map_err(|_| invalid())?
    };
    let mut next = text.to_string();
    let scalar = serde_json::to_string(&value).map_err(|_| invalid())?;
    match existing {
        None => {
            if !next.is_empty() && !next.ends_with('\n') {
                next.push('\n');
            }
            next.push_str(&format!("{section}:\n  {field}: {scalar}\n"));
        }
        Some(fields) => {
            let section_range = range(&fields, text)?;
            if let Some(old) = fields.value.get(field) {
                let value_range = range(old, text)?;
                next.replace_range(value_range, &scalar);
            } else if text[section_range.start..].starts_with('{') {
                let end = flow_end(text, section_range.start)?;
                let separator = if fields.value.is_empty() { "" } else { ", " };
                next.insert_str(
                    end,
                    &format!(
                        "{separator}{}: {scalar}",
                        serde_json::to_string(field).map_err(|_| invalid())?
                    ),
                );
            } else {
                let indent = fields.referenced.column().saturating_sub(1) as usize;
                let start = text[..section_range.start].rfind('\n').map_or(0, |i| i + 1);
                next.insert_str(start, &format!("{}{field}: {scalar}\n", " ".repeat(indent)));
            }
        }
    }
    if parse(&next)? != expected {
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

pub(crate) fn remove(text: &str, section: &str, field: &str) -> Result<String, String> {
    let mut expected = parse(text)?;
    let Some(map) = expected.get_mut(section).and_then(Value::as_object_mut) else {
        return Ok(text.into());
    };
    if map.remove(field).is_none() {
        return Ok(text.into());
    }
    let fields =
        serde_saphyr::with_deserializer_from_str(text, |d| Section(section).deserialize(d))
            .map_err(|_| invalid())?
            .ok_or_else(invalid)?;
    let old = fields.value.get(field).ok_or_else(invalid)?;
    let value_range = range(old, text)?;
    let section_range = range(&fields, text)?;
    let mut next = text.to_string();
    if text[section_range.start..].starts_with('{') {
        let end = flow_end(text, section_range.start)?;
        let colon = text[..value_range.start].rfind(':').ok_or_else(invalid)?;
        let mut start = text[..colon].rfind(field).ok_or_else(invalid)?;
        if matches!(
            text.as_bytes().get(start.wrapping_sub(1)),
            Some(b'\'' | b'"')
        ) {
            start -= 1;
        }
        if let Some(comma) = text[value_range.end..end].find(',') {
            next.replace_range(start..value_range.end + comma + 1, "");
        } else {
            if let Some(comma) = text[section_range.start..start].rfind(',') {
                start = section_range.start + comma;
            }
            next.replace_range(start..value_range.end, "");
        }
    } else {
        let start = text[..value_range.start].rfind('\n').map_or(0, |i| i + 1);
        let end = text[value_range.end..]
            .find('\n')
            .map_or(text.len(), |i| value_range.end + i + 1);
        if map.is_empty() {
            // 空块以 {} 保持映射类型；段内注释及其他顶层数据不动。
            next.replace_range(
                start..end,
                &format!(
                    "{}{{}}\n",
                    " ".repeat(fields.referenced.column().saturating_sub(1) as usize)
                ),
            );
        } else {
            next.replace_range(start..end, "");
        }
    }
    if parse(&next)? != expected {
        return Err(invalid());
    }
    Ok(next)
}

/// 分享副本使用 YAML 块结构和 JSON 标量，无需新增序列化依赖。
pub(crate) fn render(value: &Value) -> String {
    fn key(k: &str) -> String {
        if !k.is_empty()
            && k.bytes()
                .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_')
            && !matches!(k.to_ascii_lowercase().as_str(), "null" | "true" | "false")
            && !k.as_bytes()[0].is_ascii_digit()
        {
            k.into()
        } else {
            serde_json::to_string(k).unwrap()
        }
    }
    fn item(out: &mut String, prefix: &str, v: &Value, indent: usize) {
        out.push_str(&" ".repeat(indent));
        out.push_str(prefix);
        if (v.is_object() || v.is_array())
            && !v.as_object().is_some_and(|m| m.is_empty())
            && !v.as_array().is_some_and(|a| a.is_empty())
        {
            out.push('\n');
            emit(out, v, indent + 2);
        } else {
            out.push(' ');
            out.push_str(&v.to_string());
            out.push('\n');
        }
    }
    fn emit(out: &mut String, v: &Value, indent: usize) {
        match v {
            Value::Object(map) => {
                for (k, v) in map {
                    item(out, &format!("{}:", key(k)), v, indent);
                }
            }
            Value::Array(items) => {
                for v in items {
                    item(out, "-", v, indent);
                }
            }
            _ => {
                out.push_str(&v.to_string());
                out.push('\n');
            }
        }
    }
    let mut out = String::new();
    emit(&mut out, value, 0);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rendering_preserves_yaml_reserved_string_keys() {
        for key in [
            "Null", "NULL", "null", "True", "TRUE", "False", "FALSE", "01", "a:b",
        ] {
            let value = serde_json::json!({key: {"model": "x"}});
            assert_eq!(parse(&render(&value)).unwrap(), value);
        }
    }

    #[test]
    fn removes_only_requested_credential_in_block_and_flow_maps() {
        for source in [
            "refs: {KEY: old, OTHER: keep}\n",
            "refs: {OTHER: keep, KEY: old}\n",
            "refs: {KEY: old}\n",
            "refs:\n  KEY: old\n",
            "refs:\n  OTHER: keep\n  KEY: old # comment\n",
        ] {
            let out = remove(source, "refs", "KEY").unwrap();
            let mut expected = parse(source).unwrap();
            expected["refs"].as_object_mut().unwrap().remove("KEY");
            assert_eq!(parse(&out).unwrap(), expected);
        }
    }
    #[test]
    fn edits_only_target_with_comments_unicode_and_flow() {
        for source in [
            "locale: # 注释\n    preference: en # keep\n    extra: ok\nother: yes\n",
            "locale: {preference: en, extra: ok}\nother: yes\n",
            "locale:\n  extra: ok\nother: yes\n",
            "",
        ] {
            let result = set(source, "locale", "preference", Value::String("zh".into())).unwrap();
            assert_eq!(parse(&result).unwrap()["locale"]["preference"], "zh");
            if source.contains("other:") {
                assert!(result.contains("other: yes"));
            }
        }
    }
    #[test]
    fn rejects_invalid_without_returning_secret_snippets() {
        assert!(!set(
            "refs: {KEY: fake-secret, KEY: other}",
            "refs",
            "KEY",
            Value::String("new".into())
        )
        .unwrap_err()
        .contains("fake-secret"));
    }
}
