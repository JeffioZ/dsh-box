//! 用量报告导出（无密钥）：每日明细 CSV 与全量 JSON。
//!
//! 参考 dsh-usage-stats `lib/export.js` 的导出投影（UTF-8 BOM、RFC 4180
//! 转义、CSV 公式注入防护、schema 版本化），以 Rust 独立实现。刻意分歧：
//! 不做 `sessions.csv`——按会话级数据（首末活动、会话集合）不在聚合缓存
//! 中，为导出扩缓存不划算；需要时从 JSON 的按日数据已可分析。

use super::aggregate::{DayReport, UsageReport};

/// CSV 单元格：RFC 4180 转义 + 公式注入防护（`=`/`+`/`-`/`@` 开头前缀 `'`，
/// 防表格软件把数据当公式执行）。
fn csv_cell(raw: &str) -> String {
    let guarded = if raw.starts_with(['=', '+', '-', '@']) {
        format!("'{raw}")
    } else {
        raw.to_string()
    };
    if guarded.contains(',') || guarded.contains('"') || guarded.contains('\n') {
        format!("\"{}\"", guarded.replace('"', "\"\""))
    } else {
        guarded
    }
}

/// 每日×模型明细 CSV（UTF-8 BOM，便于 Excel 直接打开）。
pub(crate) fn daily_csv(report: &UsageReport) -> String {
    let mut out = String::from("\u{FEFF}");
    out.push_str("date,provider,model,input tokens,cache read,cache write,output,total,cache hit %,est. cost (USD),cost complete\r\n");
    for day in &report.days {
        for model in &day.models {
            // 成本仅在完整可信时输出金额（fail-closed，不输出低估值）
            let cost = if model.cost_complete && model.cost_usd > 0.0 {
                format!("{:.4}", model.cost_usd)
            } else {
                String::new()
            };
            let mut provider = model.model.as_str();
            let model_name = match model.model.split_once('/') {
                Some((p, m)) => {
                    provider = p;
                    m
                }
                None => "unknown",
            };
            out.push_str(&csv_cell(&day.date));
            out.push(',');
            out.push_str(&csv_cell(provider));
            out.push(',');
            out.push_str(&csv_cell(model_name));
            out.push_str(&format!(
                ",{},{},{},{},{},{},{},{}\r\n",
                model.buckets.input_tokens,
                model.buckets.cache_read_tokens,
                model.buckets.cache_write_tokens,
                model.buckets.output_tokens,
                model.tokens,
                model
                    .cache_hit_rate
                    .map(|v| v.to_string())
                    .unwrap_or_default(),
                cost,
                if model.cost_complete { "yes" } else { "no" },
            ));
        }
    }
    out
}

/// 全量导出 JSON（schema 版本化；字段 snake_case，前端/脚本可直接消费）。
pub(crate) fn export_json(report: &UsageReport) -> String {
    let days: Vec<serde_json::Value> = report.days.iter().map(day_json).collect();
    let total = serde_json::json!({
        "input_tokens": report.total.buckets.input_tokens,
        "cache_read_tokens": report.total.buckets.cache_read_tokens,
        "cache_write_tokens": report.total.buckets.cache_write_tokens,
        "output_tokens": report.total.buckets.output_tokens,
        "tokens": report.total.tokens,
        "cache_hit_rate": report.total.cache_hit_rate,
        "cost_usd": report.total.cost_usd,
        "cost_complete": report.total.cost_complete,
    });
    let payload = serde_json::json!({
        "schema_version": "1.0.0",
        "generated_at": report.updated_at,
        "cost_currency": "USD",
        "days": days,
        "total": total,
    });
    serde_json::to_string_pretty(&payload).unwrap_or_default()
}

