//! 开机自启动（跨平台）：
//! - Windows：HKCU\...\Run 注册表键（值为 `"<exe>" --minimized`）；
//! - macOS：~/Library/LaunchAgents/com.deepseek.dsh-desktop.plist；
//! - Linux：~/.config/autostart/dsh-desktop.desktop。

/// 可执行文件路径（不带引号，供 plist 等按参数拆分使用）。
fn exe_path() -> String {
    std::env::current_exe()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// 自启动命令（带 --minimized 静默进托盘）。
#[cfg(not(target_os = "macos"))]
fn app_command() -> String {
    format!("\"{}\" --minimized", exe_path())
}

pub fn is_enabled() -> bool {
    imp::is_enabled()
}

pub fn set_enabled(enabled: bool) -> Result<(), String> {
    imp::set_enabled(enabled)
}

// ---------------- Windows：注册表 ----------------

#[cfg(windows)]
mod imp {
    use super::*;
    use windows_sys::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS};
    use windows_sys::Win32::System::Registry::{
        RegCloseKey, RegDeleteValueW, RegOpenKeyExW, RegQueryValueExW, RegSetValueExW, HKEY,
        HKEY_CURRENT_USER, KEY_QUERY_VALUE, KEY_SET_VALUE, REG_SZ,
    };

    const RUN_KEY: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Run";
    const VALUE_NAME: &str = "DSHDesktop";

    fn to_wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    pub fn is_enabled() -> bool {
        let key = to_wide(RUN_KEY);
        let mut hkey: HKEY = std::ptr::null_mut();
        let status = unsafe {
            RegOpenKeyExW(
                HKEY_CURRENT_USER,
                key.as_ptr(),
                0,
                KEY_QUERY_VALUE,
                &mut hkey,
            )
        };
        if status != ERROR_SUCCESS {
            return false;
        }
        let name = to_wide(VALUE_NAME);
        let mut buf = [0u16; 2048];
        let mut len = (buf.len() * 2) as u32;
        let result = unsafe {
            RegQueryValueExW(
                hkey,
                name.as_ptr(),
                std::ptr::null(),
                std::ptr::null_mut(),
                buf.as_mut_ptr() as *mut u8,
                &mut len,
            )
        };
        unsafe {
            let _ = RegCloseKey(hkey);
        }
        result == ERROR_SUCCESS
    }

    pub fn set_enabled(enabled: bool) -> Result<(), String> {
        let key = to_wide(RUN_KEY);
        let mut hkey: HKEY = std::ptr::null_mut();
        let status =
            unsafe { RegOpenKeyExW(HKEY_CURRENT_USER, key.as_ptr(), 0, KEY_SET_VALUE, &mut hkey) };
        if status != ERROR_SUCCESS {
            return Err(format!("打开注册表 Run 键失败：{status}"));
        }
        let name = to_wide(VALUE_NAME);
        let result = if enabled {
            let cmd = to_wide(&app_command());
            unsafe {
                RegSetValueExW(
                    hkey,
                    name.as_ptr(),
                    0,
                    REG_SZ,
                    cmd.as_ptr() as *const u8,
                    (cmd.len() * 2) as u32,
                )
            }
        } else {
            unsafe { RegDeleteValueW(hkey, name.as_ptr()) }
        };
        unsafe {
            let _ = RegCloseKey(hkey);
        }
        // 删除不存在的值返回 ERROR_FILE_NOT_FOUND，视为成功。
        if result == ERROR_SUCCESS || (result == ERROR_FILE_NOT_FOUND && !enabled) {
            Ok(())
        } else {
            Err(format!("写入注册表失败：{result}"))
        }
    }
}

// ---------------- macOS：LaunchAgents plist ----------------

#[cfg(target_os = "macos")]
mod imp {
    use super::*;
    use std::path::PathBuf;

    const LABEL: &str = "com.deepseek.dsh-desktop";
    const FILE_NAME: &str = "com.deepseek.dsh-desktop.plist";

    fn plist_path() -> PathBuf {
        let home = std::env::var("HOME").unwrap_or_default();
        PathBuf::from(home)
            .join("Library/LaunchAgents")
            .join(FILE_NAME)
    }

    pub fn is_enabled() -> bool {
        plist_path().exists()
    }

    pub fn set_enabled(enabled: bool) -> Result<(), String> {
        let path = plist_path();
        if !enabled {
            let _ = std::fs::remove_file(&path);
            return Ok(());
        }
        let content = format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>{label}</string>
    <key>ProgramArguments</key>
    <array>
        <string>{exe}</string>
        <string>--minimized</string>
    </array>
    <key>RunAtLoad</key>
    <true/>
</dict>
</plist>
"#,
            label = LABEL,
            exe = exe_path(),
        );
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| format!("创建 LaunchAgents 目录失败：{e}"))?;
        }
        std::fs::write(&path, content).map_err(|e| format!("写入 plist 失败：{e}"))
    }
}

// ---------------- Linux：XDG autostart .desktop ----------------

#[cfg(target_os = "linux")]
mod imp {
    use super::*;
    use std::path::PathBuf;

    const FILE_NAME: &str = "dsh-desktop.desktop";

    fn desktop_path() -> PathBuf {
        let home = std::env::var("HOME").unwrap_or_default();
        PathBuf::from(home)
            .join(".config/autostart")
            .join(FILE_NAME)
    }

    pub fn is_enabled() -> bool {
        desktop_path().exists()
    }

    pub fn set_enabled(enabled: bool) -> Result<(), String> {
        let path = desktop_path();
        if !enabled {
            let _ = std::fs::remove_file(&path);
            return Ok(());
        }
        let content = format!(
            "[Desktop Entry]\n\
             Type=Application\n\
             Name=DeepSeek Harness Desktop\n\
             Comment=DeepSeek Harness Desktop\n\
             Exec={}\n\
             Terminal=false\n\
             X-GNOME-Autostart-enabled=true\n",
            app_command()
        );
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| format!("创建 autostart 目录失败：{e}"))?;
        }
        std::fs::write(&path, content).map_err(|e| format!("写入 .desktop 失败：{e}"))
    }
}
