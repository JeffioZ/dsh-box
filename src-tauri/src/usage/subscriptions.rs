//! 订阅额度（token plan）适配器：OpenCode Go、Z.ai、Kimi、MiniMax、Ollama。
//!
//! 参考 dsh-usage-stats 的适配器契约（endpoint 路径、凭据环境变量、响应
//! 字段映射），以 Rust 独立实现。凭据只按引用解析、不发往未经校验的地址。
//! 每个适配器把上游响应归一为「额度窗口」（session/weekly/monthly 等）。

use std::time::Duration;

use crate::app_state::Config;

use super::balance::is_private_address;
use super::providers::ProviderRoute;

/// 订阅额度窗口（与 balance::QuotaWindow 同一形状，便于前端统一渲染）。
#[derive(serde::Serialize, Clone, Debug, PartialEq)]
pub struct QuotaWindow {
    pub kind: String,
    pub used_percent: f64,
    pub remaining_percent: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resets_at: Option<String>,
}

/// 一个订阅适配器的结果快照。
#[derive(serde::Serialize, Clone)]
pub struct SubscriptionSnapshot {
    pub id: String,
    pub display_name: String,
    pub mode: &'static str,
    pub adapter: &'static str,
    pub status: &'static str,
    pub plan: String,
    pub windows: Vec<QuotaWindow>,
    pub error: Option<String>,
    /// 瞬错保旧标记：true 表示本快照是上次成功数据。
    pub stale: bool,
    /// 预警级别："none" | "warning" | "critical"。
    pub warn_level: &'static str,
}

/// 订阅适配器：以路由 id 识别。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SubscriptionAdapter {
    OpenCodeGo,
    Zai,
    Kimi,
    MiniMax,
    Ollama,
}

fn adapter_of(route_id: &str) -> Option<SubscriptionAdapter> {
    match route_id {
        "opencode-go" => Some(SubscriptionAdapter::OpenCodeGo),
        "zai" | "zai-coding-cn" => Some(SubscriptionAdapter::Zai),
        "kimi-coding" | "kimi-for-coding" => Some(SubscriptionAdapter::Kimi),
        "minimax" | "minimaxi" | "minimax-cn" | "minimax-coding" => {
            Some(SubscriptionAdapter::MiniMax)
        }
        "ollama" => Some(SubscriptionAdapter::Ollama),
        _ => None,
    }
}

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(15);

fn num_of(value: &serde_json::Value, field: &str) -> Option<f64> {
    value.get(field).and_then(|v| v.as_f64()).or_else(|| {
        value
            .get(field)
            .and_then(|v| v.as_str())
            .and_then(|s| s.parse().ok())
    })
}

fn clamp_percent(v: f64) -> f64 {
    v.clamp(0.0, 100.0)
}

fn round1(v: f64) -> f64 {
    (v * 10.0).round() / 10.0
}

/// epoch 秒或毫秒 → ISO8601。秒值 < 2e10 视为秒。
fn to_iso(value: Option<f64>) -> Option<String> {
    let v = value?;
    let ms = if v < 20_000_000_000.0 { v * 1000.0 } else { v };
    let secs = (ms / 1000.0) as i64;
    let days = secs.div_euclid(86_400);
    let tod = secs.rem_euclid(86_400);
    // civil_from_days（与 aggregate 相同算法）
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    Some(format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        tod / 3600,
        (tod % 3600) / 60,
        tod % 60
    ))
}

/// 订阅预警级别（与前端 warnLevelOf 同阈值）：最紧窗口（min
/// remaining_percent，0..100）≤ 10 → "critical"，≤ 30 → "warning"；
/// 无窗口为 "none"。
fn warn_of_windows(windows: &[QuotaWindow]) -> &'static str {
    let Some(min) = windows.iter().map(|w| w.remaining_percent).reduce(f64::min) else {
        return "none";
    };
    if min <= 10.0 {
        "critical"
    } else if min <= 30.0 {
        "warning"
    } else {
        "none"
    }
}

