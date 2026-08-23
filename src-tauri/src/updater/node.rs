//! 托管 Node.js 更新事务。

use super::*;

// ---------- Node 更新 ----------

/// 更新 Node：停服务 → 下载新版便携 Node → 换目录 → 重启（失败回滚）。
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
        || Ok(()),
        |()| {
            // 下载并安装新版便携 Node。
            let _ = runtime::install_portable_node(app, config)?;
            Ok(())
        },
    )
}
