//! 开发调试假数据（`DSH_BOX_FAKE_USAGE=1` 开启，配合 `dev-run.ps1 -FakeUsage`）。
//!
//! 目的：不配任何真实凭据/订阅也能把「用量与余额」的每个显示分支看全
//! ——余额卡（含预警色/未配置/未授权/不支持/stale）、订阅窗口（含重置
//! 时间）、用量报告（今日/本月/热力图/按模型/成本列，含未定价「—」）、
//! 状态栏统计与实时速率、当前会话上下文。
//!
//! 边界（不得破坏）：
//! - 只在环境变量显式开启时生效，生产环境不设置即完全休眠；
//! - **零网络请求、零缓存落盘**：不触发任何供应商接口，不写聚合缓存
//!   （报告经 `aggregate::render` 现算，成本经 `pricing::estimate_sample`
//!   实价估算——展示链路与真实数据完全同一条）；
//! - 数据确定性（除自然滚动的"今天"与 resets_at 时间），便于肉眼核对。

use std::collections::HashMap;

use super::aggregate::{self, render, Buckets, CostAcc, DayEntry};
use super::balance::{warn_of_balance, AccountSnapshot, Balance, QuotaWindow};
use super::pricing;
use super::subscriptions::{warn_of_windows, SubscriptionSnapshot};
use super::SessionContext;

const DAY_MS: i64 = 86_400_000;

