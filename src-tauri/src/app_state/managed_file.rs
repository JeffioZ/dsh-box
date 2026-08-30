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
///
/// 读阶段与 dsh 进程（settings/credentials 的文件监视器与热发布）存在跨进程
/// 竞争：撞上 dsh 正在打开/读目标文件时，Windows 的读或替换会返回瞬时占用/
/// 共享冲突。这里对读阶段做有限重试（读到冲突就 sleep 后重读、重跑 transform，
/// 避免基于过时内容）；transform 返回的逻辑错误（如 YAML 语法）不重试，直接
/// 原样返回；替换阶段的瞬时冲突由 `atomic_write_unlocked` 内部按同一白名单重试。
pub(crate) fn update_text_file(
    path: &Path,
    mut transform: impl FnMut(String) -> Result<String, String>,
) -> Result<(), String> {
    let _guard = MANAGED_FILE_WRITE_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    for attempt in 0..=MAX_WRITE_RETRIES {
        let text = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(e) if retryable_io(&e) && attempt < MAX_WRITE_RETRIES => {
                std::thread::sleep(WRITE_RETRY_DELAY);
                continue;
            }
            Err(e) => return Err(e.to_string()),
        };
        let next = transform(text.clone())?;
        // 跨进程丢更新窗口：读—变换期间 dsh（settings 文件监视器/热发布）
        // 可能已写入新内容，直接替换会整文件覆盖掉那次写入。替换前重读
        // 比对，变化则丢弃本次结果、重读重跑 transform。窗口由此收窄到
        // 重读与原子替换之间的毫秒级；彻底消除需要 dsh 配合的锁协议。
        let reread = match std::fs::read_to_string(path) {
            Ok(current) => current,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(e) if retryable_io(&e) && attempt < MAX_WRITE_RETRIES => {
                std::thread::sleep(WRITE_RETRY_DELAY);
                continue;
            }
            Err(e) => return Err(e.to_string()),
        };
        if reread != text {
            if attempt >= MAX_WRITE_RETRIES {
                return Err(crate::locale::owned(
                    "目标文件在写入期间被其他进程反复修改，已放弃本次变更。".to_string(),
                    "The target file kept changing during the write; the change was abandoned."
                        .to_string(),
                ));
            }
            crate::logging::log("managed-file: 写入期间目标文件被外部修改，重读后重试");
            continue;
        }
        // 替换阶段（含临时文件创建/写/ReplaceFileW）的瞬时冲突由
        // atomic_write_unlocked 内部按白名单重试；这里不再盲目整轮重试，
        // 避免把非瞬时替换错误也重复 transform 多次。
        return atomic_write_unlocked(path, &next);
    }
    unreachable!("读阶段重试循环所有路径都在尝试内返回")
}

/// 瞬时冲突类错误码白名单：这些是资源被临时占用/共享冲突，重试可成功；
/// 其余（如权限永久不足、磁盘满）不重试，直接报错。
///
/// 关键：Windows 的共享/锁冲突（ERROR_SHARING_VIOLATION=32、
/// ERROR_LOCK_VIOLATION=33）在 Rust std 里映射为 `Uncategorized`/`Other`，
/// 并不落在 `PermissionDenied`；必须用 `raw_os_error()` 显式匹配，否则真实
/// 进程占用冲突不会触发重试，导入偶发失败依旧。这两个错误码专用于 Windows，
/// 用 `#[cfg(windows)]` 限定，避免 Unix 上同名 errno（EPIPE/EDOM）被误判。
fn retryable_io(e: &std::io::Error) -> bool {
    #[cfg(windows)]
    if matches!(e.raw_os_error(), Some(32) | Some(33)) {
        return true;
    }
    matches!(
        e.kind(),
        std::io::ErrorKind::PermissionDenied | std::io::ErrorKind::WouldBlock
    )
}