fn http_get(
    agent: &ureq::Agent,
    url: &str,
    key: &str,
) -> Result<serde_json::Value, (&'static str, String)> {
    let resp = agent
        .get(url)
        .header("Authorization", &format!("Bearer {key}"))
        .header("Accept", "application/json")
        .call()
        .map_err(|e| {
            // 与 balance::query_route 同一分类口径：401/403 与限流必须区别于
            // 一般网络错误，账户监测的瞬错保旧（stale）依赖该分类。
            let status = match &e {
                ureq::Error::StatusCode(401) | ureq::Error::StatusCode(403) => "unauthorized",
                ureq::Error::StatusCode(429) => "rate-limited",
                _ => "unavailable",
            };
            (status, format!("{e}"))
        })?;
    resp.into_body()
        .read_json()
        .map_err(|e| ("invalid-response", format!("{e}")))
}

fn agent() -> &'static ureq::Agent {
    static A: std::sync::OnceLock<ureq::Agent> = std::sync::OnceLock::new();
    A.get_or_init(|| {
        ureq::Agent::config_builder()
            .tls_config(crate::default_tls_config())
            .timeout_connect(Some(Duration::from_secs(5)))
            .timeout_recv_response(Some(DEFAULT_TIMEOUT))
            .timeout_recv_body(Some(DEFAULT_TIMEOUT))
            .build()
            .new_agent()
    })
}

/// 校验订阅请求 URL：HTTPS-only、拒绝私有主机/userinfo（复用余额的防护）。
fn guard_url_https(url: &str) -> Result<String, &'static str> {
    let parsed = url::Url::parse(url).map_err(|_| "invalid-url")?;
    if parsed.scheme() != "https" {
        return Err("insecure-protocol");
    }
    if parsed.username() != "" || parsed.password().is_some() {
        return Err("url-credentials");
    }
    if let Some(host) = parsed.host_str() {
        // 对齐 balance 的 hostname_is_private 口径：localhost/.localhost
        // 与私有/回环地址一样拒绝。
        let host = host
            .trim_start_matches('[')
            .trim_end_matches(']')
            .to_lowercase();
        if host == "localhost" || host.ends_with(".localhost") || is_private_address(&host) {
            return Err("private-network");
        }
    }
    Ok(parsed.to_string())
}

/// —— OpenCode Go ——
fn parse_opencode_go(body: &serde_json::Value) -> Vec<QuotaWindow> {
    let usage = body.get("usage").unwrap_or(body);
    let mut out = Vec::new();
    for (key, kind) in [
        ("rolling", "session"),
        ("weekly", "weekly"),
        ("monthly", "monthly"),
    ] {
        if let Some(obj) = usage.get(key).and_then(|v| v.as_object()) {
            let used = obj
                .get("usagePercent")
                .or_else(|| obj.get("usedPercent"))
                .and_then(|v| v.as_f64())
                .map(clamp_percent);
            if let Some(used) = used {
                out.push(QuotaWindow {
                    kind: kind.to_string(),
                    used_percent: round1(used),
                    remaining_percent: round1(100.0 - used),
                    resets_at: None,
                });
            }
        }
    }
    out
}

