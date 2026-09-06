//! dsh `.credentials.yaml` 的最小行级读写工具（v1 布局）。
//!
//! 与 dsh 官方凭据服务（`@deepseek-ai/dsh-credentials-local`）同一格式：
//! 顶层 `version: 1`，apiKeyEnv 凭据统一放在 `refs:` 段（`records:` 段用于
//! OAuth 等记录，本工具不触碰）。只有如此，设置弹窗 / 模型导入里填的 key
//! 才会被 dsh 真正读取——扁平顶层布局是 dsh 的 pre-release 旧格式，其解析
//! 直接抛错（MISSING_CREDENTIAL）。本仓库从未发布过写扁平布局的正式版，
//! 因此只按 v1 布局读写，不做旧格式迁移。
//!
//! 凭据文件不经过通用 YAML 序列化，避免重排或改写用户的其他条目；所有写入仍
//! 由 `app_state::update_text_file` 串行并原子替换。

use crate::app_state::Config;

pub(crate) fn value(config: &Config, name: &str) -> Option<String> {
    let text = std::fs::read_to_string(config.dsh_home().join(".credentials.yaml")).ok()?;
    value_from_text(&text, name)
}

/// 从凭据文档文本取引用值：只读 v1 布局的 `refs:.NAME`（扁平布局为已废弃
/// 的 pre-release 格式，不做兼容读取）。
fn value_from_text(text: &str, name: &str) -> Option<String> {
    let document = crate::yaml_fields::parse(text).ok()?;
    let refs = document.get("refs")?.as_object()?;
    let value = refs.get(name)?.as_str()?;
    (!value.is_empty()).then(|| value.to_string())
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

pub(crate) fn save(config: &Config, name: &str, value: &str) -> Result<(), String> {
    let path = config.dsh_home().join(".credentials.yaml");
    crate::app_state::update_text_file(&path, |text| upsert_checked(&text, name, value))
}

pub(crate) fn upsert_checked(text: &str, name: &str, value: &str) -> Result<String, String> {
    let document = crate::yaml_fields::parse(text)?;
    if document
        .get("version")
        .is_some_and(|v| v.as_u64() != Some(1))
    {
        return Err(crate::locale::text(
            "凭据文件版本不受支持，原文件未更改。",
            "Unsupported credentials file version; the original file was kept.",
        )
        .into());
    }
    let mut next =
        crate::yaml_fields::set(text, "refs", name, serde_json::Value::String(value.into()))?;
    if document.get("version").is_none() {
        next = format!("version: 1\n{next}");
    }
    crate::yaml_fields::parse(&next)?;
    Ok(next)
}

pub(crate) fn remove_saved(config: &Config, name: &str) -> Result<(), String> {
    let path = config.dsh_home().join(".credentials.yaml");
    crate::app_state::update_text_file(&path, |text| {
        let doc = crate::yaml_fields::parse(&text)?;
        let matches: Vec<String> = doc
            .get("refs")
            .and_then(|v| v.as_object())
            .into_iter()
            .flat_map(|m| m.keys())
            .filter(|k| k.as_str() == name)
            .cloned()
            .collect();
        let mut next = text;
        for key in matches {
            next = crate::yaml_fields::remove(&next, "refs", &key)?;
        }
        Ok(next)
    })
}

#[cfg(test)]
pub(crate) static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(test)]
mod tests {

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
            "version: 1\nrefs:\n  DEEPSEEK_API_KEY: file-deep\n  DSHBOX_TEST_ROUTE_KEY_7Q2Z: file-route\n",
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
        let (config, root) = temp_config(
            "blank",
            "version: 1\nrefs:\n  DEEPSEEK_API_KEY: file-deep\n",
        );
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
    fn semantic_credentials_preserve_other_entries() {
        for source in [
            "version: 1\nrefs: {KEEP: old}\n",
            "version: 1\nrefs: # 注释\n    KEEP: old\n",
            "",
        ] {
            let out = super::upsert_checked(source, "CORP_KEY", "a: b # ' \" 中文").unwrap();
            assert_eq!(
                super::value_from_text(&out, "CORP_KEY").as_deref(),
                Some("a: b # ' \" 中文")
            );
            if source.contains("KEEP") {
                assert_eq!(super::value_from_text(&out, "KEEP").as_deref(), Some("old"));
            }
        }
    }
    #[test]
    fn removal_handles_flow_and_block_without_changing_neighbors() {
        for source in [
            "refs: {KEY: old, KEEP: yes}\nother: keep\n",
            "refs: {KEEP: yes, KEY: old}\nother: keep\n",
            "refs:\n    KEY: old\n    KEEP: yes\nother: keep\n",
            "refs:\n  KEY: old\n",
        ] {
            let out = crate::yaml_fields::remove(source, "refs", "KEY").unwrap();
            assert!(crate::yaml_fields::parse(&out).unwrap()["refs"]
                .get("KEY")
                .is_none());
            if source.contains("other:") {
                assert!(out.contains("other: keep"));
            }
        }
    }
    #[test]
    fn invalid_yaml_does_not_supply_credentials() {
        assert!(super::value_from_text("refs: {}\n  KEY: secret", "KEY").is_none());
    }
}
