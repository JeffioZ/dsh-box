//! 出站请求的 SSRF 防护判定：私有/回环/保留网段与本地主机名。
//!
//! 唯一定义点——状态栏余额（`balance.rs`）、多供应商账户查询
//! （`usage/balance.rs`）与订阅适配器（`usage/subscriptions.rs`）共用
//! 同一口径，改判定规则只改这里。注意：只校验主机名字符串，不解析
//! DNS——公网域名解析到内网 IP 的 rebinding 不在此防（自配 baseURL
//! 场景下的已知取舍，见改进记录）。

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

/// 主机名级判定：localhost/.localhost 或私有地址。
pub(crate) fn hostname_is_private(hostname: &str) -> bool {
    let host = hostname
        .trim()
        .trim_start_matches('[')
        .trim_end_matches(']')
        .to_lowercase();
    host == "localhost" || host.ends_with(".localhost") || is_private_address(&host)
}
