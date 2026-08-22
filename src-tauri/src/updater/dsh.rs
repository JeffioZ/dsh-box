//! dsh 包更新事务。

use super::*;

// ---------- dsh 更新 ----------

/// 更新 dsh：停服务 → 备份 → npm 更新 → 重启（失败回滚）。
pub(super) fn update_dsh(app: &AppHandle, config: &crate::app_state::Config) -> Result<(), String> {
    // 所有前置条件先检查完，再停止当前可用服务。
    let node_exe = if config.node_exe().exists() {
        config.node_exe()
    } else {
        runtime::find_system_node()
            .ok_or_else(|| crate::locale::text("未找到 Node.js", "Node.js was not found"))?
    };
    let npm_cli = node_exe
        .parent()
        .ok_or_else(|| {
            crate::locale::text(
                "Node.js 可执行文件路径无父目录",
                "The Node.js executable path has no parent directory",
            )
        })?
        .join("node_modules/npm/bin/npm-cli.js");
    if !npm_cli.exists() {
        return Err(crate::locale::text("未找到 npm", "npm was not found").into());
    }
    // 在停止当前可用服务之前锁定本次检查通道的精确目标。不能把 npm tag
    // 留到安装时解析，否则 tag 在检查与安装之间移动会导致结果与实际版本不符。
    let target_version = runtime::npm_latest_dsh_version(runtime::DshChannel::from_config(config))?;

    let current = config.dsh_dir();
    let backup = config.root.join(update_txn::DSH_BACKUP_DIR);
    let marker = config.root.join(update_txn::DSH_UPDATE_MARKER);
    if backup.exists() || marker.exists() {
        return Err(crate::locale::owned(
            format!(
                "检测到未完成的 dsh 更新，请重启应用后重试：{}",
                backup.display()
            ),
            format!(
                "An unfinished dsh update was found. Restart the app before trying again: {}",
                backup.display()
            ),
        ));
    }
    if !current.exists() {
        return Err(crate::locale::text(
            "未找到当前 dsh 安装目录",
            "The current dsh installation directory was not found",
        )
        .into());
    }

    emit_progress(
        app,
        crate::locale::text("正在停止 dsh 服务…", "Stopping the dsh service…"),
    );
    update_txn::create_marker(&marker)?;
    dsh::shutdown(app);
    navigate(app, SPLASH_ORIGIN);
    std::thread::sleep(Duration::from_millis(800));
    if let Err(e) = std::fs::rename(&current, &backup) {
        update_txn::remove_marker(&marker);
        let _ = restart_service_locked(app);
        return Err(crate::locale::owned(
            format!("备份当前 dsh 失败：{e}"),
            format!("Failed to back up the current dsh installation: {e}"),
        ));
    }

    emit_progress(
        app,
        crate::locale::text("正在更新 dsh 包…", "Updating the dsh package…"),
    );
    let args = vec![
        npm_cli.to_string_lossy().into_owned(),
        "install".into(),
        "--prefix".into(),
        config.dsh_dir().to_string_lossy().into_owned(),
        format!("@deepseek-ai/dsh@{target_version}"),
        "--dangerously-allow-all-scripts".into(),
        "--no-audit".into(),
        "--no-fund".into(),
    ];
    let envs = base_envs(&node_exe, config);
    let install_result = (|| -> Result<(), String> {
        let (code, _out, err) = processes::run_capture(&node_exe, &args, &envs, Some(&config.root))
            .map_err(|e| {
                crate::locale::owned(
                    format!("运行 npm 失败：{e}"),
                    format!("Failed to run npm: {e}"),
                )
            })?;
        if code != 0 {
            let detail = truncate(&err, 600);
            return Err(crate::locale::owned(
                format!("更新 dsh 失败（npm 退出码 {code}）：\n{detail}"),
                format!("Failed to update dsh (npm exit code {code}):\n{detail}"),
            ));
        }
        if !config.dsh_entry().exists() {
            return Err(crate::locale::text(
                "更新完成但未找到 dsh 入口文件",
                "The update completed, but the dsh entry file was not found",
            )
            .into());
        }
        let installed = runtime::installed_dsh_version(config);
        if installed.as_deref() != Some(target_version.as_str()) {
            return Err(crate::locale::owned(
                format!(
                    "更新后的 dsh 版本校验失败：实际为 {}，预期为 {target_version}",
                    installed.as_deref().unwrap_or("未知")
                ),
                format!(
                    "The updated dsh version could not be verified: installed {}, expected {target_version}",
                    installed.as_deref().unwrap_or("unknown")
                ),
            ));
        }
        Ok(())
    })();
    if let Err(e) = install_result {
        if let Err(re) = update_txn::rollback_directory(&current, &backup, &marker) {
            return Err(crate::locale::owned(
                format!("{e}；旧版本自动恢复失败：{re}。\n下次启动将自动还原旧版本。"),
                format!(
                    "{e}; automatic rollback failed: {re}.\nThe previous version will be restored on the next launch."
                ),
            ));
        }
        return match restart_service_locked(app) {
            Ok(()) => Err(crate::locale::owned(
                format!("{e}；已恢复旧版本"),
                format!("{e}; the previous version was restored"),
            )),
            Err(re) => Err(crate::locale::owned(
                format!("{e}；旧版本恢复后未能启动：{re}"),
                format!("{e}; the restored version did not start: {re}"),
            )),
        };
    }

    emit_progress(
        app,
        crate::locale::text(
            "更新完成，正在重启服务…",
            "Update complete. Restarting the service…",
        ),
    );
    if let Err(e) = restart_service_locked(app) {
        dsh::shutdown(app);
        if let Err(re) = update_txn::rollback_directory(&current, &backup, &marker) {
            return Err(crate::locale::owned(
                format!(
                    "新版本启动失败：{e}；旧版本自动恢复也失败：{re}。\n\
                     服务已停止，下次启动将自动还原旧版本。"
                ),
                format!(
                    "The new version did not start: {e}; automatic rollback also failed: {re}.\n\
                     The service is stopped; the previous version will be restored on the next launch."
                ),
            ));
        }
        let restore_result = restart_service_locked(app);
        return match restore_result {
            Ok(()) => Err(crate::locale::owned(
                format!("新版本启动失败，已恢复旧版本：{e}"),
                format!("The new version did not start; the previous version was restored: {e}"),
            )),
            Err(re) => Err(crate::locale::owned(
                format!("新版本启动失败：{e}；旧版本恢复后也未能启动：{re}"),
                format!(
                    "The new version did not start: {e}; the restored version also failed to start: {re}"
                ),
            )),
        };
    }
    std::fs::remove_file(&marker).map_err(|e| {
        crate::locale::owned(
            format!("提交 dsh 更新状态失败：{e}"),
            format!("Failed to commit the dsh update state: {e}"),
        )
    })?;
    if let Err(e) = std::fs::remove_dir_all(&backup) {
        crate::logging::log(&format!(
            "updater: 清理 dsh 备份失败（不影响当前版本）：{e}"
        ));
    }
    Ok(())
}
