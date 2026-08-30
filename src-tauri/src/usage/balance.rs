//! 供应商余额查询：按路由身份选用适配器，对上游账户接口做受限 HTTPS GET。
//!
//! 参考 dsh-usage-stats 的适配器契约（适配器名单、endpoint 相对路径与响应
//! 字段映射），以 Rust 独立实现。这些属于公开 API 事实（endpoint 路径 +
//! JSON 字段名），不复制其代码结构。
//!
//! 安全边界（与上游对齐，自托管网关放行见 guard 语义）：
//! - 仅 GET；https 放行任意主机，http 仅回环/私有地址（`net_guard` 单一口径）；
//! - 拒绝含 userinfo 的 URL；
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

#[derive(serde::Serialize, Clone, Debug)]
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

#[derive(serde::Serialize, Clone, Debug)]
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
    OrcaRouter,
    NewApi,
    Sub2Api,
}

/// 按路由 id 解析余额适配器。返回 None 表示该路由无公开余额接口。
/// New API / Sub2API 面板由用户把路由 id 命名为 `new-api` / `sub2api`
/// （或 passion 网关）；上游的主机名探测与面板指纹自动识别未移植。
fn scheme_of(route_id: &str) -> Option<BalanceScheme> {
    match route_id {
        "deepseek-official" | "deepseek" => Some(BalanceScheme::DeepSeek),
        "openrouter" => Some(BalanceScheme::OpenRouter),
        "moonshotai" | "moonshotai-cn" | "kimi" => Some(BalanceScheme::Moonshot),
        "zai" | "zai-coding-cn" => Some(BalanceScheme::Zai),
        "orcarouter" => Some(BalanceScheme::OrcaRouter),
        "new-api" | "newapi" => Some(BalanceScheme::NewApi),
        "sub2api" | "passion" => Some(BalanceScheme::Sub2Api),
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

/// 一位小数舍入（与 subscriptions::round1 同式，模块内自持避免互相依赖）。
fn round1(v: f64) -> f64 {
    (v * 10.0).round() / 10.0
}

/// 窗口预警级别（与 subscriptions::warn_of_windows 同阈值；类型为本模块
/// QuotaWindow，故单独实现）。
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
/// 再导出供 subscriptions.rs 等沿用原路径
pub(crate) use crate::net_guard::is_private_address;

/// 校验并构造余额请求 URL：委托 `net_guard::guard_https_or_lan_http`
/// （https 任意主机；http 仅回环/私有——自托管网关场景；拒绝 userinfo）。
fn guard_url(base: &str, path: &str) -> Result<String, &'static str> {
    crate::net_guard::guard_https_or_lan_http(base, path)
}

/// 查询一个路由的余额（同步、阻塞线程调用）。无 key / 无适配器时给出
/// 明确的不可用快照而非 error。
pub fn query_route(config: &Config, route: &ProviderRoute) -> AccountSnapshot {
    let adapter = scheme_of(&route.id);
    // 多请求/自托管面板适配器走专属流程（余额/窗口二选一输出）。
    match adapter {
        Some(BalanceScheme::OrcaRouter) => return query_orcarouter(config, route),
        Some(BalanceScheme::NewApi) => return query_new_api(config, route),
        Some(BalanceScheme::Sub2Api) => return query_sub2api(config, route),
        _ => {}
    }
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
    // OpenRouter 的 credits 端点用管理密钥（上游 v0.3.1 同款）：不回退路由
    // 推理 key——非管理密钥必然 401，还会把推理 key 发往 OpenRouter；也不
    // 吃 DSH_BOX_API_KEY/DEEPSEEK_API_KEY 壳级覆盖（那是 DeepSeek 专用链）。
    let (key_env, key) = if adapter == Some(BalanceScheme::OpenRouter) {
        (
            "OPENROUTER_MANAGEMENT_KEY",
            named_env_or_credentials(config, "OPENROUTER_MANAGEMENT_KEY"),
        )
    } else {
        match route.api_key_env.as_deref() {
            None => {
                return snapshot_error(
                    route,
                    "not-configured",
                    crate::locale::text(
                        "未配置该供应商的凭据。",
                        "No credential is configured for this provider.",
                    ),
                );
            }
            Some(env) => (env, resolve_credential(config, env)),
        }
    };
    let Some(key) = key else {
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
    let body: serde_json::Value = match crate::net_guard::read_json_capped(resp.into_body()) {
        Ok(v) => v,
        Err(e) => return snapshot_error(route, "invalid-response", e),
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
        BalanceScheme::OrcaRouter => "orcarouter-balance",
        BalanceScheme::NewApi => "new-api",
        BalanceScheme::Sub2Api => "sub2api",
    }
}

/// 带自定义头的 GET + JSON 读取（含 1 MiB cap 与状态归一）。
/// 404/405 归一为 "unsupported"（多端点适配器据此回退）。
fn fetch_json_headers(
    target: &str,
    headers: &[(&str, String)],
) -> Result<serde_json::Value, (&'static str, String)> {
    let mut request = balance_agent()
        .get(target)
        .header("Accept", "application/json");
    for (name, value) in headers {
        request = request.header(*name, value);
    }
    match request.call() {
        Ok(resp) => crate::net_guard::read_json_capped(resp.into_body())
            .map_err(|e| ("invalid-response", e)),
        Err(e) => {
            let status = match &e {
                ureq::Error::StatusCode(401) | ureq::Error::StatusCode(403) => "unauthorized",
                ureq::Error::StatusCode(429) => "rate-limited",
                ureq::Error::StatusCode(404) | ureq::Error::StatusCode(405) => "unsupported",
                _ => "unavailable",
            };
            Err((status, format!("{e}")))
        }
    }
}

fn fetch_json(target: &str, key: &str) -> Result<serde_json::Value, (&'static str, String)> {
    fetch_json_headers(target, &[("Authorization", format!("Bearer {key}"))])
}

// —— OrcaRouter ——（上游 v0.3.1：钱包端点优先，旧部署回退 OpenAI 形状
// 计费端点；`hard_limit_usd == 1e8` 且软/硬限额一致为不限量哨兵）

/// 计费端点统一挂在 `/v1` 前缀下：保留用户 baseURL 的 origin 与既有路径，
/// 缺 `/v1` 时补上（上游 orcaBillingURL 同款）。
fn orca_billing_url(base: &str, path: &str) -> String {
    let base = base.trim_end_matches('/');
    let prefix = if base.ends_with("/v1") {
        base.to_string()
    } else {
        format!("{base}/v1")
    };
    format!("{prefix}{path}")
}

fn orca_credit_total(
    value: Option<&serde_json::Value>,
    currency: &str,
    label: &'static str,
) -> Result<f64, String> {
    let Some(entries) = value.and_then(|v| v.as_array()) else {
        return Ok(0.0);
    };
    let mut total = 0.0;
    for entry in entries {
        let entry_currency = entry
            .get("unit")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|s| s.to_uppercase())
            .unwrap_or_else(|| currency.to_string());
        if entry_currency != currency {
            return Err(format!(
                "OrcaRouter {label} credits use a different currency"
            ));
        }
        let amount = num_field(entry, "balance_usd")
            .or_else(|| num_field(entry, "balance"))
            .filter(|v| *v >= 0.0)
            .ok_or_else(|| format!("OrcaRouter {label} credits are missing a numeric balance"))?;
        total += amount;
    }
    Ok(total)
}

fn parse_orca_wallet(body: &serde_json::Value) -> Result<Balance, String> {
    let currency = body
        .get("unit")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| s.to_uppercase())
        .ok_or_else(|| "OrcaRouter wallet response is missing currency".to_string())?;
    let paid = num_field(body, "paid_balance")
        .filter(|v| *v >= 0.0)
        .ok_or_else(|| "OrcaRouter wallet response is missing paid balance".to_string())?;
    let free = orca_credit_total(body.get("free_credit"), &currency, "free")?;
    let promo = orca_credit_total(body.get("promo_credits"), &currency, "promo")?;
    let remaining = paid + free + promo;
    Ok(Balance {
        remaining: Some(remaining),
        used: None,
        total: None,
        currency,
        unlimited: false,
        granted: None,
        topped_up: None,
    })
}

