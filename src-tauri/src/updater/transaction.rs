//! 更新事务原语：目录备份、标记、中断恢复与回滚。

use crate::app_state::Config;

pub(crate) const DSH_BACKUP_DIR: &str = "dsh-old";
pub(crate) const DSH_UPDATE_MARKER: &str = "dsh-update-in-progress";
pub(crate) const NODE_BACKUP_DIR: &str = "node-old";
pub(crate) const NODE_UPDATE_MARKER: &str = "node-update-in-progress";

/// 启动时恢复上次被强杀/断电打断的目录切换。
pub(crate) fn recover_interrupted_updates(config: &Config) -> Result<(), String> {
    recover_directory(
        &config.dsh_dir(),
        &config.root.join(DSH_BACKUP_DIR),
        &config.root.join(DSH_UPDATE_MARKER),
        "dsh",
    )?;
    recover_directory(
        &config.node_dir(),
        &config.root.join(NODE_BACKUP_DIR),
        &config.root.join(NODE_UPDATE_MARKER),
        "Node",
    )
}

/// 删除失败的新目录并原子恢复同卷备份；恢复失败时保留备份供人工处理。
pub(crate) fn restore_directory(
    current: &std::path::Path,
    backup: &std::path::Path,
) -> Result<(), String> {
    if current.exists() {
        std::fs::remove_dir_all(current).map_err(|e| {
            crate::locale::owned(
                format!("清理新版本安装目录失败：{e}"),
                format!("Failed to remove the new installation directory: {e}"),
            )
        })?;
    }
    std::fs::rename(backup, current).map_err(|e| {
        crate::locale::owned(
            format!("恢复旧版本失败：{e}"),
            format!("Failed to restore the previous version: {e}"),
        )
    })
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum RollbackOutcome {
    Complete,
    /// 旧目录已经恢复，只有事务标记尚未清理。此时 backup 已被移动，
    /// 调用方可以重启旧版本，但必须准确提示下次启动继续清理 marker。
    MarkerCleanupPending(String),
}

/// 回滚目录并在成功后结束事务。目录恢复失败时保留 marker 与备份；
/// 仅 marker 删除失败时返回部分成功，不能误报备份仍在。
pub(crate) fn rollback_directory(
    current: &std::path::Path,
    backup: &std::path::Path,
    marker: &std::path::Path,
) -> Result<RollbackOutcome, String> {
    restore_directory(current, backup)?;
    match remove_marker(marker) {
        Ok(()) => Ok(RollbackOutcome::Complete),
        Err(error) => Ok(RollbackOutcome::MarkerCleanupPending(error)),
    }
}

/// 创建更新事务标记（create_new 保证互斥；写入并 fsync 后才生效）。
pub(crate) fn create_marker(path: &std::path::Path) -> Result<(), String> {
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|e| {
            crate::locale::owned(
                format!("创建更新事务标记失败：{e}"),
                format!("Failed to create the update transaction marker: {e}"),
            )
        })?;
    use std::io::Write;
    if let Err(e) = file
        .write_all(b"in-progress\n")
        .and_then(|_| file.sync_all())
    {
        drop(file);
        let _ = std::fs::remove_file(path);
        return Err(crate::locale::owned(
            format!("写入更新事务标记失败：{e}"),
            format!("Failed to write the update transaction marker: {e}"),
        ));
    }
    Ok(())
}

/// 移除更新事务标记（不存在视为成功）。调用方必须确认移除成功后，
/// 才能清理备份；否则残留标记会让下次启动把备份当作未提交事务恢复。
pub(crate) fn remove_marker(path: &std::path::Path) -> Result<(), String> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(crate::locale::owned(
            format!("清理更新事务标记失败：{e}"),
            format!("Failed to remove the update transaction marker: {e}"),
        )),
    }
}

