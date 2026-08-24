//! 受管文本文件的进程内串行读改写与原子替换。

use std::io::Write;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

static MANAGED_FILE_WRITE_LOCK: Mutex<()> = Mutex::new(());
static TEMP_FILE_SEQUENCE: AtomicU64 = AtomicU64::new(0);

pub(crate) fn atomic_write(path: &Path, text: &str) -> Result<(), String> {
    let _guard = MANAGED_FILE_WRITE_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    atomic_write_unlocked(path, text)
}

/// 串行执行“读取—变换—原子替换”，防止同一进程内多个设置入口互相覆盖。
/// 变换返回错误时保留原文件不动。
pub(crate) fn update_text_file(
    path: &Path,
    transform: impl FnOnce(String) -> Result<String, String>,
) -> Result<(), String> {
    let _guard = MANAGED_FILE_WRITE_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(e.to_string()),
    };
    let next = transform(text)?;
    atomic_write_unlocked(path, &next)
}

fn atomic_write_unlocked(path: &Path, text: &str) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    // 唯一 .tmp 文件避免不同目标或异常重入共用一个固定临时文件；不用
    // .yaml 后缀，dsh 的目录监视器不会把半成品当成设置文件。
    let file_name = path.file_name().and_then(|n| n.to_str()).unwrap_or("data");
    let (temp, mut file) = loop {
        let sequence = TEMP_FILE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let candidate = path.with_file_name(format!(
            ".{file_name}.dshbox-{}-{sequence}.tmp",
            std::process::id()
        ));
        let mut options = std::fs::OpenOptions::new();
        options.create_new(true).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        match options.open(&candidate) {
            Ok(file) => break (candidate, file),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e.to_string()),
        }
    };
    if let Err(e) = file
        .write_all(text.as_bytes())
        .and_then(|_| file.sync_all())
    {
        drop(file);
        let _ = std::fs::remove_file(&temp);
        return Err(e.to_string());
    }
    drop(file);
    if let Err(e) = replace_file(&temp, path) {
        let _ = std::fs::remove_file(&temp);
        return Err(e.to_string());
    }
    fsync_parent_dir(path);
    Ok(())
}

/// 清理目录中崩溃残留的原子写临时文件（`.<name>.dshbox-*.tmp`）。
/// 正常路径写完即删，只有进程中断会残留；启动时扫一次即可。
pub(crate) fn cleanup_stale_temp_files(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let file_name = entry.file_name();
        let Some(name) = file_name.to_str() else {
            continue;
        };
        if name.starts_with('.') && name.contains(".dshbox-") && name.ends_with(".tmp") {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

/// 目录项变更（rename/replace）后 fsync 父目录，保证崩溃后目录项本身
/// 落盘；best-effort，失败不影响已完成的写入。Windows 无对应语义，跳过。
#[cfg(unix)]
fn fsync_parent_dir(path: &Path) {
    if let Some(parent) = path.parent() {
        if let Ok(dir) = std::fs::File::open(parent) {
            let _ = dir.sync_all();
        }
    }
}

#[cfg(not(unix))]
fn fsync_parent_dir(_path: &Path) {}

/// 行级合并一个顶层段落里的字段；只改目标字段，其他 YAML 内容原样保留。
pub(super) fn merge_section_field(text: &str, section: &str, field: &str, value: &str) -> String {
    let section_header = format!("{section}:");
    let new_line = format!("  {field}: {value}");
    let mut out = String::new();
    let mut in_section = false;
    let mut saw_section = false;
    let mut wrote = false;
    for line in text.lines() {
        // 容忍 UTF-8 BOM：文件首行的段头可能带 BOM（trim 不去 \u{feff}），
        // 失配会追加重复段
        let head = line.strip_prefix('\u{feff}').unwrap_or(line);
        if !head.starts_with(' ') && head.trim_end() == section_header {
            in_section = true;
            saw_section = true;
            out.push_str(line);
            out.push('\n');
            continue;
        }
        if in_section {
            if let Some(rest) = line.trim_start().strip_prefix(field) {
                if rest.trim_start().starts_with(':') {
                    if !wrote {
                        out.push_str(&new_line);
                        out.push('\n');
                        wrote = true;
                    }
                    continue;
                }
            }
            if !line.starts_with(' ') && !line.is_empty() {
                if !wrote {
                    out.push_str(&new_line);
                    out.push('\n');
                    wrote = true;
                }
                in_section = false;
            }
        }
        out.push_str(line);
        out.push('\n');
    }
    if !wrote {
        if saw_section {
            out.push_str(&new_line);
            out.push('\n');
        } else {
            if !out.is_empty() && !out.ends_with('\n') {
                out.push('\n');
            }
            out.push_str(&section_header);
            out.push('\n');
            out.push_str(&new_line);
            out.push('\n');
        }
    }
    out
}

#[cfg(windows)]
fn replace_file(temp: &Path, target: &Path) -> std::io::Result<()> {
    if !target.exists() {
        return std::fs::rename(temp, target);
    }
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::ReplaceFileW;
    let target: Vec<u16> = target.as_os_str().encode_wide().chain(Some(0)).collect();
    let temp: Vec<u16> = temp.as_os_str().encode_wide().chain(Some(0)).collect();
    let ok = unsafe {
        ReplaceFileW(
            target.as_ptr(),
            temp.as_ptr(),
            std::ptr::null(),
            0,
            std::ptr::null(),
            std::ptr::null(),
        )
    };
    if ok == 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(not(windows))]
fn replace_file(temp: &Path, target: &Path) -> std::io::Result<()> {
    std::fs::rename(temp, target)
}
