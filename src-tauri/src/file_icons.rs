//! 提取文件/应用关联的 16×16 小图标并编码为 PNG（右键菜单图标显示用）。
//!
//! Windows 走 SHGetFileInfoW（文件 → 关联应用图标；exe → 自身图标），
//! HICON 经 GetDIBits 取 32bpp BGRA 像素后手写 PNG 编码（flate2 压缩），
//! 避免引入 image 依赖，控制单 exe 体积。其余平台返回 None（菜单图标空缺）。

use std::path::Path;

/// 提取 `path`（文件或 exe）的小图标 → PNG 字节；失败返回 None。
/// 带进程内缓存（上限 128 项，超限整体清空；失败结果同样缓存，避免重复提取
/// 与日志刷屏）：同一文件的图标只提取一次，菜单反复打开无提取开销。
pub fn icon_png_16(path: &Path) -> Option<Vec<u8>> {
    use std::collections::HashMap;
    use std::sync::{Mutex, OnceLock};
    static CACHE: OnceLock<Mutex<HashMap<String, Option<Vec<u8>>>>> = OnceLock::new();
    let key = path.to_string_lossy().into_owned();
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    if let Some(cached) = cache.lock().ok().and_then(|m| m.get(&key).cloned()) {
        return cached;
    }
    let png = icon_png_16_uncached(path);
    if let Ok(mut map) = cache.lock() {
        if map.len() >= 128 {
            map.clear();
        }
        map.insert(key, png.clone());
    }
    png
}

/// 无缓存提取（内部实现）。
#[cfg(windows)]
fn icon_png_16_uncached(path: &Path) -> Option<Vec<u8>> {
    // Win11 起 mspaint 等系统工具迁移为 MSIX 应用，System32 原路径已不存在；
    // 同名应用执行别名仍提供打包图标，原路径失败时兜底再试一次。
    let hicon = match sh_small_icon(path) {
        Some(hicon) => hicon,
        None => sh_small_icon(&windows_apps_alias(path)?)?,
    };
    let png = hicon_to_png(hicon);
    unsafe { windows_sys::Win32::UI::WindowsAndMessaging::DestroyIcon(hicon) };
    png
}

