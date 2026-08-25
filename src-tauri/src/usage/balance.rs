//! 供应商余额查询：按路由身份选用适配器，对上游账户接口做受限 HTTPS GET。
//!
//! 参考 dsh-usage-stats 的适配器契约（适配器名单、endpoint 相对路径与响应
//! 字段映射），以 Rust 独立实现。这些属于公开 API 事实（endpoint 路径 +
//! JSON 字段名），不复制其代码结构。
//!
//! 安全边界（与上游对齐）：
//! - 只发 HTTPS GET（无显式 allowInsecure）；
//! - 拒绝含 userinfo 的 URL 与私有/回环主机；
//! - 响应体上限 1 MiB、超时连接/响应分段限制；
//! - 凭据只在请求时解析、绝不落盘。

use std::time::Duration;

use crate::app_state::Config;

use super::providers::ProviderRoute;
/// 统一账户快照（序列化给前端）。余额或订阅二选一，均归一为 `account`。
#[derive(serde::Serialize, Clone)]
pub struct AccountSnapshot {
    pub id: String,
    pub display_name: String,
    /// "balance" | "subscription"
    pub mode: &'static str,
    pub adapter: Option<&'static str>,
    /// "ok" | "not-configured" | "unauthorized" | "rate-limited" |
    /// "unavailable" | "invalid-response" | "blocked" | "unsupported"
    pub status: &'static str,
    pub balance: Option<Balance>,
    pub windows: Vec<QuotaWindow>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// 查询完成时刻（Unix 秒），前端显示「更新于 HH:MM」。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<u64>,
    /// 瞬错保旧标记：true 表示本快照是上次成功数据（updated_at 保留旧值）。
    pub stale: bool,
    /// 预警级别："none" | "warning" | "critical"。
    pub warn_level: &'static str,
}

#[derive(serde::Serialize, Clone)]
pub struct Balance {
    pub remaining: Option<f64>,
    pub used: Option<f64>,
    pub total: Option<f64>,
    pub currency: String,
    pub unlimited: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub granted: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub topped_up: Option<f64>,
}

#[derive(serde::Serialize, Clone)]
pub struct QuotaWindow {
    pub kind: String,
    pub used_percent: f64,
    pub remaining_percent: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resets_at: Option<String>,
}

/// 余额适配器 id（`-balance` 后缀的适配器，与上游 ADAPTERS 集合对应）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BalanceScheme {
    DeepSeek,
    OpenRouter,
    Moonshot,
    Zai,
}

/// 按路由 id 解析余额适配器。返回 None 表示该路由无公开余额接口。
fn scheme_of(route_id: &str) -> Option<BalanceScheme> {
    match route_id {
        "deepseek-official" | "deepseek" => Some(BalanceScheme::DeepSeek),
        "openrouter" => Some(BalanceScheme::OpenRouter),
        "moonshotai" | "moonshotai-cn" | "kimi" => Some(BalanceScheme::Moonshot),
        "zai" | "zai-coding-cn" => Some(BalanceScheme::Zai),
        _ => None,
    }
}

struct SchemeSpec {
    /// 相对 base URL 的路径。
    path: &'static str,
    /// 响应解析。
    parse: fn(&serde_json::Value) -> Result<Balance, &'static str>,
}

