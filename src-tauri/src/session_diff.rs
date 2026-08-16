//! 会话文件变更追踪：只读解析最新 dsh 会话日志（`session.jsonl.zstd`），
//! 汇总 `edit`/`write` 工具对文件的改动；支持对纯 `edit` 改动的文件
//! 反向应用（还原到会话前状态）。
//!
//! 只读复用官方会话日志，不建立第二套运行时；解析失败仅返回空结果，
//! 不影响外壳主流程。

use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};

use serde::Serialize;
use tauri::AppHandle;
use tauri::Manager;

use crate::app_state::AppState;

/// 会话 diff 窗口 label。
pub const SESSION_DIFF_WINDOW: &str = "session-diff";

/// 单个文件改动上限（防止异常会话拖垮 UI）。
const MAX_EDITS_PER_FILE: usize = 500;

#[derive(Serialize)]
pub struct EditOp {
    pub old: String,
    pub new: String,
    pub seq: u64,
}

#[derive(Serialize)]
pub struct FileChange {
    pub path: String,
    pub edits: Vec<EditOp>,
    /// 是否可还原（全部改动均为可逆的 edit 操作）。
    pub revertible: bool,
    /// 是否发生过整文件重写（write）。
    pub rewritten: bool,
}

#[derive(Serialize)]
pub struct SessionChanges {
    pub session_id: String,
    pub files: Vec<FileChange>,
}

/// 打开会话文件变更窗口（已存在则聚焦）。
pub fn open(app: &AppHandle) {
    if let Some(w) = app.get_webview_window(SESSION_DIFF_WINDOW) {
        let _ = w.show();
        let _ = w.set_focus();
        return;
    }
    let navigation_app = app.clone();
    match tauri::WebviewWindowBuilder::new(
        app,
        SESSION_DIFF_WINDOW,
        tauri::WebviewUrl::App("session-diff.html".into()),
    )
    .title(crate::locale::text("会话文件变更", "Session file changes"))
    .inner_size(720.0, 640.0)
    .min_inner_size(520.0, 480.0)
    .resizable(true)
    .on_navigation(move |url| {
        let allowed = crate::is_local_app_url(url, crate::app_dev_origin(&navigation_app).as_ref());
        if !allowed {
            crate::logging::log(&format!("session-diff: 已拦截非白名单导航 {url}"));
        }
        allowed
    })
    .build()
    {
        Ok(w) => {
            let _ = w.show();
            crate::logging::log("session-diff: 会话文件变更窗口已打开");
        }
        Err(e) => crate::logging::log(&format!("session-diff: 打开失败：{e}")),
    }
}

/// 汇总最新会话的文件改动。
pub fn changes(app: &AppHandle) -> SessionChanges {
    let config = app.state::<AppState>().config();
    let Some((session_id, path)) = latest_session(&config.dsh_home().join("sessions")) else {
        return SessionChanges {
            session_id: String::new(),
            files: vec![],
        };
    };
    let ops = parse_edits(&path);
    let mut by_file: HashMap<String, Vec<EditOp>> = HashMap::new();
    for op in ops {
        let list = by_file.entry(op.path).or_default();
        if list.len() < MAX_EDITS_PER_FILE {
            list.push(EditOp {
                old: op.old,
                new: op.new,
                seq: op.seq,
            });
        }
    }
    let mut files: Vec<FileChange> = by_file
        .into_iter()
        .map(|(path, edits)| {
            let revertible = edits.iter().all(|e| !e.old.is_empty()) && !edits.is_empty();
            let rewritten = edits.iter().any(|e| e.old.is_empty());
            FileChange {
                path,
                edits,
                revertible,
                rewritten,
            }
        })
        .collect();
    files.sort_by(|a, b| a.path.cmp(&b.path));
    SessionChanges { session_id, files }
}

/// 还原某个文件到会话前状态（反向应用全部 edit）。
/// 任一 edit 的 new_string 在当前内容中找不到即失败（文件被其他方式改过）。
pub fn revert(app: &AppHandle, path: &str) -> Result<(), String> {
    let config = app.state::<AppState>().config();
    let Some((_, log_path)) = latest_session(&config.dsh_home().join("sessions")) else {
        return Err(crate::locale::text(
            "未找到会话日志。",
            "No session log found.",
        )
        .into());
    };
    let ops = parse_edits(&log_path);
    let mut edits: Vec<EditOp> = ops
        .into_iter()
        .filter(|op| op.path == path)
        .map(|op| EditOp {
            old: op.old,
            new: op.new,
            seq: op.seq,
        })
        .collect();
    if edits.is_empty() {
        return Err(crate::locale::text(
            "该文件没有可还原的改动。",
            "This file has no revertible changes.",
        )
        .into());
    }
    if edits.iter().any(|e| e.old.is_empty()) {
        return Err(crate::locale::text(
            "该文件发生过整文件重写，无法自动还原。",
            "This file was fully rewritten and cannot be reverted automatically.",
        )
        .into());
    }
    let target = PathBuf::from(path);
    let mut content = std::fs::read_to_string(&target)
        .map_err(|e| format!("{}: {e}", crate::locale::text("读取文件失败", "Failed to read the file")))?;
    // 逆序反向应用（从最后一次改动往前），避免前面的替换破坏后续匹配
    edits.sort_by_key(|e| std::cmp::Reverse(e.seq));
    for e in &edits {
        match content.find(&e.new) {
            Some(pos) => {
                content.replace_range(pos..pos + e.new.len(), &e.old);
            }
            None => {
                return Err(crate::locale::text(
                    "文件内容已被其他方式修改，无法完整还原（已还原的部分已保存）。",
                    "The file was modified by other means; revert is incomplete (reverted parts are saved).",
                )
                .into());
            }
        }
    }
    crate::app_state::atomic_write(&target, &content)
}

