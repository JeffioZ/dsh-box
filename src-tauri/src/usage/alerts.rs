//! 用量预警后台任务：周期聚合本地日志 → 线性外推今日全天用量 →
//! 预计越过用户阈值时发一次系统通知（每天至多一次）。
//!
//! 门控与账户监测一致（仅本地托管且 Ready；外部服务的日志不归本壳）。
//! 预测投影缓存进进程内静态供 `usage_prediction_get` 秒回；阈值越限
//! 判定不缓存——读取时按最新配置重算，用户刚改完阈值立刻生效。
//! 聚合每 10 分钟一轮与用量页手动刷新并发，`usage::report` 内部有
//! 进程内锁串行（折叠增量游标不丢）。

use std::sync::Mutex;
use std::time::Duration;

use tauri::{AppHandle, Manager};

use crate::app_state::{AppState, Config};

use super::predict::{self, Prediction, PredictionInput};

/// 聚合与预测周期。日志折叠是增量的，一轮代价 O(新增事件)。
const ALERT_REFRESH_MS: Duration = Duration::from_secs(600);
const GATE_POLL: Duration = Duration::from_secs(5);

/// 最近一次预测投影（None = 本进程尚未算过）。只存与阈值无关的投影，
/// `limit_exceeded` 按读取时的配置重算。
static LAST_PROJECTION: Mutex<Option<Prediction>> = Mutex::new(None);

/// `usage-prediction-updated` 事件与 get 命令的载荷（前端契约）。
#[derive(serde::Serialize, Clone)]
pub struct Payload {
    pub prediction: Option<Prediction>,
    /// 用户阈值（token；None = 关闭）。
    pub limit_tokens: Option<u64>,
}

/// 今天已因越限发过通知（存日 key；跨天自动允许再提醒）。
static ALERTED_DAY: Mutex<Option<String>> = Mutex::new(None);

pub(crate) fn start_usage_alerts(app: AppHandle) {
    crate::background::spawn_gated_periodic(
        app,
        "usage-alerts",
        ALERT_REFRESH_MS,
        GATE_POLL,
        run_round,
    );
}

fn limit_of(config: &Config) -> Option<u64> {
    config
        .usage_token_limit_m
        .filter(|m| *m > 0)
        .map(|m| m.saturating_mul(1_000_000))
}

/// 缓存投影 + 当前阈值 → 载荷（越限判定按最新配置重算）。
pub(crate) fn cached_payload(config: &Config) -> Payload {
    let limit = limit_of(config);
    let projection = LAST_PROJECTION.lock().ok().and_then(|g| g.clone());
    Payload {
        prediction: projection.map(|mut p| {
            p.limit_exceeded = limit.is_some_and(|l| l > 0 && p.projected_today_tokens >= l);
            p
        }),
        limit_tokens: limit,
    }
}

fn run_round(app: &AppHandle) {
    let config = app.state::<AppState>().config();
    let Ok(report) = super::report(&config) else {
        return;
    };
    let today = super::day_key_now();
    let input = build_input(&report, &today, &config);
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);
    let projection = predict::predict(&input, now_ms);
    if let Ok(mut guard) = LAST_PROJECTION.lock() {
        *guard = projection.clone();
    }
    crate::emit_signed(app, "usage-prediction-updated", &cached_payload(&config));
    maybe_alert(app, &input, &projection, &today);
}

/// 从 wire 报告还原预测输入（报告已合并多会话；today 条目缺失 = 今日 0）。
fn build_input(report: &super::UsageReport, today: &str, config: &Config) -> PredictionInput {
    PredictionInput {
        today_tokens: report
            .days
            .iter()
            .find(|d| d.date == today)
            .map(|d| d.tokens)
            .unwrap_or(0),
        limit_tokens: limit_of(config),
    }
}

/// 越限提醒：预测值越过阈值且今天未提醒过 → 发一次通知。
/// 主窗口可见时也发（与任务完成通知「不可见才发」不同：预警的意义就是
/// 打断），点击通知唤起主窗口（notify 模块统一行为）。
/// 发送（COM 调用可能阻塞）不持锁：先无锁查重，成功后再落「已提醒」。
fn maybe_alert(
    app: &AppHandle,
    input: &PredictionInput,
    projection: &Option<Prediction>,
    today: &str,
) {
    let Some(limit) = input.limit_tokens else {
        return;
    };
    let exceeded = projection
        .as_ref()
        .is_some_and(|p| p.projected_today_tokens >= limit)
        || input.today_tokens >= limit;
    if !exceeded {
        return;
    }
    if ALERTED_DAY
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_deref()
        == Some(today)
    {
        return;
    }
    let projected = projection
        .as_ref()
        .map(|p| p.projected_today_tokens)
        .unwrap_or(input.today_tokens);
    if crate::notify::notify(
        app,
        crate::locale::text("今日用量预警", "Daily usage alert"),
        &crate::locale::owned(
            format!("今日预计消耗 {projected} token，已越过你设置的 {limit} 提醒阈值。"),
            format!(
                "Today's projected usage is {projected} tokens, past your {limit} alert threshold."
            ),
        ),
    )
    .is_ok()
    {
        let mut alerted = ALERTED_DAY.lock().unwrap_or_else(|e| e.into_inner());
        if alerted.as_deref() != Some(today) {
            *alerted = Some(today.to_string());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn day_report(today: &str, tokens: u64) -> super::super::UsageReport {
        let zero_buckets = || super::super::aggregate::BucketReport {
            input_tokens: 0,
            output_tokens: 0,
            cache_read_tokens: 0,
            cache_write_tokens: 0,
        };
        let day = super::super::aggregate::DayReport {
            date: today.to_string(),
            buckets: zero_buckets(),
            tokens,
            cache_hit_rate: None,
            cost_usd: 0.0,
            cost_complete: true,
            models: Vec::new(),
        };
        super::super::UsageReport {
            days: vec![day],
            total: super::super::aggregate::TotalReport {
                buckets: zero_buckets(),
                tokens,
                cache_hit_rate: None,
                cost_usd: 0.0,
                cost_complete: true,
            },
            updated_at: 0,
        }
    }

    #[test]
    fn build_input_maps_today_tokens_and_limit() {
        let mut config = Config::load();
        config.usage_token_limit_m = Some(5);
        let input = build_input(&day_report("2026-09-02", 1_234), "2026-09-02", &config);
        assert_eq!(input.today_tokens, 1_234);
        assert_eq!(input.limit_tokens, Some(5_000_000));
        // 0 视为关闭
        config.usage_token_limit_m = Some(0);
        assert_eq!(
            build_input(&day_report("d", 0), "d", &config).limit_tokens,
            None
        );
        // 今天无条目 = 0
        assert_eq!(
            build_input(&day_report("d", 9), "other", &config).today_tokens,
            0
        );
    }

    #[test]
    fn cached_payload_falls_back_to_limit_only() {
        let mut config = Config::load();
        config.usage_token_limit_m = Some(2);
        let payload = cached_payload(&config);
        assert!(payload.prediction.is_none());
        assert_eq!(payload.limit_tokens, Some(2_000_000));
    }
}
