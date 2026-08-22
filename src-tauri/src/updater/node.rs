//! 托管 Node.js 更新事务。

use super::*;

// ---------- Node 更新 ----------

/// 更新 Node：停服务 → 下载新版便携 Node → 换目录 → 重启（失败回滚）。
pub(super) fn update_node(
    app: &AppHandle,
    config: &crate::app_state::Config,
) -> Result<(), String> {
    if !config.node_exe().exists() {
        return Err(crate::locale::text(
            "当前使用的是系统 Node.js，应用不会自动更新它。",
            "The app is using the system Node.js installation and will not update it automatically.",
        )
        .into());
    }
    let old = config.node_dir();
    let backup = config.root.join(update_txn::NODE_BACKUP_DIR);
    let marker = config.root.join(update_txn::NODE_UPDATE_MARKER);
    if backup.exists() || marker.exists() {
        return Err(crate::locale::owned(
            format!(
                "检测到未完成的 Node 更新，请重启应用后重试：{}",
                backup.display()
            ),
            format!(
                "An unfinished Node.js update was found. Restart the app before trying again: {}",
                backup.display()
            ),
        ));
    }

    emit_progress(
        app,
        crate::locale::text("正在停止 dsh 服务…", "Stopping the dsh service…"),
    );
    update_txn::create_marker(&marker)?;
    dsh::shutdown(app);
    navigate_to_splash(app);
    std::thread::sleep(Duration::from_millis(800));
    if old.exists() {
        if let Err(e) = std::fs::rename(&old, &backup) {
            update_txn::remove_marker(&marker);
            let _ = restart_service_locked(app);
            return Err(crate::locale::owned(
                format!("备份当前 Node 失败：{e}"),
                format!("Failed to back up the current Node.js installation: {e}"),
            ));
        }
    }
    let result = (|| -> Result<(), String> {
        let exe = runtime::install_portable_node(app, config)?;
        let _ = exe;
        Ok(())
    })();
    if let Err(e) = result {
        if let Err(re) = update_txn::rollback_directory(&old, &backup, &marker) {
            return Err(crate::locale::owned(
                format!(
                    "{e}；旧 Node 自动恢复失败：{re}。\n\
                     已保留备份和事务标记，下次启动将再次恢复。"
                ),
                format!(
                    "{e}; automatic Node.js rollback failed: {re}.\n\
                     The backup and transaction marker were kept for recovery on the next launch."
                ),
            ));
        }
        return match restart_service_locked(app) {
            Ok(()) => Err(crate::locale::owned(
                format!("{e}；已恢复旧 Node"),
                format!("{e}; the previous Node.js version was restored"),
            )),
            Err(re) => Err(crate::locale::owned(
                format!("{e}；旧 Node 恢复后未能启动：{re}"),
                format!("{e}; the restored Node.js version did not start: {re}"),
            )),
        };
    }

    emit_progress(
        app,
        crate::locale::text(
            "Node 更新完成，正在重启服务…",
            "Node.js update complete. Restarting the service…",
        ),
    );
    if let Err(e) = restart_service_locked(app) {
        dsh::shutdown(app);
        if let Err(re) = update_txn::rollback_directory(&old, &backup, &marker) {
            return Err(crate::locale::owned(
                format!(
                    "新 Node 启动失败：{e}；旧版本自动恢复失败：{re}。\n\
                     服务已停止，下次启动将自动还原旧版本。"
                ),
                format!(
                    "The new Node.js version did not start: {e}; automatic rollback failed: {re}.\n\
                     The service is stopped; the previous version will be restored on the next launch."
                ),
            ));
        }
        let restore_result = restart_service_locked(app);
        return match restore_result {
            Ok(()) => Err(crate::locale::owned(
                format!("新 Node 启动失败，已恢复旧版本：{e}"),
                format!(
                    "The new Node.js version did not start; the previous version was restored: {e}"
                ),
            )),
            Err(re) => Err(crate::locale::owned(
                format!("新 Node 启动失败：{e}；旧版本恢复后也未能启动：{re}"),
                format!(
                    "The new Node.js version did not start: {e}; the restored version also failed to start: {re}"
                ),
            )),
        };
    }
    std::fs::remove_file(&marker).map_err(|e| {
        crate::locale::owned(
            format!("提交 Node 更新状态失败：{e}"),
            format!("Failed to commit the Node.js update state: {e}"),
        )
    })?;
    if backup.exists() {
        if let Err(e) = std::fs::remove_dir_all(&backup) {
            crate::logging::log(&format!(
                "updater: 清理 Node 备份失败（不影响当前版本）：{e}"
            ));
        }
    }
    Ok(())
}
