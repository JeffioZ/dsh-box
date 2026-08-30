//! DeepSeek API 余额查询：GET {base}/user/balance。
//!
//! API Key 解析顺序：DSH_BOX_API_KEY → DEEPSEEK_API_KEY → dsh 凭据文件。

use serde::{Deserialize, Serialize};
use std::sync::OnceLock;
use std::time::Duration;
use tauri::{AppHandle, Manager};

use crate::app_state::{AppState, Config};

/// 余额查询专用客户端（连接复用）：短超时 + 系统 TLS。
/// 复用一个 Agent 让 TLS/连接在多次查询间复用——每次新建 agent 会导致
/// 内网环境每次打开弹窗都重新 TLS 握手（数秒），正是"打开 loading 久、
/// 手动刷新快"的根因。
static BALANCE_AGENT: OnceLock<ureq::Agent> = OnceLock::new();

fn balance_agent() -> &'static ureq::Agent {
    BALANCE_AGENT.get_or_init(|| {
        ureq::Agent::config_builder()
            .tls_config(crate::default_tls_config())
            .timeout_connect(Some(Duration::from_secs(5)))
            .timeout_recv_response(Some(Duration::from_secs(5)))
            .timeout_recv_body(Some(Duration::from_secs(10)))
            .build()
            .new_agent()
    })
}

#[derive(Serialize, Clone)]
pub struct BalancePayload {
    pub ok: bool,
    pub is_available: bool,
    pub balances: Vec<BalanceEntry>,
    pub error: Option<String>,
    /// 错误类别（no_key / invalid_key），供前端差异化展示；其他错误为 None
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_kind: Option<String>,
    /// 查询完成时刻（Unix 秒），浮层显示“更新于 HH:MM”
    #[serde(skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<u64>,
}

/// 余额查询错误分类。
enum BalanceError {
    /// 未配置 API Key（error 为引导性文案）。
    NoKey(String),
    /// API Key 无效（HTTP 401/403）。
    InvalidKey,
    /// 其他错误。
    Other(String),
}

#[derive(Serialize, Clone)]
pub struct BalanceEntry {
    pub currency: String,
    pub total_balance: String,
    pub granted_balance: String,
    pub topped_up_balance: String,
    /// 机器可读剩余额度（total_balance 的数值解析，供状态栏 chip 预警色
    /// 计算 ratio；字符串字段保持原样，解析失败为 None）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remaining: Option<f64>,
    /// 机器可读总额。DeepSeek 余额接口无「总额」概念，恒 None——
    /// 前端在 total 缺失时不渲染预警色（不造假预警）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total: Option<f64>,
}

#[derive(Deserialize)]
struct RawBalance {
    is_available: bool,
    #[serde(default)]
    balance_infos: Vec<RawBalanceInfo>,
}

#[derive(Deserialize)]
struct RawBalanceInfo {
    currency: String,
    #[serde(default)]
    total_balance: String,
    #[serde(default)]
    granted_balance: String,
    #[serde(default)]
    topped_up_balance: String,
}

fn resolve_api_key(config: &Config) -> Result<String, String> {
    // 链解析统一收在 credentials::resolve_api_key（无路由声明：DSH_BOX_API_KEY
    // → DEEPSEEK_API_KEY → 凭据文件 DEEPSEEK_API_KEY，与原实现行为等价）。
    crate::credentials::resolve_api_key(config, None).ok_or_else(|| {
        crate::locale::text(
            "未找到 DeepSeek API Key。\n请在应用设置中配置，或设置 DSH_BOX_API_KEY / DEEPSEEK_API_KEY 环境变量。",
            "No DeepSeek API key was found.\nConfigure it in the app, or set DSH_BOX_API_KEY / DEEPSEEK_API_KEY.",
        )
        .into()
    })
}

/// 同步查询并记录日志（托盘线程直接调用）。
///
/// 查询路径：账户后台监测缓存中有新鲜（< ACCOUNT_REFRESH_MS）的 DeepSeek
/// 官方路由快照时直接转换复用（状态栏与监测同查一个接口，避免双通道重复
/// 请求）；缓存空/过期才回退直连查询。
pub(crate) fn query_balance(config: &Config) -> BalancePayload {
    if let Some(snapshot) = crate::usage::cached_deepseek() {
        crate::logging::log("balance: 复用账户监测缓存快照");
        return payload_from_snapshot(&snapshot);
    }
    query_direct(config)
}

