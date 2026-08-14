//! DeepSeek API 余额查询：GET {base}/user/balance。
//!
//! API Key 解析顺序：config.json 的 api_key → 环境变量 DEEPSEEK_API_KEY
//! → dsh 凭据文件（$DSH_HOME/.credentials.yaml，格式 `DEEPSEEK_API_KEY: sk-...`）。

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager};

use crate::app_state::{AppState, Config};

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
    let home = config
        .dsh_home
        .clone()
        .or_else(|| std::env::var("DSH_HOME").ok().map(PathBuf::from))
        .unwrap_or_else(|| {
            PathBuf::from(std::env::var("USERPROFILE").unwrap_or_default()).join(".dsh")
        });
    let file = home.join(".credentials.yaml");
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
    Err("未找到 DeepSeek API Key。\n可在应用数据目录的 config.json 中添加 api_key 字段，\n或设置环境变量 DEEPSEEK_API_KEY，\n或确认 dsh 凭据文件中已有该密钥。".into())
}

/// 同步查询并记录日志（托盘线程直接调用）。
pub(crate) fn query_balance(config: &Config) -> BalancePayload {
    match query(config) {
        Ok(mut payload) => {
            payload.updated_at = Some(unix_now());
            let total = payload
                .balances
                .first()
                .map(|b| format!("{} {}", b.currency, b.total_balance))
                .unwrap_or_else(|| "无余额信息".into());
            crate::logging::log(&format!(
                "balance: ok is_available={} total={}",
                payload.is_available, total
            ));
            payload
        }
        Err(e) => {
            let (kind, msg) = match e {
                BalanceError::NoKey(m) => (Some("no_key"), m),
                BalanceError::InvalidKey => (
                    Some("invalid_key"),
                    "API Key 无效，请检查配置的密钥是否正确。".to_string(),
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
    // 仅允许本地来源（标题栏/启动页）查询余额
    if let Ok(url) = webview.url() {
        if url.as_str().starts_with("http://127.0.0.1:") {
            return BalancePayload {
                ok: false,
                is_available: false,
                balances: Vec::new(),
                error: Some("此操作不允许从页面发起。".into()),
                error_kind: None,
                updated_at: None,
            };
        }
    }
    let config = app.state::<AppState>().config();
    // 网络查询放到阻塞线程，避免占用主线程/异步工作线程。
    let payload = tauri::async_runtime::spawn_blocking(move || query_balance(&config))
        .await
        .unwrap_or_else(|e| {
            crate::logging::log(&format!("balance: 线程异常 {e}"));
            BalancePayload {
                ok: false,
                is_available: false,
                balances: Vec::new(),
                error: Some(format!("余额查询线程异常：{e}")),
                error_kind: None,
                updated_at: None,
            }
        });
    payload
}

fn query(config: &Config) -> Result<BalancePayload, BalanceError> {
    let key = resolve_api_key(config).map_err(BalanceError::NoKey)?;
    let url = format!("{}/user/balance", config.api_base.trim_end_matches('/'));
    let resp = crate::runtime::client()
        .get(&url)
        .set("Authorization", &format!("Bearer {key}"))
        .call()
        .map_err(|e| {
            if matches!(e, ureq::Error::Status(401, _)) {
                BalanceError::InvalidKey
            } else {
                BalanceError::Other(format!("查询余额失败：{e}"))
            }
        })?;
    let raw: RawBalance = resp
        .into_json()
        .map_err(|e| BalanceError::Other(format!("解析余额响应失败：{e}")))?;
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
