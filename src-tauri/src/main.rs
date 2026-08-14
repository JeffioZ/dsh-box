// 发布版不弹出附加的控制台窗口。
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

//! DSHDesktop 入口。
//!
//! Windows：启动前检查 WebView2 Runtime，缺失时自动安装；
//! macOS/Linux：使用系统内置渲染（WKWebView/WebKitGTK），无需预检。

fn main() {
    #[cfg(windows)]
    if !webview2_check::ensure_webview2() {
        return;
    }
    dsh_desktop_lib::run()
}

/// WebView2 前置检查（仅 Windows：注册表检测 + 下载官方引导安装器自动安装）。
#[cfg(windows)]
mod webview2_check {
    use dsh_desktop_lib::APP_TITLE;
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

    /// 检查注册表中是否存在 WebView2 版本值。
    fn reg_value_exists(root: HKEY, path: &str, value: &str) -> bool {
        let path_w = to_wide(path);
        let value_w = to_wide(value);
        let mut hkey: HKEY = std::ptr::null_mut();
        let status = unsafe { RegOpenKeyExW(root, path_w.as_ptr(), 0, KEY_QUERY_VALUE, &mut hkey) };
        if status != 0 {
            return false;
        }
        let mut buf = [0u8; 64];
        let mut len = buf.len() as u32;
        let result = unsafe {
            RegQueryValueExW(
                hkey,
                value_w.as_ptr(),
                std::ptr::null(),
                std::ptr::null_mut(),
                buf.as_mut_ptr(),
                &mut len,
            )
        };
        unsafe {
            let _ = RegCloseKey(hkey);
        }
        result == 0
    }

    fn webview2_installed() -> bool {
        for path in REG_PATHS {
            if reg_value_exists(HKEY_LOCAL_MACHINE, path, "pv")
                || reg_value_exists(HKEY_CURRENT_USER, path, "pv")
            {
                return true;
            }
        }
        false
    }

    fn msgbox(text: &str, title: &str, style: u32) -> i32 {
        let t = to_wide(text);
        let cap = to_wide(title);
        unsafe { MessageBoxW(std::ptr::null_mut(), t.as_ptr(), cap.as_ptr(), style) }
    }

    /// 下载 Evergreen 引导安装器（微软官方直链，约 1.6 MB）。
    /// 不内嵌在 exe 中：仅在 WebView2 缺失时按需下载，保持安装包精简。
    fn download_bootstrapper(path: &std::path::Path) -> Result<(), String> {
        let resp = ureq::AgentBuilder::new()
            .timeout_connect(Duration::from_secs(15))
            .timeout_read(Duration::from_secs(120))
            .build()
            .get("https://go.microsoft.com/fwlink/p/?LinkId=2124703")
            .call()
            .map_err(|e| format!("网络请求失败：{e}"))?;
        let mut reader = resp.into_reader();
        let mut file = std::fs::File::create(path).map_err(|e| format!("创建临时文件失败：{e}"))?;
        std::io::copy(&mut reader, &mut file).map_err(|e| format!("写入安装器失败：{e}"))?;
        Ok(())
    }

    /// 确保 WebView2 可用；缺失时下载官方引导安装器自动安装。
    /// 返回 false 表示未就绪（应退出）。
    pub fn ensure_webview2() -> bool {
        if webview2_installed() {
            return true;
        }
        // 自启动（--minimized）场景无人可问，直接静默安装（仍会弹 UAC）。
        let silent = std::env::args().any(|a| a == "--minimized");
        if !silent {
            let ask = msgbox(
                &format!(
                    "{APP_TITLE} 需要 Microsoft Edge WebView2 运行时才能显示界面。\n\n本机未检测到该组件，是否立即自动安装？\n（需联网下载约 1.6 MB 安装组件，并弹出管理员授权提示，请选择“是”）"
                ),
                APP_TITLE,
                MB_YESNO | MB_ICONQUESTION,
            );
            if ask != IDYES {
                msgbox(
                    "缺少 WebView2 运行时，程序无法启动。\n可到微软官网搜索“WebView2 Runtime”手动安装后重试。",
                    APP_TITLE,
                    MB_OK | MB_ICONERROR,
                );
                return false;
            }
        }

        // 下载官方引导安装器到临时目录（WebView2 缺失时本就需要联网安装）
        let installer = std::env::temp_dir().join("MicrosoftEdgeWebview2Setup.exe");
        if let Err(e) = download_bootstrapper(&installer) {
            msgbox(
                &format!(
                    "下载 WebView2 安装组件失败：{e}\n请检查网络后重试，或到微软官网搜索“WebView2 Runtime”手动安装。"
                ),
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
                msgbox(
                    &format!("启动 WebView2 安装失败：{e}"),
                    APP_TITLE,
                    MB_OK | MB_ICONERROR,
                );
                return false;
            }
        }

        // 等待安装完成（最多 120 秒）
        for _ in 0..120 {
            std::thread::sleep(Duration::from_secs(1));
            if webview2_installed() {
                return true;
            }
        }
        msgbox(
            "WebView2 安装未能确认完成，请重试。\n或到微软官网搜索“WebView2 Runtime”手动安装。",
            APP_TITLE,
            MB_OK | MB_ICONERROR,
        );
        false
    }
}
