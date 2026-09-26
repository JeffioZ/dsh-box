//! dsh 后端同源 RPC 客户端与会话选择：供通知、插件维护与账户监测读取
//! 当前会话与活性。RPC 线格式按版本分叉（见 `rpc_session_list`）：新版
//! `session/list`（投影内嵌），旧版 `session.list` → `session.history`。
//!
//! 历史用量聚合见 `super::aggregate`。状态栏统计与实时 tok/s 已随
//! 状态栏整体移除（2026-09-23）：会话统计由 dsh 原生输入框下方的
//! 统计行承担，余额 chip 迁至标题栏（见 ui/titlebar.*）。

use std::sync::OnceLock;
use std::time::Duration;

use crate::app_state::Config;

/// localhost RPC 专用 Agent（连接复用；无需 TLS）。
static RPC_AGENT: OnceLock<ureq::Agent> = OnceLock::new();

fn rpc_agent() -> &'static ureq::Agent {
    RPC_AGENT.get_or_init(|| {
        ureq::Agent::config_builder()
            .timeout_connect(Some(Duration::from_secs(2)))
            .timeout_recv_response(Some(Duration::from_secs(3)))
            .timeout_recv_body(Some(Duration::from_secs(3)))
            .build()
            .new_agent()
    })
}

/// 调 dsh 后端 unary RPC（同源 POST /api/<method>，client-request 信封），
/// 成功返回 value；协议不匹配/服务未就绪一律 None。
///
/// 新版 dsh（≥0.1.2-alpha，上游 rpc-host + browser-auth）对 `/api` 通道启用
/// 进程级鉴权：请求必须携带会话 cookie，而 cookie 只能由精确 `GET /?token=`
/// 交换（303 + Set-Cookie）种下。托管模式下 token 来自启动日志
/// （`Config::auth_token`），这里换 cookie 并按 (port, token) 缓存；cookie
/// 失效（服务重启换了 token/secret）时作废重换一次。旧版 dsh 无鉴权：
/// token 为 None 时不带 cookie，行为与裸 POST 完全一致。
fn rpc(config: &Config, method: &str, payload: serde_json::Value) -> Option<serde_json::Value> {
    let cookie = rpc_session_cookie(config);
    match rpc_once(config, method, &payload, cookie.as_deref()) {
        RpcReply::Value(value) => Some(value),
        RpcReply::Unauthorized if config.auth_token.is_some() => {
            *rpc_session_slot().lock().unwrap_or_else(|e| e.into_inner()) = None;
            let cookie = rpc_session_cookie(config);
            match rpc_once(config, method, &payload, cookie.as_deref()) {
                RpcReply::Value(value) => Some(value),
                _ => None,
            }
        }
        _ => None,
    }
}

enum RpcReply {
    Value(serde_json::Value),
    Unauthorized,
    Failed,
}

fn rpc_once(
    config: &Config,
    method: &str,
    payload: &serde_json::Value,
    cookie: Option<&str>,
) -> RpcReply {
    let url = format!("http://127.0.0.1:{}/api/{method}", config.port);
    let mut request = rpc_agent().post(&url);
    if let Some(cookie) = cookie {
        request = request.header("cookie", cookie);
    }
    let resp = match request.send_json(serde_json::json!({
        "type": "client-request",
        "rpcId": format!("dshd-rpc-{}", rpc_seq()),
        "method": method,
        "payload": payload,
    })) {
        Ok(resp) => resp,
        Err(ureq::Error::StatusCode(401)) => return RpcReply::Unauthorized,
        Err(_) => return RpcReply::Failed,
    };
    let Ok(json) = resp.into_body().read_json::<serde_json::Value>() else {
        return RpcReply::Failed;
    };
    let Some(result) = json.get("result") else {
        return RpcReply::Failed;
    };
    match result.get("ok").and_then(|value| value.as_bool()) {
        Some(true) => result
            .get("value")
            .cloned()
            .map(RpcReply::Value)
            .unwrap_or(RpcReply::Failed),
        _ => RpcReply::Failed,
    }
}

struct RpcSession {
    port: u16,
    token: String,
    cookie: String,
}

fn rpc_session_slot() -> &'static std::sync::Mutex<Option<RpcSession>> {
    static SLOT: std::sync::Mutex<Option<RpcSession>> = std::sync::Mutex::new(None);
    &SLOT
}

