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

/// 按候选键序取第一个可解析数值（上游 snake/camel 双写兼容）。
fn num_any(value: &serde_json::Value, keys: &[&str]) -> Option<f64> {
    keys.iter().find_map(|k| num_of(value, k))
}

fn clamp_percent(v: f64) -> f64 {
    v.clamp(0.0, 100.0)
}

fn round1(v: f64) -> f64 {
    (v * 10.0).round() / 10.0
}

/// epoch 秒或毫秒 → ISO8601。秒值 < 2e10 视为秒。
pub(crate) fn to_iso(value: Option<f64>) -> Option<String> {
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
pub(crate) fn warn_of_windows(windows: &[QuotaWindow]) -> &'static str {
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

/// 请求失败：status 为归一状态码；http_code 保留原始 HTTP 状态
/// （MiniMax 多端点回退需要区分 404/405）。
struct ReqError {
    status: &'static str,
    message: String,
    http_code: Option<u16>,
}

impl ReqError {
    /// 是否为「该主机不提供此端点」类失败（可尝试下一端点）；鉴权与限流
    /// 是确定答案，不得回退（上游 v0.3.1 同款规则）。
    fn endpoint_missing(&self) -> bool {
        matches!(self.http_code, Some(404) | Some(405)) || self.status == "invalid-response"
    }
}

/// `auth` 为完整 Authorization 头值：多数适配器 `Bearer <key>`，Z.ai 编码
/// 计划端点要求裸 key。所有订阅端点都是固定云端主机：HTTPS-only、拒绝
/// userinfo/私网（防硬编码清单被意外改向内网）。
fn http_get(agent: &ureq::Agent, url: &str, auth: &str) -> Result<serde_json::Value, ReqError> {
    let target = guard_url_https(url).map_err(|reason| ReqError {
        status: "blocked",
        message: reason.to_string(),
        http_code: None,
    })?;
    let resp = agent
        .get(&target)
        .header("Authorization", auth)
        .header("Accept", "application/json")
        .call()
        .map_err(|e| {
            // 与 balance::query_route 同一分类口径：401/403 与限流必须区别于
            // 一般网络错误，账户监测的瞬错保旧（stale）依赖该分类。
            let (status, code) = match &e {
                ureq::Error::StatusCode(c @ (401 | 403)) => ("unauthorized", Some(*c)),
                ureq::Error::StatusCode(429) => ("rate-limited", Some(429)),
                ureq::Error::StatusCode(c) => ("unavailable", Some(*c)),
                _ => ("unavailable", None),
            };
            ReqError {
                status,
                message: format!("{e}"),
                http_code: code,
            }
        })?;
    crate::net_guard::read_json_capped(resp.into_body()).map_err(|e| ReqError {
        status: "invalid-response",
        message: e,
        http_code: None,
    })
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

// —— Z.ai ——（上游 v0.3.1 parseZai：token 窗口按时长升序挑 session/weekly，
// TIME_LIMIT 单列为 billing，订阅续费时间兜底 resetsAt）

/// 窗口时长（分钟）：unit 5=分钟 3=小时 1=天 6=周（Z.ai 配额 API 约定）。
fn zai_window_minutes(limit: &serde_json::Value) -> Option<f64> {
    let unit = num_of(limit, "unit")?;
    let number = num_of(limit, "number")?;
    if number <= 0.0 {
        return None;
    }
    match unit as i64 {
        5 => Some(number),
        3 => Some(number * 60.0),
        1 => Some(number * 24.0 * 60.0),
        6 => Some(number * 7.0 * 24.0 * 60.0),
        _ => None,
    }
}

fn zai_used_percent(limit: &serde_json::Value) -> Option<f64> {
    // Z.ai 的 `usage` 字段是总量而非已用：已用 = 总量-剩余 与当前值取大。
    let total = num_of(limit, "usage").filter(|t| *t > 0.0);
    if let Some(total) = total {
        let remaining = num_of(limit, "remaining");
        let current = num_any(limit, &["currentValue", "current_value"]);
        let used = match (remaining, current) {
            (None, c) => c,
            (Some(r), None) => Some(total - r),
            (Some(r), Some(c)) => Some((total - r).max(c)),
        };
        if let Some(used) = used {
            return Some(clamp_percent(used.clamp(0.0, total) / total * 100.0));
        }
    }
    num_any(limit, &["percentage", "usedPercent", "used_percent"]).map(clamp_percent)
}

fn zai_window(
    limit: &serde_json::Value,
    kind: &str,
    fallback_reset: Option<String>,
) -> Option<QuotaWindow> {
    let used = zai_used_percent(limit)?;
    let resets_at =
        to_iso(num_any(limit, &["nextResetTime", "next_reset_time"])).or(fallback_reset);
    Some(QuotaWindow {
        kind: kind.to_string(),
        used_percent: round1(used),
        remaining_percent: round1(100.0 - used),
        resets_at,
    })
}

/// 计划名美化：`_-` 转空格、GLM 大写、词首大写（上游 displayPlan）。
fn display_plan(value: &str) -> String {
    value
        .trim()
        .split(['_', '-'])
        .filter(|s| !s.is_empty())
        .map(|word| {
            if word.eq_ignore_ascii_case("glm") {
                "GLM".to_string()
            } else {
                let mut chars = word.chars();
                match chars.next() {
                    Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                    None => String::new(),
                }
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn zai_plan(quota: &serde_json::Value, subscription: Option<&serde_json::Value>) -> String {
    let row = subscription
        .and_then(|s| s.get("data"))
        .and_then(|d| d.as_array())
        .and_then(|a| a.iter().find(|e| e.is_object()));
    for source in [row, quota.get("data")] {
        let Some(source) = source else { continue };
        for key in [
            "product_name",
            "productName",
            "plan_name",
            "planName",
            "package_name",
            "packageName",
            "plan_type",
            "planType",
            "level",
        ] {
            if let Some(value) = source.get(key).and_then(|v| v.as_str()) {
                let display = display_plan(value);
                if !display.is_empty() {
                    return display;
                }
            }
        }
    }
    "GLM Coding Plan".to_string()
}

/// 解析 Z.ai 配额（+可选订阅列表）→ (窗口, 计划名)。
fn parse_zai(
    quota: &serde_json::Value,
    subscription: Option<&serde_json::Value>,
) -> (Vec<QuotaWindow>, String) {
    let limits = quota
        .get("data")
        .and_then(|d| d.get("limits"))
        .and_then(|v| v.as_array())
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    let type_of = |l: &serde_json::Value| {
        l.get("type")
            .or_else(|| l.get("limit_type"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_uppercase()
    };
    let mut token_limits: Vec<&serde_json::Value> = limits
        .iter()
        .filter(|l| {
            matches!(type_of(l).as_str(), "TOKENS_LIMIT" | "CREDIT_LIMIT")
                && zai_used_percent(l).is_some()
        })
        .collect();
    token_limits.sort_by(|a, b| {
        zai_window_minutes(a)
            .unwrap_or(f64::MAX)
            .partial_cmp(&zai_window_minutes(b).unwrap_or(f64::MAX))
            .unwrap()
    });
    let time_limit = limits
        .iter()
        .find(|l| type_of(l) == "TIME_LIMIT" && zai_used_percent(l).is_some());
    let first = token_limits.first().copied();
    let session = if token_limits.len() >= 2 {
        first
    } else {
        // 单 token 窗口 ≤6h 才算 session（上游同款启发式）
        first.filter(|l| zai_window_minutes(l).is_some_and(|m| m <= 360.0))
    };
    let weekly = if token_limits.len() >= 2 {
        token_limits.last().copied()
    } else if session.is_none() {
        first
    } else {
        None
    };
    let renew_at = to_iso(
        subscription
            .and_then(|s| s.get("data"))
            .and_then(|d| d.as_array())
            .and_then(|a| a.first())
            .and_then(|r| num_any(r, &["next_renew_time", "nextRenewTime"])),
    );
    let mut windows = Vec::new();
    if let Some(limit) = session.and_then(|l| zai_window(l, "session", None)) {
        windows.push(limit);
    }
    if let Some(limit) = weekly.and_then(|l| zai_window(l, "weekly", None)) {
        windows.push(limit);
    }
    if let Some(limit) = time_limit.and_then(|l| zai_window(l, "billing", renew_at)) {
        windows.push(limit);
    }
    (windows, zai_plan(quota, subscription))
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
/// 剩余毫秒数（相对 now 的持续时间）→ ISO 时间（上游 resetFromDuration）。
fn reset_from_duration(ms: Option<f64>) -> Option<String> {
    let ms = ms?;
    if ms < 0.0 {
        return None;
    }
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as f64)
        .unwrap_or(0.0);
    to_iso(Some(now_ms + ms))
}

/// 聊天条目：精确 "general" 优先，其次按模型名（minimax-m*/coding-plan*，
/// 新版载荷以模型自身命名，大小写不敏感）。
fn minimax_chat_entry(remains: &[serde_json::Value]) -> Option<&serde_json::Value> {
    let name_of = |e: &serde_json::Value| {
        e.get("model_name")
            .or_else(|| e.get("modelName"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_lowercase()
    };
    if let Some(named) = remains.iter().find(|e| name_of(e) == "general") {
        return Some(named);
    }
    remains
        .iter()
        .find(|e| name_of(e).starts_with("minimax-m") || name_of(e).starts_with("coding-plan"))
}

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
    let Some(entry) = minimax_chat_entry(remains) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for (
        percent_fields,
        total_field,
        used_field,
        status_field,
        end_fields,
        duration_fields,
        kind,
    ) in [
        (
            [
                "current_interval_remaining_percent",
                "currentIntervalRemainingPercent",
            ],
            "current_interval_total_count",
            "current_interval_usage_count",
            "current_interval_status",
            [
                "current_interval_end_time",
                "currentIntervalEndTime",
                "current_interval_reset_time",
            ],
            ["remains_time", "remainsTime"],
            "session",
        ),
        (
            [
                "current_weekly_remaining_percent",
                "currentWeeklyRemainingPercent",
            ],
            "current_weekly_total_count",
            "current_weekly_usage_count",
            "current_weekly_status",
            [
                "current_weekly_end_time",
                "currentWeeklyEndTime",
                "current_weekly_reset_time",
            ],
            ["weekly_remains_time", "weeklyRemainsTime"],
            "weekly",
        ),
    ] {
        let remaining_pct = num_any(entry, &percent_fields);
        let st = num_any(entry, &[status_field]);
        let remaining = match remaining_pct {
            Some(r) => clamp_percent(r),
            None => {
                // 旧版载荷是真实计数（新版把计数清零，只信 >0 的总量）。
                let total = num_of(entry, total_field).unwrap_or(0.0);
                let used = num_of(entry, used_field);
                if total > 0.0 && used.is_some() {
                    clamp_percent((1.0 - used.unwrap_or(0.0) / total) * 100.0)
                } else if st == Some(2.0) {
                    0.0
                } else if st == Some(3.0) {
                    100.0
                } else {
                    continue;
                }
            }
        };
        // 窗口状态：1=限额 2=耗尽 3=不限（缺百分比时不得隐藏窗口）。
        let resets_at = to_iso(num_any(entry, &end_fields))
            .or_else(|| reset_from_duration(num_any(entry, &duration_fields)));
        out.push(QuotaWindow {
            kind: kind.to_string(),
            used_percent: round1(100.0 - remaining),
            remaining_percent: round1(remaining),
            resets_at,
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

/// 具名设置值：环境变量 → 凭据文件（区域开关如 `ZAI_API_REGION` 与凭据
/// 同存放，上游同款约定）。
fn named_setting(config: &Config, name: &str) -> Option<String> {
    if let Ok(value) = std::env::var(name) {
        let value = value.trim().to_string();
        if !value.is_empty() {
            return Some(value);
        }
    }
    crate::credentials::value(config, name)
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

/// Z.ai 区域：显式 `ZAI_API_REGION`（"cn"/"bigmodel-cn"/含 bigmodel.cn）
/// 优先；否则国内路由 id（zai-coding-cn）或 baseURL 指向 bigmodel.cn。
fn zai_region(route: &ProviderRoute, config: &Config) -> &'static str {
    if let Some(raw) = named_setting(config, "ZAI_API_REGION") {
        let value = raw.to_lowercase();
        return if value == "bigmodel-cn" || value == "cn" || value.contains("bigmodel.cn") {
            "bigmodel-cn"
        } else {
            "global"
        };
    }
    if route.id == "zai-coding-cn"
        || route
            .base_url
            .as_deref()
            .is_some_and(|u| u.contains("bigmodel.cn"))
    {
        return "bigmodel-cn";
    }
    "global"
}

fn zai_host(region: &str) -> &'static str {
    if region == "bigmodel-cn" {
        "https://open.bigmodel.cn"
    } else {
        "https://api.z.ai"
    }
}

/// MiniMax 区域：显式 `MINIMAX_API_REGION`=="cn" 或 baseURL 含 minimaxi.com
/// → 国内站。
fn minimax_region(route: &ProviderRoute, config: &Config) -> &'static str {
    if let Some(raw) = named_setting(config, "MINIMAX_API_REGION") {
        return if raw.trim().eq_ignore_ascii_case("cn") {
            "cn"
        } else {
            "global"
        };
    }
    if route
        .base_url
        .as_deref()
        .is_some_and(|u| u.contains("minimaxi.com"))
    {
        return "cn";
    }
    "global"
}

/// (www 主站, api 站)——token plan 端点两站都服务，api 站另有 legacy 路径。
fn minimax_hosts(region: &str) -> (&'static str, &'static str) {
    if region == "cn" {
        ("https://www.minimaxi.com", "https://api.minimaxi.com")
    } else {
        ("https://www.minimax.io", "https://api.minimax.io")
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
    // Ollama 只有云端有配额端点：本地/内网 Ollama（如 localhost:11434）
    // 不得当作云端账户查询（上游 provider-identity 同款门控）。
    if adapter == SubscriptionAdapter::Ollama {
        if let Some(base) = route.base_url.as_deref() {
            let host_private = url::Url::parse(base)
                .ok()
                .and_then(|u| u.host_str().map(crate::net_guard::hostname_is_private))
                .unwrap_or(true);
            if host_private {
                return snapshot(
                    route,
                    adapter,
                    display_name,
                    plan,
                    "unsupported",
                    Vec::new(),
                    Some(
                        crate::locale::text(
                            "本地 Ollama 无云端配额查询。",
                            "Local Ollama has no cloud quota endpoint.",
                        )
                        .into(),
                    ),
                );
            }
        }
    }
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
    let outcome: Result<(Vec<QuotaWindow>, Option<String>), ReqError> = match adapter {
        SubscriptionAdapter::Zai => collect_zai(config, route, &key),
        SubscriptionAdapter::MiniMax => collect_minimax(config, route, &key),
        SubscriptionAdapter::OpenCodeGo
        | SubscriptionAdapter::Kimi
        | SubscriptionAdapter::Ollama => collect_simple(adapter, &key),
    };
    match outcome {
        Ok((windows, plan_override)) => {
            let status = if windows.is_empty() {
                "invalid-response"
            } else {
                "ok"
            };
            if let Some(label) = plan_override {
                return snapshot(route, adapter, display_name, &label, status, windows, None);
            }
            snapshot(route, adapter, display_name, plan, status, windows, None)
        }
        Err(e) => snapshot(
            route,
            adapter,
            display_name,
            plan,
            e.status,
            Vec::new(),
            Some(e.message),
        ),
    }
}

/// Z.ai：裸 key 鉴权的配额端点 + 可选订阅列表（计划名与续费时间）。
fn collect_zai(
    config: &Config,
    route: &ProviderRoute,
    key: &str,
) -> Result<(Vec<QuotaWindow>, Option<String>), ReqError> {
    let host = zai_host(zai_region(route, config));
    // 编码计划端点要裸 API key（与推理 API 的 Bearer 不同）。
    let auth = key.to_string();
    let quota = http_get(
        agent(),
        &format!("{host}/api/monitor/usage/quota/limit"),
        &auth,
    )?;
    // 计划名/续费元数据可选：失败不影响配额结果。
    let subscription = http_get(agent(), &format!("{host}/api/biz/subscription/list"), &auth).ok();
    let (windows, plan) = parse_zai(&quota, subscription.as_ref());
    Ok((windows, if plan.is_empty() { None } else { Some(plan) }))
}

/// MiniMax：区域化主机 + token-plan → api 站 token-plan → legacy 路径
/// 的端点回退链（404/405/非 JSON 才试下一个；鉴权与限流是确定答案）。
fn collect_minimax(
    config: &Config,
    route: &ProviderRoute,
    key: &str,
) -> Result<(Vec<QuotaWindow>, Option<String>), ReqError> {
    let auth = format!("Bearer {key}");
    let urls: Vec<String> = if let Some(base) = route.base_url.as_deref() {
        vec![format!(
            "{}/v1/token_plan/remains",
            base.trim_end_matches('/')
        )]
    } else {
        let (www, api) = minimax_hosts(minimax_region(route, config));
        vec![
            format!("{www}/v1/token_plan/remains"),
            format!("{api}/v1/token_plan/remains"),
            format!("{api}/v1/api/openplatform/coding_plan/remains"),
        ]
    };
    let mut last_err: Option<ReqError> = None;
    for (index, url) in urls.iter().enumerate() {
        match http_get(agent(), url, &auth) {
            Ok(body) => {
                if index > 0 {
                    // 区域主机/端点探测落到后续候选：留痕方便对齐区域配置
                    crate::logging::log(&format!(
                        "usage: minimax 首选端点不可用，实际命中第 {} 个候选",
                        index + 1
                    ));
                }
                return Ok((parse_minimax(&body), None));
            }
            Err(e) => {
                let try_next = e.endpoint_missing() && index + 1 < urls.len();
                if !try_next {
                    return Err(e);
                }
                last_err = Some(e);
            }
        }
    }
    Err(last_err.unwrap_or(ReqError {
        status: "unavailable",
        message: "no endpoint resolved".to_string(),
        http_code: None,
    }))
}

/// 单端点适配器（OpenCode Go / Kimi / Ollama 云端）。
fn collect_simple(
    adapter: SubscriptionAdapter,
    key: &str,
) -> Result<(Vec<QuotaWindow>, Option<String>), ReqError> {
    let (url, parse): (&str, fn(&serde_json::Value) -> Vec<QuotaWindow>) = match adapter {
        SubscriptionAdapter::OpenCodeGo => {
            ("https://opencode.ai/zen/go/v1/usage", parse_opencode_go)
        }
        SubscriptionAdapter::Kimi => ("https://api.kimi.com/coding/v1/usages", parse_kimi),
        SubscriptionAdapter::Ollama => ("https://ollama.com/api/usage", parse_ollama),
        SubscriptionAdapter::Zai | SubscriptionAdapter::MiniMax => {
            unreachable!("zai/minimax 走专属采集流程")
        }
    };
    let body = http_get(agent(), url, &format!("Bearer {key}"))?;
    Ok((parse(&body), None))
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
        // v0.3.1 语义：usage=总量、remaining=剩余；token 窗口按时长升序
        // 挑 session（短）/weekly（长），TIME_LIMIT 单列 billing
        let body = serde_json::json!({
            "data": { "limits": [
                {"type": "TOKENS_LIMIT", "usage": 10000, "remaining": 7000, "unit": 5, "number": 300},
                {"type": "TOKENS_LIMIT", "usage": 100000, "remaining": 40000, "unit": 6, "number": 4},
                {"type": "TIME_LIMIT", "percentage": 40}
            ]}
        });
        let (windows, plan) = parse_zai(&body, None);
        assert_eq!(windows.len(), 3);
        assert_eq!(windows[0].kind, "session");
        assert_eq!(windows[0].used_percent, 30.0);
        assert_eq!(windows[1].kind, "weekly");
        assert_eq!(windows[1].used_percent, 60.0);
        assert_eq!(windows[2].kind, "billing");
        assert_eq!(windows[2].used_percent, 40.0);
        assert_eq!(plan, "GLM Coding Plan");
    }

    #[test]
    fn zai_single_short_window_is_session_and_long_is_weekly() {
        let short = serde_json::json!({
            "data": { "limits": [
                {"type": "TOKENS_LIMIT", "usage": 100, "remaining": 50, "unit": 5, "number": 60}
            ]}
        });
        let (windows, _) = parse_zai(&short, None);
        assert_eq!(windows.len(), 1);
        assert_eq!(windows[0].kind, "session");

        let long = serde_json::json!({
            "data": { "limits": [
                {"type": "TOKENS_LIMIT", "usage": 100, "remaining": 50, "unit": 6, "number": 4}
            ]}
        });
        let (windows, _) = parse_zai(&long, None);
        assert_eq!(windows.len(), 1);
        assert_eq!(windows[0].kind, "weekly");
    }

    #[test]
    fn zai_subscription_renew_time_falls_back_to_billing_window() {
        let quota = serde_json::json!({
            "data": { "limits": [
                {"type": "TIME_LIMIT", "percentage": 10}
            ]}
        });
        // 2026-08-01T00:00:00Z = 1785542400s
        let subscription = serde_json::json!({
            "data": [{"product_name": "glm_max_monthly", "next_renew_time": 1785542400}]
        });
        let (windows, plan) = parse_zai(&quota, Some(&subscription));
        assert_eq!(windows.len(), 1);
        assert_eq!(windows[0].kind, "billing");
        assert_eq!(
            windows[0].resets_at.as_deref(),
            Some("2026-08-01T00:00:00Z")
        );
        assert_eq!(plan, "GLM Max Monthly");
    }

    #[test]
    fn zai_region_prefers_explicit_setting_and_cn_route() {
        let _guard = crate::credentials::ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let prev = std::env::var("ZAI_API_REGION").ok();
        let config = Config::load();
        let route = |id: &str, base: Option<&str>| ProviderRoute {
            id: id.to_string(),
            display_name: id.to_string(),
            api_key_env: None,
            base_url: base.map(str::to_string),
        };
        std::env::set_var("ZAI_API_REGION", "cn");
        assert_eq!(zai_region(&route("zai", None), &config), "bigmodel-cn");
        std::env::set_var("ZAI_API_REGION", "global");
        assert_eq!(zai_region(&route("zai", None), &config), "global");
        // 未显式配置时按路由 id / baseURL 推断
        match prev {
            Some(v) => std::env::set_var("ZAI_API_REGION", v),
            None => std::env::remove_var("ZAI_API_REGION"),
        }
        assert_eq!(
            zai_region(&route("zai-coding-cn", None), &config),
            "bigmodel-cn"
        );
        assert_eq!(
            zai_region(&route("zai", Some("https://open.bigmodel.cn/")), &config),
            "bigmodel-cn"
        );
        assert_eq!(zai_region(&route("zai", None), &config), "global");
        assert_eq!(zai_host("bigmodel-cn"), "https://open.bigmodel.cn");
        assert_eq!(zai_host("global"), "https://api.z.ai");
    }

    #[test]
    fn minimax_region_from_setting_or_cn_hostname() {
        let _guard = crate::credentials::ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let prev = std::env::var("MINIMAX_API_REGION").ok();
        let config = Config::load();
        let route = |base: Option<&str>| ProviderRoute {
            id: "minimax".to_string(),
            display_name: "minimax".to_string(),
            api_key_env: None,
            base_url: base.map(str::to_string),
        };
        std::env::set_var("MINIMAX_API_REGION", "CN");
        assert_eq!(minimax_region(&route(None), &config), "cn");
        match prev {
            Some(v) => std::env::set_var("MINIMAX_API_REGION", v),
            None => std::env::remove_var("MINIMAX_API_REGION"),
        }
        assert_eq!(
            minimax_region(&route(Some("https://www.minimaxi.com/")), &config),
            "cn"
        );
        assert_eq!(minimax_region(&route(None), &config), "global");
    }

    #[test]
    fn minimax_chat_entry_prefers_general_then_model_pattern() {
        // 新版载荷以模型自身命名（大小写不敏感），无 general 条目时按模式匹配
        let body = serde_json::json!({
            "model_remains": [{
                "model_name": "MiniMax-M3",
                "current_interval_remaining_percent": 80,
                "current_weekly_remaining_percent": 40
            }]
        });
        let windows = parse_minimax(&body);
        assert_eq!(windows.len(), 2);
        assert_eq!(windows[0].remaining_percent, 80.0);
    }

    #[test]
    fn minimax_resets_at_from_end_time_or_duration() {
        let with_end = serde_json::json!({
            "model_remains": [{
                "model_name": "general",
                "current_interval_remaining_percent": 80,
                "current_weekly_remaining_percent": 40,
                "current_interval_end_time": 1785542400,
                "weekly_remains_time": 3600000
            }]
        });
        let windows = parse_minimax(&with_end);
        assert_eq!(
            windows[0].resets_at.as_deref(),
            Some("2026-08-01T00:00:00Z")
        );
        assert!(windows[1].resets_at.is_some());
    }

    #[test]
    fn ollama_local_gateway_is_gated() {
        let route = ProviderRoute {
            id: "ollama".to_string(),
            display_name: "Ollama".to_string(),
            api_key_env: Some("OLLAMA_API_KEY".to_string()),
            base_url: Some("http://localhost:11434".to_string()),
        };
        let config = Config::load();
        let snapshot = query_subscription(&config, &route, SubscriptionAdapter::Ollama);
        assert_eq!(snapshot.status, "unsupported");
        // 云端（无 baseURL 或公网 baseURL）不受门控影响：无凭据时给出
        // not-configured 而非 unsupported
        let cloud_route = ProviderRoute {
            base_url: None,
            ..route
        };
        let snapshot = query_subscription(&config, &cloud_route, SubscriptionAdapter::Ollama);
        assert_eq!(snapshot.status, "not-configured");
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
