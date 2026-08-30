//! 出站请求的 SSRF 防护判定：私有/回环/保留网段与本地主机名。
//!
//! 唯一定义点——状态栏余额（`balance.rs`）、多供应商账户查询
//! （`usage/balance.rs`）与订阅适配器（`usage/subscriptions.rs`）共用
//! 同一口径，改判定规则只改这里。注意：只校验主机名字符串，不解析
//! DNS——公网域名解析到内网 IP 的 rebinding 不在此防（自配 baseURL
//! 场景下的已知取舍，见改进记录）。
//!
//! 响应体读取上限（[`read_json_capped`]）也定义在这里：各账户接口的
//! JSON 响应远小于 1 MiB，超限一律按响应无效处理。

/// 私有/回环网段判定（IPv4 与 IPv6 简化版）。
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

/// 主机名级判定：localhost/.localhost、.local（mDNS 链路本地名）或私有地址。
pub(crate) fn hostname_is_private(hostname: &str) -> bool {
    let host = hostname
        .trim()
        .trim_start_matches('[')
        .trim_end_matches(']')
        .to_lowercase();
    host == "localhost"
        || host.ends_with(".localhost")
        || host.ends_with(".local")
        || is_private_address(&host)
}

/// 账户/余额类接口的响应体上限（字节）。
pub(crate) const BODY_LIMIT: usize = 1024 * 1024;

/// 读取响应体并解析 JSON，超过 [`BODY_LIMIT`] 报错（ureq 原生 limit：超限时
/// 读取直接失败，错误文案含 "larger than request limit"）。
pub(crate) fn read_json_capped(body: ureq::Body) -> Result<serde_json::Value, String> {
    let bytes = body
        .into_with_config()
        .limit(BODY_LIMIT as u64)
        .read_to_vec()
        .map_err(|e| format!("{e}"))?;
    serde_json::from_slice(&bytes).map_err(|e| format!("{e}"))
}

/// 账户/余额查询 URL 的统一校验：仅 http/https、拒绝 userinfo；https 放行
/// 任意主机，http 仅放行回环/私有地址（自托管局域网网关场景，防止误配
/// http 公网地址导致 API Key 明文外泄）。状态栏余额与多供应商账户查询
/// 共用本判定——历史上两处各写一份且口径漂移过，收敛于此。
///
/// 两个入口：`guard_https_or_lan_http(base, path)` 负责「base + 相对路径」
/// 的拼接；`guard_full_url` 校验已拼好的完整 URL（OrcaRouter 这类先做
/// `/v1` 前缀归一再校验的适配器用，避免二次拼接）。
pub(crate) fn guard_https_or_lan_http(base: &str, path: &str) -> Result<String, &'static str> {
    let base = base.trim_end_matches('/');
    let combined = format!("{base}{path}");
    guard_full_url(&combined)
}

pub(crate) fn guard_full_url(url: &str) -> Result<String, &'static str> {
    let url = url::Url::parse(url).map_err(|_| "invalid-url")?;
    if url.username() != "" || url.password().is_some() {
        return Err("url-credentials");
    }
    match url.scheme() {
        "https" => Ok(url.to_string()),
        "http" => {
            let host = url
                .host_str()
                .unwrap_or("")
                .trim_start_matches('[')
                .trim_end_matches(']');
            if hostname_is_private(host) {
                Ok(url.to_string())
            } else {
                Err("insecure-protocol")
            }
        }
        _ => Err("insecure-protocol"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_json_capped_parses_small_body() {
        let body = ureq::Body::builder().data(br#"{"ok":true}"#.to_vec());
        let value = read_json_capped(body).unwrap();
        assert_eq!(value.get("ok").and_then(|v| v.as_bool()), Some(true));
    }

    #[test]
    fn read_json_capped_rejects_oversized_body() {
        let body = ureq::Body::builder().data(vec![b'x'; BODY_LIMIT + 1]);
        let err = read_json_capped(body).unwrap_err();
        assert!(err.contains("larger than request limit"), "err={err}");
    }

    #[test]
    fn guard_allows_https_any_host_and_lan_http() {
        assert_eq!(
            guard_https_or_lan_http("https://api.deepseek.com", "/user/balance").as_deref(),
            Ok("https://api.deepseek.com/user/balance")
        );
        // 自托管网关：https 私网放行、http 私网放行
        assert!(guard_https_or_lan_http("https://192.168.1.10:3000", "/api/usage/token/").is_ok());
        assert!(guard_https_or_lan_http("http://nas.local:3000/", "/v1/balance").is_ok());
        assert!(guard_https_or_lan_http("http://127.0.0.1:8000", "/x").is_ok());
    }

    #[test]
    fn guard_rejects_public_http_credentials_and_bad_input() {
        // http 公网：拒绝（防 API Key 明文外泄）
        assert_eq!(
            guard_https_or_lan_http("http://api.deepseek.com", "/user/balance"),
            Err("insecure-protocol")
        );
        assert_eq!(
            guard_https_or_lan_http("ftp://example.com", "/x"),
            Err("insecure-protocol")
        );
        assert_eq!(
            guard_https_or_lan_http("https://user:pass@example.com", "/x"),
            Err("url-credentials")
        );
        assert_eq!(
            guard_https_or_lan_http("not a url", "/x"),
            Err("invalid-url")
        );
    }
}
