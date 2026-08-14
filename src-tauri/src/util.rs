//! 通用小工具。

/// 截断字符串到指定字符数（超长加省略号）。
pub fn truncate(s: &str, n: usize) -> String {
    let mut t: String = s.chars().take(n).collect();
    if s.chars().count() > n {
        t.push('…');
    }
    t
}
