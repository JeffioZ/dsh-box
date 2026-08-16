// 发布版不弹出附加的控制台窗口。
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
// MSVC link.exe 会向 stdout 输出“正在创建库 …”（/NOLOGO 无法抑制），
// 被 rustc 报告为 linker_messages 警告——按预期允许，保持构建输出干净。
// 该 lint 只能在被链接的 crate 根（bin）控制，不能放在 lib 根。
#![allow(linker_messages)]

//! DSHDesktop 入口。
//!
//! Windows：启动前检查 WebView2 Runtime，缺失时自动安装；
//! macOS/Linux：使用系统内置渲染（WKWebView/WebKitGTK），无需预检。

fn main() {
    // panic = "abort"：panic 信息默认输出到 GUI 应用不可见的 stderr。
    // 挂接 hook 把 panic 信息尽力写入应用日志（desktop.log），
    // 写入失败时静默忽略；随后交还默认 hook，保留 stderr 输出与
    // RUST_BACKTRACE（dev 构建 panic=unwind 下依赖它），默认 abort 行为不变。
    let previous_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let payload = if let Some(s) = info.payload().downcast_ref::<&str>() {
            (*s).to_string()
        } else if let Some(s) = info.payload().downcast_ref::<String>() {
            s.clone()
        } else {
            String::from("（非字符串 panic 载荷）")
        };
        let location = info
            .location()
            .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()))
            .unwrap_or_default();
        dsh_desktop_lib::log_panic(&format!("{payload}（{location}）"));
        previous_hook(info);
    }));

    #[cfg(windows)]
    if !webview2_check::ensure_webview2() {
        return;
    }
    dsh_desktop_lib::run()
}

/// WebView2 前置检查（仅 Windows：注册表检测 + 下载官方引导安装器自动安装）。
#[cfg(windows)]
mod webview2_check {
    use dsh_desktop_lib::{locale, APP_TITLE};
    use std::io::{Read, Seek};
    use std::os::windows::process::CommandExt;
    use std::time::Duration;

    use windows_sys::Win32::System::Registry::{
        RegCloseKey, RegOpenKeyExW, RegQueryValueExW, HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE,
        KEY_QUERY_VALUE,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        MessageBoxW, IDYES, MB_ICONERROR, MB_ICONQUESTION, MB_OK, MB_YESNO,
    };

    /// WebView2 Evergreen Runtime 的注册表检测路径。
    const REG_PATHS: [&str; 2] = [
        // x64 视角（64 位进程读 64 位注册表视图）
        "SOFTWARE\\WOW6432Node\\Microsoft\\EdgeUpdate\\Clients\\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}",
        // 32 位视角 / 每用户安装
        "SOFTWARE\\Microsoft\\EdgeUpdate\\Clients\\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}",
    ];

