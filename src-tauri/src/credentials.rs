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

/// 是否匹配 `key:`（`key` 后紧跟 `:` 即命中，值可为空、标量或注释；
/// content 已去首空白）。`version: 1`、`refs:`、`records:` 均命中。
fn is_section_key(content: &str, key: &str) -> bool {
    content
        .strip_prefix(key)
        .is_some_and(|rest| rest.trim_start().starts_with(':'))
}

/// 是否为 `refs` 段键行：接受 `refs:`、`refs: {}`、`refs: # 注释`、
/// `refs: {} # 注释` 等。`{}` 是 YAML 内联空对象，用户/dsh 可能用它表示
/// 空段；若只按 is_section_key（要求值可空/注释）判断，`refs: {}` 会被
/// 漏判为「无 refs 段」，导致 upsert 重复建段、value/remove 读不到值、
/// 或把内联空与后续缩进子键叠成非法缩进。
fn is_refs_section(content: &str) -> bool {
    let Some(rest) = content.strip_prefix("refs") else {
        return false;
    };
    let rest = rest.trim_start();
    let Some(after) = rest.strip_prefix(':') else {
        return false;
    };
    let value = after.trim_start();
    value.is_empty() || value.starts_with('#') || refs_value_inline_empty(value)
}

/// refs 段的值为「内联空对象」`{}`（允许括号内空格与行内注释，`#` 前有无
/// 空格均可）。`{}` 是 YAML 空对象，用户/dsh 可能用它表示空段。
fn refs_value_inline_empty(value: &str) -> bool {
    let no_comment = value.split('#').next().unwrap_or("").trim();
    let compact: String = no_comment.chars().filter(|c| !c.is_whitespace()).collect();
    compact == "{}"
}

/// refs 键行是否为「内联空对象」`refs: {}`（可带行内注释）。若是，写入时
/// 应归一化为空块 `refs:`，否则内联 `{}` 后跟缩进子键会叠成非法 YAML。
fn refs_inline_empty(content: &str) -> bool {
    let Some(rest) = content.strip_prefix("refs") else {
        return false;
    };
    let Some(after) = rest.trim_start().strip_prefix(':') else {
        return false;
    };
    refs_value_inline_empty(after.trim_start())
}

/// 把 refs 键行归一化为空块并保留行内注释：`refs: {} # c` -> `refs: # c`、
/// `refs: { }` -> `refs:`。非内联空的行原样返回（含缩进）。用于 upsert/remove
/// 在向 refs 段写内容前把内联空对象改成块式，避免内联 `{}` 后挂缩进子键。
fn refs_norm_line(line: &str, stripped: &str) -> String {
    if !refs_inline_empty(stripped) {
        return line.to_string();
    }
    let indent = &line[..line.len() - stripped.len()];
    // 保留 # 起的行内注释（若存在）；注释前补一个空格。
    let comment = stripped
        .split('#')
        .nth(1)
        .map(|c| format!(" #{c}"))
        .unwrap_or_default();
    format!("{indent}refs:{comment}")
}

pub(crate) fn value(config: &Config, name: &str) -> Option<String> {
    let text = std::fs::read_to_string(config.dsh_home().join(".credentials.yaml")).ok()?;
    value_from_text(&text, name)
}

/// 从凭据文档文本取引用值：只读 v1 布局的 `refs:.NAME`（扁平布局为已废弃
/// 的 pre-release 格式，不做兼容读取）。
fn value_from_text(text: &str, name: &str) -> Option<String> {
    let mut in_refs = false;
    for line in text.lines() {
        let content = line.trim_start_matches('\u{feff}').trim_start();
        let indent = line.len() - line.trim_start().len();
        if in_refs {
            // 遇到下一个顶层键（非注释非空）即 refs 段结束
            if indent == 0 && !content.is_empty() && !content.starts_with('#') {
                break;
            }
            if indent > 0 && !content.starts_with('#') {
                if let Some(v) = key_scalar(content, name) {
                    return Some(v);
                }
            }
            continue;
        }
        if indent == 0 && is_refs_section(content) {
            in_refs = true;
        }
    }
    None
}

