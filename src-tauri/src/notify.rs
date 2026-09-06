//! 任务完成系统通知：后台轮询 dsh 会话日志（`session.jsonl.zstd`），
//! 检测到新的 `turn/end` 事件且主窗口不可见时发系统通知。
//!
//! 只读复用官方会话日志（不建立第二套运行时/数据库）；解析失败仅跳过
//! 该轮，不影响外壳主流程。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use tauri::{AppHandle, Manager};

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
    {
        use tauri_plugin_notification::NotificationExt;
        if let Err(e) = app.notification().request_permission() {
            crate::logging::log(&format!("notify: 申请通知权限失败：{e}"));
        }
    }
    std::thread::spawn(move || {
        let mut watched = HashMap::new();
        let mut started_at = watch_time();
        let mut was_enabled = app.state::<AppState>().config().task_notifications;
        loop {
            std::thread::sleep(POLL_INTERVAL);
            // 就绪门控统一走 background::service_gate（仅本地托管且 Ready
            // 放行）；外部模式没有会话日志可查，断开重连后需从头跟踪
            match crate::background::service_gate(&app) {
                crate::background::Gate::Quitting => return,
                crate::background::Gate::NotReady => {
                    if app.state::<AppState>().service_ownership().is_external() {
                        watched.clear();
                        started_at = watch_time();
                    }
                    continue;
                }
                crate::background::Gate::Ready => {}
            }
            if !app.state::<AppState>().config().task_notifications {
                was_enabled = false;
                watched.clear();
                started_at = watch_time();
                continue;
            }
            if !was_enabled {
                started_at = watch_time();
                was_enabled = true;
            }
            if let Err(e) = poll_once(&app, &mut watched, started_at) {
                crate::logging::log(&format!("notify: 轮询失败：{e}"));
            }
        }
    });
}

fn watch_time() -> i64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

fn poll_once(
    app: &AppHandle,
    watched: &mut HashMap<String, WatchedSession>,
    started_at: i64,
) -> Result<(), String> {
    let config = app.state::<AppState>().config();
    let sessions = crate::usage::list_session_logs(&config);
    let present: std::collections::HashSet<_> =
        sessions.iter().map(|(id, _)| id.as_str()).collect();
    watched.retain(|id, _| present.contains(id.as_str()));
    let mut last_error = None;
    for (id, path) in sessions {
        let entry = watched.entry(id.clone()).or_insert_with(|| WatchedSession {
            path: path.clone(),
            mtime: None,
            notified_seq: None,
            initialized: false,
        });
        if entry.path != path {
            entry.path = path.clone();
            entry.mtime = None;
        }
        if let Err(error) = poll_session(
            app,
            entry,
            &path,
            config.task_notifications,
            started_at,
            &id,
        ) {
            last_error = Some(error);
        }
    }
    last_error.map_or(Ok(()), Err)
}