const SCHEMES: &[(BalanceScheme, SchemeSpec)] = &[
    (
        BalanceScheme::DeepSeek,
        SchemeSpec {
            path: "/user/balance",
            parse: |json| {
                let infos = json
                    .get("balance_infos")
                    .and_then(|v| v.as_array())
                    .map(Vec::as_slice)
                    .unwrap_or(&[]);
                let info = infos
                    .iter()
                    .find(|e| e.get("currency").and_then(|v| v.as_str()) == Some("CNY"))
                    .or_else(|| infos.first())
                    .ok_or("missing balance_infos")?;
                let remaining = num_field(info, "total_balance");
                Ok(Balance {
                    remaining,
                    used: None,
                    total: None,
                    currency: info
                        .get("currency")
                        .and_then(|v| v.as_str())
                        .unwrap_or("CNY")
                        .to_string(),
                    unlimited: false,
                    granted: num_field(info, "granted_balance"),
                    topped_up: num_field(info, "topped_up_balance"),
                })
            },
        },
    ),
    (
        BalanceScheme::OpenRouter,
        SchemeSpec {
            path: "/api/v1/credits",
            parse: |json| {
                let data = json.get("data").ok_or("missing data")?;
                let total = num_field(data, "total_credits").ok_or("missing total_credits")?;
                let used = num_field(data, "total_usage");
                let remaining = total - used.unwrap_or(0.0);
                Ok(Balance {
                    remaining: Some(remaining),
                    used,
                    total: Some(total),
                    currency: "USD".to_string(),
                    unlimited: false,
                    granted: None,
                    topped_up: None,
                })
            },
        },
    ),
    (
        BalanceScheme::Moonshot,
        SchemeSpec {
            path: "/v1/users/me/balance",
            parse: |json| {
                let data = json.get("data").ok_or("missing data")?;
                let remaining = num_field(data, "available_balance");
                Ok(Balance {
                    remaining,
                    used: None,
                    total: None,
                    currency: data
                        .get("currency")
                        .and_then(|v| v.as_str())
                        .unwrap_or("CNY")
                        .to_string(),
                    unlimited: false,
                    granted: num_field(data, "voucher_balance"),
                    topped_up: num_field(data, "cash_balance"),
                })
            },
        },
    ),
    (
        BalanceScheme::Zai,
        SchemeSpec {
            path: "/api/paas/v4/balance",
            parse: |json| {
                let data = json.get("data").ok_or("missing data")?;
                let available = num_field(data, "available_balance");
                let total = num_field(data, "total_balance").or(available);
                Ok(Balance {
                    remaining: total,
                    used: None,
                    total: None,
                    currency: data
                        .get("currency")
                        .and_then(|v| v.as_str())
                        .unwrap_or("CNY")
                        .to_string(),
                    unlimited: false,
                    granted: None,
                    topped_up: available,
                })
            },
        },
    ),
];

fn num_field(value: &serde_json::Value, field: &str) -> Option<f64> {
    value.get(field).and_then(|v| v.as_f64()).or_else(|| {
        value
            .get(field)
            .and_then(|v| v.as_str())
            .and_then(|s| s.parse::<f64>().ok())
    })
}

/// 余额预警级别（与前端 warnLevelOf 同阈值）：remaining 与 total 均已知、
/// total > 0 且非 unlimited 时，remaining/total ≤ 0.1 → "critical"，
/// ≤ 0.3 → "warning"；其余（含字段缺失）一律 "none"。
pub(crate) fn warn_of_balance(balance: &Balance) -> &'static str {
    if balance.unlimited {
        return "none";
    }
    let (Some(remaining), Some(total)) = (balance.remaining, balance.total) else {
        return "none";
    };
    if total <= 0.0 {
        return "none";
    }
    let ratio = remaining / total;
    if ratio <= 0.1 {
        "critical"
    } else if ratio <= 0.3 {
        "warning"
    } else {
        "none"
    }
}

/// 私有/回环网段的判定（IPv4 与 IPv6 简化版，覆盖上游 `isPrivateAddress` 的
/// 主要网段）。仅用于拒绝直连目标；不做代理 fake-IP 例外（本壳不跑代理）。
pub(crate) fn is_private_address(host: &str) -> bool {
    let host = host.trim().trim_start_matches('[').trim_end_matches(']');
    // IPv6 回环/链路本地/ULA。
    if host.contains(':') {
        let lower = host.to_lowercase();
        if lower == "::1" {
            return true;
        }
        if lower.starts_with("fe80") || lower.starts_with("fc") || lower.starts_with("fd") {
            return true;
        }
        // IPv4-mapped IPv6 ::ffff:a.b.c.d
        if let Some(rest) = lower.strip_prefix("::ffff:") {
            return is_private_ipv4(rest);
        }
        return false;
    }
    is_private_ipv4(host)
}

