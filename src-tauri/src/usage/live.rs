//! 状态栏实时会话统计：轮询 dsh 后端同源 RPC（session.list → session.history）
//! 读取持久投影 sessionStats / tokenUsage，格式化为状态栏显示组并广播
//! `session-stats-updated`；另含尾帧估算的实时 tok/s 与会话活性探测。
//!
//! 历史/跨会话用量聚合见 `super::aggregate`；本模块只负责「当前会话」的
//! 轻量实时指标，供底部状态栏与通知/插件维护使用。

use serde::{Deserialize, Serialize};
use std::sync::OnceLock;
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager};

use crate::app_state::{AppState, Config};

/// 轮询间隔：统计为累计值、变化不频繁，5s 的实时感足够。
const POLL_INTERVAL: Duration = Duration::from_secs(5);

/// localhost RPC 专用 Agent（连接复用；无需 TLS）。
static STATS_AGENT: OnceLock<ureq::Agent> = OnceLock::new();

fn stats_agent() -> &'static ureq::Agent {
    STATS_AGENT.get_or_init(|| {
        ureq::Agent::config_builder()
            .timeout_connect(Some(Duration::from_secs(2)))
            .timeout_recv_response(Some(Duration::from_secs(3)))
            .timeout_recv_body(Some(Duration::from_secs(3)))
            .build()
            .new_agent()
    })
}

/// 状态栏显示组：key 决定前端图标（counts/durations/speeds/cache/tokens），
/// text 为已格式化文案（双语文案在 Rust 侧按当前 locale 选定）。
#[derive(Serialize, Clone)]
pub struct StatsGroup {
    pub key: &'static str,
    pub text: String,
}

/// 状态栏载荷：前端按 key 配图标渲染、组间加分隔线。
/// show_stats=false 表示"隐藏会话统计"开关已关闭（dsh 页面自己显示
/// 统计行）——前端清空统计区，余额 chip 保留常驻。
/// avg_tps 为平均解码速率（tok/s），前端在无实时速率时回退显示。
/// tooltip 明细行（各组文本 + 缓存读/写/未命中拆分等组内放不下的细节）。
#[derive(Serialize, Clone)]
pub struct StatsDetail {
    pub key: String,
    pub lines: Vec<String>,
}

#[derive(Serialize, Clone)]
pub struct StatsPayload {
    pub ok: bool,
    pub show_stats: bool,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub groups: Vec<StatsGroup>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub avg_tps: Option<f64>,
    /// 按组的 tooltip 明细：每组独立悬停提示内容。
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub details: Vec<StatsDetail>,
}

/// 启动周期轮询（后台线程，退出中自动停止）。
pub(crate) fn start_periodic(app: AppHandle) {
    std::thread::spawn(move || loop {
        std::thread::sleep(POLL_INTERVAL);
        if app.state::<AppState>().is_quitting() {
            return;
        }
        let config = app.state::<AppState>().config();
        if config.hide_statusbar || !config.hide_stats_line || !crate::main_is_visible(&app) {
            continue;
        }
        let payload = poll_once(&config);
        let _ = app.emit("session-stats-updated", payload);
    });
}

/// 立即拉取一次并广播（navigate 到 dsh / 设置切换时调用）：
/// 状态栏数据不必等下一个 5s 周期。
pub(crate) fn refresh_once(app: AppHandle) {
    std::thread::spawn(move || {
        let config = app.state::<AppState>().config();
        if config.hide_statusbar || !crate::main_is_visible(&app) {
            return;
        }
        let payload = poll_once(&config);
        let _ = app.emit("session-stats-updated", payload);
    });
}

/// 一次轮询：当前会话 → tail 页投影 → 格式化。任何一步失败返回空组。
/// 开关关闭（hide_stats_line=false）时不拉数据——dsh 页面自己显示统计，
/// 状态栏统计区互斥隐藏。
fn poll_once(config: &Config) -> StatsPayload {
    if !config.hide_stats_line {
        return StatsPayload {
            ok: true,
            show_stats: false,
            groups: Vec::new(),
            avg_tps: None,
            details: Vec::new(),
        };
    }
    match build_groups(config) {
        Some((groups, avg_tps, details)) => StatsPayload {
            ok: true,
            show_stats: true,
            groups,
            avg_tps,
            details,
        },
        None => StatsPayload {
            ok: true,
            show_stats: true,
            groups: Vec::new(),
            avg_tps: None,
            details: Vec::new(),
        },
    }
}