// ---------- 解析 ----------

struct ParsedEdit {
    path: String,
    old: String,
    new: String,
    seq: u64,
}

/// 解压并解析会话日志中的 edit/write 工具调用（按 seq 升序）。
fn parse_edits(log_path: &Path) -> Vec<ParsedEdit> {
    let Ok(file) = std::fs::File::open(log_path) else {
        return vec![];
    };
    let Ok(mut decoder) = zstd::stream::read::Decoder::new(file) else {
        return vec![];
    };
    let mut buf = Vec::new();
    if decoder.read_to_end(&mut buf).is_err() {
        return vec![];
    }
    let text = String::from_utf8_lossy(&buf);
    let mut out = vec![];
    for line in text.lines() {
        if !line.contains("\"tool/call\"") {
            continue;
        }
        let Ok(obj) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        let Some(data) = obj.get("data") else { continue };
        let Some(name) = data.get("name").and_then(|v| v.as_str()) else {
            continue;
        };
        if name != "edit" && name != "write" {
            continue;
        }
        let Ok(args) = serde_json::from_str::<serde_json::Value>(
            data.get("arguments").and_then(|v| v.as_str()).unwrap_or(""),
        ) else {
            continue;
        };
        let Some(file_path) = args.get("file_path").and_then(|v| v.as_str()) else {
            continue;
        };
        let seq = obj.get("seq").and_then(|v| v.as_u64()).unwrap_or(0);
        let old = args
            .get("old_string")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let new = if name == "write" {
            // 整文件重写：new_string 语义 = 完整内容；old 为空标记不可逆
            args.get("content")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string()
        } else {
            args.get("new_string")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string()
        };
        out.push(ParsedEdit {
            path: file_path.to_string(),
            old,
            new,
            seq,
        });
    }
    out
}

/// 最新会话：sessions/ 下 mtime 最新的 session.jsonl.zstd（排除备份）。
fn latest_session(sessions_root: &Path) -> Option<(String, PathBuf)> {
    if !sessions_root.is_dir() {
        return None;
    }
    let mut best: Option<(u64, String, PathBuf)> = None;
    let mut stack = vec![sessions_root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for ent in entries.flatten() {
            let p = ent.path();
            if p.is_dir() {
                stack.push(p);
            } else if p.file_name().and_then(|n| n.to_str()) == Some("session.jsonl.zstd")
                && !p.to_string_lossy().contains(".pre-repair-bak")
            {
                if let Ok(meta) = ent.metadata() {
                    if let Ok(modified) = meta.modified() {
                        if let Ok(dur) = modified.duration_since(std::time::UNIX_EPOCH) {
                            let ts = dur.as_secs();
                            if best.as_ref().is_none_or(|(t, _, _)| ts > *t) {
                                let session_id = dir
                                    .file_name()
                                    .and_then(|n| n.to_str())
                                    .unwrap_or("?")
                                    .to_string();
                                best = Some((ts, session_id, p));
                            }
                        }
                    }
                }
            }
        }
    }
    best.map(|(_, id, path)| (id, path))
}

#[cfg(test)]
mod tests {
    use super::parse_edits;
    use std::io::Write;

    fn zstd_write(path: &std::path::Path, lines: &[&str]) {
        let file = std::fs::File::create(path).unwrap();
        let mut enc = zstd::stream::write::Encoder::new(file, 3).unwrap();
        for l in lines {
            writeln!(enc, "{l}").unwrap();
        }
        enc.finish().unwrap();
    }

    #[test]
    fn parses_edit_and_write_ops_in_seq_order() {
        let dir = std::env::temp_dir().join(format!("dshd-diff-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let log = dir.join("session.jsonl.zstd");
        zstd_write(
            &log,
            &[
                r#"{"type":"turn/start","seq":1}"#,
                r#"{"type":"tool/call","seq":10,"data":{"name":"edit","arguments":"{\"file_path\":\"a.txt\",\"old_string\":\"x\",\"new_string\":\"y\"}"}}"#,
                r#"{"type":"tool/call","seq":11,"data":{"name":"edit","arguments":"{\"file_path\":\"a.txt\",\"old_string\":\"y\",\"new_string\":\"z\"}"}}"#,
                r#"{"type":"tool/call","seq":12,"data":{"name":"write","arguments":"{\"file_path\":\"b.txt\",\"content\":\"full\"}"}}"#,
                r#"{"type":"tool/call","seq":13,"data":{"name":"read","arguments":"{\"file_path\":\"a.txt\"}"}}"#,
            ],
        );
        let ops = parse_edits(&log);
        assert_eq!(ops.len(), 3);
        assert_eq!(ops[0].path, "a.txt");
        assert_eq!(ops[0].old, "x");
        assert_eq!(ops[0].new, "y");
        assert_eq!(ops[0].seq, 10);
        assert_eq!(ops[1].seq, 11);
        // write：old 为空标记不可逆
        assert_eq!(ops[2].path, "b.txt");
        assert!(ops[2].old.is_empty());
        assert_eq!(ops[2].new, "full");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