/// 解析一行 `NAME: value`，命中 name（忽略大小写）则解码标量。
fn key_scalar(content: &str, name: &str) -> Option<String> {
    let (candidate, value) = content.split_once(':')?;
    if candidate.trim().eq_ignore_ascii_case(name) {
        decode_scalar(value)
    } else {
        None
    }
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

/// 行级新增或更新 `refs:.NAME`（v1 布局）。文档缺 `version`（补 `version: 1`）
/// 或 `refs:` 段时自动补全；`records` 等其它顶层键与 refs 内顺带注释原样保留。
pub(crate) fn upsert(text: &str, name: &str, value: &str) -> String {
    let target_line = format!("  {name}: {}", encode_scalar(value));
    let mut out = String::new();
    let mut in_refs = false;
    let mut wrote = false;
    let mut wrote_version = false;
    let mut has_refs = false;
    let append_target = |out: &mut String, wrote: &mut bool| {
        if !*wrote {
            out.push_str(&target_line);
            out.push('\n');
            *wrote = true;
        }
    };
    for line in text.lines() {
        let content = line.trim_start_matches('\u{feff}');
        let stripped = content.trim_start();
        let indent = content.len() - stripped.len();
        let is_blank = stripped.is_empty();
        let is_comment = stripped.starts_with('#');
        let top = indent == 0
            && !is_blank
            && !is_comment
            && !stripped.starts_with('%')
            && !stripped.starts_with("---")
            && !stripped.starts_with("...");
        if in_refs {
            if top {
                // refs 段边界：本段内未写入目标行时在段尾补一行
                append_target(&mut out, &mut wrote);
                in_refs = false;
                // 边界行若是 version：这条路径不会落到下方专门的 version 分支，
                // 需在此标记已存在，避免文件头再补一个重复的 version。
                if is_section_key(stripped, "version") {
                    wrote_version = true;
                }
                out.push_str(line);
                out.push('\n');
                continue;
            }
            if !is_blank
                && !is_comment
                && key_matches(stripped.split_once(':').map_or(stripped, |(k, _)| k), name)
            {
                if !wrote {
                    out.push_str(&target_line);
                    out.push('\n');
                    wrote = true;
                }
                continue; // 丢弃旧行
            }
            out.push_str(line);
            out.push('\n');
            continue;
        }
        if top {
            if is_refs_section(stripped) {
                has_refs = true;
                // 保留原行（含可能的行内注释）；内联空 `refs: {}` 归一化为
                // 空块 `refs:`（保留注释），避免其后的缩进子键叠成非法 YAML。
                out.push_str(&refs_norm_line(line, stripped));
                out.push('\n');
                in_refs = true;
                continue;
            }
            if is_section_key(stripped, "version") {
                // 保留文档既有 version 值（可能是未来版本号），绝不改写；
                // 仅当文档无 version 时在末尾补 `version: 1`（dsh 当前读取版本）。
                wrote_version = true;
                out.push_str(line);
                out.push('\n');
                continue;
            }
            out.push_str(line);
            out.push('\n');
            continue;
        }
        out.push_str(line);
        out.push('\n');
    }
    if in_refs {
        append_target(&mut out, &mut wrote);
    }
    // 文件头补 `version: 1`（若文档确实没有 version）与 refs 段（若整份文档都没有
    // refs 段）。version 缺失才补：已有（无论位于哪个顶层位置）都保留原样。
    if !wrote_version {
        out = format!("version: 1\n{out}");
    }
    if !has_refs {
        // 补 refs 段；目标行若还没写，则一并补进 refs。仅当 out 恰好是刚补
        // 的 `version: 1\n`（fresh 文件）时 refs 直接紧随；否则前面已有其它
        // 顶层内容，用空行把 refs 段分隔开。
        if !out.ends_with("version: 1\n") {
            out.push('\n');
        }
        out.push_str("refs:\n");
        if !wrote {
            out.push_str(&target_line);
            out.push('\n');
        }
    }
    out
}

pub(crate) fn remove(text: &str, name: &str) -> String {
    let mut out = String::new();
    let mut in_refs = false;
    for line in text.lines() {
        let content = line.trim_start_matches('\u{feff}');
        let stripped = content.trim_start();
        let indent = content.len() - stripped.len();
        let is_blank = stripped.is_empty();
        let is_comment = stripped.starts_with('#');
        let top = indent == 0
            && !is_blank
            && !is_comment
            && !stripped.starts_with('%')
            && !stripped.starts_with("---")
            && !stripped.starts_with("...");
        if in_refs {
            if top {
                in_refs = false;
                out.push_str(line);
                out.push('\n');
                continue;
            }
            if !is_blank
                && !is_comment
                && key_matches(stripped.split_once(':').map_or(stripped, |(k, _)| k), name)
            {
                continue; // 删除该行
            }
            out.push_str(line);
            out.push('\n');
            continue;
        }
        if top && is_refs_section(stripped) {
            in_refs = true;
            out.push_str(&refs_norm_line(line, stripped));
            out.push('\n');
            continue;
        }
        out.push_str(line);
        out.push('\n');
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
    // 块标量指示符（| / >- 等）不是值本身：dsh 自身不写此形态，手改文件
    // 才会出现——按读不到处理（None），而不是把指示符当凭据返回。
    if value == "|" || value == ">" || value.starts_with("|-") || value.starts_with(">-") {
        return None;
    }
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
    fn upsert_writes_v1_layout_with_refs_section() {
        // 空文档：补 version 头与 refs 段，目标键缩进两格
        let out = upsert("", "CORP_KEY", "v");
        assert!(
            out.starts_with("version: 1\nrefs:\n  CORP_KEY: 'v'\n"),
            "actual: {out}"
        );
    }

    #[test]
    fn upsert_replaces_duplicate_refs_without_touching_neighbors() {
        let text =
            "version: 1\nrefs:\n  DEEPSEEK_API_KEY: keep\n  corp_key: old\n  CORP_KEY: duplicate\n";
        let out = upsert(text, "CORP_KEY", "new: value # exact");
        assert_eq!(out.matches("CORP_KEY:").count(), 1);
        assert!(!out.contains("corp_key:"));
        assert!(out.contains("  CORP_KEY: 'new: value # exact'"));
        assert!(out.contains("  DEEPSEEK_API_KEY: keep"));
        assert!(out.starts_with("version: 1"));
    }

    #[test]
    fn upsert_preserves_refs_inline_comment() {
        // refs 段键行若带行内注释应原样保留（与 version 处理一致）
        let text = "version: 1\nrefs: # 凭据引用\n  A: a\n";
        let out = upsert(text, "B", "b");
        assert!(out.contains("refs: # 凭据引用"));
        assert!(out.contains("  A: a"));
        assert!(out.contains("  B: 'b'"));
    }

    #[test]
    fn upsert_does_not_duplicate_version_when_refs_precedes_version() {
        // 文档里 refs 段在 version 之前：version 行经 refs 边界路径保留，但不应
        // 在文件头再补一个重复的 version（回归：此前会 prepend 重复 version）。
        let text = "refs:\n  A: a\nversion: 1\n";
        let out = upsert(text, "B", "b");
        assert_eq!(out.matches("version: 1").count(), 1);
        assert!(out.contains("  A: a"));
        assert!(out.contains("  B: 'b'"));
    }

    #[test]
    fn upsert_preserves_records_and_other_top_level_keys() {
        let text = "version: 1\nrecords:\n  scope/id:\n    type: oauth\nrefs:\n  A: a\n";
        let out = upsert(text, "B", "b");
        assert!(
            out.contains("records:\n  scope/id:\n    type: oauth"),
            "actual: {out}"
        );
        assert!(out.contains("  A: a"));
        assert!(out.contains("  B: 'b'"));
        assert!(out.starts_with("version: 1"));
    }

    #[test]
    fn upsert_creates_refs_when_only_records_exist() {
        // 已是 v1 但无 refs 段（只有 records 等）：追加 refs 段并用空行分隔。
        let text = "version: 1\nrecords:\n  scope/id:\n    type: oauth\n";
        let out = upsert(text, "CORP_KEY", "v");
        assert!(out.contains("records:\n  scope/id:\n    type: oauth"));
        assert!(out.contains("refs:\n  CORP_KEY: 'v'"));
        assert!(out.starts_with("version: 1"));
    }

    #[test]
    fn yaml_scalar_round_trip_preserves_special_characters() {
        let out = upsert("", "KEY", " leading: value # 'quoted' ");
        let refs_line = out.lines().find(|l| l.starts_with("  KEY:")).unwrap();
        let raw = refs_line.split_once(':').unwrap().1.trim();
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
    fn remove_drops_matching_refs_entries_only() {
        let text = "version: 1\nrefs:\n  KEEP: one\n  DeepSeek_API_Key: old\n  DEEPSEEK_API_KEY: duplicate\n";
        assert_eq!(
            remove(text, "DEEPSEEK_API_KEY"),
            "version: 1\nrefs:\n  KEEP: one\n"
        );
    }

    #[test]
    fn plain_scalar_stops_at_unquoted_comment() {
        // ` #`（# 前有空白）起为行内注释；`abc#def` 的 # 前无空白，不截断
        // 块标量指示符不是值：按读不到处理，不把 | / > 当凭据返回
        assert_eq!(decode_scalar("|"), None);
        assert_eq!(decode_scalar(">"), None);
        assert_eq!(decode_scalar("|-"), None);
        assert_eq!(decode_scalar(">-2"), None);
        assert_eq!(decode_scalar(" abc # 备注").as_deref(), Some("abc"));
        assert_eq!(decode_scalar(" abc#def").as_deref(), Some("abc#def"));
    }

    #[test]
    fn double_quoted_fallback_strips_quotes_on_nonstandard_escape() {
        // 非 JSON 兼容转义：按 YAML 语义去引号返回原文，不连引号返回
        assert_eq!(decode_scalar("\"a\\qb\"").as_deref(), Some("a\\qb"));
    }

    #[test]
    fn read_refs_only_ignores_flat_layout() {
        // 只读 v1 的 refs 段；扁平布局是已废弃的 pre-release 格式，不再兼容读取
        let v1 = "version: 1\nrefs:\n  DEEPSEEK_API_KEY: v1val\n";
        assert_eq!(
            super::value_from_text(v1, "DEEPSEEK_API_KEY").as_deref(),
            Some("v1val")
        );
        // 扁平顶层键不再被读取
        let flat = "DEEPSEEK_API_KEY: flat-should-not-be-read\n";
        assert_eq!(
            super::value_from_text(flat, "DEEPSEEK_API_KEY").as_deref(),
            None
        );
        // 文档起始 BOM 容错：v1 首行（version）前带 BOM 也能正常读取
        let bom = "\u{feff}version: 1\nrefs:\n  DEEPSEEK_API_KEY: b\n";
        assert_eq!(
            super::value_from_text(bom, "DEEPSEEK_API_KEY").as_deref(),
            Some("b")
        );
    }

    #[test]
    fn upsert_tolerates_bom_on_first_key() {
        // v1 文档首键（version 行）带 BOM 时读写仍容错，不会产生同名重复键
        let text = "\u{feff}version: 1\nrefs:\n  CORP_KEY: old\n  KEEP: one\n";
        let out = upsert(text, "CORP_KEY", "new");
        assert_eq!(out.matches("CORP_KEY:").count(), 1);
        assert!(!out.contains("old"));
        assert!(out.contains("  CORP_KEY: 'new'"));
        assert!(out.contains("  KEEP: one"));
    }

    #[test]
    fn remove_tolerates_bom_on_first_key() {
        let text = "\u{feff}version: 1\nrefs:\n  DEEPSEEK_API_KEY: old\n  KEEP: one\n";
        let out = remove(text, "DEEPSEEK_API_KEY");
        assert!(!out.contains("DEEPSEEK_API_KEY"));
        assert!(out.contains("KEEP: one"));
        assert_eq!(out.matches("version: 1").count(), 1);
    }

    #[test]
    fn upsert_normalizes_inline_empty_refs() {
        // refs: {}（内联空对象）应被识别为 refs 段并归一化为空块，写入键后
        // 形成合法块结构；不会重复建段或残留内联 {}。
        let text = "version: 1\nrefs: {}\n";
        let out = upsert(text, "IBRAIN_API_KEY", "v");
        assert_eq!(out, "version: 1\nrefs:\n  IBRAIN_API_KEY: 'v'\n");
    }

    #[test]
    fn upsert_fixes_bad_indent_under_inline_empty_refs() {
        // 已损坏文件：refs: {} 下带了缩进子键。upsert 更新同名键时应把它
        // 归一化为合法 refs: 块，并保留其它 refs 内容。
        let text = "version: 1\nrefs: {}\n  IBRAIN_API_KEY: old\n  KEEP: one\n";
        let out = upsert(text, "IBRAIN_API_KEY", "new");
        assert_eq!(
            out,
            "version: 1\nrefs:\n  IBRAIN_API_KEY: 'new'\n  KEEP: one\n"
        );
        assert_eq!(out.matches("refs:").count(), 1);
    }

    #[test]
    fn value_reads_key_under_inline_empty_refs() {
        // 读取端也要认 refs: {} 进入 refs 段，才能读到其下的缩进子键。
        let text = "version: 1\nrefs: {}\n  IBRAIN_API_KEY: 'v'\n";
        assert_eq!(
            super::value_from_text(text, "IBRAIN_API_KEY").as_deref(),
            Some("v")
        );
    }

    #[test]
    fn remove_fixes_inline_empty_refs_after_deleting_key() {
        // remove 识别 refs: {} 并归一化，删除目标键后不残留内联空 + 缩进键的非法结构。
        let text = "version: 1\nrefs: {}\n  IBRAIN_API_KEY: old\n  KEEP: one\n";
        let out = remove(text, "IBRAIN_API_KEY");
        assert_eq!(out, "version: 1\nrefs:\n  KEEP: one\n");
        assert_eq!(out.matches("refs:").count(), 1);
    }

    #[test]
    fn refs_inline_empty_handles_no_space_comment_and_inner_spaces() {
        // refs: {}#c（# 前无空格）、refs: { }（括号内带空格）都应判定为内联空，
        // 归一化后写键为合法块结构（不会在 {} 下挂缩进子键）。
        let t1 = "version: 1\nrefs: {}# keep\n";
        let out1 = upsert(t1, "K", "v");
        assert_eq!(out1, "version: 1\nrefs: # keep\n  K: 'v'\n");
        let t2 = "version: 1\nrefs: { }\n";
        let out2 = upsert(t2, "K", "v");
        assert_eq!(out2, "version: 1\nrefs:\n  K: 'v'\n");
    }

    #[test]
    fn refs_norm_line_preserves_inline_comment() {
        // 归一化 refs: {} 时保留其行内注释（`refs: {} # c` -> `refs: # c`）。
        let text = "version: 1\nrefs: {} # keep\n";
        let out = upsert(text, "K", "v");
        assert_eq!(out, "version: 1\nrefs: # keep\n  K: 'v'\n");
        assert!(out.contains("refs: # keep"));
    }
}