/// 直连查询并记录日志（缓存空/过期时的回退路径）。
fn query_direct(config: &Config) -> BalancePayload {
    match query(config) {
        Ok(mut payload) => {
            payload.updated_at = Some(unix_now());
            // 日志只记录账户可用状态，不写入余额金额
            crate::logging::log(&format!(
                "balance: 查询成功 is_available={}",
                payload.is_available
            ));
            payload
        }
        Err(e) => {
            let (kind, msg) = match e {
                BalanceError::NoKey(m) => (Some("no_key"), m),
                BalanceError::InvalidKey => (
                    Some("invalid_key"),
                    crate::locale::text(
                        "API Key 无效，请检查配置的密钥是否正确。",
                        "The API key is invalid. Check the configured key.",
                    )
                    .to_string(),
                ),
                BalanceError::Other(m) => (None, m),
            };
            crate::logging::log(&format!("balance: 查询失败 {msg}"));
            BalancePayload {
                ok: false,
                is_available: false,
                balances: Vec::new(),
                error: Some(msg),
                error_kind: kind.map(String::from),
                updated_at: Some(unix_now()),
            }
        }
    }
}

/// 当前 Unix 秒（无第三方时间库）。
fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// 监测快照 → 状态栏余额载荷（字段映射对齐现有契约：remaining/total 为
/// 机器可读值，字符串金额保留两位小数；updated_at 沿用快照的查询完成时刻）。
fn payload_from_snapshot(snapshot: &crate::usage::AccountSnapshot) -> BalancePayload {
    if snapshot.status == "ok" {
        if let Some(balance) = &snapshot.balance {
            return BalancePayload {
                ok: true,
                // DeepSeek 语义：is_available = 账户余额大于 0；监测快照不存
                // 该标志，按同口径由 remaining 推导（remaining 缺失视为可用）。
                is_available: balance.remaining.map(|r| r > 0.0).unwrap_or(true),
                balances: vec![BalanceEntry {
                    currency: balance.currency.clone(),
                    total_balance: fmt_amount(balance.remaining),
                    granted_balance: fmt_amount(balance.granted),
                    topped_up_balance: fmt_amount(balance.topped_up),
                    remaining: balance.remaining,
                    total: balance.total,
                }],
                error: None,
                error_kind: None,
                updated_at: snapshot.updated_at,
            };
        }
    }
    BalancePayload {
        ok: false,
        is_available: false,
        balances: Vec::new(),
        error: Some(
            snapshot.error.clone().unwrap_or_else(|| {
                crate::locale::text("查询余额失败", "Balance query failed").into()
            }),
        ),
        // 与直连路径同一口径：401/403 → invalid_key、缺凭据 → no_key；
        // 其余瞬态/解析错误不归类（前端按通用失败 + stale 保留处理）。
        error_kind: match snapshot.status {
            "unauthorized" => Some("invalid_key".to_string()),
            "not-configured" => Some("no_key".to_string()),
            _ => None,
        },
        updated_at: snapshot.updated_at,
    }
}

/// 金额格式化为两位小数字符串（None → 空串，与直连解析的缺省一致）。
fn fmt_amount(value: Option<f64>) -> String {
    value.map(|v| format!("{v:.2}")).unwrap_or_default()
}

static REFRESH_STARTED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// 后台周期刷新余额：立即查询一次，此后每 5 分钟刷新并广播 balance-updated
/// （标题栏余额 chip 消费）。只启动一次；就绪门控统一走
/// background::service_gate（仅本地托管且 Ready 放行）。界面隐藏/余额
/// 隐藏时任务体直接跳过，节奏退化为 5 分钟一轮空转。
pub(crate) fn start_periodic_refresh(app: AppHandle) {
    use std::sync::atomic::Ordering;
    if REFRESH_STARTED.swap(true, Ordering::SeqCst) {
        return;
    }
    crate::background::spawn_gated_periodic(
        app,
        "balance-refresh",
        std::time::Duration::from_secs(300),
        std::time::Duration::from_secs(5),
        |app| {
            let config = app.state::<AppState>().config();
            if config.hide_statusbar || config.hide_balance || !crate::main_is_visible(app) {
                return;
            }
            let payload = query_balance(&config);
            crate::emit_signed(app, "balance-updated", &payload);
        },
    );
}