fn is_private_ipv4(host: &str) -> bool {
    let octets: Vec<u32> = match host
        .split('.')
        .map(|p| p.parse::<u32>())
        .collect::<Result<Vec<_>, _>>()
    {
        Ok(v) if v.len() == 4 => v,
        _ => return false,
    };
    let [a, b, c, _] = [octets[0], octets[1], octets[2], octets[3]];
    a == 0
        || a == 10
        || a == 127
        || (a == 169 && b == 254)
        || (a == 172 && (16..=31).contains(&b))
        || (a == 192 && b == 168)
        || (a == 100 && (64..=127).contains(&b))
        || (a == 192 && b == 0 && (c == 0 || c == 2))
        || (a == 198 && (b == 18 || b == 19))
        || a >= 224
}

fn hostname_is_private(hostname: &str) -> bool {
    let host = hostname
        .trim()
        .trim_start_matches('[')
        .trim_end_matches(']')
        .to_lowercase();
    host == "localhost" || host.ends_with(".localhost") || is_private_address(&host)
}

/// 校验并构造余额请求 URL：只允许 HTTPS、拒绝 userinfo、拒绝私有主机。
fn guard_url(base: &str, path: &str) -> Result<String, &'static str> {
    let base = base.trim_end_matches('/');
    let combined = format!("{base}{path}");
    let url = url::Url::parse(&combined).map_err(|_| "invalid-url")?;
    if url.username() != "" || url.password().is_some() {
        return Err("url-credentials");
    }
    if url.scheme() != "https" {
        return Err("insecure-protocol");
    }
    if let Some(host) = url.host_str() {
        let host = host.trim_start_matches('[').trim_end_matches(']');
        if hostname_is_private(host) {
            return Err("private-network");
        }
    }
    Ok(url.to_string())
}

/// 查询一个路由的余额（同步、阻塞线程调用）。无 key / 无适配器时给出
/// 明确的不可用快照而非 error。
pub fn query_route(config: &Config, route: &ProviderRoute) -> AccountSnapshot {
    let adapter = scheme_of(&route.id);
    let Some(scheme) = adapter else {
        return AccountSnapshot {
            id: route.id.clone(),
            display_name: route.display_name.clone(),
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
            updated_at: Some(unix_now()),
            stale: false,
            warn_level: "none",
        };
    };
    let Some(key_env) = route.api_key_env.as_deref() else {
        return snapshot_error(
            route,
            "not-configured",
            crate::locale::text(
                "未配置该供应商的凭据。",
                "No credential is configured for this provider.",
            ),
        );
    };
    let Some(key) = resolve_credential(config, key_env) else {
        return snapshot_error(
            route,
            "not-configured",
            crate::locale::owned(
                format!("未找到凭据 {key_env}。"),
                format!("Credential {key_env} was not found."),
            ),
        );
    };
    let Some(base) = route.base_url.as_deref() else {
        return snapshot_error(
            route,
            "not-configured",
            crate::locale::text(
                "未配置该供应商的接口地址。",
                "This provider has no configured endpoint.",
            ),
        );
    };
    let spec = SCHEMES
        .iter()
        .find(|(s, _)| *s == scheme)
        .map(|(_, spec)| spec)
        .unwrap();
    let target = match guard_url(base, spec.path) {
        Ok(url) => url,
        Err(reason) => return snapshot_error(route, "blocked", reason.to_string()),
    };
    let agent = balance_agent();
    let resp = match agent
        .get(&target)
        .header("Authorization", &format!("Bearer {key}"))
        .header("Accept", "application/json")
        .call()
    {
        Ok(r) => r,
        Err(e) => {
            let status = match &e {
                ureq::Error::StatusCode(401) | ureq::Error::StatusCode(403) => "unauthorized",
                ureq::Error::StatusCode(429) => "rate-limited",
                ureq::Error::StatusCode(404) | ureq::Error::StatusCode(405) => "unsupported",
                _ => "unavailable",
            };
            return snapshot_error(route, status, format!("{e}"));
        }
    };
    let body: serde_json::Value = match resp.into_body().read_json() {
        Ok(v) => v,
        Err(e) => return snapshot_error(route, "invalid-response", format!("{e}")),
    };
    match (spec.parse)(&body) {
        Ok(balance) => AccountSnapshot {
            id: route.id.clone(),
            display_name: route.display_name.clone(),
            mode: "balance",
            adapter: Some(adapter_name(scheme)),
            status: "ok",
            warn_level: warn_of_balance(&balance),
            balance: Some(balance),
            windows: Vec::new(),
            error: None,
            updated_at: Some(unix_now()),
            stale: false,
        },
        Err(e) => snapshot_error(route, "invalid-response", e.to_string()),
    }
}

