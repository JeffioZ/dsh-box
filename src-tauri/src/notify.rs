//! 任务完成系统通知：后台轮询 dsh 会话日志（`session.jsonl.zstd`），
//! 检测到新的 `turn/end` 事件且主窗口不可见时发系统通知。
//!
//! 只读复用官方会话日志（不建立第二套运行时/数据库）；解析失败仅跳过
//! 该轮，不影响外壳主流程。

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use tauri::AppHandle;
use tauri::Manager;
use tauri_plugin_notification::NotificationExt;

use crate::app_state::AppState;

/// 轮询间隔：会话日志由 mtime 变化驱动，不需要太密。
const POLL_INTERVAL: Duration = Duration::from_secs(20);
struct WatchedSession {
    path: PathBuf,
    mtime: Option<SystemTime>,
    /// 已通知过的最大 turn/end 序号（None = 首次见到，不通知）。
    notified_seq: Option<u64>,
    /// 区分“刚开始观察，不通知历史事件”和“观察时尚无完成事件”。
    initialized: bool,
}

/// 启动任务完成监视（后台线程，退出中自动停止）。
pub fn start_task_watch(app: AppHandle) {
    // macOS 首次发系统通知前必须向系统申请一次权限（Windows/Linux 无此流程）；
    // 失败仅记日志，后续 show 仍按系统实际授权结果成败
    #[cfg(target_os = "macos")]
    if let Err(e) = app.notification().request_permission() {
        crate::logging::log(&format!("notify: 申请通知权限失败：{e}"));
    }
    std::thread::spawn(move || {
        let mut watched: Option<WatchedSession> = None;
        loop {
            std::thread::sleep(POLL_INTERVAL);
            let state = app.state::<AppState>();
            if state.is_quitting() {
                return;
            }
            if state.service_ownership().is_external() {
                watched = None;
                continue;
            }
            if state.service_ownership() != crate::app_state::ServiceOwnership::Managed
                || state.phase() != crate::app_state::BootPhase::Ready
            {
                continue;
            }
            if let Err(e) = poll_once(&app, &mut watched) {
                crate::logging::log(&format!("notify: 轮询失败：{e}"));
            }
        }
    });
}

fn poll_once(app: &AppHandle, watched: &mut Option<WatchedSession>) -> Result<(), String> {
    let config = app.state::<AppState>().config();
    let Some(session_id) = crate::usage::current_session_id(&config) else {
        return Ok(());
    };
    let Some(path) = crate::usage::session_log_path(&config, &session_id) else {
        return Ok(());
    };
    let Ok(mtime) = std::fs::metadata(&path).and_then(|meta| meta.modified()) else {
        return Ok(());
    };
    if watched.as_ref().is_none_or(|entry| entry.path != path) {
        *watched = Some(WatchedSession {
            path: path.clone(),
            mtime: None,
            notified_seq: None,
            initialized: false,
        });
    }
    let Some(entry) = watched.as_mut() else {
        return Ok(());
    };
    if entry.mtime == Some(mtime) {
        return Ok(());
    }
    // 只有尾帧读取/解析成功后才提交 mtime；瞬时读失败留到下一轮重试，
    // 避免把一次错误永久记成“已处理”而漏掉完成通知。
    let latest = latest_turn_end_seq(&path)?;
    if !entry.initialized {
        entry.mtime = Some(mtime);
        entry.initialized = true;
        entry.notified_seq = latest;
        return Ok(());
    }
    let Some(seq) = latest else {
        entry.mtime = Some(mtime);
        return Ok(());
    };
    let is_new = entry.notified_seq.is_none_or(|notified| seq > notified);
    if is_new && config.task_notifications && !crate::main_is_visible(app) {
        crate::logging::log(&format!(
            "notify: 当前会话出现新完成轮次（seq {seq}），发送系统通知"
        ));
        // 发送失败不提交 seq/mtime：下一轮对同一事件重试。取舍为宁可极端
        // 情况下（发送成功但提交前进程退出）重复通知一次，也不漏发。
        show_notification(app)?;
    }
    entry.notified_seq = Some(seq);
    entry.mtime = Some(mtime);
    Ok(())
}

#[derive(serde::Deserialize)]
struct EventHeader {
    #[serde(rename = "type")]
    kind: String,
    seq: Option<u64>,
}

/// 只解压追加式会话日志的尾帧，返回最后一个顶层 `turn/end` 事件序号。
/// 大会话的轮询成本因此固定在尾部窗口，不再每 20 秒全量解压数百 MB。
fn latest_turn_end_seq(path: &Path) -> Result<Option<u64>, String> {
    let frames = crate::session_log::read_tail_frames(path, 8)
        .map_err(|error| format!("读取会话尾帧失败：{error}"))?;
    Ok(frames
        .iter()
        .find_map(|text| text.lines().rev().find_map(parse_turn_end_seq)))
}

fn parse_turn_end_seq(line: &str) -> Option<u64> {
    let event: EventHeader = serde_json::from_str(line).ok()?;
    (event.kind == "turn/end").then_some(event.seq).flatten()
}

fn show_notification(app: &AppHandle) -> Result<(), String> {
    app.notification()
        .builder()
        .title(crate::locale::text("任务完成", "Task complete").to_string())
        .body(
            crate::locale::text(
                "任务已完成，可回到 DSHBox 查看结果。",
                "Task complete. Return to DSHBox to view the result.",
            )
            .to_string(),
        )
        .show()
        .map_err(|e| format!("发送系统通知失败：{e}"))
}

#[cfg(test)]
mod tests {
    use super::parse_turn_end_seq;

    #[test]
    fn extracts_seq_from_event_line() {
        assert_eq!(
            parse_turn_end_seq(r#"{"type":"turn/end","seq":187,"time":1786779959436,"data":{}}"#),
            Some(187)
        );
        assert_eq!(parse_turn_end_seq(r#"{"type":"turn/start","seq":6}"#), None);
        assert_eq!(parse_turn_end_seq(r#"{"type":"session"}"#), None);
        assert_eq!(parse_turn_end_seq(""), None);
    }

    #[test]
    fn nested_seq_cannot_override_the_top_level_sequence() {
        assert_eq!(
            parse_turn_end_seq(
                r#"{"type":"turn/end","seq":7,"data":{"seq":999,"type":"turn/end"}}"#
            ),
            Some(7)
        );
    }
}
