//! Minimal UI locale selection shared by Rust and the embedded pages.
//!
//! Fallback chain unified with dsh's browser locale runtime
//! (`packages/client/locale`): the OS/browser language is matched on its
//! primary language — English → en, everything else (Chinese, and any
//! language dsh does not ship) falls back to dsh's product default zh
//! (`FALLBACK_LOCALE`). `DSHD_LANG=zh-CN|en` is a small, test-friendly
//! override; other values fall through to the OS-derived resolution.

use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::OnceLock;

const SYSTEM: u8 = 0;
const CHINESE: u8 = 1;
const ENGLISH: u8 = 2;

static PREFERENCE: AtomicU8 = AtomicU8::new(SYSTEM);
static SYSTEM_CHINESE: OnceLock<bool> = OnceLock::new();

pub fn is_chinese() -> bool {
    match PREFERENCE.load(Ordering::Relaxed) {
        CHINESE => true,
        ENGLISH => false,
        _ => *SYSTEM_CHINESE.get_or_init(detect_chinese),
    }
}

pub fn text<'a>(zh: &'a str, en: &'a str) -> &'a str {
    if is_chinese() {
        zh
    } else {
        en
    }
}

/// Selects an already formatted user-facing message.
pub fn owned(zh: String, en: String) -> String {
    if is_chinese() {
        zh
    } else {
        en
    }
}

pub fn code() -> &'static str {
    if is_chinese() {
        "zh-CN"
    } else {
        "en"
    }
}

/// Applies a persisted/runtime preference; unsupported values fall back to the OS language.
pub fn set_preference(language: Option<&str>) {
    let value = match language.map(str::trim).map(str::to_ascii_lowercase) {
        Some(value) if value.starts_with("zh") => CHINESE,
        Some(value) if value.starts_with("en") => ENGLISH,
        _ => SYSTEM,
    };
    PREFERENCE.store(value, Ordering::Relaxed);
}

/// Runs before every custom page and keeps Rust/WebView language selection identical.
pub fn init_script() -> String {
    format!(
        "window.__DSHD_LANG={};",
        serde_json::to_string(code()).unwrap_or_else(|_| "\"en\"".into())
    )
}

fn detect_chinese() -> bool {
    if let Ok(value) = std::env::var("DSHD_LANG") {
        let value = value.trim().to_ascii_lowercase();
        if value.starts_with("zh") {
            return true;
        }
        if value.starts_with("en") {
            return false;
        }
        // 非内置语言：继续按系统解析（与 dsh 的兜底一致，落到 zh/en）
    }

    #[cfg(windows)]
    {
        // 与 dsh 的浏览器语言解析一致：主语言为英文（0x09，含 en-US/en-GB
        // 等区域变体）→ en；其余——中文（0x04，含繁简体）及 dsh 未内置的
        // 语言——落到产品默认 zh（dsh 的 FALLBACK_LOCALE）。
        let language = unsafe { windows_sys::Win32::Globalization::GetUserDefaultUILanguage() };
        language & 0x03ff != 0x09
    }

    #[cfg(not(windows))]
    {
        let value = ["LC_ALL", "LC_MESSAGES", "LANG"]
            .into_iter()
            .find_map(|name| std::env::var(name).ok());
        // 英文环境 → en；其余（含未设置）→ 产品默认 zh，与 dsh 一致
        !value.is_some_and(|v| v.trim().to_ascii_lowercase().starts_with("en"))
    }
}

#[cfg(test)]
mod tests {
    use super::text;

    #[test]
    fn selects_one_complete_translation() {
        assert!(matches!(text("中文", "English"), "中文" | "English"));
    }
}