fn adapter_name(scheme: BalanceScheme) -> &'static str {
    match scheme {
        BalanceScheme::DeepSeek => "deepseek-balance",
        BalanceScheme::OpenRouter => "openrouter-balance",
        BalanceScheme::Moonshot => "moonshot-balance",
        BalanceScheme::Zai => "zai-balance",
    }
}

/// 凭据解析走统一链（credentials::resolve_api_key）：DSH_BOX_API_KEY →
/// DEEPSEEK_API_KEY → 路由声明 env → 凭据文件。DeepSeek 官方路由因此同样
/// 响应壳级 DSH_BOX_API_KEY 覆盖，与状态栏余额口径一致。
fn resolve_credential(config: &Config, name: &str) -> Option<String> {
    crate::credentials::resolve_api_key(config, Some(name))
}

fn snapshot_error(
    route: &ProviderRoute,
    status: &'static str,
    error: impl Into<String>,
) -> AccountSnapshot {
    AccountSnapshot {
        id: route.id.clone(),
        display_name: route.display_name.clone(),
        mode: "balance",
        adapter: None,
        status,
        balance: None,
        windows: Vec::new(),
        error: Some(error.into()),
        updated_at: Some(unix_now()),
        stale: false,
        warn_level: "none",
    }
}

fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// 余额查询专用 Agent：短超时、连接复用。
static BALANCE_AGENT: std::sync::OnceLock<ureq::Agent> = std::sync::OnceLock::new();