fn parse_orca_subscription(
    subscription: &serde_json::Value,
    usage: &serde_json::Value,
) -> Result<Balance, String> {
    let total = num_field(subscription, "hard_limit_usd")
        .or_else(|| num_field(subscription, "soft_limit_usd"))
        .filter(|v| *v >= 0.0)
        .ok_or_else(|| "OrcaRouter billing response is missing numeric quota data".to_string())?;
    // OpenAI 兼容计费的用量单位是美分
    let cents = num_field(usage, "total_usage")
        .filter(|v| *v >= 0.0)
        .ok_or_else(|| "OrcaRouter billing response is missing numeric quota data".to_string())?;
    let used = cents / 100.0;
    let unlimited = total == 100_000_000.0
        && num_field(subscription, "soft_limit_usd") == Some(total)
        && num_field(subscription, "system_hard_limit_usd") == Some(total);
    Ok(Balance {
        remaining: Some(if unlimited { total } else { total - used }),
        used: Some(used),
        total: if unlimited { None } else { Some(total) },
        currency: "USD".to_string(),
        unlimited,
        granted: None,
        topped_up: None,
    })
}

fn query_orcarouter(config: &Config, route: &ProviderRoute) -> AccountSnapshot {
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
    let key = match route.api_key_env.as_deref() {
        Some(env) => resolve_credential(config, env),
        None => named_env_or_credentials(config, "ORCAROUTER_API_KEY"),
    };
    let Some(key) = key else {
        return snapshot_error(
            route,
            "not-configured",
            crate::locale::owned(
                "未找到凭据 ORCAROUTER_API_KEY。".to_string(),
                "Credential ORCAROUTER_API_KEY was not found.".to_string(),
            ),
        );
    };
    let balance = (|| -> Result<Balance, (&'static str, String)> {
        // 先做 `/v1` 前缀归一再整串校验（guard 的 base+path 入口会二次拼接）
        let guard = |path: &'static str| -> Result<String, (&'static str, String)> {
            crate::net_guard::guard_full_url(&orca_billing_url(base, path))
                .map_err(|r| ("blocked", r.to_string()))
        };
        let wallet_url = guard("/balance")?;
        // 钱包端点不存在（unsupported）才回退旧版计费端点；回退端点的错误
        // 原样透传——unavailable 属瞬错，账户监测要按瞬错保旧（stale）。
        match fetch_json(&wallet_url, &key) {
            Ok(body) => parse_orca_wallet(&body).map_err(|e| ("invalid-response", e)),
            Err(("unsupported", _)) => {
                crate::logging::log("usage: orcarouter 钱包端点不存在，回退 OpenAI 形状计费端点");
                let subscription_url = guard("/dashboard/billing/subscription")?;
                let usage_url = guard("/dashboard/billing/usage")?;
                let subscription = fetch_json(&subscription_url, &key)?;
                let usage = fetch_json(&usage_url, &key)?;
                parse_orca_subscription(&subscription, &usage).map_err(|e| ("invalid-response", e))
            }
            Err(e) => Err(e),
        }
    })();
    match balance {
        Ok(balance) => AccountSnapshot {
            id: route.id.clone(),
            display_name: route.display_name.clone(),
            mode: "balance",
            adapter: Some(adapter_name(BalanceScheme::OrcaRouter)),
            status: "ok",
            warn_level: warn_of_balance(&balance),
            balance: Some(balance),
            windows: Vec::new(),
            error: None,
            updated_at: Some(unix_now()),
            stale: false,
        },
        Err((status, message)) => snapshot_error(route, status, message),
    }
}

