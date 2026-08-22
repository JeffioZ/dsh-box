//! 本地文件动作：默认程序打开 / 文件管理器定位 / 系统“打开方式”选择器。
//!
//! Windows 走 ShellExecuteW（无控制台闪烁、正确处理含空格路径、直接复用
//! 系统文件关联）；macOS 走 `open`；Linux 走 `xdg-open`。
//! 供 dshd:// 自定义协议处理函数调用（dsh 页面右键文件路径时触发）。

use std::io::Read;
use std::path::{Path, PathBuf};

/// 是否为绝对路径（Windows 盘符/UNC，POSIX 以 / 开头）。
/// 相对路径一律拒绝：页面上下文的工作区根目录只有 dsh 后端知道，
/// 桌面端无法正确解析，交由“打开”动作（复用 dsh 自己的打开逻辑）处理。
pub fn is_absolute(path: &str) -> bool {
    Path::new(path).is_absolute()
}

/// 展开浏览器常见的主目录/MSYS 路径写法，供右键菜单在交给系统动作前规范化。
pub fn normalize_user_path(path: &str) -> Option<PathBuf> {
    if path == "~" || path.starts_with("~/") || path.starts_with("~\\") {
        let home = dirs::home_dir()?;
        let rest = path
            .strip_prefix('~')
            .unwrap_or(path)
            .trim_start_matches(['/', '\\']);
        return Some(if rest.is_empty() {
            home
        } else {
            home.join(rest)
        });
    }
    #[cfg(windows)]
    {
        let bytes = path.as_bytes();
        if bytes.len() >= 3
            && bytes[0] == b'/'
            && bytes[1].is_ascii_alphabetic()
            && bytes[2] == b'/'
        {
            let drive = (bytes[1] as char).to_ascii_uppercase();
            let rest = path[3..].replace('/', "\\");
            return Some(PathBuf::from(format!("{drive}:\\{rest}")));
        }
    }
    let path = PathBuf::from(path);
    path.is_absolute().then_some(path)
}

fn current_platform() -> &'static str {
    #[cfg(windows)]
    return "windows";
    #[cfg(target_os = "macos")]
    return "macos";
    #[cfg(target_os = "linux")]
    return "linux";
    #[allow(unreachable_code)]
    "other"
}

fn dangerous_extension(ext: &str, platform: &str) -> bool {
    match platform {
        "windows" => matches!(
            ext,
            "exe"
                | "com"
                | "bat"
                | "cmd"
                | "ps1"
                | "psm1"
                | "vbs"
                | "vbe"
                | "js"
                | "jse"
                | "wsf"
                | "wsh"
                | "msi"
                | "msp"
                | "mst"
                | "scr"
                | "pif"
                | "cpl"
                | "reg"
                | "lnk"
                | "appx"
                | "msix"
                | "msixbundle"
        ),
        "macos" => matches!(ext, "app" | "command" | "pkg" | "dmg" | "scpt"),
        "linux" => matches!(ext, "appimage" | "desktop" | "run" | "sh"),
        _ => false,
    }
}