/// 立即查询并广播一次余额（状态栏首帧数据，不等 5 分钟轮询周期）。
pub(crate) fn refresh_once(app: AppHandle) {
    std::thread::spawn(move || {
        let state = app.state::<AppState>();
        if state.service_ownership().is_external() {
            return;
        }
        if state.service_ownership() != crate::app_state::ServiceOwnership::Managed
            || state.phase() != crate::app_state::BootPhase::Ready
        {
            return;
        }
        let config = state.config();
        if !config.hide_statusbar && !config.hide_balance && crate::main_is_visible(&app) {
            let payload = query_balance(&config);
            crate::emit_signed(&app, "balance-updated", &payload);
        }
    });
}

#[tauri::command]
pub async fn api_balance(app: AppHandle, webview: tauri::Webview) -> BalancePayload {
    if let Err(error) = crate::commands::ensure_local_origin(&webview) {
        return denied_payload(error);
    }
    if app.state::<AppState>().service_ownership().is_external() {
        return denied_payload(
            crate::locale::text(
                "余额使用外部 dsh 的凭据，请在原服务环境中查询。",
                        "The external dsh service manages the credentials used for balance queries. Check the balance in that service's environment.",
            )
            .into(),
        );
    }
    run_balance_query(app).await
}

fn denied_payload(error: String) -> BalancePayload {
    BalancePayload {
        ok: false,
        is_available: false,
        balances: Vec::new(),
        error: Some(error),
        error_kind: None,
        updated_at: None,
    }
}

/// 公共查询：网络请求放到阻塞线程，避免占用主线程/异步工作线程。
async fn run_balance_query(app: AppHandle) -> BalancePayload {
    let config = app.state::<AppState>().config();
    tauri::async_runtime::spawn_blocking(move || query_balance(&config))
        .await
        .unwrap_or_else(|e| {
            crate::logging::log(&format!("balance: 线程异常 {e}"));
            denied_payload(
                crate::locale::text(
                    "余额查询失败（内部错误）",
                    "Balance query failed (internal error)",
                )
                .into(),
            )
        })
}

/// 校验并构造余额查询 URL：委托 `net_guard::guard_https_or_lan_http`——
/// 与 usage/balance.rs 的 guard_url 现在真正共用同一实现（历史上两处
/// 各写一份且口径漂移过，已收敛到 net_guard 单一定义点）。
fn guarded_balance_url(api_base: &str) -> Result<String, BalanceError> {
    crate::net_guard::guard_https_or_lan_http(api_base, "/user/balance")
        .map_err(|_| invalid_balance_url(api_base))
}

fn invalid_balance_url(api_base: &str) -> BalanceError {
    BalanceError::Other(format!(
        "{}: {api_base}",
        crate::locale::text(
            "API 地址无效（仅支持 https，或 http 的回环/局域网地址）",
            "Invalid API base URL (only HTTPS, or HTTP to a loopback/private address, is allowed)"
        )
    ))
}

/// 金额字符串解析为 f64（空串/非数值/非有限值一律 None，不向客户端
/// 吐 NaN/Infinity——serde_json 无法序列化非有限浮点）。
fn parse_amount(raw: &str) -> Option<f64> {
    let value = raw.trim().parse::<f64>().ok()?;
    value.is_finite().then_some(value)
}

