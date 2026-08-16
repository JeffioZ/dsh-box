//! 提取文件/应用的 16×16 小图标并编码为 PNG（右键菜单图标显示用）。
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
    use std::mem::zeroed;
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::UI::Shell::{SHGetFileInfoW, SHFILEINFOW, SHGFI_ICON, SHGFI_SMALLICON};

    let path_w: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
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
    if ok == 0 || info.hIcon.is_null() {
        return None;
    }
    let png = hicon_to_png(info.hIcon);
    unsafe { windows_sys::Win32::UI::WindowsAndMessaging::DestroyIcon(info.hIcon) };
    png
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
        // 位图需选入 DC 后 GetDIBits 才能可靠取像素（DDB 转 DIB）
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
}