fn poll_session(
    app: &AppHandle,
    entry: &mut WatchedSession,
    path: &Path,
    enabled: bool,
    started_at: i64,
    session_id: &str,
) -> Result<(), String> {
    let Ok(mtime) = std::fs::metadata(path).and_then(|meta| meta.modified()) else {
        return Ok(());
    };
    if entry.mtime == Some(mtime) {
        return Ok(());
    }
    // 冷启动先排除历史文件，保持未初始化以便之后仍按事件时间过滤旧轮次。
    if !entry.initialized
        && mtime
            .duration_since(SystemTime::UNIX_EPOCH)
            .is_ok_and(|time| (time.as_millis() as i64) < started_at)
    {
        entry.mtime = Some(mtime);
        return Ok(());
    }
    // 只有尾帧读取/解析成功后才提交 mtime；瞬时读失败留到下一轮重试，
    // 避免把一次错误永久记成“已处理”而漏掉完成通知。
    let latest_event = latest_turn_end(path)?;
    let latest = latest_event.as_ref().and_then(|event| event.seq);
    if !entry.initialized
        && !latest_event
            .as_ref()
            .and_then(|event| event.time)
            .is_some_and(|time| time >= started_at)
    {
        entry.mtime = Some(mtime);
        entry.initialized = true;
        entry.notified_seq = latest;
        return Ok(());
    }
    entry.initialized = true;
    let Some(seq) = latest else {
        entry.mtime = Some(mtime);
        return Ok(());
    };
    let is_new = entry.notified_seq.is_none_or(|notified| seq > notified);
    if is_new && enabled && !crate::main_is_visible(app) {
        crate::logging::log(&format!(
            "notify: 当前会话出现新完成轮次（seq {seq}），发送系统通知"
        ));
        // 发送失败不提交 seq/mtime：下一轮对同一事件重试。取舍为宁可极端
        // 情况下（发送成功但提交前进程退出）重复通知一次，也不漏发。
        show_notification(app, session_id)?;
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
    time: Option<i64>,
}

/// 只解压追加式会话日志的尾帧，返回最后一个顶层 `turn/end` 事件序号。
/// 大会话的轮询成本因此固定在尾部窗口，不再每 20 秒全量解压数百 MB。
fn latest_turn_end(path: &Path) -> Result<Option<EventHeader>, String> {
    let frames = crate::session_log::read_tail_frames(path, 8)
        .map_err(|error| format!("读取会话尾帧失败：{error}"))?;
    Ok(frames.iter().find_map(|text| {
        text.lines().rev().find_map(|line| {
            let event: EventHeader = serde_json::from_str(line).ok()?;
            (event.kind == "turn/end" && event.seq.is_some()).then_some(event)
        })
    }))
}

#[cfg(test)]
fn parse_turn_end_seq(line: &str) -> Option<u64> {
    let event: EventHeader = serde_json::from_str(line).ok()?;
    (event.kind == "turn/end").then_some(event.seq).flatten()
}

fn show_notification(app: &AppHandle, session_id: &str) -> Result<(), String> {
    notify_target(
        app,
        crate::locale::text("任务完成", "Task complete"),
        crate::locale::text(
            "任务已完成，可回到 DSHBox 查看结果。",
            "Task complete. Return to DSHBox to view the result.",
        ),
        Some(session_id),
    )
}

/// 发送系统通知（notify.rs 之外的模块——用量预警等——复用同一入口）。
/// Windows：带「查看」按钮与正文点击回调（in-process on_activated），
/// 点击即唤起主窗口；其余平台走插件通知（无点击回调，点击行为交给系统默认）。
pub(crate) fn notify(app: &AppHandle, title: &str, body: &str) -> Result<(), String> {
    notify_target(app, title, body, None)
}

fn notify_target(
    app: &AppHandle,
    title: &str,
    body: &str,
    session_id: Option<&str>,
) -> Result<(), String> {
    #[cfg(windows)]
    {
        let action_app = app.clone();
        let selected = session_id
            .filter(|id| crate::usage::session_is_listed(&app.state::<AppState>().config(), id))
            .map(|id| serde_json::json!({"sessionId": id}).to_string());
        tauri_winrt_notification::Toast::new(&app.config().identifier)
            .title(title)
            .text1(body)
            .add_button(
                crate::locale::text("查看", "View"),
                crate::locale::text("查看", "View"),
            )
            .on_activated(move |_action| {
                // COM 激活回调不在应用主线程：窗口操作统一调度回主线程执行。
                let app = action_app.clone();
                let selected = selected.clone();
                let _ = action_app.run_on_main_thread(move || {
                    crate::show_main(&app);
                    if let (Some(selected), Some(webview)) = (selected, crate::main_webview(&app)) {
                        let config = app.state::<AppState>().config();
                        if webview.url().ok().is_some_and(|url| crate::is_dsh_url(&url, &config)) {
                            // 使用上游持久化选择恢复正常页面，避免访问框架私有对象。
                            let text = serde_json::to_string(&selected).unwrap_or_default();
                            let _ = webview.eval(format!("if (localStorage.getItem('dsh.sessions.current') !== null) {{ localStorage.setItem('dsh.sessions.current', {text}); location.reload(); }}"));
                        }
                    }
                });
                Ok(())
            })
            .show()
            .map_err(|e| format!("发送系统通知失败：{e}"))
    }
    #[cfg(not(windows))]
    {
        let _ = session_id;
        app_notification(app, title, body)
    }
}

#[cfg(not(windows))]
fn app_notification(app: &AppHandle, title: &str, body: &str) -> Result<(), String> {
    use tauri_plugin_notification::NotificationExt;
    app.notification()
        .builder()
        .title(title.to_string())
        .body(body.to_string())
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