fn query(config: &Config) -> Result<BalancePayload, BalanceError> {
    let key = resolve_api_key(config).map_err(BalanceError::NoKey)?;
    let url = guarded_balance_url(&config.api_base)?;
    // 短超时 + 连接复用（见 balance_agent 注释）
    let resp = balance_agent()
        .get(&url)
        .header("Authorization", &format!("Bearer {key}"))
        .call()
        .map_err(|e| {
            if matches!(
                e,
                ureq::Error::StatusCode(401) | ureq::Error::StatusCode(403)
            ) {
                BalanceError::InvalidKey
            } else {
                BalanceError::Other(format!(
                    "{}: {e}",
                    crate::locale::text("查询余额失败", "Balance query failed")
                ))
            }
        })?;
    let value = crate::net_guard::read_json_capped(resp.into_body()).map_err(|e| {
        BalanceError::Other(format!(
            "{}: {e}",
            crate::locale::text("解析余额响应失败", "Failed to parse the balance response")
        ))
    })?;
    let raw: RawBalance = serde_json::from_value(value).map_err(|e| {
        BalanceError::Other(format!(
            "{}: {e}",
            crate::locale::text("解析余额响应失败", "Failed to parse the balance response")
        ))
    })?;
    let balances = raw
        .balance_infos
        .into_iter()
        .map(|b| BalanceEntry {
            remaining: parse_amount(&b.total_balance),
            currency: b.currency,
            total_balance: b.total_balance,
            granted_balance: b.granted_balance,
            topped_up_balance: b.topped_up_balance,
            total: None,
        })
        .collect();
    Ok(BalancePayload {
        ok: true,
        is_available: raw.is_available,
        balances,
        error: None,
        error_kind: None,
        updated_at: None,
    })
}

#[cfg(test)]
mod tests {
    use super::BalancePayload;

    #[test]
    fn frontend_contract_serializes_error_kind_in_snake_case() {
        let payload = BalancePayload {
            ok: false,
            is_available: false,
            balances: Vec::new(),
            error: Some("missing".into()),
            error_kind: Some("no_key".into()),
            updated_at: None,
        };
        let json = serde_json::to_value(payload).unwrap();
        assert_eq!(json["error_kind"], "no_key");
        assert!(json.get("errorKind").is_none());
    }

    #[test]
    fn balance_entry_exposes_machine_readable_amounts_in_snake_case() {
        let entry = super::BalanceEntry {
            currency: "CNY".into(),
            total_balance: "50.50".into(),
            granted_balance: "10.00".into(),
            topped_up_balance: "40.50".into(),
            remaining: super::parse_amount("50.50"),
            total: None,
        };
        let json = serde_json::to_value(entry).unwrap();
        assert_eq!(json["remaining"], 50.5);
        assert!(json.get("total").is_none(), "total 为 None 时跳过序列化");
        assert!(json.get("totalBalance").is_none());
        // 字符串字段原样保留（线协议不变）。
        assert_eq!(json["total_balance"], "50.50");
    }

    #[test]
    fn parse_amount_rejects_blank_invalid_and_non_finite() {
        assert_eq!(super::parse_amount("50.50"), Some(50.5));
        assert_eq!(super::parse_amount(" 12.5 "), Some(12.5));
        assert_eq!(super::parse_amount(""), None);
        assert_eq!(super::parse_amount("abc"), None);
        assert_eq!(super::parse_amount("NaN"), None);
        assert_eq!(super::parse_amount("inf"), None);
    }

    /// 构造一条 DeepSeek 官方路由的监测快照（status 可换）。
    fn snapshot_of(status: &'static str, updated_at: u64) -> crate::usage::AccountSnapshot {
        let ok = status == "ok";
        crate::usage::AccountSnapshot {
            id: "deepseek-official".to_string(),
            display_name: "DeepSeek".to_string(),
            mode: "balance",
            adapter: Some("deepseek-balance"),
            status,
            balance: ok.then_some(crate::usage::Balance {
                remaining: Some(50.5),
                used: None,
                total: None,
                currency: "CNY".to_string(),
                unlimited: false,
                granted: Some(10.0),
                topped_up: Some(40.5),
            }),
            windows: Vec::new(),
            error: (!ok).then(|| "boom".to_string()),
            updated_at: Some(updated_at),
            stale: false,
            warn_level: "none",
        }
    }

    #[test]
    fn snapshot_payload_maps_ok_balance_to_contract_fields() {
        let payload = super::payload_from_snapshot(&snapshot_of("ok", 100));
        assert!(payload.ok);
        assert!(payload.is_available);
        assert_eq!(payload.updated_at, Some(100));
        assert!(payload.error.is_none() && payload.error_kind.is_none());
        let entry = &payload.balances[0];
        assert_eq!(entry.currency, "CNY");
        assert_eq!(entry.total_balance, "50.50");
        assert_eq!(entry.granted_balance, "10.00");
        assert_eq!(entry.topped_up_balance, "40.50");
        assert_eq!(entry.remaining, Some(50.5));
        assert_eq!(entry.total, None);
    }