pub(crate) fn snapshot(config: &Config) -> StatsPayload {
    poll_once(config)
}

/// 调 dsh 后端 unary RPC（同源 POST /api/<method>，client-request 信封），
/// 成功返回 value；协议不匹配/服务未就绪一律 None。
fn rpc(config: &Config, method: &str, payload: serde_json::Value) -> Option<serde_json::Value> {
    let url = format!("http://127.0.0.1:{}/api/{method}", config.port);
    let resp = stats_agent()
        .post(&url)
        .send_json(serde_json::json!({
            "type": "client-request",
            "rpcId": format!("dshd-stats-{}", rpc_seq()),
            "method": method,
            "payload": payload,
        }))
        .ok()?;
    let json: serde_json::Value = resp.into_body().read_json().ok()?;
    let result = json.get("result")?;
    if result.get("ok")?.as_bool()? {
        result.get("value").cloned()
    } else {
        None
    }
}

fn rpc_seq() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQ: AtomicU64 = AtomicU64::new(0);
    SEQ.fetch_add(1, Ordering::Relaxed)
}

/// 推断当前展示会话：running 优先、其次 updatedAt 最新（与注入脚本
/// resolveAbsPath 的选取逻辑一致——dsh 页面当前打开的正是该会话）。
fn current_session(config: &Config) -> Option<(String, bool)> {
    let value = rpc(config, "session.list", serde_json::json!({}))?;
    let items = value.get("items")?.as_array()?;
    let mut best: Option<(&str, bool, f64)> = None;
    for item in items {
        let Some(sid) = item.get("sessionId").and_then(|v| v.as_str()) else {
            continue;
        };
        let running = item
            .get("running")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let updated = item
            .get("updatedAt")
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0);
        let replace = match best {
            None => true,
            Some((_, best_running, best_updated)) => {
                (running && !best_running) || (running == best_running && updated > best_updated)
            }
        };
        if replace {
            best = Some((sid, running, updated));
        }
    }
    best.map(|(sid, running, _)| (sid.to_string(), running))
}

pub(crate) fn current_session_id(config: &Config) -> Option<String> {
    current_session(config).map(|(id, _)| id)
}