/// 最多重试次数（初始尝试之外）。循环 `0..=MAX_WRITE_RETRIES` 因此是
/// 1 次初始尝试 + 3 次重试 = 4 次尝试。
const MAX_WRITE_RETRIES: u32 = 3;
const WRITE_RETRY_DELAY: std::time::Duration = std::time::Duration::from_millis(50);

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
    // 替换阶段（ReplaceFileW / rename）可能因 dsh 正打开目标文件而瞬时
    // 共享冲突：对白名单错误码短重试，避免一次性写入失败。
    for attempt in 0..=MAX_WRITE_RETRIES {
        match replace_file(&temp, path) {
            Ok(()) => {
                fsync_parent_dir(path);
                return Ok(());
            }
            Err(e) if retryable_io(&e) && attempt < MAX_WRITE_RETRIES => {
                std::thread::sleep(WRITE_RETRY_DELAY);
            }
            Err(e) => {
                let _ = std::fs::remove_file(&temp);
                return Err(e.to_string());
            }
        }
    }
    unreachable!("替换重试循环所有路径都在尝试内返回")
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

#[cfg(test)]
mod tests {
    use super::{retryable_io, update_text_file};

    #[test]
    fn retryable_io_classifies_conflict_kinds() {
        // 瞬时占用/共享冲突：应重试。
        assert!(retryable_io(&std::io::Error::from(
            std::io::ErrorKind::PermissionDenied
        )));
        assert!(retryable_io(&std::io::Error::from(
            std::io::ErrorKind::WouldBlock
        )));
        // Windows 真实共享/锁冲突（raw 32/33）映射为 Uncategorized/Other，
        // 但仍应重试——这是核心修复点，仅 Windows 侧测试（retryable_io 的
        // raw 32/33 分支用 #[cfg(windows)] 限定，Linux 上这两个 errno 是
        // EPIPE/EDOM，语义不同）。
        #[cfg(windows)]
        {
            assert!(retryable_io(&std::io::Error::from_raw_os_error(32)));
            assert!(retryable_io(&std::io::Error::from_raw_os_error(33)));
        }
        // 确定性/非瞬时错误：不应重试。
        assert!(!retryable_io(&std::io::Error::from(
            std::io::ErrorKind::NotFound
        )));
        assert!(!retryable_io(&std::io::Error::from(
            std::io::ErrorKind::InvalidData
        )));
        // 平台无关的 raw 码：ENOENT(2) 映射 NotFound，任何平台都不重试。
        // 注意不能在 Linux 上断言 from_raw_os_error(1)：EPERM 映射
        // PermissionDenied 会被判定为可重试（重试合理），此处不断言。
        assert!(!retryable_io(&std::io::Error::from_raw_os_error(2)));
    }

    #[test]
    fn update_text_file_retries_when_external_write_lands_during_transform() {
        // 模拟 dsh 在本壳"读—变换"期间写入：transform 闭包首轮改写目标
        // 文件，替换前重读发现内容变化 → 丢弃旧结果重跑；最终两边的
        // 变更都在（外部写入不被整文件覆盖）。
        let root = std::env::temp_dir().join(format!(
            "dshbox-mf-race-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("a.txt");
        std::fs::write(
            &path, "base: 1
",
        )
        .unwrap();
        let mut externally_touched = false;
        update_text_file(&path, |text| {
            if !externally_touched {
                externally_touched = true;
                std::fs::write(
                    &path,
                    "external: true
",
                )
                .unwrap();
            }
            Ok(text
                + "mine: true
")
        })
        .unwrap();
        let final_text = std::fs::read_to_string(&path).unwrap();
        assert!(
            final_text.contains("external: true"),
            "unexpected: {final_text}"
        );
        assert!(
            final_text.contains("mine: true"),
            "unexpected: {final_text}"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn update_text_file_writes_when_file_missing() {
        // 文件不存在：读阶段返回空串，transform 后写入，成功且内容正确。
        let root = std::env::temp_dir().join(format!(
            "dshbox-mf-missing-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("a.txt");
        update_text_file(&path, |text| {
            assert_eq!(text, "");
            Ok("hello".to_string())
        })
        .unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "hello");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn update_text_file_propagates_transform_error_without_write() {
        // transform 返回逻辑错误：不重试、不写盘，原样返回该错误。
        let root = std::env::temp_dir().join(format!(
            "dshbox-mf-transform-err-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("a.txt");
        std::fs::write(&path, "orig").unwrap();
        let err = update_text_file(&path, |_text| Err("逻辑错误".to_string())).unwrap_err();
        assert_eq!(err, "逻辑错误");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "orig");
        let _ = std::fs::remove_dir_all(&root);
    }
}
