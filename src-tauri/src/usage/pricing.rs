//! DeepSeek 官方历史定价引擎（上游 v0.3.1 `lib/pricing.js` 的 Rust 移植）。
//!
//! 定价事实（规则目录、时间带、生效边界）是公开 API 事实，按 MIT 来源
//! 移植（见 `THIRD_PARTY_NOTICES.md`）；结构以 Rust 静态表重写。
//!
//! 语义要点：
//! - 只给官方 DeepSeek 身份定价：日志归因 `provider == "deepseek"`。上游
//!   还会校验路由 baseURL 主机名是否 `api.deepseek.com`；本壳折叠时拿不到
//!   baseURL，取日志归因为准（自定义路由把 provider 写成 `deepseek` 且
//!   模型名恰好是 v4 官方名才会误报，见 docs/usage-sync.md 分歧清单）。
//! - cacheWrite 无官方单价（表值为无价）：样本 cache_write > 0 即整笔
//!   `complete = false`（不静默低估）。
//! - 峰谷按「事件时刻的上海钟面」判定：上海 1991 年后无夏令时，固定
//!   UTC+8，无需时区库；定价时区与本地时区无关（日聚合仍按本地时区）。
//! - 官方调价或规则目录变更时：更新本表并**递增 `usage/cache.rs` 的
//!   CACHE_VERSION**（旧缓存静默重折，成本随 token 一起重算）。

/// 上海时区固定 UTC+8（1991 年后无夏令时）。
const SHANGHAI_OFFSET_MS: i64 = 8 * 3_600_000;
const MS_PER_DAY: i64 = 86_400_000;
/// 时间带 v1 起点：2026-08-16T16:00:00Z（此后峰谷双价）。
const TIME_BAND_V1_FROM_MS: i64 = 1_786_896_000_000;
/// 工作日排程起点：2026-08-22T16:00:00Z（此后仅工作日高峰）。
const WEEKDAY_SCHEDULE_FROM_MS: i64 = 1_787_414_400_000;
const TOKENS_PER_MILLION: f64 = 1_000_000.0;

/// 上海高峰时段（钟面分钟，半开区间）：09:00–12:00 与 14:00–18:00。
const PEAK_WINDOWS: [(i64, i64); 2] = [(9 * 60, 12 * 60), (14 * 60, 18 * 60)];

/// 每百万 token 单价（USD）。`cache_write: None` 表示无官方单价。
#[derive(Clone, Copy)]
struct UnitPrices {
    input: f64,
    cache_read: f64,
    cache_write: Option<f64>,
    output: f64,
}

struct PricingRule {
    /// 规则溯源 id（目录校验测试读；调价对账时也能对上官方条款）。
    #[allow(dead_code)]
    id: &'static str,
    model: &'static str,
    /// 生效边界（毫秒 epoch）：from 含、to 不含。
    effective_from: Option<i64>,
    effective_to: Option<i64>,
    /// 平价规则（无时间带）。
    flat: Option<UnitPrices>,
    off_peak: Option<UnitPrices>,
    peak: Option<UnitPrices>,
    /// 高峰星期（0=周日..6=周六）；空 = 平价规则。
    peak_days: &'static [u8],
}

const ALL_DAYS: &[u8] = &[0, 1, 2, 3, 4, 5, 6];
const WEEKDAYS: &[u8] = &[1, 2, 3, 4, 5];