// —— New API ——（上游 v0.3.1：token 端点 + /api/status 配额换算；
// 404/405 回退管理 PAT 的 /api/user/self）

/// New API 原始配额 → 展示金额：先按 quota_per_unit 折 USD，再按面板
/// 汇率折展示币种。
#[derive(Debug)]
struct NewApiQuotaStatus {
    quota_per_unit: f64,
    display: String,
    usd_exchange_rate: f64,
}

const LEGACY_NEW_API_QUOTA_PER_UNIT: f64 = 500_000.0;

fn legacy_new_api_quota_status() -> NewApiQuotaStatus {
    NewApiQuotaStatus {
        quota_per_unit: LEGACY_NEW_API_QUOTA_PER_UNIT,
        display: "USD".to_string(),
        usd_exchange_rate: 1.0,
    }
}

/// `/api/status` 的字段在 `data` 包内（上游 `body?.data?.quota_per_unit`）；
/// 缺 `data`（老面板）按 legacy 处理：500000 配额单位 + USD。
fn parse_new_api_quota_status(body: &serde_json::Value) -> Result<NewApiQuotaStatus, &'static str> {
    let Some(data) = body.get("data").filter(|d| d.is_object()) else {
        return Ok(legacy_new_api_quota_status());
    };
    let raw_unit = num_field(data, "quota_per_unit");
    let quota_per_unit = raw_unit
        .filter(|v| *v > 0.0)
        .unwrap_or(LEGACY_NEW_API_QUOTA_PER_UNIT);
    let display = data
        .get("quota_display_type")
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_uppercase())
        .unwrap_or_else(|| "USD".to_string());
    match display.as_str() {
        "USD" => Ok(NewApiQuotaStatus {
            quota_per_unit,
            display,
            usd_exchange_rate: 1.0,
        }),
        "CNY" => {
            let rate = num_field(data, "usd_exchange_rate")
                .filter(|v| *v > 0.0)
                .ok_or("invalid-response")?;
            Ok(NewApiQuotaStatus {
                quota_per_unit,
                display,
                usd_exchange_rate: rate,
            })
        }
        _ => Err("unsupported"),
    }
}

