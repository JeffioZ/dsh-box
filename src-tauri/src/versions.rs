//! 版本领域：semver 风格比较、Node 版本解析与最低版本判定（纯逻辑，无平台依赖）。

/// dsh 要求的最低 Node：^22.19.0 || >=24.0.0
pub const NODE_MIN_MAJOR: u32 = 24;
pub const NODE_22_MIN_MINOR: u32 = 19;

/// 解析 `node --version` 输出（形如 `v24.19.0`）为 (major, minor, patch)。
pub fn parse_node_version(text: &str) -> Option<(u32, u32, u32)> {
    let t = text.trim().trim_start_matches('v');
    let mut parts = t.split('.');
    let major = parts.next()?.parse::<u32>().ok()?;
    let minor = parts.next()?.parse::<u32>().ok()?;
    let patch = parts.next()?.parse::<u32>().ok()?;
    Some((major, minor, patch))
}

/// Node 版本是否满足 dsh 要求。
pub fn node_satisfies(major: u32, minor: u32) -> bool {
    major >= NODE_MIN_MAJOR || (major == 22 && minor >= NODE_22_MIN_MINOR)
}

/// 比较 semver 风格版本号（容忍 `v` 前缀与 `-rc.N`/`-beta.N` 后缀）。
/// 返回 a 与 b 的 Ordering：数字段逐段比较；主版本相同且一方带预发布后缀时，带后缀的更旧。
pub fn compare_versions(a: &str, b: &str) -> std::cmp::Ordering {
    use std::cmp::Ordering;

    fn norm(s: &str) -> (Vec<u64>, Vec<u64>) {
        let s = s.trim().trim_start_matches('v');
        let (core, pre) = match s.split_once('-') {
            Some((c, p)) => (c, Some(p)),
            None => (s, None),
        };
        let nums = core
            .split('.')
            .filter_map(|p| p.parse::<u64>().ok())
            .collect();
        let prenums = pre
            .map(|p| p.split('.').filter_map(|x| x.parse::<u64>().ok()).collect())
            .unwrap_or_default();
        (nums, prenums)
    }

    let (an, ap) = norm(a);
    let (bn, bp) = norm(b);
    for i in 0..an.len().max(bn.len()) {
        let x = an.get(i).copied().unwrap_or(0);
        let y = bn.get(i).copied().unwrap_or(0);
        if x != y {
            return x.cmp(&y);
        }
    }
    // 数值段相同 → 有预发布后缀的版本更旧。
    match (ap.is_empty(), bp.is_empty()) {
        (true, false) => Ordering::Greater,
        (false, true) => Ordering::Less,
        (false, false) => {
            for i in 0..ap.len().max(bp.len()) {
                let x = ap.get(i).copied().unwrap_or(0);
                let y = bp.get(i).copied().unwrap_or(0);
                if x != y {
                    return x.cmp(&y);
                }
            }
            Ordering::Equal
        }
        (true, true) => Ordering::Equal,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cmp::Ordering;

    #[test]
    fn version_compare_numeric() {
        assert_eq!(compare_versions("0.1.0", "0.1.0"), Ordering::Equal);
        assert_eq!(compare_versions("0.1.1", "0.1.0"), Ordering::Greater);
        assert_eq!(compare_versions("0.1.0", "0.1.1"), Ordering::Less);
        // 关键回归：0.10 > 0.9（字符串比较会判反）
        assert_eq!(compare_versions("0.10.0", "0.9.0"), Ordering::Greater);
        assert_eq!(compare_versions("0.9.0", "0.10.0"), Ordering::Less);
        assert_eq!(compare_versions("1.0.0", "0.99.99"), Ordering::Greater);
    }

    #[test]
    fn version_compare_prerelease() {
        assert_eq!(
            compare_versions("0.1.0-rc.6", "0.1.0-rc.5"),
            Ordering::Greater
        );
        assert_eq!(
            compare_versions("0.1.0-rc.9", "0.1.0-rc.10"),
            Ordering::Less
        );
        assert_eq!(compare_versions("0.1.0", "0.1.0-rc.6"), Ordering::Greater);
        assert_eq!(compare_versions("0.1.0-rc.6", "0.1.0"), Ordering::Less);
    }

    #[test]
    fn version_compare_v_prefix() {
        assert_eq!(compare_versions("v24.19.0", "v24.10.0"), Ordering::Greater);
        assert_eq!(compare_versions("v24.9.0", "v24.10.0"), Ordering::Less);
        assert_eq!(compare_versions("v24.19.1", "v24.19.0"), Ordering::Greater);
    }

    #[test]
    fn node_version_parse() {
        assert_eq!(parse_node_version("v24.19.1\n"), Some((24, 19, 1)));
        assert_eq!(parse_node_version("v22.19.0"), Some((22, 19, 0)));
        assert_eq!(parse_node_version("garbage"), None);
        assert!(node_satisfies(24, 0));
        assert!(node_satisfies(22, 19));
        assert!(!node_satisfies(22, 18));
        assert!(!node_satisfies(23, 99));
    }
}