/// 取路径的小图标 HICON（调用方负责 DestroyIcon）；失败返回 None。
/// SHGetFileInfoW 在进程首次并发取图标时会因 shell 图标缓存冷启动竞争
/// 短暂失败、立即重试即成功（实测 8 线程并发首调大面积失败、二调全过），
/// 故失败时短歇重试两次再放弃；真正的缺失文件三次都失败，仍返回 None。
#[cfg(windows)]
fn sh_small_icon(path: &Path) -> Option<windows_sys::Win32::UI::WindowsAndMessaging::HICON> {
    use std::mem::zeroed;
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::UI::Shell::{SHGetFileInfoW, SHFILEINFOW, SHGFI_ICON, SHGFI_SMALLICON};

    let path_w: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    for attempt in 0..3 {
        let mut info: SHFILEINFOW = unsafe { zeroed() };
        let ok = unsafe {
            SHGetFileInfoW(
                path_w.as_ptr(),
                0,
                &mut info,
                std::mem::size_of::<SHFILEINFOW>() as u32,
                SHGFI_ICON | SHGFI_SMALLICON,
            )
        };
        if ok != 0 && !info.hIcon.is_null() {
            return Some(info.hIcon);
        }
        if attempt < 2 {
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    }
    None
}

/// System32 直下且不存在的 exe，可能在 %LOCALAPPDATA%\Microsoft\WindowsApps
/// 有同名应用执行别名（MSIX 迁移，如 mspaint）。仅限该目录兜底：避免把任意
/// 缺失路径映射到用户可写的 WindowsApps 目录而被偷换图标。
#[cfg(windows)]
pub(crate) fn windows_apps_alias(path: &Path) -> Option<std::path::PathBuf> {
    if path.exists() {
        return None;
    }
    let system32 = std::env::var_os("SystemRoot")
        .map(std::path::PathBuf::from)?
        .join("System32");
    let parent = path.parent()?;
    if !parent
        .to_string_lossy()
        .eq_ignore_ascii_case(&system32.to_string_lossy())
    {
        return None;
    }
    let name = path.file_name()?;
    let alias = std::env::var_os("LOCALAPPDATA")
        .map(std::path::PathBuf::from)?
        .join("Microsoft")
        .join("WindowsApps")
        .join(name);
    alias.is_file().then_some(alias)
}

#[cfg(not(windows))]
fn icon_png_16_uncached(_path: &Path) -> Option<Vec<u8>> {
    None
}

/// HICON → 32bpp BGRA 像素 → PNG（含清理位图对象）。
#[cfg(windows)]
fn hicon_to_png(hicon: windows_sys::Win32::UI::WindowsAndMessaging::HICON) -> Option<Vec<u8>> {
    use windows_sys::Win32::Graphics::Gdi::{
        CreateCompatibleDC, DeleteDC, DeleteObject, GetDIBits, GetObjectW, SelectObject, BITMAP,
        BITMAPINFO, BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{GetIconInfo, ICONINFO};
    unsafe {
        let mut ii: ICONINFO = std::mem::zeroed();
        if GetIconInfo(hicon, &mut ii) == 0 {
            return None;
        }
        if ii.hbmColor.is_null() {
            if !ii.hbmMask.is_null() {
                DeleteObject(ii.hbmMask);
            }
            return None;
        }
        let mut bm: BITMAP = std::mem::zeroed();
        if GetObjectW(
            ii.hbmColor,
            std::mem::size_of::<BITMAP>() as i32,
            &mut bm as *mut BITMAP as *mut core::ffi::c_void,
        ) == 0
        {
            DeleteObject(ii.hbmColor);
            if !ii.hbmMask.is_null() {
                DeleteObject(ii.hbmMask);
            }
            return None;
        }
        let (w, h) = (bm.bmWidth as i32, bm.bmHeight as i32);
        if w <= 0 || h <= 0 || w > 128 || h > 128 {
            DeleteObject(ii.hbmColor);
            if !ii.hbmMask.is_null() {
                DeleteObject(ii.hbmMask);
            }
            return None;
        }
        let mut buf = vec![0u8; (w * h * 4) as usize];
        let mut bih: BITMAPINFOHEADER = std::mem::zeroed();
        bih.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
        bih.biWidth = w;
        bih.biHeight = -h; // 自顶向下，无需翻转
        bih.biPlanes = 1;
        bih.biBitCount = 32;
        bih.biCompression = BI_RGB;
        let mut bi: BITMAPINFO = std::mem::zeroed();
        bi.bmiHeader = bih;
        let dc = CreateCompatibleDC(std::ptr::null_mut());
        if dc.is_null() {
            DeleteObject(ii.hbmColor);
            if !ii.hbmMask.is_null() {
                DeleteObject(ii.hbmMask);
            }
            return None;
        }
        // 与 MSDN 约定相反但实测可行：MSDN 要求 GetDIBits 调用时位图不得选入
        // 任何 DC，而对 DDB 图标位图只有先选入兼容 DC 才能可靠取出像素
        // （DDB 转 DIB）。notepad_icon_encodes_to_png 全链路测试覆盖该用法，
        // 不要按文档写法“修正”此处。
        let old = SelectObject(dc, ii.hbmColor);
        let lines = GetDIBits(
            dc,
            ii.hbmColor,
            0,
            h as u32,
            buf.as_mut_ptr() as *mut core::ffi::c_void,
            &mut bi,
            DIB_RGB_COLORS,
        );
        SelectObject(dc, old);
        DeleteDC(dc);
        DeleteObject(ii.hbmColor);
        if !ii.hbmMask.is_null() {
            DeleteObject(ii.hbmMask);
        }
        if lines == 0 {
            return None;
        }
        // BGRA → RGBA
        for px in buf.chunks_exact_mut(4) {
            px.swap(0, 2);
        }
        encode_png(w as u32, h as u32, &buf)
    }
}

/// 最小 PNG 编码（8bit RGBA，无隔行，无 ancillary 块）。
#[cfg(windows)]
fn encode_png(width: u32, height: u32, rgba: &[u8]) -> Option<Vec<u8>> {
    use flate2::write::ZlibEncoder;
    use flate2::Compression;
    use std::io::Write;

    let stride = (width * 4) as usize;
    let mut raw = Vec::with_capacity(height as usize * (1 + stride));
    for y in 0..height as usize {
        raw.push(0); // filter: None
        raw.extend_from_slice(&rgba[y * stride..(y + 1) * stride]);
    }
    let mut enc = ZlibEncoder::new(Vec::new(), Compression::fast());
    enc.write_all(&raw).ok()?;
    let idat = enc.finish().ok()?;

    let mut out = Vec::with_capacity(idat.len() + 64);
    out.extend_from_slice(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]);
    let mut ihdr = Vec::with_capacity(13);
    ihdr.extend_from_slice(&width.to_be_bytes());
    ihdr.extend_from_slice(&height.to_be_bytes());
    ihdr.extend_from_slice(&[8, 6, 0, 0, 0]); // 8bit / RGBA / 默认压缩
    chunk(&mut out, b"IHDR", &ihdr);
    chunk(&mut out, b"IDAT", &idat);
    chunk(&mut out, b"IEND", &[]);
    Some(out)
}

/// 写一个 PNG chunk（长度 + 类型 + 数据 + CRC32）。
#[cfg(windows)]
fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    let start = out.len();
    out.extend_from_slice(kind);
    out.extend_from_slice(data);
    let crc = crc32(&out[start..]);
    out.extend_from_slice(&crc.to_be_bytes());
}