fn day_json(day: &DayReport) -> serde_json::Value {
    let models: Vec<serde_json::Value> = day
        .models
        .iter()
        .map(|m| {
            let (provider, model) = m
                .model
                .split_once('/')
                .unwrap_or(("unknown", m.model.as_str()));
            serde_json::json!({
                "provider": provider,
                "model": model,
                "input_tokens": m.buckets.input_tokens,
                "cache_read_tokens": m.buckets.cache_read_tokens,
                "cache_write_tokens": m.buckets.cache_write_tokens,
                "output_tokens": m.buckets.output_tokens,
                "tokens": m.tokens,
                "cache_hit_rate": m.cache_hit_rate,
                "cost_usd": m.cost_usd,
                "cost_complete": m.cost_complete,
            })
        })
        .collect();
    serde_json::json!({
        "date": day.date,
        "input_tokens": day.buckets.input_tokens,
        "cache_read_tokens": day.buckets.cache_read_tokens,
        "cache_write_tokens": day.buckets.cache_write_tokens,
        "output_tokens": day.buckets.output_tokens,
        "tokens": day.tokens,
        "cache_hit_rate": day.cache_hit_rate,
        "cost_usd": day.cost_usd,
        "cost_complete": day.cost_complete,
        "models": models,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::usage::aggregate::{BucketReport, DayReport, ModelReport, TotalReport};

    fn model(name: &str, cost: f64, complete: bool) -> ModelReport {
        ModelReport {
            model: name.to_string(),
            buckets: BucketReport {
                input_tokens: 10,
                output_tokens: 5,
                cache_read_tokens: 100,
                cache_write_tokens: 0,
            },
            tokens: 115,
            cache_hit_rate: Some(90.9),
            cost_usd: cost,
            cost_complete: complete,
        }
    }

    fn sample_report() -> UsageReport {
        UsageReport {
            days: vec![DayReport {
                date: "2026-08-30".to_string(),
                buckets: BucketReport {
                    input_tokens: 10,
                    output_tokens: 5,
                    cache_read_tokens: 100,
                    cache_write_tokens: 0,
                },
                tokens: 115,
                cache_hit_rate: Some(90.9),
                cost_usd: 0.0012,
                cost_complete: false,
                models: vec![
                    model("deepseek/deepseek-v4-flash", 0.0012, true),
                    model("custom-relay/gpt-x", 0.0, false),
                ],
            }],
            total: TotalReport {
                buckets: BucketReport {
                    input_tokens: 10,
                    output_tokens: 5,
                    cache_read_tokens: 100,
                    cache_write_tokens: 0,
                },
                tokens: 115,
                cache_hit_rate: Some(90.9),
                cost_usd: 0.0012,
                cost_complete: false,
            },
            updated_at: 1_787_000_000_000,
        }
    }

    #[test]
    fn csv_has_bom_header_and_escapes() {
        let csv = daily_csv(&sample_report());
        assert!(csv.starts_with('\u{FEFF}'));
        assert!(csv.contains("date,provider,model,input tokens"));
        // provider/model 拆列
        assert!(csv.contains("2026-08-30,deepseek,deepseek-v4-flash,"));
        assert!(csv.contains(",custom-relay,gpt-x,"));
        // 完整成本输出金额，不完整输出空且标记 no
        assert!(csv.contains(",0.0012,yes"));
        assert!(csv.contains(",,no"));
    }

    #[test]
    fn csv_cell_guards_formula_injection() {
        assert_eq!(csv_cell("=SUM(A1)"), "'=SUM(A1)");
        assert_eq!(csv_cell("+1"), "'+1");
        assert_eq!(csv_cell("a,b"), "\"a,b\"");
        assert_eq!(csv_cell("say \"hi\""), "\"say \"\"hi\"\"\"");
        assert_eq!(csv_cell("plain"), "plain");
        // 日期与常见模型名不受公式防护影响（不以 =+-@ 开头）
        assert_eq!(csv_cell("2026-08-30"), "2026-08-30");
        assert_eq!(csv_cell("deepseek-v4-flash"), "deepseek-v4-flash");
    }

    #[test]
    fn json_is_schema_versioned_and_secret_free() {
        let json = export_json(&sample_report());
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(value["schema_version"], "1.0.0");
        assert_eq!(value["days"][0]["models"][0]["provider"], "deepseek");
        assert_eq!(value["days"][0]["models"][0]["cost_usd"], 0.0012);
        assert_eq!(value["total"]["cost_complete"], false);
        // 无任何凭据键（导出投影白名单构造，此断言防将来手滑加字段）
        let text = json.to_lowercase();
        for banned in ["apikey", "api_key", "credential", "token"] {
            if banned == "token" {
                // token 计数列名（*_tokens）合法；键名本身不得出现
                assert!(!text.contains("\"token\":"));
            } else {
                assert!(!text.contains(banned), "泄漏键 {banned}");
            }
        }
    }
}
