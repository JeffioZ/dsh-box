//! 托管 Node.js 更新事务。

use super::*;

// ---------- Node 更新 ----------

/// 更新 Node：下载新版归档（prepare，未停服）→ 停服务 → 换目录 → 重启（失败回滚）。
/// 系统 Node 拒绝自动更新；事务骨架复用 `with_directory_transaction`。
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
    let current = config.node_dir();
    let backup = config.root.join(update_txn::NODE_BACKUP_DIR);
    let marker = config.root.join(update_txn::NODE_UPDATE_MARKER);

    with_directory_transaction(
        app,
        &current,
        &backup,
        &marker,
        "Node.js",
        false,
        RollbackRecoveryNote::KeepMarker,
        // 下载与校验前移到 prepare：网络失败不产生停机窗口（此前整个
        // 下载都发生在停服之后，慢网/换源重试期间用户全程无服务）。
        // 下载进度同时转发到检查更新弹窗（此时主窗口仍是 dsh 页面，
        // 启动页通道的百分比用户看不到）。
        || {
            let sink = |done: u64, total: u64| {
                let pct = ((done as f64 / total as f64 * 100.0) as i64).min(100);
                emit_progress(
                    app,
                    &crate::locale::owned(
                        format!(
                            "正在下载 Node.js… {pct}%（{:.1}/{:.1} MB）",
                            done as f64 / 1048576.0,
                            total as f64 / 1048576.0
                        ),
                        format!(
                            "Downloading Node.js… {pct}% ({:.1}/{:.1} MB)",
                            done as f64 / 1048576.0,
                            total as f64 / 1048576.0
                        ),
                    ),
                );
            };
            runtime::prepare_node_archive_with(app, config, Some(&sink))
        },
        |prepared| {
            // 停服与备份完成后：解压、拍平、验证（失败走既有回滚路径）。
            runtime::install_node_from_archive(app, config, &prepared)?;
            Ok(())
        },
    )
}