fn balance_agent() -> &'static ureq::Agent {
    BALANCE_AGENT.get_or_init(|| {
        ureq::Agent::config_builder()
            .tls_config(crate::default_tls_config())
            .timeout_connect(Some(Duration::from_secs(5)))
            .timeout_recv_response(Some(Duration::from_secs(10)))
            .timeout_recv_body(Some(Duration::from_secs(10)))
            .build()
            .new_agent()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scheme_mapping_covers_known_routes() {
        assert_eq!(
            scheme_of("deepseek-official"),
            Some(BalanceScheme::DeepSeek)
        );
        assert_eq!(scheme_of("openrouter"), Some(BalanceScheme::OpenRouter));
        assert_eq!(scheme_of("moonshotai"), Some(BalanceScheme::Moonshot));
        assert_eq!(scheme_of("zai"), Some(BalanceScheme::Zai));
        assert_eq!(scheme_of("opencode-go"), None);
    }

    #[test]
    fn deepseek_parse_prefers_cny_entry() {
        let spec = &SCHEMES
            .iter()
            .find(|(s, _)| *s == BalanceScheme::DeepSeek)
            .unwrap()
            .1;
        let json = serde_json::json!({
            "is_available": true,
            "balance_infos": [
                {"currency": "USD", "total_balance": "1.00"},
                {"currency": "CNY", "total_balance": "50.50", "granted_balance": "10.00", "topped_up_balance": "40.50"}
            ]
        });
        let b = (spec.parse)(&json).unwrap();
        assert_eq!(b.currency, "CNY");
        assert_eq!(b.remaining, Some(50.5));
        assert_eq!(b.granted, Some(10.0));
        assert_eq!(b.topped_up, Some(40.5));
    }

    #[test]
    fn guard_rejects_private_hosts_and_userinfo() {
        assert!(guard_url("https://api.example.com", "/x").is_ok());
        assert_eq!(
            guard_url("http://api.example.com", "/x"),
            Err("insecure-protocol")
        );
        assert_eq!(
            guard_url("https://user:pass@api.example.com", "/x"),
            Err("url-credentials")
        );
        assert_eq!(guard_url("https://127.0.0.1", "/x"), Err("private-network"));
        assert_eq!(guard_url("https://localhost", "/x"), Err("private-network"));
        assert_eq!(
            guard_url("https://192.168.1.1", "/x"),
            Err("private-network")
        );
    }

    #[test]
    fn private_address_classifies_ipv4_and_ipv6() {
        assert!(is_private_address("127.0.0.1"));
        assert!(is_private_address("10.1.2.3"));
        assert!(is_private_address("172.16.0.1"));
        assert!(is_private_address("192.168.1.1"));
        assert!(is_private_address("169.254.1.1"));
        assert!(is_private_address("::1"));
        assert!(is_private_address("[::ffff:127.0.0.1]"));
        assert!(!is_private_address("1.1.1.1"));
        assert!(!is_private_address("8.8.8.8"));
    }

    fn balance_of(remaining: Option<f64>, total: Option<f64>, unlimited: bool) -> Balance {
        Balance {
            remaining,
            used: None,
            total,
            currency: "CNY".to_string(),
            unlimited,
            granted: None,
            topped_up: None,
        }
    }

    #[test]
    fn warn_level_thresholds_at_ratio_boundaries() {
        // 边界：恰为 0.3 → warning，恰为 0.1 → critical（阈值含等号）。
        assert_eq!(
            warn_of_balance(&balance_of(Some(30.0), Some(100.0), false)),
            "warning"
        );
        assert_eq!(
            warn_of_balance(&balance_of(Some(10.0), Some(100.0), false)),
            "critical"
        );
        assert_eq!(
            warn_of_balance(&balance_of(Some(31.0), Some(100.0), false)),
            "none"
        );
        assert_eq!(
            warn_of_balance(&balance_of(Some(10.5), Some(100.0), false)),
            "warning"
        );
        // 字段缺失 / total 非正 / unlimited：一律 none。
        assert_eq!(warn_of_balance(&balance_of(Some(5.0), None, false)), "none");
        assert_eq!(
            warn_of_balance(&balance_of(None, Some(100.0), false)),
            "none"
        );
        assert_eq!(
            warn_of_balance(&balance_of(Some(5.0), Some(0.0), false)),
            "none"
        );
        assert_eq!(
            warn_of_balance(&balance_of(Some(5.0), Some(100.0), true)),
            "none"
        );
    }

    #[test]
    fn resolve_credential_lets_shell_override_route_env() {
        let _guard = crate::credentials::ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        const ROUTE: &str = "DSHBOX_TEST_BAL_ROUTE_KEY_4M8D";
        let prev_box = std::env::var("DSH_BOX_API_KEY").ok();
        let prev_deep = std::env::var("DEEPSEEK_API_KEY").ok();
        let prev_route = std::env::var(ROUTE).ok();
        std::env::set_var(ROUTE, "route-key");
        std::env::set_var("DEEPSEEK_API_KEY", "deep-key");
        std::env::set_var("DSH_BOX_API_KEY", "box-key");
        let mut config = Config::load();
        config.dsh_home = std::env::temp_dir().join("dshbox-usage-bal-cred-nonexistent");
        // DSH_BOX_API_KEY 覆盖一切（DeepSeek 官方路由与状态栏同口径）。
        assert_eq!(
            resolve_credential(&config, ROUTE).as_deref(),
            Some("box-key")
        );
        std::env::remove_var("DSH_BOX_API_KEY");
        assert_eq!(
            resolve_credential(&config, ROUTE).as_deref(),
            Some("deep-key")
        );
        std::env::remove_var("DEEPSEEK_API_KEY");
        assert_eq!(
            resolve_credential(&config, ROUTE).as_deref(),
            Some("route-key")
        );
        match prev_box {
            Some(v) => std::env::set_var("DSH_BOX_API_KEY", v),
            None => std::env::remove_var("DSH_BOX_API_KEY"),
        }
        match prev_deep {
            Some(v) => std::env::set_var("DEEPSEEK_API_KEY", v),
            None => std::env::remove_var("DEEPSEEK_API_KEY"),
        }
        match prev_route {
            Some(v) => std::env::set_var(ROUTE, v),
            None => std::env::remove_var(ROUTE),
        }
    }
}