/// 取当前 (port, token) 对应的会话 cookie；无 token（旧版 dsh）返回 None。
fn rpc_session_cookie(config: &Config) -> Option<String> {
    let token = config.auth_token.as_ref()?;
    let mut slot = rpc_session_slot().lock().unwrap_or_else(|e| e.into_inner());
    if let Some(session) = slot.as_ref() {
        if session.port == config.port && session.token == *token {
            return Some(session.cookie.clone());
        }
    }
    let cookie = mint_session_cookie(config.port, token)?;
    *slot = Some(RpcSession {
        port: config.port,
        token: token.clone(),
        cookie: cookie.clone(),
    });
    Some(cookie)
}

/// 用启动 token 执行上游 browser-auth 的 cookie 交换（`GET /?token=`）。
/// 走裸 TCP 而非 ureq：303 的 Set-Cookie 在重定向链的中间响应上，ureq
/// 跟随后只暴露最终响应头，拿不到要复制的 cookie。
fn mint_session_cookie(port: u16, token: &str) -> Option<String> {
    use std::io::{Read, Write};
    use std::net::{SocketAddr, TcpStream};
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    let mut stream = TcpStream::connect_timeout(&addr, Duration::from_millis(800)).ok()?;
    let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(2)));
    let request = format!(
        "GET /?token={token} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nAccept: text/html\r\nConnection: close\r\n\r\n"
    );
    stream.write_all(request.as_bytes()).ok()?;
    let mut response = Vec::with_capacity(4096);
    let mut chunk = [0u8; 2048];
    loop {
        match stream.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => {
                response.extend_from_slice(&chunk[..n]);
                if response.len() > 64 * 1024 {
                    break;
                }
            }
            Err(_) => break,
        }
    }
    session_cookie_from_exchange(&response)
}

/// 从 token 交换响应头中取会话 cookie：状态行必须是 303，且只认
/// `dsh-auth-` 前缀的 Set-Cookie（上游 authorizeIndex 的固定行为），
/// 属性段（Max-Age/Path 等）截断，返回可直接回带的 `name=value`。
fn session_cookie_from_exchange(response: &[u8]) -> Option<String> {
    let text = std::str::from_utf8(response).ok()?;
    let mut lines = text.split("\r\n").flat_map(|line| line.split('\n'));
    let status = lines.next()?;
    if !status.starts_with("HTTP/1.1 303") && !status.starts_with("HTTP/1.0 303") {
        return None;
    }
    for line in lines {
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        if !name.eq_ignore_ascii_case("set-cookie") {
            continue;
        }
        let pair = value.trim().split(';').next()?.trim();
        if let Some(cookie) = pair.strip_prefix("dsh-auth-") {
            if !cookie.is_empty() && pair.contains('=') {
                return Some(pair.to_string());
            }
        }
    }
    None
}

fn rpc_seq() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQ: AtomicU64 = AtomicU64::new(0);
    SEQ.fetch_add(1, Ordering::Relaxed)
}

/// dsh ≥0.1.2-alpha 的线格式与旧版不兼容（三处同时变化）：
/// ① endpoint 由 `ns.method` 改为 `ns/method`（typert 网关按斜杠段认领）；
/// ② `payload` 必须是 `{"args": {...}}`（按参数名传参，列表方法参数名
///    固定 `_request`）；③ `session.history` 拆成 `session/page`（请求结构
///    大改），但投影值直接嵌在 `session/list` 列表项的
/// `projections.values` 里，取当前会话统计不再需要第二次调用。
/// token 只在新版启动日志中出现，据此选择协议；旧版（无 token）走原路径。
fn rpc_session_list(config: &Config) -> Option<serde_json::Value> {
    if config.auth_token.is_some() {
        rpc(
            config,
            "session/list",
            serde_json::json!({ "args": { "_request": {} } }),
        )
    } else {
        rpc(config, "session.list", serde_json::json!({}))
    }
}

/// 从 `session/list`（或旧版 `session.list`）响应 value 中选当前会话：
/// running 优先、其次 updatedAt 最新（与注入脚本 resolveAbsPath 的选取
/// 逻辑一致——dsh 页面当前打开的正是该会话）。
///
/// 空白会话（`sessionListMetadata.blank`，dsh 首启/新开页自动创建、从未
/// 有用户活动）的 updatedAt 会反超真实会话——dsh 0.1.5 升级首启后正是它
/// 劫持了兜底选择。因此同级内非空白绝对优先；全部空白
/// 时保持旧行为（选谁都无用户活动可跟随）。running 仍是最高优先：正在
/// 运行的空白会话就是用户正在交互的当前会话。
fn pick_session_item(value: &serde_json::Value) -> Option<&serde_json::Value> {
    let items = value.get("items")?.as_array()?;
    let blank = |item: &serde_json::Value| {
        item.pointer("/projections/values/sessionListMetadata/blank")
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
    };
    let mut best: Option<(&serde_json::Value, bool, bool, f64)> = None;
    for item in items {
        if item.get("sessionId").and_then(|v| v.as_str()).is_none() {
            continue;
        }
        let item_blank = blank(item);
        let running = item
            .get("running")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let updated = item
            .get("updatedAt")
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0);
        let replace = match best {
            None => true,
            Some((_, best_running, best_blank, best_updated)) => {
                (running && !best_running)
                    || (running == best_running && item_blank != best_blank && !item_blank)
                    || (running == best_running
                        && item_blank == best_blank
                        && updated > best_updated)
            }
        };
        if replace {
            best = Some((item, running, item_blank, updated));
        }
    }
    best.map(|(item, _, _, _)| item)
}