fn new_api_amount(value: Option<f64>, quota: &NewApiQuotaStatus) -> Option<f64> {
    value.map(|raw| raw / quota.quota_per_unit * quota.usd_exchange_rate)
}

fn query_new_api(config: &Config, route: &ProviderRoute) -> AccountSnapshot {
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
    let balance = (|| -> Result<Balance, (&'static str, String)> {
        let token_url = crate::net_guard::guard_https_or_lan_http(base, "/api/usage/token/")
            .map_err(|r| ("blocked", r.to_string()))?;
        let status_url = crate::net_guard::guard_https_or_lan_http(base, "/api/status")
            .map_err(|r| ("blocked", r.to_string()))?;
        let quota_status = match fetch_json_headers(&status_url, &[]) {
            Ok(body) => parse_new_api_quota_status(&body).map_err(|s| (s, s.to_string()))?,
            Err(("unsupported", _)) => legacy_new_api_quota_status(),
            Err(e) => return Err(e),
        };
        let body = match fetch_json(&token_url, &key) {
            Ok(body) => body,
            Err(("unsupported", _)) => {
                crate::logging::log(
                    "usage: new-api 面板无 token 端点，回退 /api/user/self（需 NEW_API_TOKEN）",
                );
                return new_api_fallback(config, base, &quota_status);
            }
            Err(e) => return Err(e),
        };
        if body.get("code").and_then(|v| v.as_bool()) != Some(true) {
            return Err((
                "invalid-response",
                "New API token response is invalid".to_string(),
            ));
        }
        let Some(data) = body.get("data").filter(|d| d.is_object()) else {
            return Err((
                "invalid-response",
                "New API token response is invalid".to_string(),
            ));
        };
        let unlimited = data.get("unlimited_quota").and_then(|v| v.as_bool()) == Some(true);
        let available = num_field(data, "total_available");
        if !unlimited && available.is_none() {
            return Err((
                "invalid-response",
                "New API token response is missing total_available".to_string(),
            ));
        }
        Ok(Balance {
            remaining: new_api_amount(available, &quota_status),
            used: new_api_amount(num_field(data, "total_used"), &quota_status),
            total: new_api_amount(num_field(data, "total_granted"), &quota_status),
            currency: quota_status.display.clone(),
            unlimited,
            granted: None,
            topped_up: None,
        })
    })();
    match balance {
        Ok(balance) => AccountSnapshot {
            id: route.id.clone(),
            display_name: route.display_name.clone(),
            mode: "balance",
            adapter: Some(adapter_name(BalanceScheme::NewApi)),
            status: "ok",
            warn_level: warn_of_balance(&balance),
            balance: Some(balance),
            windows: Vec::new(),
            error: None,
            updated_at: Some(unix_now()),
            stale: false,
        },
        Err((status, message)) => snapshot_error(route, status, message),
    }
}

