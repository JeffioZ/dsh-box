//! 任务完成系统通知：后台轮询 dsh 会话日志（`session.jsonl.zstd`），
//! 检测到新的 `turn/end` 事件且主窗口不可见时发系统通知。
//!
//! 只读复用官方会话日志（不建立第二套运行时/数据库）；解析失败仅跳过
//! 该轮，不影响外壳主流程。

use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use tauri::AppHandle;
use tauri::Manager;
use tauri_plugin_notification::NotificationExt;

use crate::app_state::AppState;

/// 轮询间隔：会话日志由 mtime 变化驱动，不需要太密。
const POLL_INTERVAL: Duration = Duration::from_secs(20);
/// 单个会话文件压缩体积上限：异常大文件跳过，避免轮询卡顿。
const MAX_SESSION_COMPRESSED: u64 = 256 * 1024 * 1024;

struct WatchedSession {
    mtime: Option<SystemTime>,
    /// 文件尾部最近一次观察到的 turn/end 序号。
    last_turn_end_seq: Option<u64>,
    /// 已通知过的最大 turn/end 序号（None = 首次见到，不通知）。
    notified_seq: Option<u64>,
}

/// 启动任务完成监视（后台线程，退出中自动停止）。
pub fn start_task_watch(app: AppHandle) {
    std::thread::spawn(move || {
        let mut watched: HashMap<PathBuf, WatchedSession> = HashMap::new();
        loop {
            std::thread::sleep(POLL_INTERVAL);
            if app.state::<AppState>().is_quitting() {
                return;
            }
            if let Err(e) = poll_once(&app, &mut watched) {
                crate::logging::log(&format!("notify: 轮询失败：{e}"));
            }
        }
    });
}

fn poll_once(
    app: &AppHandle,
    watched: &mut HashMap<PathBuf, WatchedSession>,
) -> Result<(), String> {
    let sessions_root = app.state::<AppState>().config().dsh_home().join("sessions");
    if !sessions_root.is_dir() {
        return Ok(()); // dsh 尚未产生会话目录，静默等待
    }
    let mut current: HashMap<PathBuf, SystemTime> = HashMap::new();
    collect_sessions(&sessions_root, &mut current)?;

    // 清理已删除的会话
    watched.retain(|p, _| current.contains_key(p));

    for (path, mtime) in current {
        let entry = watched.entry(path.clone()).or_insert(WatchedSession {
            mtime: None,
            last_turn_end_seq: None,
            notified_seq: None,
        });
        if entry.mtime == Some(mtime) {
            continue; // 文件未变：不重复解压
        }
        entry.mtime = Some(mtime);
        let Some(seq) = latest_turn_end_seq(&path)? else {
            continue; // 没有 turn/end（空会话/异常文件），等待下次变化
        };
        let is_new = entry.notified_seq.is_some_and(|notified| seq > notified);
        entry.last_turn_end_seq = Some(seq);
        entry.notified_seq = Some(seq);
        if !is_new {
            continue;
        }
        // 主窗口不可见（最小化/隐藏到托盘）时才通知，避免打扰正在查看的用户
        let visible = crate::main_window(app)
            .and_then(|w| w.is_visible().ok())
            .unwrap_or(true);
        if !visible {
            crate::logging::log(&format!(
                "notify: 会话 {} 出现新完成轮次（seq {seq}），发送系统通知",
                path.file_name().and_then(|n| n.to_str()).unwrap_or("?")
            ));
            show_notification(app)?;
        }
    }
    Ok(())
}

/// 递归收集所有 `session.jsonl.zstd` 及其 mtime。
fn collect_sessions(root: &Path, out: &mut HashMap<PathBuf, SystemTime>) -> Result<(), String> {
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let entries = std::fs::read_dir(&dir).map_err(|e| format!("读取会话目录失败：{e}"))?;
        for ent in entries.flatten() {
            let p = ent.path();
            if p.is_dir() {
                stack.push(p);
            } else if p.file_name().and_then(|n| n.to_str()) == Some("session.jsonl.zstd")
                && !p.to_string_lossy().contains(".pre-repair-bak")
            {
                if let Ok(meta) = ent.metadata() {
                    if let Ok(mtime) = meta.modified() {
                        out.insert(p, mtime);
                    }
                }
            }
        }
    }
    Ok(())
}

/// 解压会话日志，返回最后一个 `turn/end` 事件的 seq（无则 None）。
/// 只做尾部行级扫描，不全量 JSON 解析。
fn latest_turn_end_seq(path: &Path) -> Result<Option<u64>, String> {
    let meta = std::fs::metadata(path).map_err(|e| format!("读取会话文件失败：{e}"))?;
    if meta.len() > MAX_SESSION_COMPRESSED {
        return Ok(None); // 异常大文件：跳过，不阻塞轮询
    }
    let file = std::fs::File::open(path).map_err(|e| format!("打开会话文件失败：{e}"))?;
    let mut decoder =
        zstd::stream::read::Decoder::new(file).map_err(|e| format!("zstd 解压失败：{e}"))?;
    let mut buf = Vec::new();
    decoder
        .read_to_end(&mut buf)
        .map_err(|e| format!("zstd 解压失败：{e}"))?;
    let text = String::from_utf8_lossy(&buf);
    // 从尾部找最后一个 turn/end 行，提取其 seq
    let marker = "\"type\":\"turn/end\"";
    let Some(pos) = text.rfind(marker) else {
        return Ok(None);
    };
    let line_start = text[..pos].rfind('\n').map(|i| i + 1).unwrap_or(0);
    let line = &text[line_start..];
    let line = line.split('\n').next().unwrap_or(line);
    let seq = parse_seq(line);
    Ok(seq)
}

/// 从事件行提取 `"seq":<n>` 字段（行级轻量解析）。
fn parse_seq(line: &str) -> Option<u64> {
    let idx = line.find("\"seq\"")?;
    let rest = &line[idx + 5..];
    let rest = rest.trim_start();
    let rest = rest.strip_prefix(':')?.trim_start();
    let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
    if digits.is_empty() {
        None
    } else {
        digits.parse().ok()
    }
}

fn show_notification(app: &AppHandle) -> Result<(), String> {
    app.notification()
        .builder()
        .title(crate::locale::text("任务完成", "Task complete").to_string())
        .body(
            crate::locale::text(
                "Agent 已完成一轮任务，点击窗口查看。",
                "The agent finished a task. Open the window to view it.",
            )
            .to_string(),
        )
        .show()
        .map_err(|e| format!("发送系统通知失败：{e}"))
}

#[cfg(test)]
mod tests {
    use super::parse_seq;

    #[test]
    fn extracts_seq_from_event_line() {
        assert_eq!(
            parse_seq(r#"{"type":"turn/end","seq":187,"time":1786779959436,"data":{}}"#),
            Some(187)
        );
        assert_eq!(parse_seq(r#"{"type":"turn/start","seq":6}"#), Some(6));
        assert_eq!(parse_seq(r#"{"type":"session"}"#), None);
        assert_eq!(parse_seq(""), None);
    }
}
