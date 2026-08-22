//! dsh `.credentials.yaml` 的最小行级读写工具。
//!
//! 凭据文件不经过通用 YAML 序列化，避免重排或改写用户的其他条目；所有写入仍
//! 由 `app_state::update_text_file` 串行并原子替换。

use crate::app_state::Config;

pub(crate) fn value(config: &Config, name: &str) -> Option<String> {
    let text = std::fs::read_to_string(config.dsh_home().join(".credentials.yaml")).ok()?;
    text.lines().find_map(|line| {
        line.split_once(':').and_then(|(candidate, value)| {
            if candidate.trim().eq_ignore_ascii_case(name) {
                decode_scalar(value)
            } else {
                None
            }
        })
    })
}

pub(crate) fn has(config: &Config, name: &str) -> bool {
    value(config, name).is_some()
}

pub(crate) fn upsert(text: &str, name: &str, value: &str) -> String {
    let encoded = encode_scalar(value);
    let mut out = String::new();
    let mut wrote = false;
    for line in text.lines() {
        let is_target = line
            .split_once(':')
            .is_some_and(|(candidate, _)| candidate.trim().eq_ignore_ascii_case(name));
        if is_target {
            if !wrote {
                out.push_str(&format!("{name}: {encoded}\n"));
                wrote = true;
            }
            continue;
        }
        out.push_str(line);
        out.push('\n');
    }
    if !wrote {
        out.push_str(&format!("{name}: {encoded}\n"));
    }
    out
}

pub(crate) fn remove(text: &str, name: &str) -> String {
    let mut out = String::new();
    for line in text.lines() {
        let is_target = line
            .split_once(':')
            .is_some_and(|(candidate, _)| candidate.trim().eq_ignore_ascii_case(name));
        if !is_target {
            out.push_str(line);
            out.push('\n');
        }
    }
    out
}

/// 单引号 YAML 标量不会把 `#`、`: `、前后空格等凭据内容误解为语法；
/// YAML 以两个连续单引号表示值内的单引号。
fn encode_scalar(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

fn decode_scalar(value: &str) -> Option<String> {
    let value = value.trim();
    if value.len() >= 2 && value.starts_with('\'') && value.ends_with('\'') {
        let decoded = value[1..value.len() - 1].replace("''", "'");
        return (!decoded.is_empty()).then_some(decoded);
    }
    if value.len() >= 2 && value.starts_with('"') && value.ends_with('"') {
        if let Ok(decoded) = serde_json::from_str::<String>(value) {
            return (!decoded.is_empty()).then_some(decoded);
        }
    }
    (!value.is_empty()).then(|| value.to_string())
}

pub(crate) fn save(config: &Config, name: &str, value: &str) -> Result<(), String> {
    let path = config.dsh_home().join(".credentials.yaml");
    crate::app_state::update_text_file(&path, |text| Ok(upsert(&text, name, value)))
}

pub(crate) fn remove_saved(config: &Config, name: &str) -> Result<(), String> {
    let path = config.dsh_home().join(".credentials.yaml");
    crate::app_state::update_text_file(&path, |text| Ok(remove(&text, name)))
}

#[cfg(test)]
mod tests {
    use super::{decode_scalar, remove, upsert};

    #[test]
    fn upsert_replaces_duplicates_without_touching_neighbors() {
        let text = "DEEPSEEK_API_KEY: keep\ncorp_key: old\nCORP_KEY: duplicate\n";
        let out = upsert(text, "CORP_KEY", "new: value # exact");
        assert_eq!(out.matches("CORP_KEY:").count(), 1);
        assert!(!out.contains("corp_key:"));
        assert!(out.contains("CORP_KEY: 'new: value # exact'"));
        assert!(out.contains("DEEPSEEK_API_KEY: keep"));
    }

    #[test]
    fn yaml_scalar_round_trip_preserves_special_characters() {
        let out = upsert("", "KEY", " leading: value # 'quoted' ");
        let raw = out.split_once(':').unwrap().1.trim();
        assert_eq!(
            decode_scalar(raw).as_deref(),
            Some(" leading: value # 'quoted' ")
        );
        assert_eq!(
            decode_scalar("\"escaped\\nvalue\"").as_deref(),
            Some("escaped\nvalue")
        );
    }

    #[test]
    fn remove_drops_all_matching_entries_only() {
        let text = "KEEP: one\nDeepSeek_API_Key: old\nDEEPSEEK_API_KEY: duplicate\nTAIL: two\n";
        assert_eq!(remove(text, "DEEPSEEK_API_KEY"), "KEEP: one\nTAIL: two\n");
    }
}