/// —— Z.ai ——
fn parse_zai(quota: &serde_json::Value) -> Vec<QuotaWindow> {
    let limits = quota
        .get("data")
        .and_then(|d| d.get("limits"))
        .and_then(|v| v.as_array())
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    let mut out = Vec::new();
    for limit in limits {
        let kind_upper = limit
            .get("type")
            .or_else(|| limit.get("limit_type"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_uppercase();
        let used = num_of(limit, "usage")
            .or_else(|| num_of(limit, "currentValue"))
            .or_else(|| num_of(limit, "current_value"))
            .or_else(|| num_of(limit, "percentage"))
            .or_else(|| num_of(limit, "usedPercent"))
            .or_else(|| num_of(limit, "used_percent"));
        let remaining = num_of(limit, "remaining");
        // 有显式 percentage 时按百分比直接用（部分字段是 0..100）。
        let pct = limit
            .get("percentage")
            .or_else(|| limit.get("usedPercent"))
            .or_else(|| limit.get("used_percent"))
            .and_then(|v| v.as_f64());
        let used_pct = match pct {
            Some(p) => p,
            None => {
                let total = used.unwrap_or(0.0);
                let rem = remaining.unwrap_or(0.0);
                let limit_total = total + rem;
                if limit_total > 0.0 {
                    total / limit_total * 100.0
                } else {
                    continue;
                }
            }
        };
        let kind = match kind_upper.as_str() {
            "TIME_LIMIT" => "billing",
            _ => "session",
        };
        let resets_at = to_iso(
            limit
                .get("nextResetTime")
                .or_else(|| limit.get("next_reset_time"))
                .and_then(|v| v.as_f64()),
        );
        out.push(QuotaWindow {
            kind: kind.to_string(),
            used_percent: round1(clamp_percent(used_pct)),
            remaining_percent: round1(100.0 - clamp_percent(used_pct)),
            resets_at,
        });
    }
    out
}

/// —— Kimi ——
fn parse_kimi(body: &serde_json::Value) -> Vec<QuotaWindow> {
    let data = body.get("data").unwrap_or(body);
    let mut out = Vec::new();
    let usage = data.get("usage");
    if let Some(u) = usage {
        if let (Some(limit), Some(remaining)) = (num_of(u, "limit"), num_of(u, "remaining")) {
            if limit > 0.0 {
                let used = (limit - remaining) / limit * 100.0;
                out.push(QuotaWindow {
                    kind: "weekly".to_string(),
                    used_percent: round1(clamp_percent(used)),
                    remaining_percent: round1(100.0 - clamp_percent(used)),
                    resets_at: None,
                });
            }
        }
    }
    // session 窗口来自 limits[] 的 detail。
    if let Some(limits) = data.get("limits").and_then(|v| v.as_array()) {
        for entry in limits {
            let detail = entry.get("detail").unwrap_or(entry);
            if let (Some(limit), Some(remaining)) =
                (num_of(detail, "limit"), num_of(detail, "remaining"))
            {
                if limit > 0.0 {
                    let used = (limit - remaining) / limit * 100.0;
                    out.push(QuotaWindow {
                        kind: "session".to_string(),
                        used_percent: round1(clamp_percent(used)),
                        remaining_percent: round1(100.0 - clamp_percent(used)),
                        resets_at: None,
                    });
                }
            }
        }
    }
    out
}

/// —— MiniMax ——
fn parse_minimax(body: &serde_json::Value) -> Vec<QuotaWindow> {
    let status = body
        .get("base_resp")
        .or_else(|| body.get("baseResp"))
        .and_then(|v| num_of(v, "status_code").or_else(|| num_of(v, "statusCode")));
    if let Some(code) = status {
        if code != 0.0 {
            return Vec::new();
        }
    }
    let remains = body
        .get("model_remains")
        .or_else(|| body.get("data").and_then(|d| d.get("model_remains")))
        .and_then(|v| v.as_array())
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    let general = remains.iter().find(|e| {
        let name = e
            .get("model_name")
            .or_else(|| e.get("modelName"))
            .and_then(|v| v.as_str())
            .unwrap_or("");
        name == "general" || name.starts_with("minimax-m") || name.starts_with("coding-plan")
    });
    let Some(entry) = general else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for (percent_field, total_field, used_field, status_field, kind) in [
        (
            "current_interval_remaining_percent",
            "current_interval_total_count",
            "current_interval_usage_count",
            "current_interval_status",
            "session",
        ),
        (
            "current_weekly_remaining_percent",
            "current_weekly_total_count",
            "current_weekly_usage_count",
            "current_weekly_status",
            "weekly",
        ),
    ] {
        let remaining_pct = num_of(entry, percent_field);
        let st = num_of(entry, status_field);
        let remaining = match remaining_pct {
            Some(r) => clamp_percent(r),
            None => {
                let total = num_of(entry, total_field).unwrap_or(0.0);
                let used = num_of(entry, used_field).unwrap_or(0.0);
                if total > 0.0 {
                    clamp_percent((1.0 - used / total) * 100.0)
                } else if st == Some(2.0) {
                    0.0
                } else if st == Some(3.0) {
                    100.0
                } else {
                    continue;
                }
            }
        };
        out.push(QuotaWindow {
            kind: kind.to_string(),
            used_percent: round1(100.0 - remaining),
            remaining_percent: round1(remaining),
            resets_at: None,
        });
    }
    out
}

/// —— Ollama ——
fn parse_ollama(body: &serde_json::Value) -> Vec<QuotaWindow> {
    let Some(limits) = body.get("limits") else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for (key, kind) in [("session", "session"), ("weekly", "weekly")] {
        if let Some(ratio) = limits.get(key).and_then(|v| num_of(v, "usage")) {
            let used = clamp_percent(ratio * 100.0);
            out.push(QuotaWindow {
                kind: kind.to_string(),
                used_percent: round1(used),
                remaining_percent: round1(100.0 - used),
                resets_at: None,
            });
        }
    }
    out
}

/// 适配器默认凭据环境变量名（路由未声明 `apiKeyEnv` 时的回退）。
fn default_key_env(adapter: SubscriptionAdapter) -> &'static str {
    match adapter {
        SubscriptionAdapter::OpenCodeGo => "OPENCODE_GO_API_KEY",
        SubscriptionAdapter::Zai => "ZAI_API_KEY",
        SubscriptionAdapter::Kimi => "KIMI_API_KEY",
        SubscriptionAdapter::MiniMax => "MINIMAX_API_KEY",
        SubscriptionAdapter::Ollama => "OLLAMA_API_KEY",
    }
}

/// 凭据环境变量名解析：与 balance::query_route 同口径——优先路由声明的
/// `apiKeyEnv`，未声明（或空白）时回退适配器默认。
fn key_env_of(route: &ProviderRoute, adapter: SubscriptionAdapter) -> &str {
    route
        .api_key_env
        .as_deref()
        .filter(|v| !v.trim().is_empty())
        .unwrap_or_else(|| default_key_env(adapter))
}

/// 订阅凭据解析链：路由声明 env → 适配器默认 env → `.credentials.yaml`
/// （按同序键名各查一次）。路由声明了但环境未提供时继续向链尾回退，
/// 而不是直接判 not-configured。
fn resolve_subscription_key(
    config: &Config,
    route: &ProviderRoute,
    adapter: SubscriptionAdapter,
) -> Option<String> {
    let declared = route
        .api_key_env
        .as_deref()
        .map(str::trim)
        .filter(|v| !v.is_empty());
    let names: Vec<&str> = declared
        .into_iter()
        .chain([default_key_env(adapter)])
        .collect();
    for name in &names {
        if let Ok(value) = std::env::var(name) {
            let value = value.trim();
            if !value.is_empty() {
                return Some(value.to_string());
            }
        }
    }
    names
        .iter()
        .find_map(|name| crate::credentials::value(config, name))
}

/// 查询一个订阅适配器。
pub fn query_subscription(
    config: &Config,
    route: &ProviderRoute,
    adapter: SubscriptionAdapter,
) -> SubscriptionSnapshot {
    let key_env = key_env_of(route, adapter);
    let display_name = match adapter {
        SubscriptionAdapter::OpenCodeGo => "OpenCode Go",
        SubscriptionAdapter::Zai => "Z.ai",
        SubscriptionAdapter::Kimi => "Kimi For Coding",
        SubscriptionAdapter::MiniMax => "MiniMax Coding Plan",
        SubscriptionAdapter::Ollama => "Ollama",
    };
    let plan = match adapter {
        SubscriptionAdapter::OpenCodeGo => "Go",
        SubscriptionAdapter::Zai => "GLM Coding Plan",
        SubscriptionAdapter::Kimi => "Kimi For Coding",
        SubscriptionAdapter::MiniMax => "MiniMax Coding Plan",
        SubscriptionAdapter::Ollama => "Ollama",
    };
    let Some(key) = resolve_subscription_key(config, route, adapter) else {
        return snapshot(
            route,
            adapter,
            display_name,
            plan,
            "not-configured",
            Vec::new(),
            Some(crate::locale::owned(
                format!("未找到凭据 {key_env}。"),
                format!("Credential {key_env} was not found."),
            )),
        );
    };
    let (url, parse): (&str, fn(&serde_json::Value) -> Vec<QuotaWindow>) = match adapter {
        SubscriptionAdapter::OpenCodeGo => {
            ("https://opencode.ai/zen/go/v1/usage", parse_opencode_go)
        }
        SubscriptionAdapter::Zai => ("https://api.z.ai/api/monitor/usage/quota/limit", parse_zai),
        SubscriptionAdapter::Kimi => ("https://api.kimi.com/coding/v1/usages", parse_kimi),
        SubscriptionAdapter::MiniMax => (
            "https://www.minimax.io/v1/token_plan/remains",
            parse_minimax,
        ),
        SubscriptionAdapter::Ollama => ("https://ollama.com/api/usage", parse_ollama),
    };
    let target = match guard_url_https(url) {
        Ok(u) => u,
        Err(reason) => {
            return snapshot(
                route,
                adapter,
                display_name,
                plan,
                "blocked",
                Vec::new(),
                Some(reason.to_string()),
            )
        }
    };
    match http_get(agent(), &target, &key) {
        Ok(body) => {
            let windows = parse(&body);
            let status = if windows.is_empty() {
                "invalid-response"
            } else {
                "ok"
            };
            snapshot(route, adapter, display_name, plan, status, windows, None)
        }
        Err((status, e)) => snapshot(
            route,
            adapter,
            display_name,
            plan,
            status,
            Vec::new(),
            Some(e),
        ),
    }
}

fn snapshot(
    route: &ProviderRoute,
    _adapter: SubscriptionAdapter,
    display_name: &str,
    plan: &str,
    status: &'static str,
    windows: Vec<QuotaWindow>,
    error: Option<String>,
) -> SubscriptionSnapshot {
    SubscriptionSnapshot {
        id: route.id.clone(),
        display_name: display_name.to_string(),
        mode: "subscription",
        adapter: adapter_name_of(_adapter),
        status,
        plan: plan.to_string(),
        warn_level: warn_of_windows(&windows),
        windows,
        error,
        stale: false,
    }
}

fn adapter_name_of(adapter: SubscriptionAdapter) -> &'static str {
    match adapter {
        SubscriptionAdapter::OpenCodeGo => "opencode-go",
        SubscriptionAdapter::Zai => "zai-token-plan",
        SubscriptionAdapter::Kimi => "kimi-token-plan",
        SubscriptionAdapter::MiniMax => "minimax-token-plan",
        SubscriptionAdapter::Ollama => "ollama",
    }
}