/// 官方 DeepSeek USD 规则目录（每百万 token 单价；来源
/// <https://api-docs.deepseek.com/quick_start/pricing/>，目录时点 2026-08-23）。
const RULES: &[PricingRule] = &[
    PricingRule {
        id: "deepseek-v4-flash-usd-flat-before-2026-08-16",
        model: "deepseek-v4-flash",
        effective_from: None,
        effective_to: Some(TIME_BAND_V1_FROM_MS),
        flat: Some(UnitPrices {
            input: 0.14,
            cache_read: 0.0028,
            cache_write: None,
            output: 0.28,
        }),
        off_peak: None,
        peak: None,
        peak_days: &[],
    },
    PricingRule {
        id: "deepseek-v4-pro-usd-flat-before-2026-08-16",
        model: "deepseek-v4-pro",
        effective_from: None,
        effective_to: Some(TIME_BAND_V1_FROM_MS),
        flat: Some(UnitPrices {
            input: 0.435,
            cache_read: 0.003625,
            cache_write: None,
            output: 0.87,
        }),
        off_peak: None,
        peak: None,
        peak_days: &[],
    },
    PricingRule {
        id: "deepseek-v4-flash-usd-time-band-v1",
        model: "deepseek-v4-flash",
        effective_from: Some(TIME_BAND_V1_FROM_MS),
        effective_to: Some(WEEKDAY_SCHEDULE_FROM_MS),
        flat: None,
        off_peak: Some(UnitPrices {
            input: 0.22,
            cache_read: 0.007,
            cache_write: None,
            output: 0.66,
        }),
        peak: Some(UnitPrices {
            input: 0.44,
            cache_read: 0.014,
            cache_write: None,
            output: 1.32,
        }),
        peak_days: ALL_DAYS,
    },
    PricingRule {
        id: "deepseek-v4-pro-usd-time-band-v1",
        model: "deepseek-v4-pro",
        effective_from: Some(TIME_BAND_V1_FROM_MS),
        effective_to: Some(WEEKDAY_SCHEDULE_FROM_MS),
        flat: None,
        off_peak: Some(UnitPrices {
            input: 0.66,
            cache_read: 0.022,
            cache_write: None,
            output: 1.98,
        }),
        peak: Some(UnitPrices {
            input: 1.32,
            cache_read: 0.044,
            cache_write: None,
            output: 3.96,
        }),
        peak_days: ALL_DAYS,
    },
    PricingRule {
        id: "deepseek-v4-flash-usd-weekday-schedule",
        model: "deepseek-v4-flash",
        effective_from: Some(WEEKDAY_SCHEDULE_FROM_MS),
        effective_to: None,
        flat: None,
        off_peak: Some(UnitPrices {
            input: 0.22,
            cache_read: 0.007,
            cache_write: None,
            output: 0.66,
        }),
        peak: Some(UnitPrices {
            input: 0.44,
            cache_read: 0.014,
            cache_write: None,
            output: 1.32,
        }),
        peak_days: WEEKDAYS,
    },
    PricingRule {
        id: "deepseek-v4-pro-usd-weekday-schedule",
        model: "deepseek-v4-pro",
        effective_from: Some(WEEKDAY_SCHEDULE_FROM_MS),
        effective_to: None,
        flat: None,
        off_peak: Some(UnitPrices {
            input: 0.66,
            cache_read: 0.022,
            cache_write: None,
            output: 1.98,
        }),
        peak: Some(UnitPrices {
            input: 1.32,
            cache_read: 0.044,
            cache_write: None,
            output: 3.96,
        }),
        peak_days: WEEKDAYS,
    },
];

/// 单样本成本：`counted` 表示是否计入成本账（token 数 > 0 才计）；
/// `complete == false` 表示有量但无可信单价（渲染为「—」而不是低估）。
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct SampleCost {
    pub usd: f64,
    pub complete: bool,
    pub counted: bool,
}

/// 单样本成本估算（不取整；上游 estimateTokenCost 同款）。
pub(crate) fn estimate_sample(
    provider: &str,
    model: &str,
    time_ms: i64,
    b: super::aggregate::Buckets,
) -> SampleCost {
    if b.total() == 0 {
        return SampleCost {
            usd: 0.0,
            complete: true,
            counted: false,
        };
    }
    let Some(rule) = match_rule(provider, model, time_ms) else {
        return SampleCost {
            usd: 0.0,
            complete: false,
            counted: true,
        };
    };
    let prices = resolve_tariff(rule, time_ms);
    if b.cache_write > 0 && prices.cache_write.is_none() {
        return SampleCost {
            usd: 0.0,
            complete: false,
            counted: true,
        };
    }
    let per_million = |tokens: u64, price: f64| tokens as f64 / TOKENS_PER_MILLION * price;
    let usd = per_million(b.input, prices.input)
        + per_million(b.cache_read, prices.cache_read)
        + per_million(b.cache_write, prices.cache_write.unwrap_or(0.0))
        + per_million(b.output, prices.output);
    SampleCost {
        usd,
        complete: true,
        counted: true,
    }
}

