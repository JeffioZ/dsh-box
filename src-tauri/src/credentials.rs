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

/// 统一 API Key 解析链：DSH_BOX_API_KEY → DEEPSEEK_API_KEY → 路由声明的
/// apiKeyEnv → `$DSH_HOME/.credentials.yaml`（查路由声明键，缺省查
/// DEEPSEEK_API_KEY）。环境变量值全部 trim 后判空；壳级覆盖优先于路由
/// 声明，状态栏余额与用量页账户监测共用同一口径。
pub(crate) fn resolve_api_key(config: &Config, route_env: Option<&str>) -> Option<String> {
    let route_env = route_env.map(str::trim).filter(|name| !name.is_empty());
    for name in ["DSH_BOX_API_KEY", "DEEPSEEK_API_KEY"]
        .into_iter()
        .chain(route_env)
    {
        if let Ok(value) = std::env::var(name) {
            let value = value.trim();
            if !value.is_empty() {
                return Some(value.to_string());
            }
        }
    }
    value(config, route_env.unwrap_or("DEEPSEEK_API_KEY"))
}

pub(crate) fn upsert(text: &str, name: &str, value: &str) -> String {
    let encoded = encode_scalar(value);
    let mut out = String::new();
    let mut wrote = false;
    for line in text.lines() {
        let is_target = line
            .split_once(':')
            .is_some_and(|(candidate, _)| key_matches(candidate, name));
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
            .is_some_and(|(candidate, _)| key_matches(candidate, name));
        if !is_target {
            out.push_str(line);
            out.push('\n');
        }
    }
    out
}

/// 键名匹配：忽略大小写；容忍 UTF-8 BOM（trim 不去 \u{feff}，文件首键
/// 带 BOM 时失配会导致同名键被重复追加）。
fn key_matches(candidate: &str, name: &str) -> bool {
    candidate
        .trim_start_matches('\u{feff}')
        .trim()
        .eq_ignore_ascii_case(name)
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
        // 非 JSON 兼容的转义（如 \q）：按 YAML 双引号标量语义去掉外层引号
        // 返回原文，避免把引号本身带进凭据或误判凭据不存在。
        let raw = &value[1..value.len() - 1];
        return (!raw.is_empty()).then(|| raw.to_string());
    }
    // YAML plain scalar：未加引号的 ` #` 起为行内注释，须截断；
    // `abc#def` 的 # 前无空白，不是注释，不截断。
    let value = value.split(" #").next().unwrap_or_default().trim_end();
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
pub(crate) static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(test)]
mod tests {
    use super::{decode_scalar, remove, upsert};

    /// 保存/恢复环境变量，返回恢复闭包（env 为进程全局，测试须串行并还原）。
    fn set_env(name: &str, value: Option<&str>) -> impl FnOnce() {
        let prev = std::env::var(name).ok();
        let name = name.to_string();
        match value {
            Some(v) => std::env::set_var(&name, v),
            None => std::env::remove_var(&name),
        }
        move || match prev {
            Some(v) => std::env::set_var(&name, v),
            None => std::env::remove_var(&name),
        }
    }

    fn temp_config(tag: &str, credentials: &str) -> (crate::app_state::Config, std::path::PathBuf) {
        let root = std::env::temp_dir().join(format!(
            "dshbox-cred-resolve-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join(".credentials.yaml"), credentials).unwrap();
        let mut config = crate::app_state::Config::load();
        config.dsh_home = root.clone();
        (config, root)
    }

    #[test]
    fn resolve_api_key_prefers_shell_override_then_route_env_then_file() {
        let _guard = super::ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        const ROUTE: &str = "DSHBOX_TEST_ROUTE_KEY_7Q2Z";
        let (config, root) = temp_config(
            "chain",
            "DEEPSEEK_API_KEY: file-deep\nDSHBOX_TEST_ROUTE_KEY_7Q2Z: file-route\n",
        );
        // 逐级撤掉更高优先级，验证顺序 DSH_BOX → DEEPSEEK → 路由 env → 凭据文件。
        let r1 = set_env("DSH_BOX_API_KEY", Some("box"));
        let r2 = set_env("DEEPSEEK_API_KEY", Some("deep"));
        let r3 = set_env(ROUTE, Some("route"));
        assert_eq!(
            super::resolve_api_key(&config, Some(ROUTE)).as_deref(),
            Some("box")
        );
        r1();
        assert_eq!(
            super::resolve_api_key(&config, Some(ROUTE)).as_deref(),
            Some("deep")
        );
        r2();
        assert_eq!(
            super::resolve_api_key(&config, Some(ROUTE)).as_deref(),
            Some("route")
        );
        r3();
        assert_eq!(
            super::resolve_api_key(&config, Some(ROUTE)).as_deref(),
            Some("file-route")
        );
        // 无路由声明时凭据文件查 DEEPSEEK_API_KEY（与状态栏余额口径一致）。
        assert_eq!(
            super::resolve_api_key(&config, None).as_deref(),
            Some("file-deep")
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn resolve_api_key_treats_blank_env_as_unset() {
        let _guard = super::ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let (config, root) = temp_config("blank", "DEEPSEEK_API_KEY: file-deep\n");
        let r1 = set_env("DSH_BOX_API_KEY", Some("   "));
        let r2 = set_env("DEEPSEEK_API_KEY", None);
        assert_eq!(
            super::resolve_api_key(&config, None).as_deref(),
            Some("file-deep")
        );
        r1();
        r2();
        let _ = std::fs::remove_dir_all(&root);
    }

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

    #[test]
    fn plain_scalar_stops_at_unquoted_comment() {
        // ` #`（# 前有空白）起为行内注释；`abc#def` 的 # 前无空白，不截断
        assert_eq!(decode_scalar(" abc # 备注").as_deref(), Some("abc"));
        assert_eq!(decode_scalar(" abc#def").as_deref(), Some("abc#def"));
    }

    #[test]
    fn double_quoted_fallback_strips_quotes_on_nonstandard_escape() {
        // 非 JSON 兼容转义：按 YAML 语义去引号返回原文，不连引号返回
        assert_eq!(decode_scalar("\"a\\qb\"").as_deref(), Some("a\\qb"));
    }

    #[test]
    fn upsert_tolerates_bom_on_first_key() {
        // BOM 文件首键失配会导致同名键重复追加
        let text = "\u{feff}CORP_KEY: old\nKEEP: one\n";
        let out = upsert(text, "CORP_KEY", "new");
        assert_eq!(out.matches("CORP_KEY:").count(), 1);
        assert!(!out.contains("old"));
        assert!(out.contains("CORP_KEY: 'new'"));
    }

    #[test]
    fn remove_tolerates_bom_on_first_key() {
        let text = "\u{feff}DEEPSEEK_API_KEY: old\nKEEP: one\n";
        assert_eq!(remove(text, "DEEPSEEK_API_KEY"), "KEEP: one\n");
    }
}