/// PNG 使用的 CRC32（ISO 3309 多项式）。
#[cfg(windows)]
fn crc32(data: &[u8]) -> u32 {
    let mut c = 0xFFFF_FFFFu32;
    for &b in data {
        c ^= b as u32;
        for _ in 0..8 {
            c = if c & 1 != 0 {
                0xEDB8_8320 ^ (c >> 1)
            } else {
                c >> 1
            };
        }
    }
    c ^ 0xFFFF_FFFF
}

#[cfg(test)]
mod tests {
    #[cfg(windows)]
    use super::icon_png_16;

    /// mspaint 回归：Win11 MSIX 迁移后 System32 原路径可能已不存在，
    /// 提取必须经 WindowsApps 应用执行别名兜底成功（原路径仍在的旧系统
    /// 则直接走原路径）。两者都不存在的机器无 Paint 可用，跳过。
    #[test]
    #[cfg(windows)]
    fn mspaint_icon_encodes_to_png() {
        let system = std::path::PathBuf::from(
            std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into()),
        )
        .join(r"System32\mspaint.exe");
        let alias_available = std::env::var("LOCALAPPDATA")
            .map(|local| {
                std::path::PathBuf::from(local)
                    .join(r"Microsoft\WindowsApps\mspaint.exe")
                    .is_file()
            })
            .unwrap_or(false);
        if !system.exists() && !alias_available {
            eprintln!("mspaint 不存在（System32 与别名均缺），跳过回归断言");
            return;
        }
        let png = icon_png_16(&system).expect("mspaint 图标提取失败");
        assert_eq!(
            &png[..8],
            &[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A],
            "PNG 签名错误"
        );
        assert!(png.len() > 100, "PNG 过小，疑似空图标");
    }

    /// 用系统 notepad.exe 验证 SHGetFileInfo → GetDIBits → PNG 编码全链路。
    #[test]
    #[cfg(windows)]
    fn notepad_icon_encodes_to_png() {
        let png = icon_png_16(std::path::Path::new(r"C:\Windows\System32\notepad.exe"))
            .expect("notepad 图标提取失败");
        assert_eq!(
            &png[..8],
            &[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A],
            "PNG 签名错误"
        );
        assert!(png.len() > 100, "PNG 过小，疑似空图标");
    }

    /// 并发冷启动回归：菜单会同时预热多个应用图标（code/notepad/paint），
    /// 并发的首次 SHGetFileInfoW 可能因 shell 图标缓存竞争短暂失败，
    /// 经重试必须全部成功。绕过进程内缓存直压提取函数，复现真实竞争。
    #[test]
    #[cfg(windows)]
    fn concurrent_first_extracts_succeed() {
        let mut paths = vec![std::path::PathBuf::from(r"C:\Windows\System32\notepad.exe")];
        let mspaint = std::path::PathBuf::from(
            std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into()),
        )
        .join(r"System32\mspaint.exe");
        // MSIX 迁移系统上别名存在才加入（旧系统直接有原路径）
        if let Some(alias) = super::windows_apps_alias(&mspaint) {
            paths.push(alias);
        } else if mspaint.exists() {
            paths.push(mspaint);
        }
        let mut handles = Vec::new();
        for _ in 0..2 {
            for path in paths.clone() {
                handles.push(std::thread::spawn(move || {
                    super::icon_png_16_uncached(&path).is_some()
                }));
            }
        }
        for handle in handles {
            assert!(handle.join().unwrap(), "并发首次图标提取失败");
        }
    }
}