    fn to_wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    /// 读取注册表字符串值（如 WebView2 版本号）。
    fn reg_read_string(root: HKEY, path: &str, value: &str) -> Option<String> {
        let path_w = to_wide(path);
        let value_w = to_wide(value);
        let mut hkey: HKEY = std::ptr::null_mut();
        if unsafe { RegOpenKeyExW(root, path_w.as_ptr(), 0, KEY_QUERY_VALUE, &mut hkey) } != 0 {
            return None;
        }
        let mut len = 0u32;
        let status = unsafe {
            RegQueryValueExW(
                hkey,
                value_w.as_ptr(),
                std::ptr::null(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                &mut len,
            )
        };
        if status != 0 || len == 0 {
            unsafe {
                let _ = RegCloseKey(hkey);
            }
            return None;
        }
        let mut buf = vec![0u16; (len / 2) as usize];
        let status = unsafe {
            RegQueryValueExW(
                hkey,
                value_w.as_ptr(),
                std::ptr::null(),
                std::ptr::null_mut(),
                buf.as_mut_ptr() as *mut u8,
                &mut len,
            )
        };
        unsafe {
            let _ = RegCloseKey(hkey);
        }
        if status != 0 {
            return None;
        }
        let s = String::from_utf16_lossy(&buf);
        let s = s.trim_end_matches('\0').trim().to_string();
        (!s.is_empty()).then_some(s)
    }

    /// 读取本机 WebView2 Evergreen Runtime 版本（任一注册表路径命中即可）。
    fn webview2_version() -> Option<String> {
        for path in REG_PATHS {
            if let Some(v) = reg_read_string(HKEY_LOCAL_MACHINE, path, "pv") {
                return Some(v);
            }
            if let Some(v) = reg_read_string(HKEY_CURRENT_USER, path, "pv") {
                return Some(v);
            }
        }
        None
    }

    /// 最低要求的 WebView2 主版本：tauri v2 依赖的 evergreen 运行时下限。
    /// 低于此值视为“过旧”，引导用户运行官方安装器修复。
    const MIN_WEBVIEW2_MAJOR: u32 = 100;

    fn version_too_old(version: &str) -> bool {
        let major = version
            .split('.')
            .next()
            .and_then(|s| s.parse::<u32>().ok())
            .unwrap_or(0);
        major < MIN_WEBVIEW2_MAJOR
    }

    #[cfg(test)]
    fn version_too_old_pub(version: &str) -> bool {
        version_too_old(version)
    }

    fn msgbox(text: &str, title: &str, style: u32) -> i32 {
        let t = to_wide(text);
        let cap = to_wide(title);
        unsafe { MessageBoxW(std::ptr::null_mut(), t.as_ptr(), cap.as_ptr(), style) }
    }

    const MAX_BOOTSTRAPPER_BYTES: u64 = 32 * 1024 * 1024;

    /// 下载 Evergreen 引导安装器（微软官方链接）。
    /// 不内嵌在 exe 中：仅在 WebView2 缺失时按需下载，保持安装包精简。
    fn download_bootstrapper(path: &std::path::Path) -> Result<(), String> {
        let resp = ureq::Agent::config_builder()
            .tls_config(dsh_desktop_lib::default_tls_config())
            .timeout_connect(Some(Duration::from_secs(15)))
            .timeout_recv_response(Some(Duration::from_secs(90)))
            // body 整体时限放宽：慢网下载 32MB 引导安装器可能超过常规读时限
            .timeout_recv_body(Some(Duration::from_secs(3600)))
            .build()
            .new_agent()
            .get("https://go.microsoft.com/fwlink/p/?LinkId=2124703")
            .call()
            .map_err(|e| {
                format!(
                    "{}: {e}",
                    locale::text("网络请求失败", "Network request failed")
                )
            })?;
        let mut reader = resp
            .into_body()
            .into_reader()
            .take(MAX_BOOTSTRAPPER_BYTES + 1);
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)
            .map_err(|e| {
                format!(
                    "{}: {e}",
                    locale::text("创建临时文件失败", "Failed to create the temporary file")
                )
            })?;
        let written = std::io::copy(&mut reader, &mut file).map_err(|e| {
            format!(
                "{}: {e}",
                locale::text("写入安装器失败", "Failed to write the installer")
            )
        })?;
        if written > MAX_BOOTSTRAPPER_BYTES {
            drop(file);
            let _ = std::fs::remove_file(path);
            return Err(locale::text(
                "安装文件超过 32 MB 上限",
                "The installer exceeds the 32 MB safety limit",
            )
            .into());
        }
        file.sync_all().map_err(|e| {
            format!(
                "{}: {e}",
                locale::text("保存安装器失败", "Failed to save the installer")
            )
        })?;
        file.rewind().map_err(|e| {
            format!(
                "{}: {e}",
                locale::text("校验安装器失败", "Failed to validate the installer")
            )
        })?;
        let mut magic = [0u8; 2];
        file.read_exact(&mut magic).map_err(|e| {
            format!(
                "{}: {e}",
                locale::text("校验安装器失败", "Failed to validate the installer")
            )
        })?;
        if magic != *b"MZ" {
            drop(file);
            let _ = std::fs::remove_file(path);
            return Err(locale::text(
                "下载的文件不是有效的 Windows 程序",
                "The downloaded file is not a valid Windows executable",
            )
            .into());
        }
        Ok(())
    }