    #[test]
    fn snapshot_payload_maps_error_status_to_error_kind() {
        // 与直连路径同一口径：unauthorized → invalid_key，not-configured →
        // no_key，其余错误不归类；updated_at 沿用快照时刻。
        let payload = super::payload_from_snapshot(&snapshot_of("unauthorized", 100));
        assert!(!payload.ok);
        assert_eq!(payload.error_kind.as_deref(), Some("invalid_key"));
        assert_eq!(payload.updated_at, Some(100));
        let payload = super::payload_from_snapshot(&snapshot_of("not-configured", 100));
        assert_eq!(payload.error_kind.as_deref(), Some("no_key"));
        let payload = super::payload_from_snapshot(&snapshot_of("unavailable", 100));
        assert!(payload.error_kind.is_none());
        assert_eq!(payload.error.as_deref(), Some("boom"));
    }

    #[test]
    fn query_balance_reuses_fresh_cache_and_falls_back_when_stale() {
        // CACHE 与 env 均为进程全局：两把锁串行（与 monitor/credentials 的
        // 同类用例同约定），并还原环境变量。
        let _cache_guard = crate::usage::CACHE_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let _env_guard = crate::credentials::ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let prev_box = std::env::var("DSH_BOX_API_KEY").ok();
        let prev_deep = std::env::var("DEEPSEEK_API_KEY").ok();
        std::env::remove_var("DSH_BOX_API_KEY");
        std::env::remove_var("DEEPSEEK_API_KEY");
        let mut config = crate::app_state::Config::load();
        config.dsh_home = std::env::temp_dir().join("dshbox-balance-cache-nonexistent");
        let now = super::unix_now();
        // 新鲜缓存命中：直接转换复用（直连在无 key 时必返回 no_key 错误，
        // 拿到 ok 载荷即证明未走网络）。
        crate::usage::set_cache_for_test(vec![snapshot_of("ok", now - 10)]);
        let payload = super::query_balance(&config);
        assert!(payload.ok, "新鲜缓存应直接复用：{:?}", payload.error);
        assert_eq!(payload.updated_at, Some(now - 10));
        assert_eq!(payload.balances.len(), 1);
        // 过期缓存（> ACCOUNT_REFRESH_MS=300s）：回退直连，无 key → no_key，
        // 证明确实走了回退路径而非缓存。
        crate::usage::set_cache_for_test(vec![snapshot_of("ok", now - 400)]);
        let payload = super::query_balance(&config);
        assert!(!payload.ok);
        assert_eq!(payload.error_kind.as_deref(), Some("no_key"));
        // 还原：缓存清空、环境变量复位。
        crate::usage::set_cache_for_test(Vec::new());
        match prev_box {
            Some(v) => std::env::set_var("DSH_BOX_API_KEY", v),
            None => std::env::remove_var("DSH_BOX_API_KEY"),
        }
        match prev_deep {
            Some(v) => std::env::set_var("DEEPSEEK_API_KEY", v),
            None => std::env::remove_var("DEEPSEEK_API_KEY"),
        }
    }

    #[test]
    fn guarded_url_allows_https_anywhere_and_http_only_on_private_hosts() {
        use super::guarded_balance_url;
        // https：任意主机放行（含自带路径的网关）
        assert_eq!(
            guarded_balance_url("https://api.deepseek.com")
                .ok()
                .as_deref(),
            Some("https://api.deepseek.com/user/balance")
        );
        assert!(guarded_balance_url("https://gateway.example.com:8443/v1").is_ok());
        // http：仅放行回环/私有地址（自托管局域网网关）
        assert!(guarded_balance_url("http://127.0.0.1:8317").is_ok());
        assert!(guarded_balance_url("http://localhost:8317").is_ok());
        assert!(guarded_balance_url("http://192.168.1.10:8317").is_ok());
        assert!(guarded_balance_url("http://[::1]:8317").is_ok());
        // http 公网地址：拒绝（防 API Key 明文外泄）
        assert!(guarded_balance_url("http://api.deepseek.com").is_err());
        // userinfo / 其他协议 / 非法 URL：一律拒绝
        assert!(guarded_balance_url("https://user:pass@api.deepseek.com").is_err());
        assert!(guarded_balance_url("ftp://api.deepseek.com").is_err());
        assert!(guarded_balance_url("not a url").is_err());
    }
}