fn current_session(config: &Config) -> Option<(String, bool)> {
    let value = rpc_session_list(config)?;
    let item = selected_session_item(config, &value)?;
    let id = item.get("sessionId")?.as_str()?.to_string();
    let running = item
        .get("running")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    Some((id, running))
}

pub(crate) fn current_session_id(config: &Config) -> Option<String> {
    if let Some(selected) = visible_session(config) {
        return selected;
    }
    current_session(config).map(|(id, _)| id)
}

#[cfg(windows)]
pub(crate) fn session_is_listed(config: &Config, id: &str) -> bool {
    rpc_session_list(config)
        .and_then(|value| {
            value.get("items").and_then(|v| v.as_array()).map(|items| {
                items
                    .iter()
                    .any(|item| item.get("sessionId").and_then(|v| v.as_str()) == Some(id))
            })
        })
        .unwrap_or(false)
}

type VisibleSession = (u16, Option<String>, std::time::Instant);
static VISIBLE_SESSION: std::sync::Mutex<Option<VisibleSession>> = std::sync::Mutex::new(None);

pub(crate) fn set_visible_session(port: u16, known: bool, id: Option<String>) {
    *VISIBLE_SESSION.lock().unwrap_or_else(|e| e.into_inner()) =
        known.then(|| (port, id, std::time::Instant::now()));
}

pub(crate) fn visible_session(config: &Config) -> Option<Option<String>> {
    VISIBLE_SESSION
        .lock()
        .ok()?
        .as_ref()
        .filter(|(port, _, time)| *port == config.port && time.elapsed() < Duration::from_secs(30))
        .map(|(_, id, _)| id.clone())
}

fn selected_session_item<'a>(
    config: &Config,
    value: &'a serde_json::Value,
) -> Option<&'a serde_json::Value> {
    match visible_session(config) {
        Some(Some(id)) => value
            .get("items")?
            .as_array()?
            .iter()
            .find(|item| item.get("sessionId").and_then(|v| v.as_str()) == Some(id.as_str())),
        Some(None) => None,
        None => pick_session_item(value),
    }
}