/// 支持订阅查询的已知路由 id（每适配器一条）。
///
/// 同一适配器只保留一个 id：`zai-coding-cn` 与 `zai` 同属 Zai 适配器，
/// 下方 `find` 按适配器相等匹配会命中同一条已配置路由，列两次会造成
/// 同一账户重复查询、重复卡片（kimi/minimax 同理只列一个代表 id）。
const KNOWN_IDS: &[(&str, SubscriptionAdapter)] = &[
    ("opencode-go", SubscriptionAdapter::OpenCodeGo),
    ("zai", SubscriptionAdapter::Zai),
    ("kimi-coding", SubscriptionAdapter::Kimi),
    ("minimax", SubscriptionAdapter::MiniMax),
    ("ollama", SubscriptionAdapter::Ollama),
];

/// 阶段 3 入口：枚举所有支持订阅的路由并查询。
pub fn subscriptions(config: &Config) -> Vec<SubscriptionSnapshot> {
    let mut out = Vec::new();
    // 以已配置路由为主，缺失时用已知 id 的默认路由（外部凭据可能未在
    // settings.yaml 建模，但环境变量已提供 key）。
    let routes = super::providers::configured_routes(config);
    for (id, adapter) in KNOWN_IDS {
        let route = routes
            .iter()
            .find(|r| adapter_of(&r.id) == Some(*adapter))
            .cloned()
            .unwrap_or_else(|| ProviderRoute {
                id: id.to_string(),
                display_name: id.to_string(),
                api_key_env: None,
                base_url: None,
            });
        out.push(query_subscription(config, &route, *adapter));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn route(id: &str, api_key_env: Option<&str>) -> ProviderRoute {
        ProviderRoute {
            id: id.to_string(),
            display_name: id.to_string(),
            api_key_env: api_key_env.map(str::to_string),
            base_url: None,
        }
    }

    #[test]
    fn adapter_mapping_covers_subscription_routes() {
        assert_eq!(
            adapter_of("opencode-go"),
            Some(SubscriptionAdapter::OpenCodeGo)
        );
        assert_eq!(adapter_of("zai-coding-cn"), Some(SubscriptionAdapter::Zai));
        assert_eq!(adapter_of("kimi-coding"), Some(SubscriptionAdapter::Kimi));
        assert_eq!(adapter_of("minimax-cn"), Some(SubscriptionAdapter::MiniMax));
        assert_eq!(adapter_of("ollama"), Some(SubscriptionAdapter::Ollama));
        assert_eq!(adapter_of("openrouter"), None);
    }

    #[test]
    fn key_env_prefers_route_declaration_then_adapter_default() {
        // 与 balance::query_route 同口径：路由声明的 apiKeyEnv 优先，
        // 未声明/空白时回退适配器默认环境变量名。
        assert_eq!(
            key_env_of(&route("zai", Some("MY_ZAI_KEY")), SubscriptionAdapter::Zai),
            "MY_ZAI_KEY"
        );
        assert_eq!(
            key_env_of(&route("zai", None), SubscriptionAdapter::Zai),
            "ZAI_API_KEY"
        );
        assert_eq!(
            key_env_of(&route("kimi-coding", Some("  ")), SubscriptionAdapter::Kimi),
            "KIMI_API_KEY"
        );
    }

    #[test]
    fn missing_declared_credential_reports_declared_env_name() {
        // 未配置凭据时走 not-configured 短路（不发请求），错误信息应引用
        // 路由声明的 apiKeyEnv 而非适配器默认名。
        // 回退链会读适配器默认 env（ZAI_API_KEY）：env 进程全局，测试须
        // 串行并暂时移除，避免真实环境/并行用例干扰判定。
        let _guard = crate::credentials::ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let prev_default = std::env::var("ZAI_API_KEY").ok();
        std::env::remove_var("ZAI_API_KEY");
        let root = std::env::temp_dir().join(format!(
            "dshbox-usage-sub-cred-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let mut config = Config::load();
        config.dsh_home = root.clone();
        let snap = query_subscription(
            &config,
            &route("zai", Some("DSHBOX_TEST_UNSET_ZAI_KEY_9F3K")),
            SubscriptionAdapter::Zai,
        );
        match prev_default {
            Some(v) => std::env::set_var("ZAI_API_KEY", v),
            None => std::env::remove_var("ZAI_API_KEY"),
        }
        assert_eq!(snap.status, "not-configured");
        assert!(snap
            .error
            .unwrap()
            .contains("DSHBOX_TEST_UNSET_ZAI_KEY_9F3K"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn subscription_key_chain_route_env_then_default_env_then_file() {
        let _guard = crate::credentials::ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        const DECLARED: &str = "DSHBOX_TEST_SUB_DECLARED_KEY_6T1W";
        let root = std::env::temp_dir().join(format!(
            "dshbox-usage-sub-chain-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(
            root.join(".credentials.yaml"),
            "version: 1\nrefs:\n  ZAI_API_KEY: file-zai\n",
        )
        .unwrap();
        let mut config = Config::load();
        config.dsh_home = root.clone();
        let prev_declared = std::env::var(DECLARED).ok();
        let prev_default = std::env::var("ZAI_API_KEY").ok();
        // 路由声明优先于适配器默认（两者都设置时取声明值）。
        std::env::set_var(DECLARED, "declared-key");
        std::env::set_var("ZAI_API_KEY", "default-key");
        let declared_route = route("zai", Some(DECLARED));
        assert_eq!(
            resolve_subscription_key(&config, &declared_route, SubscriptionAdapter::Zai).as_deref(),
            Some("declared-key")
        );
        // 声明 env 未提供：回退适配器默认 env。
        std::env::remove_var(DECLARED);
        assert_eq!(
            resolve_subscription_key(&config, &declared_route, SubscriptionAdapter::Zai).as_deref(),
            Some("default-key")
        );
        // 链尾：环境全无 → 凭据文件（按适配器默认键名）。
        std::env::remove_var("ZAI_API_KEY");
        assert_eq!(
            resolve_subscription_key(&config, &declared_route, SubscriptionAdapter::Zai).as_deref(),
            Some("file-zai")
        );
        match prev_declared {
            Some(v) => std::env::set_var(DECLARED, v),
            None => std::env::remove_var(DECLARED),
        }
        match prev_default {
            Some(v) => std::env::set_var("ZAI_API_KEY", v),
            None => std::env::remove_var("ZAI_API_KEY"),
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn warn_level_uses_tightest_window() {
        let window = |remaining_percent: f64| QuotaWindow {
            kind: "session".to_string(),
            used_percent: 100.0 - remaining_percent,
            remaining_percent,
            resets_at: None,
        };
        // 阈值含等号：30 → warning，10 → critical；取最紧（min）窗口。
        assert_eq!(warn_of_windows(&[window(30.0)]), "warning");
        assert_eq!(warn_of_windows(&[window(10.0)]), "critical");
        assert_eq!(warn_of_windows(&[window(70.0), window(25.0)]), "warning");
        assert_eq!(warn_of_windows(&[window(80.0), window(9.0)]), "critical");
        assert_eq!(warn_of_windows(&[window(31.0)]), "none");
        assert_eq!(warn_of_windows(&[]), "none");
    }

    #[test]
    fn known_ids_have_unique_adapters() {
        // 同一适配器出现两次会按适配器匹配命中同一路由，产生重复查询与
        // 重复卡片；每个 id 也必须能被 adapter_of 识别。
        let mut seen: Vec<SubscriptionAdapter> = Vec::new();
        for (id, adapter) in KNOWN_IDS {
            assert!(!seen.contains(adapter), "适配器重复：{id}");
            seen.push(*adapter);
            assert_eq!(adapter_of(id), Some(*adapter));
        }
    }

    #[test]
    fn guard_rejects_localhost_and_private_hosts() {
        assert_eq!(
            guard_url_https("https://localhost/api"),
            Err("private-network")
        );
        assert_eq!(
            guard_url_https("https://agent.localhost/api"),
            Err("private-network")
        );
        assert_eq!(
            guard_url_https("https://127.0.0.1/api"),
            Err("private-network")
        );
        assert_eq!(guard_url_https("http://api.z.ai"), Err("insecure-protocol"));
        assert!(guard_url_https("https://api.z.ai/api/monitor/usage/quota/limit").is_ok());
    }

    #[test]
    fn parses_ollama_usage_ratios() {
        let body = serde_json::json!({
            "limits": {
                "session": {"usage": 0.25},
                "weekly": {"usage": 0.5}
            }
        });
        let windows = parse_ollama(&body);
        assert_eq!(windows.len(), 2);
        assert_eq!(windows[0].used_percent, 25.0);
        assert_eq!(windows[1].remaining_percent, 50.0);
    }

    #[test]
    fn parses_opencode_go_windows() {
        let body = serde_json::json!({
            "usage": {
                "rolling": {"usagePercent": 30},
                "weekly": {"usagePercent": 70},
                "monthly": {"usagePercent": 12}
            }
        });
        let windows = parse_opencode_go(&body);
        assert_eq!(windows.len(), 3);
        assert_eq!(windows[0].kind, "session");
        assert_eq!(windows[1].used_percent, 70.0);
    }

    #[test]
    fn parses_zai_token_limits() {
        let body = serde_json::json!({
            "data": { "limits": [
                {"type": "TOKENS_LIMIT", "usage": 3000, "remaining": 7000},
                {"type": "TIME_LIMIT", "percentage": 40}
            ]}
        });
        let windows = parse_zai(&body);
        assert_eq!(windows.len(), 2);
        assert_eq!(windows[0].kind, "session");
        assert_eq!(windows[0].used_percent, 30.0);
    }

    #[test]
    fn parses_kimi_usage_and_session_limits() {
        let body = serde_json::json!({
            "data": {
                "usage": {"limit": 1000, "remaining": 750},
                "limits": [{"detail": {"limit": 500, "remaining": 100}}]
            }
        });
        let windows = parse_kimi(&body);
        assert!(windows
            .iter()
            .any(|w| w.kind == "weekly" && w.used_percent == 25.0));
        assert!(windows
            .iter()
            .any(|w| w.kind == "session" && w.used_percent == 80.0));
    }

    #[test]
    fn parses_minimax_remaining_percent() {
        let body = serde_json::json!({
            "base_resp": {"status_code": 0},
            "model_remains": [{
                "model_name": "general",
                "current_interval_remaining_percent": 80,
                "current_weekly_remaining_percent": 40
            }]
        });
        let windows = parse_minimax(&body);
        assert_eq!(windows.len(), 2);
        assert_eq!(windows[0].kind, "session");
        assert_eq!(windows[0].remaining_percent, 80.0);
        assert_eq!(windows[0].used_percent, 20.0);
    }
}
