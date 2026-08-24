//! 开机自启动（跨平台）：
//! - Windows：HKCU\...\Run 注册表键（值为 `"<exe>" --minimized`）；
//! - macOS：~/Library/LaunchAgents/com.deepseek.dsh-box.plist；
//! - Linux：~/.config/autostart/dsh-box.desktop。

/// 可执行文件路径（不带引号，供 plist 等按参数拆分使用）。
/// 取不到时返回错误：把空串写进 Run 键/plist/desktop 会留下指向空路径的
/// 自启动项（静默坏配置），不如直接失败并告知。
fn exe_path() -> Result<String, String> {
    std::env::current_exe()
        .map(|p| p.to_string_lossy().into_owned())
        .map_err(|e| {
            format!(
                "{}: {e}",
                crate::locale::text(
                    "无法确定当前程序路径",
                    "Could not determine the current executable path"
                )
            )
        })
}

/// 自启动命令（带 --minimized 静默进托盘）。
#[cfg(windows)]
fn app_command() -> Result<String, String> {
    Ok(format!("\"{}\" --minimized", exe_path()?))
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
    const VALUE_NAME: &str = "DSHBox";

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
            return Err(format!(
                "{}: {status}",
                crate::locale::text(
                    "打开注册表 Run 键失败",
                    "Failed to open the registry Run key"
                )
            ));
        }
        let name = to_wide(VALUE_NAME);
        let result = if enabled {
            let cmd = to_wide(&app_command()?);
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
            Err(format!(
                "{}: {result}",
                crate::locale::text("写入注册表失败", "Failed to update the registry")
            ))
        }
    }
}

// ---------------- macOS：LaunchAgents plist ----------------

#[cfg(target_os = "macos")]
mod imp {
    use super::*;
    use std::path::PathBuf;

    const LABEL: &str = "com.deepseek.dsh-box";
    const FILE_NAME: &str = "com.deepseek.dsh-box.plist";

    fn plist_path() -> Option<PathBuf> {
        dirs::home_dir().map(|home| home.join("Library/LaunchAgents").join(FILE_NAME))
    }

    fn xml_escape(value: &str) -> String {
        value
            .replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
            .replace('"', "&quot;")
            .replace('\'', "&apos;")
    }

    pub fn is_enabled() -> bool {
        plist_path().is_some_and(|path| path.exists())
    }

    pub fn set_enabled(enabled: bool) -> Result<(), String> {
        let path = plist_path().ok_or_else(|| {
            crate::locale::text(
                "无法确定当前用户主目录",
                "Could not determine the current user's home directory",
            )
        })?;
        if !enabled {
            return match std::fs::remove_file(&path) {
                Ok(()) => Ok(()),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(e) => Err(format!(
                    "{}: {e}",
                    crate::locale::text("删除 plist 失败", "Failed to remove the plist")
                )),
            };
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
            exe = xml_escape(&exe_path()?),
        );
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| {
                format!(
                    "{}: {e}",
                    crate::locale::text(
                        "创建 LaunchAgents 目录失败",
                        "Failed to create the LaunchAgents directory"
                    )
                )
            })?;
        }
        std::fs::write(&path, content).map_err(|e| {
            format!(
                "{}: {e}",
                crate::locale::text("写入 plist 失败", "Failed to write the plist")
            )
        })
    }
}

// ---------------- Linux：XDG autostart .desktop ----------------

#[cfg(target_os = "linux")]
mod imp {
    use super::*;
    use std::path::PathBuf;

    const FILE_NAME: &str = "dsh-box.desktop";

    fn desktop_path() -> Option<PathBuf> {
        dirs::home_dir().map(|home| home.join(".config/autostart").join(FILE_NAME))
    }

    fn desktop_exec_arg(value: &str) -> String {
        let escaped = value
            .replace('\\', "\\\\")
            .replace('"', "\\\"")
            .replace('$', "\\$")
            .replace('`', "\\`")
            .replace('%', "%%");
        format!("\"{escaped}\"")
    }

    pub fn is_enabled() -> bool {
        desktop_path().is_some_and(|path| path.exists())
    }

    pub fn set_enabled(enabled: bool) -> Result<(), String> {
        let path = desktop_path().ok_or_else(|| {
            crate::locale::text(
                "无法确定当前用户主目录",
                "Could not determine the current user's home directory",
            )
        })?;
        if !enabled {
            return match std::fs::remove_file(&path) {
                Ok(()) => Ok(()),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(e) => Err(format!(
                    "{}: {e}",
                    crate::locale::text(
                        "删除 .desktop 文件失败",
                        "Failed to remove the .desktop file"
                    )
                )),
            };
        }
        let exec = format!("{} --minimized", desktop_exec_arg(&exe_path()?));
        let content = format!(
            "[Desktop Entry]\n\
             Type=Application\n\
             Name=DSHBox\n\
             Comment=DeepSeek Harness desktop client\n\
             Exec={exec}\n\
             Terminal=false\n\
             X-GNOME-Autostart-enabled=true\n"
        );
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| {
                format!(
                    "{}: {e}",
                    crate::locale::text(
                        "创建 autostart 目录失败",
                        "Failed to create the autostart directory"
                    )
                )
            })?;
        }
        std::fs::write(&path, content).map_err(|e| {
            format!(
                "{}: {e}",
                crate::locale::text("写入 .desktop 失败", "Failed to write the .desktop file")
            )
        })
    }
}

#[cfg(test)]
mod tests {
    use super::exe_path;

    #[test]
    fn exe_path_resolves_to_a_non_empty_path() {
        // 约定：成功时绝不返回空串（空串写进自启动项是静默坏配置）
        assert!(!exe_path().unwrap().is_empty());
    }
}