    /// 确保 WebView2 可用；缺失或版本过旧时下载官方引导安装器安装/修复。
    /// 返回 false 表示未就绪（应退出）。
    pub fn ensure_webview2() -> bool {
        match webview2_version() {
            Some(v) if !version_too_old(&v) => {
                return true; // 已安装且版本合格
            }
            _ => {}
        }
        let too_old = webview2_version().is_some();
        // 自启动（--minimized）场景无人可问，直接静默安装（仍会弹 UAC）。
        let silent = std::env::args().any(|a| a == "--minimized");
        if !silent {
            let prompt = if too_old {
                if locale::is_chinese() {
                    format!(
                        "{APP_TITLE} 需要 Microsoft Edge WebView2 运行时才能显示界面。\n\n本机检测到的 WebView2 版本过旧（{}），是否立即更新？\n（需联网下载安装组件，并弹出管理员授权提示，请选择“是”）",
                        webview2_version().unwrap_or_default()
                    )
                } else {
                    format!(
                        "{APP_TITLE} requires the Microsoft Edge WebView2 Runtime to display its interface.\n\nAn outdated WebView2 Runtime was found ({}). Update it now?\n(Internet access and an administrator approval prompt are required.)",
                        webview2_version().unwrap_or_default()
                    )
                }
            } else if locale::is_chinese() {
                format!(
                    "{APP_TITLE} 需要 Microsoft Edge WebView2 运行时才能显示界面。\n\n本机未检测到该组件，是否立即自动安装？\n（需联网下载安装组件，并弹出管理员授权提示，请选择“是”）"
                )
            } else {
                format!(
                    "{APP_TITLE} requires the Microsoft Edge WebView2 Runtime to display its interface.\n\nIt was not found on this computer. Install it now?\n(Internet access and an administrator approval prompt are required.)"
                )
            };
            let ask = msgbox(&prompt, APP_TITLE, MB_YESNO | MB_ICONQUESTION);
            if ask != IDYES {
                msgbox(
                    locale::text(
                        "缺少 WebView2 运行时，程序无法启动。\n可到微软官网搜索“WebView2 Runtime”手动安装后重试。",
                        "The app cannot start without the WebView2 Runtime.\nSearch Microsoft's website for “WebView2 Runtime”, install it, and try again.",
                    ),
                    APP_TITLE,
                    MB_OK | MB_ICONERROR,
                );
                return false;
            }
        }

        // 下载官方引导安装器到临时目录（WebView2 缺失时本就需要联网安装）
        let mut nonce = [0u8; 8];
        if let Err(e) = getrandom::fill(&mut nonce) {
            msgbox(
                &format!(
                    "{}: {e}",
                    locale::text(
                        "无法生成安装器临时文件名",
                        "Could not create a secure temporary installer name"
                    )
                ),
                APP_TITLE,
                MB_OK | MB_ICONERROR,
            );
            return false;
        }
        let installer = std::env::temp_dir().join(format!(
            "MicrosoftEdgeWebview2Setup-{:016x}.exe",
            u64::from_le_bytes(nonce)
        ));
        if let Err(e) = download_bootstrapper(&installer) {
            let _ = std::fs::remove_file(&installer);
            msgbox(
                &if locale::is_chinese() {
                    format!(
                        "下载 WebView2 安装组件失败：{e}\n请检查网络后重试，或到微软官网搜索“WebView2 Runtime”手动安装。"
                    )
                } else {
                    format!(
                        "Failed to download the WebView2 installer: {e}\nCheck the network and try again, or install “WebView2 Runtime” from Microsoft's website."
                    )
                },
                APP_TITLE,
                MB_OK | MB_ICONERROR,
            );
            return false;
        }

        // 静默安装（Evergreen 引导安装器：下载最新运行时并安装，UAC 提示由安装器弹出）
        let launched = std::process::Command::new(&installer)
            .args(["--silent", "--install"])
            .creation_flags(0x0800_0000) // CREATE_NO_WINDOW
            .spawn();
        match launched {
            Ok(_) => {}
            Err(e) => {
                let _ = std::fs::remove_file(&installer);
                msgbox(
                    &format!(
                        "{}: {e}",
                        locale::text(
                            "启动 WebView2 安装失败",
                            "Failed to start the WebView2 installer"
                        )
                    ),
                    APP_TITLE,
                    MB_OK | MB_ICONERROR,
                );
                return false;
            }
        }

        // 等待安装完成（最多 120 秒）
        for _ in 0..120 {
            std::thread::sleep(Duration::from_secs(1));
            if webview2_version().is_some_and(|v| !version_too_old(&v)) {
                let _ = std::fs::remove_file(&installer);
                return true;
            }
        }
        let _ = std::fs::remove_file(&installer);
        msgbox(
            locale::text(
                "未能确认 WebView2 安装完成，请重试。\n或到微软官网搜索“WebView2 Runtime”手动安装。",
                "The WebView2 installation could not be confirmed. Try again, or install “WebView2 Runtime” from Microsoft's website.",
            ),
            APP_TITLE,
            MB_OK | MB_ICONERROR,
        );
        false
    }

    #[cfg(test)]
    mod tests {
        use super::version_too_old;

        #[test]
        fn webview2_version_floor() {
            assert!(version_too_old("99.0.0.0"));
            assert!(version_too_old(""));
            assert!(version_too_old("abc"));
            assert!(!version_too_old("100.0.0.0"));
            assert!(!version_too_old("118.0.2088.69"));
        }
    }
}