/// 管理回退：`/api/user/self` 用管理 PAT（`NEW_API_TOKEN`），可选
/// `NEW_API_USER_ID` 请求头。旧面板没有 token 端点时走这条路。
fn new_api_fallback(
    config: &Config,
    base: &str,
    quota_status: &NewApiQuotaStatus,
) -> Result<Balance, (&'static str, String)> {
    let Some(token) = named_env_or_credentials(config, "NEW_API_TOKEN") else {
        return Err((
            "unsupported",
            "New API panel has no token endpoint; NEW_API_TOKEN is required".to_string(),
        ));
    };
    let mut headers = vec![("Authorization", format!("Bearer {token}"))];
    if let Some(user_id) = named_env_or_credentials(config, "NEW_API_USER_ID") {
        headers.push(("new-api-user", user_id));
    }
    let target = crate::net_guard::guard_https_or_lan_http(base, "/api/user/self")
        .map_err(|r| ("blocked", r.to_string()))?;
    let body = fetch_json_headers(&target, &headers)?;
    if body.get("success").and_then(|v| v.as_bool()) == Some(false) {
        return Err((
            "invalid-response",
            "New API user response is invalid".to_string(),
        ));
    }
    let Some(data) = body.get("data").filter(|d| d.is_object()) else {
        return Err((
            "invalid-response",
            "New API user response is invalid".to_string(),
        ));
    };
    let remaining_quota = num_field(data, "quota");
    let used_quota = num_field(data, "used_quota");
    let Some(remaining_quota) = remaining_quota else {
        return Err((
            "invalid-response",
            "New API user response is missing quota".to_string(),
        ));
    };
    let total =
        used_quota.and_then(|used| new_api_amount(Some(remaining_quota + used), quota_status));
    Ok(Balance {
        remaining: new_api_amount(Some(remaining_quota), quota_status),
        used: new_api_amount(used_quota, quota_status),
        total,
        currency: quota_status.display.clone(),
        unlimited: false,
        granted: None,
        topped_up: None,
    })
}

// —— Sub2API ——（上游 v0.3.1：/v1/usage 同端点出余额或配额窗口，
// quota_limited 模式含 quota + rate_limits 窗口；passion 网关同协议）

/// 数值窗口（上游 amountWindow）：limit 必须 >0；used 缺失时按
/// limit-remaining 推导。
fn amount_window(
    kind: &str,
    used: Option<f64>,
    limit: Option<f64>,
    remaining: Option<f64>,
    resets_at_ms: Option<f64>,
) -> Option<QuotaWindow> {
    let limit = limit?;
    if limit <= 0.0 {
        return None;
    }
    let used = used.or_else(|| remaining.map(|r| limit - r))?;
    let used_percent = round1((used / limit * 100.0).clamp(0.0, 100.0));
    Some(QuotaWindow {
        kind: kind.to_string(),
        used_percent,
        remaining_percent: round1(100.0 - used_percent),
        resets_at: super::subscriptions::to_iso(resets_at_ms),
    })
}

fn sub2api_window_kind(value: &serde_json::Value) -> String {
    match value.as_str().map(str::trim) {
        Some("5h") => "session".to_string(),
        Some("1d") => "daily".to_string(),
        Some("7d") => "weekly".to_string(),
        other => other.unwrap_or("quota").to_string(),
    }
}

/// 解析 /v1/usage → 余额或窗口。Err(("unauthorized", ..)) 表示 key 无效。
fn parse_sub2api_usage(body: &serde_json::Value) -> Result<Sub2ApiOutcome, (&'static str, String)> {
    if !body.is_object() {
        return Err((
            "invalid-response",
            "Sub2API response must be an object".to_string(),
        ));
    }
    if body.get("isValid").and_then(|v| v.as_bool()) == Some(false)
        || body.get("is_active").and_then(|v| v.as_bool()) == Some(false)
    {
        return Err(("unauthorized", "Sub2API key is invalid".to_string()));
    }
    let has_subscription = body.get("subscription").is_some_and(|s| s.is_object());
    if body.get("mode").and_then(|v| v.as_str()) == Some("quota_limited") || has_subscription {
        let mut windows = Vec::new();
        if body.get("mode").and_then(|v| v.as_str()) == Some("quota_limited") {
            if let Some(quota) = body.get("quota").filter(|q| q.is_object()) {
                if let Some(window) = amount_window(
                    "quota",
                    num_field(quota, "used"),
                    num_field(quota, "limit"),
                    num_field(quota, "remaining"),
                    num_field(body, "expires_at"),
                ) {
                    windows.push(window);
                }
                if let Some(rate_limits) = body.get("rate_limits").and_then(|v| v.as_array()) {
                    for entry in rate_limits {
                        if !entry.is_object() {
                            continue;
                        }
                        if let Some(window) = amount_window(
                            &sub2api_window_kind(
                                entry.get("window").unwrap_or(&serde_json::Value::Null),
                            ),
                            num_field(entry, "used"),
                            num_field(entry, "limit"),
                            num_field(entry, "remaining"),
                            num_field(entry, "reset_at"),
                        ) {
                            windows.push(window);
                        }
                    }
                }
            }
        } else if let Some(subscription) = body.get("subscription") {
            for period in ["daily", "weekly", "monthly"] {
                if let Some(window) = amount_window(
                    period,
                    num_field(subscription, &format!("{period}_usage_usd")),
                    num_field(subscription, &format!("{period}_limit_usd")),
                    None,
                    None,
                ) {
                    windows.push(window);
                }
            }
        }
        if windows.is_empty() {
            return Err((
                "invalid-response",
                "Sub2API response has no usable quota windows".to_string(),
            ));
        }
        return Ok(Sub2ApiOutcome::Windows(windows));
    }
    let remaining = num_field(body, "balance")
        .or_else(|| num_field(body, "remaining"))
        .ok_or_else(|| {
            (
                "invalid-response",
                "Sub2API response is missing a numeric balance".to_string(),
            )
        })?;
    Ok(Sub2ApiOutcome::Balance(Balance {
        remaining: Some(remaining),
        used: None,
        total: None,
        currency: body
            .get("unit")
            .and_then(|v| v.as_str())
            .unwrap_or("USD")
            .to_string(),
        unlimited: false,
        granted: None,
        topped_up: None,
    }))
}