/// 默认打开会触发执行或安装的高风险文件；只应用当前平台真正会执行的后缀，
/// 避免在 macOS/Linux 因 Windows 专属扩展名产生无意义确认。
pub fn is_potentially_executable(path: &Path) -> bool {
    let ext = path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if dangerous_extension(&ext, current_platform()) {
        return true;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        path.metadata()
            .is_ok_and(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
    }
    #[cfg(not(unix))]
    false
}

/// 用默认程序打开文件。
pub fn open_default(path: &Path) -> Result<(), String> {
    #[cfg(windows)]
    {
        shell_execute("open", path, None)
    }
    #[cfg(target_os = "macos")]
    {
        let mut cmd = std::process::Command::new("open");
        cmd.arg(path.as_os_str());
        status_ok(cmd, "默认程序打开", "Open with the default app")
    }
    #[cfg(target_os = "linux")]
    {
        let mut cmd = std::process::Command::new("xdg-open");
        cmd.arg(path.as_os_str());
        status_ok(cmd, "默认程序打开", "Open with the default app")
    }
    #[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
    {
        Err(crate::locale::text("当前平台不支持", "This platform is not supported").into())
    }
}

/// 在文件管理器中定位该文件。
pub fn reveal(path: &Path) -> Result<(), String> {
    #[cfg(windows)]
    {
        // explorer /select,<path>：即使文件不存在也能定位到其所在目录
        let params = format!("/select,\"{}\"", path.display());
        shell_execute("open", Path::new("explorer.exe"), Some(&params))
    }
    #[cfg(target_os = "macos")]
    {
        let mut cmd = std::process::Command::new("open");
        cmd.arg("-R").arg(path.as_os_str());
        status_ok(cmd, "在 Finder 中定位", "Reveal in Finder")
    }
    #[cfg(target_os = "linux")]
    {
        // 无跨桌面统一的“选中文件”协议，退化为打开所在目录
        let dir = path
            .parent()
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."));
        let mut cmd = std::process::Command::new("xdg-open");
        cmd.arg(dir.as_os_str());
        status_ok(cmd, "打开所在目录", "Open the containing folder")
    }
    #[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
    {
        Err(crate::locale::text("当前平台不支持", "This platform is not supported").into())
    }
}

/// 打开系统“打开方式”选择器（Windows 专属，官方 SHOpenWithDialog API）。
/// 调用方应放在独立 STA 线程执行（模态对话框阻塞该线程）。
/// parent_hwnd 为主窗口句柄（可空）：先 SetForegroundWindow 再弹窗，
/// 保证对话框置顶于主窗口之上。
pub fn open_with_picker(path: &Path, parent_hwnd: Option<isize>) -> Result<(), String> {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::System::Com::{
            CoInitializeEx, CoUninitialize, COINIT_APARTMENTTHREADED,
        };
        use windows_sys::Win32::UI::Shell::{
            SHOpenWithDialog, OAIF_ALLOW_REGISTRATION, OAIF_EXEC, OAIF_REGISTER_EXT, OPENASINFO,
            OPEN_AS_INFO_FLAGS,
        };
        use windows_sys::Win32::UI::WindowsAndMessaging::SetForegroundWindow;

        let owner = (parent_hwnd.unwrap_or(0)) as windows_sys::Win32::Foundation::HWND;
        if !owner.is_null() {
            // 用户刚在应用内点击：进程持有前台权限，置顶主窗口让对话框落在其上方
            unsafe { SetForegroundWindow(owner) };
        }
        let path_w: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
        let mut flags: OPEN_AS_INFO_FLAGS = 0;
        flags |= OAIF_ALLOW_REGISTRATION | OAIF_REGISTER_EXT | OAIF_EXEC;
        let info = OPENASINFO {
            pcszFile: path_w.as_ptr(),
            pcszClass: std::ptr::null(),
            oaifInFlags: flags,
        };
        let init = unsafe { CoInitializeEx(std::ptr::null(), COINIT_APARTMENTTHREADED as u32) };
        if init < 0 {
            return Err(crate::locale::owned(
                format!("COM 初始化失败（HRESULT 0x{init:08X}）"),
                format!("COM initialization failed (HRESULT 0x{init:08X})"),
            ));
        }
        let hr = unsafe { SHOpenWithDialog(owner, &info) };
        // S_OK 与 S_FALSE 都会增加当前线程的 COM 初始化引用计数，必须成对释放。
        unsafe { CoUninitialize() };
        // S_OK(0)=已选择并启动应用；S_FALSE(1)=用户取消——均视为正常
        if hr >= 0 {
            Ok(())
        } else {
            Err(crate::locale::owned(
                format!("SHOpenWithDialog 失败（HRESULT 0x{hr:08X}）"),
                format!("SHOpenWithDialog failed (HRESULT 0x{hr:08X})"),
            ))
        }
    }
    #[cfg(not(windows))]
    {
        let _ = (path, parent_hwnd);
        Err(crate::locale::text(
            "“打开方式”仅支持 Windows",
            "\"Open with\" is only supported on Windows",
        )
        .into())
    }
}

