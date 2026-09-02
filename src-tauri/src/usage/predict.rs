//! 用量消耗速度预测（纯逻辑，无 IO）。
//!
//! 刻意不做小时内桶聚合：`FoldState` 的缓存结构与上游 dsh-usage-stats
//! 对齐是既有不变量，向 `DayEntry` 塞小时直方图会破坏该对齐；而「今日
//! 已用量 ÷ 已过小时数」的线性外推足以支撑当日预警这一目标（预测的是
//! 「今天大概会用到多少」，不是精确曲线）。
//!
//! 分母口径是「自本地午夜起的小时数」：预测对象是日历日的全天总量，
//! 深夜才开始使用的用户按剩余小时外推反而失真。elapsed 下限 0.5h：
//! 午夜后头几分钟的突发用量外推到全天会产生荒谬高值。

/// 预测输入（由调用方从聚合报告与当前时刻组装）。
#[derive(Clone, Debug, Default)]
pub struct PredictionInput {
    /// 今天已消耗 token。
    pub today_tokens: u64,
    /// 用户设置的每日 token 提醒阈值（None/0 = 关闭）。
    pub limit_tokens: Option<u64>,
}

/// 预测结果（序列化给前端；字段名是前端契约）。
#[derive(Clone, Debug, serde::Serialize, PartialEq)]
pub struct Prediction {
    /// 今日已过小时数（自本地午夜起，下限 0.5）。
    pub elapsed_hours: f64,
    /// 当前平均消耗速度（token/小时）。
    pub tokens_per_hour: f64,
    /// 线性外推的今日全天 token 预计值。
    pub projected_today_tokens: u64,
    /// 预测是否已越过用户阈值（阈值关闭时恒 false）。
    pub limit_exceeded: bool,
}

/// 最低可信外推时长：不足时不开预测（避免晨间突发被放大成全天巨值）。
const MIN_ELAPSED_HOURS: f64 = 0.5;

/// 距本地午夜已过的毫秒数（用 day_key 同款时区口径，跨 DST 边界由
/// 进程内缓存偏移决定，误差可接受）。
pub(crate) fn elapsed_since_local_midnight_ms(now_ms: i64) -> i64 {
    let offset = super::aggregate::local_offset_seconds();
    let local_secs = now_ms.div_euclid(1000) + offset;
    let midnight_local_secs = local_secs.div_euclid(86_400) * 86_400;
    (local_secs - midnight_local_secs) * 1000
}

/// 计算预测：今天尚无用量时返回 None（调用方按「无预测」呈现，不显示
/// 误导性的 0）。阈值对 `limit_exceeded` 的判定在 `with_limit` 里按最新
/// 配置重算，缓存中的旧判定不会泄漏给前端。
pub fn predict(input: &PredictionInput, now_ms: i64) -> Option<Prediction> {
    if input.today_tokens == 0 {
        return None;
    }
    let since_ms = elapsed_since_local_midnight_ms(now_ms).max(0);
    let elapsed_hours = (since_ms as f64 / 3_600_000.0).max(MIN_ELAPSED_HOURS);
    let rate = input.today_tokens as f64 / elapsed_hours;
    let projected = rate * 24.0;
    Some(Prediction {
        elapsed_hours: (elapsed_hours * 10.0).round() / 10.0,
        tokens_per_hour: rate,
        projected_today_tokens: projected.round() as u64,
        limit_exceeded: input
            .limit_tokens
            .is_some_and(|limit| limit > 0 && projected >= limit as f64),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW_MS: i64 = 1_786_000_000_000;

    fn input(today_tokens: u64, limit: Option<u64>) -> PredictionInput {
        PredictionInput {
            today_tokens,
            limit_tokens: limit,
        }
    }

    #[test]
    fn no_usage_today_yields_no_prediction() {
        assert!(predict(&input(0, Some(1_000_000)), NOW_MS).is_none());
    }

    #[test]
    fn elapsed_floor_prevents_morning_blowup() {
        // 已知自午夜起的时长（按本机时区计算），验证下限与外推公式；
        // 构造一个「自午夜起 0ms」的时刻：取当天本地午夜 + 1ms。
        let offset = super::super::aggregate::local_offset_seconds();
        let local_secs = NOW_MS.div_euclid(1000) + offset;
        let midnight_ms = (local_secs.div_euclid(86_400) * 86_400 - offset) * 1000;
        let p = predict(&input(1_000, None), midnight_ms + 1).unwrap();
        assert_eq!(p.elapsed_hours, 0.5);
        assert_eq!(p.tokens_per_hour, 2_000.0);
        assert_eq!(p.projected_today_tokens, 48_000);
    }

    #[test]
    fn projection_scales_with_elapsed() {
        // 同样的已用量，6 小时后外推减半：rate = tokens/elapsed、projected = rate*24
        let early = predict(&input(600_000, None), NOW_MS).unwrap();
        let base = elapsed_since_local_midnight_ms(NOW_MS) as f64 / 3_600_000.0;
        let expected_rate = 600_000.0 / base.max(0.5);
        assert!((early.tokens_per_hour - expected_rate).abs() < 1e-6);
        assert_eq!(
            early.projected_today_tokens,
            (expected_rate * 24.0).round() as u64
        );
    }

    #[test]
    fn limit_exceeded_follows_projection() {
        // 阈值 0（关闭）恒不越限；超大阈值恒不越限；极小阈值必越限
        assert!(
            !predict(&input(1_000_000, None), NOW_MS)
                .unwrap()
                .limit_exceeded
        );
        assert!(
            !predict(&input(1_000_000, Some(0)), NOW_MS)
                .unwrap()
                .limit_exceeded
        );
        assert!(
            !predict(&input(1_000_000, Some(u64::MAX)), NOW_MS)
                .unwrap()
                .limit_exceeded
        );
        assert!(
            predict(&input(1_000_000, Some(1)), NOW_MS)
                .unwrap()
                .limit_exceeded
        );
    }

    #[test]
    fn elapsed_since_midnight_is_non_negative_and_sub_daily() {
        let e = elapsed_since_local_midnight_ms(NOW_MS);
        assert!((0..86_400_000).contains(&e));
    }
}