#[derive(Debug)]
enum Sub2ApiOutcome {
    Balance(Balance),
    Windows(Vec<QuotaWindow>),
}

fn query_sub2api(config: &Config, route: &ProviderRoute) -> AccountSnapshot {
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
    let outcome = (|| -> Result<Sub2ApiOutcome, (&'static str, String)> {
        let target = crate::net_guard::guard_https_or_lan_http(base, "/v1/usage")
            .map_err(|r| ("blocked", r.to_string()))?;
        let body = fetch_json(&target, &key)?;
        parse_sub2api_usage(&body)
    })();
    match outcome {
        Ok(Sub2ApiOutcome::Balance(balance)) => AccountSnapshot {
            id: route.id.clone(),
            display_name: route.display_name.clone(),
            mode: "balance",
            adapter: Some(adapter_name(BalanceScheme::Sub2Api)),
            status: "ok",
            warn_level: warn_of_balance(&balance),
            balance: Some(balance),
            windows: Vec::new(),
            error: None,
            updated_at: Some(unix_now()),
            stale: false,
        },
        Ok(Sub2ApiOutcome::Windows(windows)) => AccountSnapshot {
            id: route.id.clone(),
            display_name: route.display_name.clone(),
            mode: "subscription",
            adapter: Some(adapter_name(BalanceScheme::Sub2Api)),
            status: "ok",
            warn_level: warn_of_windows(&windows),
            balance: None,
            windows,
            error: None,
            updated_at: Some(unix_now()),
            stale: false,
        },
        Err((status, message)) => snapshot_error(route, status, message),
    }
}

/// 凭据解析走统一链（credentials::resolve_api_key）：DSH_BOX_API_KEY →
/// DEEPSEEK_API_KEY → 路由声明 env → 凭据文件。DeepSeek 官方路由因此同样
/// 响应壳级 DSH_BOX_API_KEY 覆盖，与状态栏余额口径一致。
fn resolve_credential(config: &Config, name: &str) -> Option<String> {
    crate::credentials::resolve_api_key(config, Some(name))
}