/// 匹配 (provider, model, 时刻) 的唯一规则；无则 None。
fn match_rule(provider: &str, model: &str, time_ms: i64) -> Option<&'static PricingRule> {
    if provider != "deepseek" {
        return None;
    }
    RULES.iter().find(|rule| {
        rule.model == model
            && rule.effective_from.is_none_or(|from| time_ms >= from)
            && rule.effective_to.is_none_or(|to| time_ms < to)
    })
}

/// 解析规则在指定时刻的适用单价（平价或按上海钟面峰谷）。
fn resolve_tariff(rule: &PricingRule, time_ms: i64) -> UnitPrices {
    if let Some(flat) = rule.flat {
        return flat;
    }
    let local = time_ms + SHANGHAI_OFFSET_MS;
    let days = local.div_euclid(MS_PER_DAY);
    let secs_of_day = local.rem_euclid(MS_PER_DAY) / 1000;
    // 1970-01-01 是周四（0=周日）：days=0 → weekday=4
    let weekday = (days + 4).rem_euclid(7) as u8;
    let minute = secs_of_day / 60;
    let in_peak = rule.peak_days.contains(&weekday)
        && PEAK_WINDOWS
            .iter()
            .any(|&(start, end)| minute >= start && minute < end);
    if in_peak {
        rule.peak
            .unwrap_or_else(|| rule.off_peak.expect("时间带规则必有 peak 单价"))
    } else {
        rule.off_peak.expect("时间带规则必有 offPeak 单价")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn buckets(
        input: u64,
        output: u64,
        cache_read: u64,
        cache_write: u64,
    ) -> super::super::aggregate::Buckets {
        super::super::aggregate::Buckets {
            input,
            output,
            cache_read,
            cache_write,
        }
    }

    /// 上海钟面换算的锚点：2026-08-17T00:30:00Z = 上海 08:30（周一）。
    const SHANGHAI_0830_MONDAY_MS: i64 = TIME_BAND_V1_FROM_MS + 8 * 3_600_000 + 1_800_000;

    #[test]
    fn catalog_rules_are_complete_and_non_overlapping() {
        // 每条规则：平价与时间带互斥、时间带必有峰谷单价与高峰日
        for rule in RULES {
            let has_flat = rule.flat.is_some();
            let has_bands =
                rule.off_peak.is_some() && rule.peak.is_some() && !rule.peak_days.is_empty();
            assert!(
                has_flat != has_bands,
                "规则 {} 必须是平价或时间带之一",
                rule.id
            );
            if let (Some(from), Some(to)) = (rule.effective_from, rule.effective_to) {
                assert!(from < to, "规则 {} 边界倒置", rule.id);
            }
        }
        // 同模型规则生效窗口不得重叠
        for (i, a) in RULES.iter().enumerate() {
            for b in RULES.iter().skip(i + 1) {
                if a.model != b.model {
                    continue;
                }
                let a_from = a.effective_from.unwrap_or(i64::MIN);
                let a_to = a.effective_to.unwrap_or(i64::MAX);
                let b_from = b.effective_from.unwrap_or(i64::MIN);
                let b_to = b.effective_to.unwrap_or(i64::MAX);
                let overlap = a_from.max(b_from) < a_to.min(b_to);
                assert!(!overlap, "{} 与 {} 生效窗口重叠", a.id, b.id);
            }
        }
    }

    #[test]
    fn boundary_constants_match_their_iso_instants() {
        // 2026-08-16T16:00:00Z 与 2026-08-22T16:00:00Z（上游常量的 epoch 毫秒）
        assert_eq!(TIME_BAND_V1_FROM_MS % MS_PER_DAY, 16 * 3_600_000);
        assert_eq!(
            super::super::subscriptions::to_iso(Some(TIME_BAND_V1_FROM_MS as f64)).as_deref(),
            Some("2026-08-16T16:00:00Z")
        );
        assert_eq!(
            super::super::subscriptions::to_iso(Some(WEEKDAY_SCHEDULE_FROM_MS as f64)).as_deref(),
            Some("2026-08-22T16:00:00Z")
        );
    }

    #[test]
    fn flat_rule_prices_before_time_band_v1() {
        // v1 前：flash 平价 1M input = 0.14 USD
        let t = TIME_BAND_V1_FROM_MS - 1;
        let cost = estimate_sample(
            "deepseek",
            "deepseek-v4-flash",
            t,
            buckets(1_000_000, 0, 0, 0),
        );
        assert!(cost.counted && cost.complete);
        assert!((cost.usd - 0.14).abs() < 1e-9);
        // cacheRead 1M = 0.0028
        let cost = estimate_sample(
            "deepseek",
            "deepseek-v4-flash",
            t,
            buckets(0, 0, 1_000_000, 0),
        );
        assert!((cost.usd - 0.0028).abs() < 1e-9);
    }

    #[test]
    fn time_band_resolves_peak_and_off_peak() {
        // 上海 08:30 周一 = 谷价（高峰 09:00 起）
        let off = estimate_sample(
            "deepseek",
            "deepseek-v4-flash",
            SHANGHAI_0830_MONDAY_MS,
            buckets(1_000_000, 0, 0, 0),
        );
        assert!((off.usd - 0.22).abs() < 1e-9, "usd={}", off.usd);
        // 上海 10:00 周一 = 峰价（v1 期全周高峰）
        let peak_time = SHANGHAI_0830_MONDAY_MS + 90 * 60_000;
        let peak = estimate_sample(
            "deepseek",
            "deepseek-v4-flash",
            peak_time,
            buckets(1_000_000, 0, 0, 0),
        );
        assert!((peak.usd - 0.44).abs() < 1e-9, "usd={}", peak.usd);
        // 中午 12:00–14:00 间隙回谷价
        let noon_gap = SHANGHAI_0830_MONDAY_MS + 210 * 60_000;
        let gap = estimate_sample(
            "deepseek",
            "deepseek-v4-flash",
            noon_gap,
            buckets(1_000_000, 0, 0, 0),
        );
        assert!((gap.usd - 0.22).abs() < 1e-9, "usd={}", gap.usd);
    }

    #[test]
    fn weekday_schedule_skips_weekend_peak() {
        // WEEKDAY_SCHEDULE_FROM 的上海钟面 = 2026-08-23T00:00（周日）。
        // 周日 10:00（原高峰时段）在排程期不再是峰价
        let sunday_peak_clock = WEEKDAY_SCHEDULE_FROM_MS + 10 * 3_600_000;
        let cost = estimate_sample(
            "deepseek",
            "deepseek-v4-flash",
            sunday_peak_clock,
            buckets(1_000_000, 0, 0, 0),
        );
        assert!(
            (cost.usd - 0.22).abs() < 1e-9,
            "周日应为谷价 usd={}",
            cost.usd
        );
        // 周一 10:00 上海（排程期）仍是峰价
        let monday_peak_clock = WEEKDAY_SCHEDULE_FROM_MS + MS_PER_DAY + 10 * 3_600_000;
        let cost = estimate_sample(
            "deepseek",
            "deepseek-v4-flash",
            monday_peak_clock,
            buckets(1_000_000, 0, 0, 0),
        );
        assert!(
            (cost.usd - 0.44).abs() < 1e-9,
            "周一应为峰价 usd={}",
            cost.usd
        );
    }

    #[test]
    fn cache_write_tokens_make_estimate_incomplete() {
        let t = TIME_BAND_V1_FROM_MS - 1;
        let cost = estimate_sample("deepseek", "deepseek-v4-pro", t, buckets(100, 100, 0, 1));
        assert!(cost.counted);
        assert!(!cost.complete, "cacheWrite 无官方价 → 整笔 unknown");
        assert_eq!(cost.usd, 0.0);
    }

    #[test]
    fn unknown_provider_or_model_is_incomplete_not_fake_priced() {
        let t = TIME_BAND_V1_FROM_MS - 1;
        for (provider, model) in [
            ("unknown", "deepseek-v4-flash"),
            ("deepseek", "deepseek-v3"),
            ("custom", "gpt-5"),
        ] {
            let cost = estimate_sample(provider, model, t, buckets(100, 100, 0, 0));
            assert!(cost.counted && !cost.complete, "{provider}/{model}");
        }
    }

    #[test]
    fn zero_token_sample_is_not_counted() {
        let cost = estimate_sample("deepseek", "deepseek-v4-flash", 0, buckets(0, 0, 0, 0));
        assert!(!cost.counted);
        assert!(cost.complete);
    }
}