/// 当前是否有正在执行的会话。Some(false) 也覆盖“会话列表为空”；
/// None 表示 RPC 不可用，维护任务应保守等待，避免误打断服务。
pub(crate) fn session_activity(config: &Config) -> Option<bool> {
    let value = rpc(config, "session.list", serde_json::json!({}))?;
    let items = value.get("items")?.as_array()?;
    if items.is_empty() {
        return Some(false);
    }
    Some(
        items
            .iter()
            .any(|item| item.get("running").and_then(|value| value.as_bool()) == Some(true)),
    )
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawSessionStats {
    turns: u64,
    steps: u64,
    llm_ms: f64,
    tool_ms: f64,
    ttft_ms: f64,
    ttft_steps: u64,
    decode_ms: f64,
    decode_tokens: f64,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawTokenUsage {
    uncached_input_tokens: f64,
    output_tokens: f64,
    cache_read_tokens: f64,
    cache_write_tokens: f64,
}

/// build_groups 的返回：显示组 / 平均速率 / 组级额外明细。
type BuiltGroups = (Vec<StatsGroup>, Option<f64>, Vec<StatsDetail>);

/// 组装显示组（与 dsh StatsLine 的分组逻辑一一对应）。
/// tok/s 不在组内——由 avg_tps 返回（前端实时速率优先、平均值回退）。
/// details 仅承载状态栏未显示的额外数据（如缓存拆分），tooltip 不重复已显示文本。
fn build_groups(config: &Config) -> Option<BuiltGroups> {
    let session_id = current_session_id(config)?;
    // maxMessages=1：投影挂在 tail page，最小页即可拿到，响应体极小
    let value = rpc(
        config,
        "session.history",
        serde_json::json!({ "sessionId": session_id, "maxMessages": 1 }),
    )?;
    let values = value.get("projections")?.get("values")?;
    let stats: Option<RawSessionStats> =
        serde_json::from_value(values.get("sessionStats").cloned()?).ok();
    let usage: Option<RawTokenUsage> =
        serde_json::from_value(values.get("tokenUsage").cloned()?).ok();

    let mut groups: Vec<StatsGroup> = Vec::new();
    let mut details: Vec<StatsDetail> = Vec::new();
    let group = |key: &'static str, text: String| StatsGroup { key, text };
    // details 只承载状态栏未显示的额外数据（如缓存拆分）——组文本已显示，
    // tooltip 不重复；无额外数据的组不产生 detail 条目
    let mut push_group = |key: &'static str, text: String, extra: Vec<String>| {
        groups.push(group(key, text));
        if !extra.is_empty() {
            details.push(StatsDetail {
                key: key.to_string(),
                lines: extra,
            });
        }
    };
    let mut avg_tps: Option<f64> = None;
    if let Some(stats) = stats {
        if stats.steps > 0 {
            let counts = crate::locale::owned(
                format!("{} 轮 · {} 步", stats.turns, stats.steps),
                format!(
                    "{} turn{} · {} step{}",
                    stats.turns,
                    if stats.turns == 1 { "" } else { "s" },
                    stats.steps,
                    if stats.steps == 1 { "" } else { "s" }
                ),
            );
            push_group("counts", counts, Vec::new());
            let mut durations = Vec::new();
            if stats.llm_ms > 0.0 {
                durations.push(format!("LLM {}", format_duration(stats.llm_ms)));
            }
            if stats.tool_ms > 0.0 {
                durations.push(crate::locale::owned(
                    format!("工具 {}", format_duration(stats.tool_ms)),
                    format!("tools {}", format_duration(stats.tool_ms)),
                ));
            }
            if !durations.is_empty() {
                push_group("durations", durations.join(" · "), Vec::new());
            }
            if stats.ttft_steps > 0 {
                let avg = stats.ttft_ms / stats.ttft_steps as f64;
                let text = crate::locale::owned(
                    format!("首 token {}", format_duration(avg)),
                    format!("first token {}", format_duration(avg)),
                );
                push_group("speeds", text, Vec::new());
            }
            if stats.decode_ms > 0.0 {
                avg_tps =
                    Some((stats.decode_tokens / (stats.decode_ms / 1000.0) * 10.0).round() / 10.0);
            }
        }
    }
    if let Some(usage) = usage {
        let billed =
            usage.uncached_input_tokens + usage.cache_read_tokens + usage.cache_write_tokens;
        if billed > 0.0 || usage.output_tokens > 0.0 {
            if billed > 0.0 {
                // 缓存命中封顶 99%（上游 dsh-status-bar 同款处理：四舍五入
                // 后可能出现 100% 而实际未全命中）
                let percent = ((usage.cache_read_tokens / billed * 100.0).round() as u64).min(99);
                let text = crate::locale::owned(
                    format!("缓存命中 {percent}%"),
                    format!("cache hit {percent}%"),
                );
                // 缓存拆分：命中率之外组内放不下的读/写/未命中明细
                let split = crate::locale::owned(
                    format!(
                        "缓存读 {} · 缓存写 {} · 未命中 {}",
                        format_tokens(usage.cache_read_tokens),
                        format_tokens(usage.cache_write_tokens),
                        format_tokens(usage.uncached_input_tokens)
                    ),
                    format!(
                        "cache read {} · cache write {} · uncached {}",
                        format_tokens(usage.cache_read_tokens),
                        format_tokens(usage.cache_write_tokens),
                        format_tokens(usage.uncached_input_tokens)
                    ),
                );
                push_group("cache", text, vec![split]);
            }
            let text = crate::locale::owned(
                format!(
                    "输入 {} tok · 输出 {} tok",
                    format_tokens(billed),
                    format_tokens(usage.output_tokens)
                ),
                format!(
                    "input {} tok · output {} tok",
                    format_tokens(billed),
                    format_tokens(usage.output_tokens)
                ),
            );
            push_group("tokens", text, Vec::new());
        }
    }
    Some((groups, avg_tps, details))
}

/// 紧凑时长：<60s 保留一位小数秒，之后 m+s（与 dsh 前端一致）。
pub(crate) fn format_duration(ms: f64) -> String {
    let s = ms / 1000.0;
    if s < 60.0 {
        format!("{:.1}s", (s * 10.0).round() / 10.0)
    } else {
        let whole = s.round() as u64;
        format!("{}m{}s", whole / 60, whole % 60)
    }
}

/// 紧凑 token 计数：517 / 12.2K / 517K / 1.2M（与 dsh 前端一致）。
pub(crate) fn format_tokens(n: f64) -> String {
    fn scaled(v: f64) -> String {
        if v >= 100.0 {
            format!("{}", v.round())
        } else {
            format!("{:.1}", (v * 10.0).round() / 10.0)
        }
    }
    if n < 1e3 {
        format!("{}", n.round())
    } else if n < 1e6 {
        format!("{}K", scaled(n / 1e3))
    } else {
        format!("{}M", scaled(n / 1e6))
    }
}

// —— 实时生成速率（live-rate）：尾帧解码会话日志，估算流式 tok/s ——

/// 实时速率统计窗口（毫秒）。
const LIVE_RATE_WINDOW_MS: i64 = 3000;
/// 实时速率轮询间隔。
const LIVE_RATE_INTERVAL: Duration = Duration::from_secs(2);
/// 当前会话 ID 缓存时长（避免 2s 一次 RPC）。
const SESSION_ID_TTL: Duration = Duration::from_secs(5);

/// 启动实时速率轮询（后台线程，退出中自动停止）。流式期间每 2s 广播
/// `live-rate-updated { tps }`；空闲时 tps 为 null（前端回落平均值）。
pub(crate) fn start_live_rate(app: AppHandle) {
    std::thread::spawn(move || {
        let mut cached_sid: Option<String> = None;
        let mut cached_at = std::time::Instant::now() - SESSION_ID_TTL;
        loop {
            std::thread::sleep(LIVE_RATE_INTERVAL);
            let state = app.state::<AppState>();
            if state.is_quitting() {
                return;
            }
            if state.service_ownership().is_external() {
                cached_sid = None;
                cached_at = std::time::Instant::now() - SESSION_ID_TTL;
                let _ = app.emit("live-rate-updated", serde_json::json!({ "tps": null }));
                continue;
            }
            if state.service_ownership() != crate::app_state::ServiceOwnership::Managed
                || state.phase() != crate::app_state::BootPhase::Ready
            {
                continue;
            }
            let config = state.config();
            if config.hide_statusbar || !config.hide_stats_line || !crate::main_is_visible(&app) {
                cached_sid = None;
                cached_at = std::time::Instant::now() - SESSION_ID_TTL;
                continue;
            }
            if cached_at.elapsed() >= SESSION_ID_TTL {
                cached_sid = current_session_id(&config);
                cached_at = std::time::Instant::now();
            }
            let tps = cached_sid
                .as_deref()
                .and_then(|sid| live_rate_once(sid, &config));
            let _ = app.emit("live-rate-updated", serde_json::json!({ "tps": tps }));
        }
    });
}

/// 一次实时速率采样：读当前会话日志最后一帧，统计最近 3s 的 token
/// 增量。会话空闲（窗口内无 delta 事件）返回 None。
fn live_rate_once(session_id: &str, config: &Config) -> Option<f64> {
    let text = read_tail_frame(session_id, config)?;
    let now_ms = unix_ms();
    live_rate_from_lines(&text, now_ms)
}

/// 读取会话日志最后一帧（dsh 流式追加 zstd = 多帧序列，只解码尾帧）。
/// 压缩数据内可能出现伪 magic 字节序列：解压失败时继续向前尝试
/// 下一个候选（最多 3 个），全部失败才放弃。
fn read_tail_frame(session_id: &str, config: &Config) -> Option<String> {
    let path = super::session_log_path(config, session_id)?;
    crate::session_log::read_tail_frame(&path).ok().flatten()
}

/// 从事件行文本计算最近窗口的实时速率（token 估算 = 字符数/4，
/// 与 dsh-status-bar 的 live-rate 折叠同款启发式）。
fn live_rate_from_lines(text: &str, now_ms: i64) -> Option<f64> {
    let window_start = now_ms - LIVE_RATE_WINDOW_MS;
    let mut tokens = 0u64;
    let mut first_ms: Option<i64> = None;
    let mut last_ms: Option<i64> = None;
    for line in text.lines() {
        let Ok(json) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if json.get("type").and_then(|v| v.as_str()) != Some("assistant/chunk") {
            continue;
        }
        let Some(ts) = json.get("time").and_then(|v| v.as_i64()) else {
            continue;
        };
        if ts < window_start || ts > now_ms {
            continue;
        }
        let Some(chunk) = json.get("data").and_then(|data| data.get("chunk")) else {
            continue;
        };
        let is_delta = chunk.get("type").and_then(|t| t.as_str()) == Some("delta");
        let text_len = chunk
            .get("text")
            .and_then(|t| t.as_str())
            .map(|t| t.chars().count() as u64)
            .unwrap_or(0);
        if !is_delta || text_len == 0 {
            continue;
        }
        tokens += text_len.div_ceil(4);
        first_ms = Some(first_ms.map_or(ts, |f| f.min(ts)));
        last_ms = Some(ts);
    }
    let (first, last) = (first_ms?, last_ms?);
    // 时间跨度下限 500ms：极短窗口（单次 flush 突发）会虚高
    let span = (last - first).max(500) as f64;
    Some((tokens as f64 / span * 1000.0 * 10.0).round() / 10.0)
}

fn unix_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn duration_formats_seconds_and_minutes() {
        assert_eq!(format_duration(45_230.0), "45.2s");
        assert_eq!(format_duration(59_900.0), "59.9s");
        assert_eq!(format_duration(60_000.0), "1m0s");
        assert_eq!(format_duration(32_020_000.0), "533m40s");
    }

    #[test]
    fn tokens_format_scaled() {
        assert_eq!(format_tokens(517.0), "517");
        assert_eq!(format_tokens(999.0), "999");
        assert_eq!(format_tokens(12_200.0), "12.2K");
        assert_eq!(format_tokens(517_000.0), "517K");
        assert_eq!(format_tokens(1_276_000_000.0), "1276M");
        assert_eq!(format_tokens(2_600_000.0), "2.6M");
    }

    #[test]
    fn tokens_format_rounds_below_three_digits() {
        // <100 保留一位小数；>=100 取整
        assert_eq!(format_tokens(96_500.0), "96.5K");
        assert_eq!(format_tokens(100_400.0), "100K");
        assert_eq!(format_tokens(96.5), "97");
    }

    #[test]
    fn live_rate_estimates_tokens_from_delta_chunks() {
        // 3 秒内 24 字符 → 6 token → 2 tok/s；500ms 下限防突发虚高
        let now = 1_700_000_000_000i64;
        let line = |ts: i64, text: &str| {
            format!(
                r#"{{"type":"assistant/chunk","time":{ts},"data":{{"turn":1,"step":1,"chunk":{{"type":"delta","text":"{text}"}}}}}}"#
            )
        };
        let text = format!(
            "{}\n{}\n{}",
            line(now - 2000, "abcdefgh"), // 8 字符 → 2 token
            line(now - 1000, "abcdefgh"), // 2 token
            line(now - 500, "abcdefgh"),  // 2 token
        );
        // span = 2000-500 = 1500ms，6 token → 4 tok/s
        let tps = live_rate_from_lines(&text, now).unwrap();
        assert!((tps - 4.0).abs() < 0.2, "tps={tps}");
    }

    #[test]
    fn live_rate_ignores_stale_and_non_delta_events() {
        let now = 1_700_000_000_000i64;
        let delta = |ts: i64, text: &str| {
            format!(
                r#"{{"type":"assistant/chunk","time":{ts},"data":{{"turn":1,"step":1,"chunk":{{"type":"delta","text":"{text}"}}}}}}"#
            )
        };
        let missing_data = format!(r#"{{"type":"assistant/chunk","time":{now}}}"#);
        let non_delta = format!(
            r#"{{"type":"assistant/chunk","time":{now},"data":{{"chunk":{{"type":"usage"}}}}}}"#
        );
        let text = format!(
            "{}\n{}\n{}\n{}",
            delta(now - 10_000, "abcdefgh"), // 窗口外：忽略
            missing_data,                    // 合法事件但缺 data：跳过，不能中止整帧
            non_delta,                       // 非 delta：忽略
            delta(now - 1000, "abcdefgh"),   // 窗口内：2 token
        );
        let tps = live_rate_from_lines(&text, now).unwrap();
        // span 下限 500ms，2 token → 4 tok/s
        assert!((tps - 4.0).abs() < 0.2, "tps={tps}");
    }
}
