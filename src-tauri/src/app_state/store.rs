//! 用户配置与内部运行状态的 JSON 持久化边界。
//!
//! `config.json` 只保存用户可理解、可修改的选项；`state.json` 保存窗口位置、
//! 首次引导和后台维护标记；两类文件没有交叉读取或隐式迁移。

use std::path::Path;
use std::sync::Mutex;

static JSON_WRITE_LOCK: Mutex<()> = Mutex::new(());

fn read_object(path: &Path) -> Result<serde_json::Map<String, serde_json::Value>, String> {
    match std::fs::read_to_string(path) {
        Ok(text) => serde_json::from_str::<serde_json::Value>(&text)
            .map_err(|e| {
                crate::locale::owned(
                    format!("{} 解析失败，已保留原文件：{e}", path.display()),
                    format!(
                        "Failed to parse {}; the original file was preserved: {e}",
                        path.display()
                    ),
                )
            })?
            .as_object()
            .cloned()
            .ok_or_else(|| {
                crate::locale::owned(
                    format!("{} 顶层不是对象，已保留原文件", path.display()),
                    format!(
                        "The top level of {} is not an object; the original file was preserved",
                        path.display()
                    ),
                )
            }),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(serde_json::Map::new()),
        Err(e) => Err(crate::locale::owned(
            format!("读取 {} 失败：{e}", path.display()),
            format!("Failed to read {}: {e}", path.display()),
        )),
    }
}

fn save_value(
    root: &Path,
    file_name: &str,
    key: &str,
    value: serde_json::Value,
) -> Result<(), String> {
    let _guard = JSON_WRITE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let path = root.join(file_name);
    let mut object = read_object(&path)?;
    object.insert(key.to_string(), value);
    let text = serde_json::to_string_pretty(&object).map_err(|e| e.to_string())?;
    super::atomic_write(&path, &text)
}

pub(crate) fn save_config_value(
    root: &Path,
    key: &str,
    value: serde_json::Value,
) -> Result<(), String> {
    save_value(root, "config.json", key, value)
}

pub(crate) fn save_state_value(
    root: &Path,
    key: &str,
    value: serde_json::Value,
) -> Result<(), String> {
    save_value(root, "state.json", key, value)
}

/// 真删 state.json 中的键（与写入 Null 的“清空”不同：不在文件里留残留键）。
pub(crate) fn remove_state_value(root: &Path, key: &str) -> Result<(), String> {
    let _guard = JSON_WRITE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let path = root.join("state.json");
    let mut object = read_object(&path)?;
    object.remove(key);
    let text = serde_json::to_string_pretty(&object).map_err(|e| e.to_string())?;
    super::atomic_write(&path, &text)
}

pub(crate) fn load_state_value(root: &Path, key: &str) -> Option<serde_json::Value> {
    read_object(&root.join("state.json"))
        .ok()
        .and_then(|object| object.get(key).cloned())
}

#[cfg(test)]
mod tests {
    use super::{load_state_value, remove_state_value, save_config_value, save_state_value};

    fn temp_root(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("dshbox-store-{name}-{}", std::process::id()))
    }

    #[test]
    fn internal_state_prefers_state_file_and_keeps_user_config_clean() {
        let root = temp_root("split");
        let _ = std::fs::remove_dir_all(&root);
        save_config_value(&root, "language", serde_json::json!("en")).unwrap();
        save_state_value(&root, "window", serde_json::json!({ "lx": 1 })).unwrap();

        let config = std::fs::read_to_string(root.join("config.json")).unwrap();
        assert!(!config.contains("window"));
        assert_eq!(load_state_value(&root, "window").unwrap()["lx"], 1);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn remove_state_value_deletes_the_key_without_touching_others() {
        let root = temp_root("remove");
        let _ = std::fs::remove_dir_all(&root);
        save_state_value(&root, "marker", serde_json::json!({"spec": "x"})).unwrap();
        save_state_value(&root, "keep", serde_json::json!(true)).unwrap();

        remove_state_value(&root, "marker").unwrap();
        assert!(load_state_value(&root, "marker").is_none());
        assert_eq!(
            load_state_value(&root, "keep").and_then(|v| v.as_bool()),
            Some(true)
        );
        // 键是真删而非 Null 残留
        let text = std::fs::read_to_string(root.join("state.json")).unwrap();
        assert!(!text.contains("marker"));
        // 删除不存在的键同样成功（幂等）
        remove_state_value(&root, "marker").unwrap();
        let _ = std::fs::remove_dir_all(root);
    }
}