/// 是否启用假数据（`1`/`true`，大小写不敏感；其余值/未设置一律关闭）。
pub(crate) fn enabled() -> bool {
    std::env::var("DSH_BOX_FAKE_USAGE")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn window(kind: &str, used_percent: f64, resets_at: Option<String>) -> QuotaWindow {
    QuotaWindow {
        kind: kind.to_string(),
        used_percent,
        remaining_percent: ((100.0 - used_percent) * 10.0).round() / 10.0,
        resets_at,
    }
}

/// 订阅卡的窗口（`subscriptions::QuotaWindow` 与余额侧是同形不同类型）。
fn sub_window(
    kind: &str,
    used_percent: f64,
    resets_at: Option<String>,
) -> super::subscriptions::QuotaWindow {
    super::subscriptions::QuotaWindow {
        kind: kind.to_string(),
        used_percent,
        remaining_percent: ((100.0 - used_percent) * 10.0).round() / 10.0,
        resets_at,
    }
}

/// ISO 时间（相对现在的偏移毫秒）。
fn iso_after(ms_from_now: i64) -> Option<String> {
    super::subscriptions::to_iso(Some((now_ms() + ms_from_now) as f64))
}

/// 假余额卡：覆盖 ok（CNY/USD、含明细行）、预警两档、unlimited、
/// 余额/窗口两种 mode、以及全部非 ok 状态的卡片样式。
pub(crate) fn accounts() -> Vec<AccountSnapshot> {
    let now = now_ms() as u64;
    let balance_card = |id: &str,
                        name: &str,
                        adapter: Option<&'static str>,
                        status: &'static str,
                        balance: Option<Balance>| AccountSnapshot {
        id: id.to_string(),
        display_name: name.to_string(),
        mode: "balance",
        adapter,
        status,
        balance: balance.clone(),
        windows: Vec::new(),
        error: None,
        updated_at: Some(now),
        stale: false,
        warn_level: balance.as_ref().map(warn_of_balance).unwrap_or("none"),
    };
    let cny =
        |remaining: f64, total: Option<f64>, granted: Option<f64>, topped: Option<f64>| Balance {
            remaining: Some(remaining),
            used: None,
            total,
            currency: "CNY".to_string(),
            unlimited: false,
            granted,
            topped_up: topped,
        };
    vec![
        balance_card(
            "deepseek-official",
            "DeepSeek",
            Some("deepseek-balance"),
            "ok",
            Some(cny(46.20, None, Some(10.00), Some(36.20))),
        ),
        balance_card(
            "openrouter",
            "OpenRouter",
            Some("openrouter-balance"),
            "ok",
            Some(Balance {
                remaining: Some(12.34),
                used: Some(7.66),
                total: Some(20.00),
                currency: "USD".to_string(),
                unlimited: false,
                granted: None,
                topped_up: None,
            }),
        ),
        balance_card(
            "orcarouter",
            "OrcaRouter",
            Some("orcarouter-balance"),
            "ok",
            Some(Balance {
                remaining: Some(88.80),
                used: Some(31.20),
                total: Some(120.00),
                currency: "USD".to_string(),
                unlimited: false,
                granted: None,
                topped_up: None,
            }),
        ),
        balance_card(
            "new-api",
            "New API 面板",
            Some("new-api"),
            "ok",
            Some(Balance {
                remaining: Some(63.50),
                used: Some(12.80),
                total: Some(76.30),
                currency: "CNY".to_string(),
                unlimited: false,
                granted: None,
                topped_up: None,
            }),
        ),
        // 预警两档：0.3 边界 warning、≤0.1 critical
        balance_card(
            "moonshotai",
            "Moonshot",
            Some("moonshot-balance"),
            "ok",
            Some(cny(8.10, Some(27.00), None, None)),
        ),
        balance_card(
            "zai",
            "Z.ai",
            Some("zai-balance"),
            "ok",
            Some(cny(2.97, Some(30.00), None, None)),
        ),
        // unlimited（∞ 展示）
        AccountSnapshot {
            id: "orcarouter-team".to_string(),
            display_name: "OrcaRouter 团队版".to_string(),
            mode: "balance",
            adapter: Some("orcarouter-balance"),
            status: "ok",
            balance: Some(Balance {
                remaining: Some(100_000_000.0),
                used: None,
                total: None,
                currency: "USD".to_string(),
                unlimited: true,
                granted: None,
                topped_up: None,
            }),
            windows: Vec::new(),
            error: None,
            updated_at: Some(now),
            stale: false,
            warn_level: "none",
        },
        // 窗口型余额卡（Sub2API 面板：quota + 5h 会话窗 + 周窗，含重置时间）
        AccountSnapshot {
            id: ("sub2api").to_string(),
            display_name: ("Sub2API").to_string(),
            mode: "subscription",
            adapter: Some("sub2api"),
            status: "ok",
            balance: None,
            windows: vec![
                window("quota", 40.0, iso_after(11 * 3_600_000)),
                window("session", 20.0, iso_after(2 * 3_600_000)),
                window("weekly", 72.0, iso_after(3 * DAY_MS)),
            ],
            error: None,
            updated_at: Some(now),
            stale: false,
            warn_level: "warning",
        },
        AccountSnapshot {
            id: ("my-relay").to_string(),
            display_name: ("自建中转").to_string(),
            mode: "balance",
            adapter: None,
            status: "unsupported",
            balance: None,
            windows: Vec::new(),
            error: Some(
                crate::locale::text(
                    "该供应商未提供公开余额查询接口。",
                    "This provider has no public balance query interface.",
                )
                .into(),
            ),
            updated_at: Some(now),
            stale: false,
            warn_level: "none",
        },
        AccountSnapshot {
            id: ("ollama").to_string(),
            display_name: ("Ollama").to_string(),
            mode: "subscription",
            adapter: None,
            status: "unsupported",
            balance: None,
            windows: Vec::new(),
            error: Some(
                crate::locale::text(
                    "本地 Ollama 无云端配额查询。",
                    "Local Ollama has no cloud quota endpoint.",
                )
                .into(),
            ),
            updated_at: Some(now),
            stale: false,
            warn_level: "none",
        },
        AccountSnapshot {
            id: ("minimax").to_string(),
            display_name: ("MiniMax Coding Plan").to_string(),
            mode: "subscription",
            adapter: Some("minimax-token-plan"),
            status: "not-configured",
            balance: None,
            windows: Vec::new(),
            error: Some(crate::locale::owned(
                "未找到凭据 MINIMAX_API_KEY。".to_string(),
                "Credential MINIMAX_API_KEY was not found.".to_string(),
            )),
            updated_at: Some(now),
            stale: false,
            warn_level: "none",
        },
        AccountSnapshot {
            id: ("kimi-coding").to_string(),
            display_name: ("Kimi For Coding").to_string(),
            mode: "subscription",
            adapter: None,
            status: "unauthorized",
            balance: None,
            windows: Vec::new(),
            error: Some("401 Unauthorized: invalid api key".to_string()),
            updated_at: Some(now),
            stale: false,
            warn_level: "none",
        },
    ]
}

/// 假订阅卡：五个适配器，覆盖 session/weekly/monthly/billing 窗口、
/// 重置时间、预警两档与 stale 标记。
pub(crate) fn subscriptions() -> Vec<SubscriptionSnapshot> {
    let sub_card = |id: &str,
                    name: &str,
                    adapter: &'static str,
                    plan: &str,
                    windows: Vec<super::subscriptions::QuotaWindow>,
                    stale: bool| SubscriptionSnapshot {
        id: id.to_string(),
        display_name: name.to_string(),
        mode: "subscription",
        adapter,
        status: "ok",
        plan: plan.to_string(),
        warn_level: warn_of_windows(&windows),
        windows,
        error: None,
        stale,
    };
    vec![
        sub_card(
            "zai",
            "Z.ai",
            "zai-token-plan",
            "GLM Max Monthly",
            vec![
                sub_window("session", 62.0, None),
                sub_window("weekly", 34.0, None),
                sub_window("billing", 12.0, iso_after(30 * DAY_MS)),
            ],
            false,
        ),
        sub_card(
            "kimi-coding",
            "Kimi For Coding",
            "kimi-token-plan",
            "Kimi For Coding",
            vec![
                sub_window("weekly", 25.0, None),
                sub_window("session", 80.0, iso_after(4 * 3_600_000)),
            ],
            false,
        ),
        sub_card(
            "minimax",
            "MiniMax Coding Plan",
            "minimax-token-plan",
            "MiniMax Coding Plan",
            vec![
                sub_window("session", 96.0, iso_after(35 * 60_000)),
                sub_window("weekly", 54.0, None),
            ],
            false,
        ),
        // stale：瞬错保旧的旧快照
        sub_card(
            "opencode-go",
            "OpenCode Go",
            "opencode-go",
            "Go",
            vec![
                sub_window("session", 40.0, None),
                sub_window("weekly", 10.0, None),
            ],
            true,
        ),
        sub_card(
            "ollama",
            "Ollama",
            "ollama",
            "Ollama",
            vec![
                sub_window("session", 12.0, None),
                sub_window("weekly", 5.0, None),
            ],
            false,
        ),
    ]
}

/// 假用量报告：走真实 `render` 管线（聚合、命中率、成本全真算）。
/// 当月各天全部官方 DeepSeek（成本完整可显示）；上一月留一天自定义路由
/// （未定价 → 该日与「累计」显示「—」，演示 fail-closed 语义）。
pub(crate) fn report() -> super::UsageReport {
    let now = now_ms();
    let mut days: HashMap<String, DayEntry> = HashMap::new();

    let mut add_model = |days_ago: i64,
                         model_key: &str,
                         input: u64,
                         cache_read: u64,
                         cache_write: u64,
                         output: u64| {
        // 采样时刻：当天内随 days_ago 错开，使峰/谷价在不同日自然混合。
        // 当天（days_ago=0）不回拨：午夜后半小时内回拨 30min 会落到昨天，
        // 破坏"今天必须有数据"的测试与开发期首屏数据
        let ts = now
            - days_ago * DAY_MS
            - (days_ago % 3) * 3_600_000
            - if days_ago == 0 { 0 } else { 1_800_000 };
        let buckets = Buckets {
            input,
            output,
            cache_read,
            cache_write,
        };
        let (provider, model) = model_key.split_once('/').unwrap_or(("unknown", model_key));
        let cost = pricing::estimate_sample(provider, model, ts, buckets);
        let key = aggregate::day_key(ts);
        let entry = days.entry(key).or_insert_with(|| DayEntry {
            totals: Buckets::default(),
            totals_cost: CostAcc::default(),
            models: HashMap::new(),
        });
        entry.totals.add_into(buckets);
        entry.totals_cost.add(cost);
        let me = entry.models.entry(model_key.to_string()).or_default();
        me.buckets.add_into(buckets);
        me.cost.add(cost);
    };

    // 当月及更早的官方 DeepSeek 天（含今天，保证「今日/本月」有数）
    for (i, days_ago) in [0i64, 1, 2, 3, 5, 7, 10, 13, 16, 20, 24, 28, 45]
        .into_iter()
        .enumerate()
    {
        let f = 1.0 + (i as f64 % 5.0) * 0.15;
        let (in_tok, cr, out) = (
            (180_000.0 * f) as u64,
            (1_200_000.0 * f) as u64,
            (52_000.0 * f) as u64,
        );
        add_model(days_ago, "deepseek/deepseek-v4-flash", in_tok, cr, 0, out);
        if i % 2 == 1 {
            add_model(
                days_ago,
                "deepseek/deepseek-v4-pro",
                (70_000.0 * f) as u64,
                (520_000.0 * f) as u64,
                0,
                (26_000.0 * f) as u64,
            );
        }
    }
    // 上一月的自定义路由天：未定价 → 「—」演示（不影响当月汇总）
    add_model(35, "custom-relay/gpt-x", 240_000, 310_000, 0, 61_000);

    render(&days, now as u64)
}

/// 假当前会话上下文（账户卡上的当前会话徽标）。
pub(crate) fn session_context() -> SessionContext {
    SessionContext {
        route_id: Some("deepseek-official".to_string()),
        display_name: Some("DeepSeek".to_string()),
        model: Some("deepseek-v4-pro".to_string()),
    }
}

/// 实时速率假值：18–52 tok/s 三角波（12s 周期），状态栏可见动态变化。
pub(crate) fn live_tps() -> f64 {
    let phase = (now_ms() % 12_000) as f64 / 12_000.0;
    let tri = 1.0 - (2.0 * phase - 1.0).abs();
    ((18.0 + 34.0 * tri) * 10.0).round() / 10.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enabled_off_by_default_and_parses_values() {
        let _guard = crate::credentials::ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let prev = std::env::var("DSH_BOX_FAKE_USAGE").ok();
        std::env::remove_var("DSH_BOX_FAKE_USAGE");
        assert!(!enabled());
        std::env::set_var("DSH_BOX_FAKE_USAGE", "TRUE");
        assert!(enabled());
        std::env::set_var("DSH_BOX_FAKE_USAGE", "0");
        assert!(!enabled());
        match prev {
            Some(v) => std::env::set_var("DSH_BOX_FAKE_USAGE", v),
            None => std::env::remove_var("DSH_BOX_FAKE_USAGE"),
        }
    }

    #[test]
    fn fake_accounts_cover_all_display_branches() {
        let accounts = accounts();
        let statuses: Vec<_> = accounts.iter().map(|a| a.status).collect();
        for expected in ["ok", "not-configured", "unauthorized", "unsupported"] {
            assert!(statuses.contains(&expected), "缺少状态 {expected}");
        }
        // 预警两档 + unlimited + 窗口型 + 含明细行的余额
        assert!(accounts.iter().any(|a| a.warn_level == "warning"));
        assert!(accounts.iter().any(|a| a.warn_level == "critical"));
        assert!(accounts
            .iter()
            .any(|a| a.balance.as_ref().is_some_and(|b| b.unlimited)));
        assert!(accounts
            .iter()
            .any(|a| a.mode == "subscription" && a.windows.iter().any(|w| w.resets_at.is_some())));
        assert!(accounts
            .iter()
            .any(|a| a.balance.as_ref().is_some_and(|b| b.granted.is_some())));
    }

    #[test]
    fn fake_subscriptions_cover_adapters_and_warn_levels() {
        let subs = subscriptions();
        assert_eq!(subs.len(), 5);
        assert!(subs.iter().any(|s| s.warn_level == "critical"));
        assert!(subs.iter().any(|s| s.stale));
        assert!(subs.iter().any(|s| s
            .windows
            .iter()
            .any(|w| w.kind == "billing" && w.resets_at.is_some())));
    }

    #[test]
    fn fake_report_mixes_priced_and_unpriced_days() {
        let report = report();
        assert!(!report.days.is_empty());
        let today = aggregate::day_key(now_ms());
        assert!(
            report.days.iter().any(|d| d.date == today),
            "今天必须有数据"
        );
        // 有可显示金额的天，也有 fail-closed「—」的天
        assert!(report
            .days
            .iter()
            .any(|d| d.cost_complete && d.cost_usd > 0.0));
        assert!(report.days.iter().any(|d| !d.cost_complete));
        // 总账因含未定价天而 fail-closed；按日升序（render 保证）
        assert!(!report.total.cost_complete);
        let dates: Vec<&str> = report.days.iter().map(|d| d.date.as_str()).collect();
        let mut sorted = dates.clone();
        sorted.sort();
        assert_eq!(dates, sorted);
    }

    #[test]
    fn live_tps_stays_in_wave_band() {
        for _ in 0..50 {
            let tps = live_tps();
            assert!((18.0..=52.0).contains(&tps), "tps={tps}");
        }
    }
}