/// 在默认浏览器中打开链接（自身校验：仅接受有效的 http/https 链接）。
pub fn open_browser(url: &str) -> Result<(), String> {
    let parsed = url::Url::parse(url)
        .map_err(|_| crate::locale::text("链接格式无效", "Invalid link format"))?;
    if !matches!(parsed.scheme(), "http" | "https") || parsed.host_str().is_none() {
        return Err(crate::locale::text(
            "仅支持打开 http/https 链接",
            "Only valid HTTP or HTTPS links can be opened",
        )
        .into());
    }
    #[cfg(windows)]
    {
        shell_execute("open", Path::new(url), None)
    }
    #[cfg(target_os = "macos")]
    {
        let mut cmd = std::process::Command::new("open");
        cmd.arg(url);
        status_ok(cmd, "在浏览器打开", "Open in the browser")
    }
    #[cfg(target_os = "linux")]
    {
        let mut cmd = std::process::Command::new("xdg-open");
        cmd.arg(url);
        status_ok(cmd, "在浏览器打开", "Open in the browser")
    }
    #[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
    {
        Err(crate::locale::text("当前平台不支持", "This platform is not supported").into())
    }
}

/// 用指定应用打开文件（Windows 专属；app 为 code/notepad/paint）。
pub fn open_with_app(app: &str, path: &Path) -> Result<(), String> {
    #[cfg(windows)]
    {
        let exe = match app {
            "code" => vscode_exe().ok_or_else(|| {
                crate::locale::text("未找到 VS Code", "VS Code was not found").to_string()
            })?,
            "notepad" => system32("notepad.exe"),
            "paint" => system32("mspaint.exe"),
            other => {
                return Err(crate::locale::owned(
                    format!("未知应用：{other}"),
                    format!("Unknown app: {other}"),
                ))
            }
        };
        let params = format!("\"{}\"", path.display());
        shell_execute("open", &exe, Some(&params))
    }
    #[cfg(not(windows))]
    {
        let _ = (app, path);
        Err(crate::locale::text(
            "“指定应用打开”仅支持 Windows",
            "\"Open with app\" is only supported on Windows",
        )
        .into())
    }
}

/// 定位 VS Code 可执行文件（标准安装路径；结果进程内缓存）。
pub fn vscode_exe() -> Option<PathBuf> {
    #[cfg(windows)]
    {
        use std::sync::OnceLock;
        static CACHE: OnceLock<Option<PathBuf>> = OnceLock::new();
        CACHE
            .get_or_init(|| {
                let mut candidates = Vec::new();
                if let Ok(p) = std::env::var("LOCALAPPDATA") {
                    candidates.push(
                        PathBuf::from(p)
                            .join("Programs")
                            .join("Microsoft VS Code")
                            .join("Code.exe"),
                    );
                }
                if let Ok(p) = std::env::var("ProgramFiles") {
                    candidates.push(PathBuf::from(p).join("Microsoft VS Code").join("Code.exe"));
                }
                candidates.into_iter().find(|p| p.exists())
            })
            .clone()
    }
    #[cfg(not(windows))]
    {
        None
    }
}

/// Windows 系统目录下的可执行文件路径。
#[cfg(windows)]
fn system32(name: &str) -> PathBuf {
    std::env::var("SystemRoot")
        .map(|r| PathBuf::from(r).join("System32").join(name))
        .unwrap_or_else(|_| PathBuf::from(format!("C:\\Windows\\System32\\{name}")))
}

/// 读取文本文件内容（“复制文件内容”用）：限制大小，检测二进制
/// （前 8KB 含 NUL 或非 UTF-8）直接拒绝，避免把二进制内容写入剪贴板。
pub fn read_text_file(path: &Path, max_bytes: usize) -> Result<String, String> {
    let file = std::fs::File::open(path).map_err(|e| {
        crate::locale::owned(format!("读取失败：{e}"), format!("Failed to read: {e}"))
    })?;
    let metadata = file.metadata().map_err(|e| {
        crate::locale::owned(format!("读取失败：{e}"), format!("Failed to read: {e}"))
    })?;
    if !metadata.is_file() {
        return Err(crate::locale::text(
            "只能复制普通文件的内容",
            "Only regular file contents can be copied",
        )
        .into());
    }
    if metadata.len() > max_bytes as u64 {
        return Err(crate::locale::owned(
            format!("文件超过 {} MB 上限", max_bytes / 1024 / 1024),
            format!("The file exceeds the {} MB limit", max_bytes / 1024 / 1024),
        ));
    }
    let mut data = Vec::with_capacity(max_bytes.min(64 * 1024));
    file.take(max_bytes as u64 + 1)
        .read_to_end(&mut data)
        .map_err(|e| {
            crate::locale::owned(format!("读取失败：{e}"), format!("Failed to read: {e}"))
        })?;
    if data.len() > max_bytes {
        return Err(crate::locale::owned(
            format!("文件超过 {} MB 上限", max_bytes / 1024 / 1024),
            format!("The file exceeds the {} MB limit", max_bytes / 1024 / 1024),
        ));
    }
    if data.iter().take(8192).any(|&b| b == 0) {
        return Err(crate::locale::text(
            "二进制文件，无法复制内容",
            "Binary files cannot be copied as text",
        )
        .into());
    }
    String::from_utf8(data).map_err(|_| {
        crate::locale::text(
            "非 UTF-8 文本，无法复制内容",
            "The file is not UTF-8 text and cannot be copied",
        )
        .to_string()
    })
}

