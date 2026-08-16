//! DeepSeek API 余额查询：GET {base}/user/balance。
//!
//! API Key 解析顺序：环境变量 DSH_DESKTOP_API_KEY → config.json 的 api_key
//! → 环境变量 DEEPSEEK_API_KEY → dsh 凭据文件（$DSH_HOME/.credentials.yaml，
//! 格式 `DEEPSEEK_API_KEY: sk-...`）。

use serde::{Deserialize, Serialize};
use std::sync::OnceLock;
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager};

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
    /// API Key 无效（HTTP 401）。
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

/// 解析 dsh 凭据文件（$DSH_HOME/.credentials.yaml）。
fn key_from_dsh_credentials(config: &Config) -> Option<String> {
    let file = config.dsh_home().join(".credentials.yaml");
    let text = std::fs::read_to_string(file).ok()?;
    for line in text.lines() {
        let line = line.trim();
        if let Some((key, value)) = line.split_once(':') {
            if key.trim().eq_ignore_ascii_case("DEEPSEEK_API_KEY") {
                let v = value.trim().trim_matches(['"', '\'']);
                if !v.is_empty() {
                    return Some(v.to_string());
                }
            }
        }
    }
    None
}

fn resolve_api_key(config: &Config) -> Result<String, String> {
    if let Some(k) = &config.api_key {
        return Ok(k.clone());
    }
    if let Ok(k) = std::env::var("DEEPSEEK_API_KEY") {
        if !k.is_empty() {
            return Ok(k);
        }
    }
    if let Some(k) = key_from_dsh_credentials(config) {
        return Ok(k);
    }
    Err(crate::locale::text(
        "未找到 DeepSeek API Key。\n可在应用数据目录的 config.json 中添加 api_key 字段，\n或设置环境变量 DSH_DESKTOP_API_KEY / DEEPSEEK_API_KEY，\n或确认 dsh 凭据文件中已有该密钥。",
        "No DeepSeek API Key was found.\nAdd an api_key field to config.json in the app data directory,\nset DSH_DESKTOP_API_KEY or DEEPSEEK_API_KEY,\nor confirm that the dsh credentials file contains the key.",
    )
    .into())
}

/// 同步查询并记录日志（托盘线程直接调用）。
pub(crate) fn query_balance(config: &Config) -> BalancePayload {
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
                        "The API Key is invalid. Check the configured key.",
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

static REFRESH_STARTED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// 后台周期刷新余额：立即查询一次，此后每 5 分钟刷新并广播 balance-updated
/// （标题栏余额 chip 消费）。只启动一次。
pub(crate) fn start_periodic_refresh(app: AppHandle) {
    use std::sync::atomic::Ordering;
    if REFRESH_STARTED.swap(true, Ordering::SeqCst) {
        return;
    }
    std::thread::spawn(move || loop {
        let state = app.state::<AppState>();
        if state.is_quitting() {
            return;
        }
        let payload = query_balance(&state.config());
        let _ = app.emit("balance-updated", payload);
        std::thread::sleep(std::time::Duration::from_secs(300));
    });
}

#[tauri::command]
pub async fn api_balance(app: AppHandle, webview: tauri::Webview) -> BalancePayload {
    if let Err(error) = crate::commands::ensure_local_origin(&webview) {
        return denied_payload(error);
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

fn query(config: &Config) -> Result<BalancePayload, BalanceError> {
    let key = resolve_api_key(config).map_err(BalanceError::NoKey)?;
    let url = format!("{}/user/balance", config.api_base.trim_end_matches('/'));
    // 短超时 + 连接复用（见 balance_agent 注释）
    let resp = balance_agent()
        .get(&url)
        .header("Authorization", &format!("Bearer {key}"))
        .call()
        .map_err(|e| {
            if matches!(e, ureq::Error::StatusCode(401)) {
                BalanceError::InvalidKey
            } else {
                BalanceError::Other(format!(
                    "{}: {e}",
                    crate::locale::text("查询余额失败", "Balance query failed")
                ))
            }
        })?;
    let raw: RawBalance = resp.into_body().read_json().map_err(|e| {
        BalanceError::Other(format!(
            "{}: {e}",
            crate::locale::text("解析余额响应失败", "Failed to parse the balance response")
        ))
    })?;
    let balances = raw
        .balance_infos
        .into_iter()
        .map(|b| BalanceEntry {
            currency: b.currency,
            total_balance: b.total_balance,
            granted_balance: b.granted_balance,
            topped_up_balance: b.topped_up_balance,
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