/// 具名环境变量 → 凭据文件（OpenRouter 管理密钥等非 DeepSeek 链）。
fn named_env_or_credentials(config: &Config, name: &str) -> Option<String> {
    if let Ok(value) = std::env::var(name) {
        let value = value.trim().to_string();
        if !value.is_empty() {
            return Some(value);
        }
    }
    crate::credentials::value(config, name)
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
    fn guard_allows_https_and_lan_http_but_rejects_public_http() {
        assert!(guard_url("https://api.example.com", "/x").is_ok());
        // 自托管网关：私网 http/https 放行
        assert!(guard_url("http://127.0.0.1:3000", "/x").is_ok());
        assert!(guard_url("http://192.168.1.10:3000", "/x").is_ok());
        assert!(guard_url("https://10.0.0.8", "/x").is_ok());
        assert_eq!(
            guard_url("http://api.example.com", "/x"),
            Err("insecure-protocol")
        );
        assert_eq!(
            guard_url("https://user:pass@api.example.com", "/x"),
            Err("url-credentials")
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

    #[test]
    fn scheme_mapping_covers_new_adapters() {
        assert_eq!(scheme_of("orcarouter"), Some(BalanceScheme::OrcaRouter));
        assert_eq!(scheme_of("new-api"), Some(BalanceScheme::NewApi));
        assert_eq!(scheme_of("newapi"), Some(BalanceScheme::NewApi));
        assert_eq!(scheme_of("sub2api"), Some(BalanceScheme::Sub2Api));
        assert_eq!(scheme_of("passion"), Some(BalanceScheme::Sub2Api));
    }

    #[test]
    fn orca_billing_url_appends_v1_prefix() {
        assert_eq!(
            orca_billing_url("https://api.orcarouter.ai", "/balance"),
            "https://api.orcarouter.ai/v1/balance"
        );
        assert_eq!(
            orca_billing_url("https://host/v1/", "/balance"),
            "https://host/v1/balance"
        );
        assert_eq!(
            orca_billing_url("https://host/proxy", "/dashboard/billing/usage"),
            "https://host/proxy/v1/dashboard/billing/usage"
        );
    }

    #[test]
    fn orca_wallet_sums_credits_and_checks_currency() {
        let body = serde_json::json!({
            "unit": "usd",
            "paid_balance": 10.5,
            "free_credit": [{"unit": "USD", "balance_usd": 2.0}],
            "promo_credits": [{"balance": 1.5}, {"balance": 0.5}]
        });
        let b = parse_orca_wallet(&body).unwrap();
        assert_eq!(b.currency, "USD");
        assert_eq!(b.remaining, Some(14.5));

        let mixed = serde_json::json!({
            "unit": "USD", "paid_balance": 1.0,
            "free_credit": [{"unit": "CNY", "balance_usd": 2.0}]
        });
        assert!(parse_orca_wallet(&mixed).is_err());
    }

    #[test]
    fn orca_subscription_converts_cents_and_detects_unlimited() {
        let sub = serde_json::json!({
            "hard_limit_usd": 20.0, "soft_limit_usd": 20.0, "system_hard_limit_usd": 20.0
        });
        let usage = serde_json::json!({ "total_usage": 500 });
        let b = parse_orca_subscription(&sub, &usage).unwrap();
        assert_eq!(b.used, Some(5.0));
        assert_eq!(b.remaining, Some(15.0));
        assert!(!b.unlimited);

        let unlimited_sub = serde_json::json!({
            "hard_limit_usd": 100_000_000.0,
            "soft_limit_usd": 100_000_000.0,
            "system_hard_limit_usd": 100_000_000.0
        });
        let b = parse_orca_subscription(&unlimited_sub, &usage).unwrap();
        assert!(b.unlimited);
        assert_eq!(b.remaining, Some(100_000_000.0));
        assert_eq!(b.total, None);
    }

    #[test]
    fn new_api_quota_status_converts_currency() {
        // `/api/status` 的字段在 data 包内；缺 data / 缺字段回退 legacy
        // （500000 配额单位 + USD）——完整信封直接传入，不预拆 data
        let legacy = serde_json::json!({});
        let q = parse_new_api_quota_status(&legacy).unwrap();
        assert_eq!(q.display, "USD");
        assert_eq!(q.quota_per_unit, 500_000.0);
        assert_eq!(q.usd_exchange_rate, 1.0);
        let no_data_fields =
            serde_json::json!({ "quota_per_unit": 999, "quota_display_type": "CNY" });
        let q = parse_new_api_quota_status(&no_data_fields).unwrap();
        assert_eq!(q.display, "USD", "顶层字段不得替代 data 包");

        let cny = serde_json::json!({
            "data": { "quota_per_unit": 500000, "quota_display_type": "CNY", "usd_exchange_rate": 7.2 }
        });
        let q = parse_new_api_quota_status(&cny).unwrap();
        assert_eq!(q.display, "CNY");
        assert_eq!(q.usd_exchange_rate, 7.2);
        // 原始配额 500000 → 1 USD → 7.2 CNY
        assert_eq!(new_api_amount(Some(500_000.0), &q), Some(7.2));

        // CNY 但汇率缺失/非正 → invalid
        let bad = serde_json::json!({
            "data": { "quota_display_type": "CNY", "usd_exchange_rate": 0 }
        });
        assert_eq!(
            parse_new_api_quota_status(&bad).unwrap_err(),
            "invalid-response"
        );
        // 未知展示币种 → unsupported
        let other = serde_json::json!({ "data": { "quota_display_type": "EUR" } });
        assert_eq!(
            parse_new_api_quota_status(&other).unwrap_err(),
            "unsupported"
        );
    }

    #[test]
    fn orca_guarded_wallet_url_has_no_double_path() {
        // 回归：guard 的 base+path 入口与 orca_billing_url 组合曾把路径拼两次
        // （host/balance/v1/balance）。先归一 /v1 再整串校验必须得到唯一路径。
        let base = "https://api.orcarouter.ai";
        let wallet = crate::net_guard::guard_full_url(&orca_billing_url(base, "/balance")).unwrap();
        assert_eq!(wallet, "https://api.orcarouter.ai/v1/balance");
        let usage =
            crate::net_guard::guard_full_url(&orca_billing_url(base, "/dashboard/billing/usage"))
                .unwrap();
        assert_eq!(
            usage,
            "https://api.orcarouter.ai/v1/dashboard/billing/usage"
        );
        // 自托管 LAN（http 私网）同样不得双拼
        let lan = crate::net_guard::guard_full_url(&orca_billing_url(
            "http://10.0.0.8:3000/proxy",
            "/balance",
        ))
        .unwrap();
        assert_eq!(lan, "http://10.0.0.8:3000/proxy/v1/balance");
    }

    #[test]
    fn sub2api_usage_yields_windows_or_balance() {
        let quota_limited = serde_json::json!({
            "mode": "quota_limited",
            "quota": {"used": 4.0, "limit": 10.0, "remaining": 6.0},
            "rate_limits": [
                {"window": "5h", "used": 1.0, "limit": 5.0, "remaining": 4.0},
                {"window": "7d", "used": 2.5, "limit": 50.0}
            ],
            "expires_at": 1785542400
        });
        match parse_sub2api_usage(&quota_limited).unwrap() {
            Sub2ApiOutcome::Windows(windows) => {
                assert_eq!(windows.len(), 3);
                assert_eq!(windows[0].kind, "quota");
                assert_eq!(windows[0].used_percent, 40.0);
                assert_eq!(windows[1].kind, "session");
                assert_eq!(windows[2].kind, "weekly");
                assert_eq!(windows[2].used_percent, 5.0);
                assert_eq!(
                    windows[0].resets_at.as_deref(),
                    Some("2026-08-01T00:00:00Z")
                );
            }
            other => panic!("expected windows, got balance: {other:?}"),
        }

        let subscription = serde_json::json!({
            "subscription": {
                "daily_usage_usd": 3.0, "daily_limit_usd": 10.0,
                "monthly_usage_usd": 60.0, "monthly_limit_usd": 100.0
            }
        });
        match parse_sub2api_usage(&subscription).unwrap() {
            Sub2ApiOutcome::Windows(windows) => {
                assert_eq!(windows.len(), 2);
                assert_eq!(windows[0].kind, "daily");
                assert_eq!(windows[1].kind, "monthly");
                assert_eq!(windows[1].used_percent, 60.0);
            }
            other => panic!("expected windows, got balance: {other:?}"),
        }

        let balance = serde_json::json!({ "balance": 12.5, "unit": "USD" });
        match parse_sub2api_usage(&balance).unwrap() {
            Sub2ApiOutcome::Balance(b) => {
                assert_eq!(b.remaining, Some(12.5));
                assert_eq!(b.currency, "USD");
            }
            other => panic!("expected balance, got windows: {other:?}"),
        }

        let invalid_key = serde_json::json!({ "isValid": false });
        assert_eq!(
            parse_sub2api_usage(&invalid_key).unwrap_err().0,
            "unauthorized"
        );
    }

    #[test]
    fn sub2api_amount_window_requires_positive_limit() {
        assert!(amount_window("quota", Some(1.0), Some(0.0), None, None).is_none());
        assert!(amount_window("quota", None, Some(10.0), None, None).is_none());
        // used 缺失时按 limit-remaining 推导
        let w = amount_window("quota", None, Some(10.0), Some(7.5), None).unwrap();
        assert_eq!(w.used_percent, 25.0);
    }

    #[test]
    fn named_env_or_credentials_prefers_env() {
        let _guard = crate::credentials::ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        const NAME: &str = "DSHBOX_TEST_NAMED_KEY_7K2F";
        let prev = std::env::var(NAME).ok();
        std::env::remove_var(NAME);
        // 环境未提供且凭据文件按该名查不到（随机名）→ None
        assert!(named_env_or_credentials(&Config::load(), NAME).is_none());
        std::env::set_var(NAME, " mgmt-key-with-spaces ");
        assert_eq!(
            named_env_or_credentials(&Config::load(), NAME).as_deref(),
            Some("mgmt-key-with-spaces")
        );
        match prev {
            Some(v) => std::env::set_var(NAME, v),
            None => std::env::remove_var(NAME),
        }
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