/// 等待命令退出并按状态码判断成功与否（macOS/Linux）。
#[cfg(not(windows))]
fn status_ok(mut cmd: std::process::Command, zh: &str, en: &str) -> Result<(), String> {
    match cmd.status() {
        Ok(s) if s.success() => Ok(()),
        Ok(s) => Err(crate::locale::owned(
            format!("{zh}失败（退出码 {:?}）", s.code()),
            format!("{en} failed (exit code {:?})", s.code()),
        )),
        Err(e) => Err(crate::locale::owned(
            format!("{zh}失败：{e}"),
            format!("{en} failed: {e}"),
        )),
    }
}

/// ShellExecuteW 封装：返回值 > 32 视为成功，否则为错误码。
#[cfg(windows)]
fn shell_execute(verb: &str, path: &Path, params: Option<&str>) -> Result<(), String> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::UI::Shell::ShellExecuteW;
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

    fn wide(s: &str) -> Vec<u16> {
        std::ffi::OsStr::new(s)
            .encode_wide()
            .chain(Some(0))
            .collect()
    }
    let verb_w = wide(verb);
    let path_w: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    let params_w = params.map(wide);
    let ret = unsafe {
        ShellExecuteW(
            std::ptr::null_mut(),
            verb_w.as_ptr(),
            path_w.as_ptr(),
            params_w.as_ref().map_or(std::ptr::null(), |p| p.as_ptr()),
            std::ptr::null(),
            SW_SHOWNORMAL,
        )
    };
    if ret as isize > 32 {
        Ok(())
    } else {
        Err(crate::locale::owned(
            format!("ShellExecuteW 失败（错误码 {}）", ret as isize),
            format!("ShellExecuteW failed (error code {})", ret as isize),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::{dangerous_extension, is_potentially_executable, normalize_user_path};
    use std::path::Path;

    #[test]
    fn executable_extensions_are_case_insensitive() {
        assert_eq!(
            is_potentially_executable(Path::new("setup.EXE")),
            cfg!(windows)
        );
        assert_eq!(
            is_potentially_executable(Path::new("bootstrap.ps1")),
            cfg!(windows)
        );
        assert!(!is_potentially_executable(Path::new("notes.md")));
    }

    #[test]
    fn dangerous_extensions_are_platform_specific() {
        assert!(dangerous_extension("exe", "windows"));
        assert!(!dangerous_extension("exe", "macos"));
        assert!(!dangerous_extension("exe", "linux"));
        assert!(dangerous_extension("app", "macos"));
        assert!(dangerous_extension("desktop", "linux"));
        assert!(!dangerous_extension("desktop", "other"));
    }

    #[test]
    fn home_path_expands_without_touching_lookalikes() {
        assert!(normalize_user_path("~/notes.md").is_some());
        assert!(normalize_user_path("~\\notes.md").is_some());
        assert!(normalize_user_path("~someone/notes.md").is_none());
    }

    #[cfg(windows)]
    #[test]
    fn msys_drive_path_normalizes_on_windows() {
        assert_eq!(
            normalize_user_path("/c/work/file.txt").unwrap(),
            std::path::PathBuf::from(r"C:\work\file.txt")
        );
    }
}
