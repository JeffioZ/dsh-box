//! 版本领域：semver 风格比较、Node 版本解析与最低版本判定（纯逻辑，无平台依赖）。
//! 另含构建时刻元数据（build.rs 注入，纯展示、不参与任何版本比较）。

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

/// 按 SemVer 2.0 比较版本号，容忍 Node.js 常见的 `v` 前缀。
/// 无法解析的上游版本使用稳定的字符串比较，避免更新检查直接失败。
pub fn compare_versions(a: &str, b: &str) -> std::cmp::Ordering {
    fn normalize(value: &str) -> &str {
        value.trim().trim_start_matches('v')
    }
    let a = normalize(a);
    let b = normalize(b);
    match (semver::Version::parse(a), semver::Version::parse(b)) {
        (Ok(a), Ok(b)) => a.cmp_precedence(&b),
        _ => a.cmp(b),
    }
}

/// build.rs 注入的构建时刻（Unix 秒）。正常构建必然存在；缺失（如改动
/// 引入前的旧缓存产物）时调用方跳过展示，不影响任何功能。
pub fn build_epoch() -> Option<u64> {
    option_env!("DSHBOX_BUILD_EPOCH").and_then(|raw| raw.parse().ok())
}

/// 构建时刻的 RFC3339 UTC 串（如 `2026-09-16T06:12:35Z`）。前端用
/// `new Date()` 解析后按应用语言本地化展示；固定 UTC 保证跨时区构建
/// （本地 build.ps1 / CI）语义一致。
pub fn build_time_rfc3339() -> Option<String> {
    Some(format_rfc3339_utc(build_epoch()?))
}

fn format_rfc3339_utc(epoch: u64) -> String {
    let days = epoch / 86_400;
    let secs_of_day = epoch % 86_400;
    let (year, month, day) = civil_from_days(days as i64);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        secs_of_day / 3600,
        secs_of_day / 60 % 60,
        secs_of_day % 60
    )
}

/// 天数 → 公历年月日（Howard Hinnant 的 civil_from_days 移位算法，
/// 覆盖 1970 起的全部日期，无需历法库依赖）。
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32; // [1, 12]
    (if m <= 2 { y + 1 } else { y }, m, d)
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
        assert_eq!(
            compare_versions("0.1.0-alpha.9", "0.1.0-beta.1"),
            Ordering::Less
        );
        assert_eq!(
            compare_versions("0.1.0+build.1", "0.1.0+build.2"),
            Ordering::Equal
        );
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

    #[test]
    fn build_time_formats_known_epochs() {
        assert_eq!(format_rfc3339_utc(0), "1970-01-01T00:00:00Z");
        // 闰日边界：2024-02-29T12:00:00Z
        assert_eq!(format_rfc3339_utc(1_709_208_000), "2024-02-29T12:00:00Z");
        // 2026-09-16T06:30:05Z
        assert_eq!(format_rfc3339_utc(1_789_540_205), "2026-09-16T06:30:05Z");
    }
}