/// 当前是否有正在执行的会话。Some(false) 也覆盖“会话列表为空”；
/// None 表示 RPC 不可用，维护任务应保守等待，避免误打断服务。
pub(crate) fn session_activity(config: &Config) -> Option<bool> {
    let value = rpc_session_list(config)?;
    let items = value.get("items")?.as_array()?;
    if items.is_empty() {
        return Some(false);
    }
    Some(
        items
            .iter()
            .any(|item| item.get("running").and_then(|value| value.as_bool()) == Some(true)),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pick_session_item_skips_blank_fresh_sessions_in_fallback() {
        // dsh 0.1.5 首启自动创建的空白会话（blank:true、updatedAt 最新）会
        // 劫持兜底选择；同级内非空白绝对优先，running 仍最高。
        let value = serde_json::json!({"items": [
            {"sessionId": "blank-new", "running": false, "updatedAt": 300.0,
             "projections": {"values": {"sessionListMetadata": {"blank": true}}}},
            {"sessionId": "real-mid", "running": false, "updatedAt": 200.0,
             "projections": {"values": {"sessionStats": {"turns": 2, "steps": 5}}}},
            {"sessionId": "real-old", "running": false, "updatedAt": 100.0,
             "projections": {"values": {"sessionStats": {"turns": 1, "steps": 1}}}}
        ]});
        assert_eq!(
            pick_session_item(&value).unwrap().get("sessionId").unwrap(),
            "real-mid"
        );
        // running 的空白会话（用户正在新会话里交互）仍胜出
        let value = serde_json::json!({"items": [
            {"sessionId": "blank-running", "running": true, "updatedAt": 300.0,
             "projections": {"values": {"sessionListMetadata": {"blank": true}}}},
            {"sessionId": "real-old", "running": false, "updatedAt": 100.0,
             "projections": {"values": {"sessionStats": {"turns": 1, "steps": 1}}}}
        ]});
        assert_eq!(
            pick_session_item(&value).unwrap().get("sessionId").unwrap(),
            "blank-running"
        );
        // 全部空白：保持旧行为（updatedAt 最新者胜出）
        let value = serde_json::json!({"items": [
            {"sessionId": "blank-a", "running": false, "updatedAt": 100.0,
             "projections": {"values": {"sessionListMetadata": {"blank": true}}}},
            {"sessionId": "blank-b", "running": false, "updatedAt": 200.0,
             "projections": {"values": {"sessionListMetadata": {"blank": true}}}}
        ]});
        assert_eq!(
            pick_session_item(&value).unwrap().get("sessionId").unwrap(),
            "blank-b"
        );
        // 无 projections 的老会话（旧代次日志）按非空白参与，不受影响
        let value = serde_json::json!({"items": [
            {"sessionId": "no-proj", "running": false, "updatedAt": 50.0},
            {"sessionId": "blank-new", "running": false, "updatedAt": 300.0,
             "projections": {"values": {"sessionListMetadata": {"blank": true}}}}
        ]});
        assert_eq!(
            pick_session_item(&value).unwrap().get("sessionId").unwrap(),
            "no-proj"
        );
    }

    #[test]
    fn extracts_dsh_auth_cookie_from_token_exchange() {
        let response = concat!(
            "HTTP/1.1 303 See Other\r\n",
            "cache-control: no-store\r\n",
            "location: /\r\n",
            "set-cookie: dsh-auth-QVdE=dmFsdWU; Max-Age=2592000; Path=/; HttpOnly; SameSite=Strict\r\n",
            "connection: close\r\n",
            "\r\n",
        );
        assert_eq!(
            session_cookie_from_exchange(response.as_bytes()).as_deref(),
            Some("dsh-auth-QVdE=dmFsdWU")
        );
    }

    #[test]
    fn cookie_exchange_rejects_non_303_and_foreign_cookies() {
        // 旧版 dsh 直接 200 出页面：无交换发生
        let ok_page = "HTTP/1.1 200 OK\r\nset-cookie: other=1\r\n\r\n<body>";
        assert_eq!(session_cookie_from_exchange(ok_page.as_bytes()), None);
        // 401：token 错误/过期
        let unauthorized = "HTTP/1.1 401 unauthorized\r\n\r\nunauthorized";
        assert_eq!(session_cookie_from_exchange(unauthorized.as_bytes()), None);
        // 303 但种下的不是 dsh-auth cookie：不作会话凭据
        let foreign = "HTTP/1.1 303 See Other\r\nset-cookie: sid=x\r\n\r\n";
        assert_eq!(session_cookie_from_exchange(foreign.as_bytes()), None);
        // LF-only 头（防御非规范服务端）也能解析
        let lf_only = "HTTP/1.1 303 See Other\nset-cookie: dsh-auth-a=b; Path=/\n\n";
        assert_eq!(
            session_cookie_from_exchange(lf_only.as_bytes()).as_deref(),
            Some("dsh-auth-a=b")
        );
    }

    #[test]
    fn picks_running_session_then_latest_updated() {
        let value = serde_json::json!({
            "items": [
                { "sessionId": "a", "running": false, "updatedAt": 90.0 },
                { "sessionId": "b", "running": true,  "updatedAt": 10.0 },
                { "sessionId": "c", "running": false, "updatedAt": 50.0 }
            ]
        });
        // running 优先于 updatedAt
        assert_eq!(
            pick_session_item(&value)
                .and_then(|item| item.get("sessionId"))
                .and_then(|v| v.as_str()),
            Some("b")
        );
        // 无 running 时取 updatedAt 最新
        let idle = serde_json::json!({
            "items": [
                { "sessionId": "a", "running": false, "updatedAt": 10.0 },
                { "sessionId": "c", "running": false, "updatedAt": 50.0 }
            ]
        });
        assert_eq!(
            pick_session_item(&idle)
                .and_then(|item| item.get("sessionId"))
                .and_then(|v| v.as_str()),
            Some("c")
        );
        // 空/缺字段不 panic，返回 None
        assert_eq!(pick_session_item(&serde_json::json!({ "items": [] })), None);
        assert_eq!(
            pick_session_item(&serde_json::json!({ "items": [{ "running": true }] })),
            None
        );
    }
}