/// 恢复一个目录的事务状态：
/// - marker 存在（更新未提交）：优先恢复备份；备份与当前都不存在则报错；
/// - marker 不存在但备份存在：更新已提交，清理备份；当前缺失则恢复备份。
fn recover_directory(
    current: &std::path::Path,
    backup: &std::path::Path,
    marker: &std::path::Path,
    name: &str,
) -> Result<(), String> {
    if marker.exists() {
        if backup.exists() {
            if current.exists() {
                std::fs::remove_dir_all(current).map_err(|e| {
                    crate::locale::owned(
                        format!("清理未完成的 {name} 更新失败：{e}"),
                        format!("Failed to remove the incomplete {name} update: {e}"),
                    )
                })?;
            }
            std::fs::rename(backup, current).map_err(|e| {
                crate::locale::owned(
                    format!("恢复更新前的 {name} 失败：{e}"),
                    format!("Failed to restore {name} to its pre-update state: {e}"),
                )
            })?;
            crate::logging::log(&format!("update: 检测到中断的 {name} 更新，已恢复旧版本"));
        } else if !current.exists() {
            return Err(crate::locale::owned(
                format!("{name} 更新中断，且当前目录和备份均不存在"),
                format!(
                    "The {name} update was interrupted, and neither the current directory nor its backup exists"
                ),
            ));
        }
        remove_marker(marker)?;
    } else if backup.exists() {
        if current.exists() {
            if let Err(e) = std::fs::remove_dir_all(backup) {
                crate::logging::log(&format!("update: 清理已提交的 {name} 备份失败：{e}"));
            }
        } else {
            std::fs::rename(backup, current).map_err(|e| {
                crate::locale::owned(
                    format!("恢复遗留的 {name} 备份失败：{e}"),
                    format!("Failed to restore the remaining {name} backup: {e}"),
                )
            })?;
            crate::logging::log(&format!("update: 当前 {name} 目录缺失，已恢复遗留备份"));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn temp_dir(name: &str) -> PathBuf {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("dsh-box-{name}-{unique}"));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn interrupted_update_restores_backup() {
        let root = temp_dir("restore");
        let current = root.join("current");
        let backup = root.join("backup");
        let marker = root.join("marker");
        std::fs::create_dir_all(&current).unwrap();
        std::fs::create_dir_all(&backup).unwrap();
        std::fs::write(current.join("version"), "partial").unwrap();
        std::fs::write(backup.join("version"), "old").unwrap();
        std::fs::write(&marker, "in-progress").unwrap();

        recover_directory(&current, &backup, &marker, "test").unwrap();

        assert_eq!(
            std::fs::read_to_string(current.join("version")).unwrap(),
            "old"
        );
        assert!(!backup.exists());
        assert!(!marker.exists());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn committed_update_keeps_current_and_cleans_backup() {
        let root = temp_dir("commit");
        let current = root.join("current");
        let backup = root.join("backup");
        let marker = root.join("marker");
        std::fs::create_dir_all(&current).unwrap();
        std::fs::create_dir_all(&backup).unwrap();
        std::fs::write(current.join("version"), "new").unwrap();
        std::fs::write(backup.join("version"), "old").unwrap();

        recover_directory(&current, &backup, &marker, "test").unwrap();

        assert_eq!(
            std::fs::read_to_string(current.join("version")).unwrap(),
            "new"
        );
        assert!(!backup.exists());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn failed_rollback_keeps_marker_and_backup() {
        let root = temp_dir("rollback-failure");
        let current = root.join("current");
        let backup = root.join("backup");
        let marker = root.join("marker");
        std::fs::write(&current, "not-a-directory").unwrap();
        std::fs::create_dir_all(&backup).unwrap();
        std::fs::write(backup.join("version"), "old").unwrap();
        std::fs::write(&marker, "in-progress").unwrap();

        assert!(rollback_directory(&current, &backup, &marker).is_err());
        assert!(marker.exists());
        assert!(backup.exists());

        std::fs::remove_file(current).unwrap();
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn removing_a_missing_marker_is_idempotent() {
        let root = temp_dir("missing-marker");
        remove_marker(&root.join("missing")).unwrap();
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn restored_directory_is_reported_when_only_marker_cleanup_fails() {
        let root = temp_dir("rollback-marker-pending");
        let current = root.join("current");
        let backup = root.join("backup");
        let marker = root.join("marker");
        std::fs::create_dir_all(&current).unwrap();
        std::fs::write(current.join("version"), "partial").unwrap();
        std::fs::create_dir_all(&backup).unwrap();
        std::fs::write(backup.join("version"), "old").unwrap();
        // remove_file 对目录必然失败，可稳定模拟 marker 单独清理失败。
        std::fs::create_dir_all(&marker).unwrap();

        let outcome = rollback_directory(&current, &backup, &marker).unwrap();

        assert!(matches!(outcome, RollbackOutcome::MarkerCleanupPending(_)));
        assert_eq!(
            std::fs::read_to_string(current.join("version")).unwrap(),
            "old"
        );
        assert!(!backup.exists());
        assert!(marker.exists());
        std::fs::remove_dir_all(root).unwrap();
    }
}
