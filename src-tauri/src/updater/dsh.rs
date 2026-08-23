//! dsh 包更新事务。

use super::*;

// ---------- dsh 更新 ----------

/// 更新 dsh：准备安装器 → 停服务 → 备份 → pnpm 精确安装 → 重启（失败回滚）。
/// 事务骨架复用 `with_directory_transaction`；差异仅在“停服前准备”钩子
/// （锁定精确 target + 准备 pnpm）与 install 闭包。
pub(super) fn update_dsh(app: &AppHandle, config: &crate::app_state::Config) -> Result<(), String> {
    let current = config.dsh_dir();
    let backup = config.root.join(update_txn::DSH_BACKUP_DIR);
    let marker = config.root.join(update_txn::DSH_UPDATE_MARKER);

    // 停服前准备：锁定精确版本 + 准备 pnpm，任何网络/安装器失败都不产生停机窗口。
    let prepare = || -> Result<(std::path::PathBuf, std::path::PathBuf, String), String> {
        let node_exe = if config.node_exe().exists() {
            config.node_exe()
        } else {
            runtime::find_system_node()
                .ok_or_else(|| crate::locale::text("未找到 Node.js", "Node.js was not found"))?
        };
        let target_version =
            runtime::npm_latest_dsh_version(runtime::DshChannel::from_config(config))?;
        let mut reporter = |message: &str, detail: &str| {
            let text = if detail.is_empty() {
                message.to_string()
            } else {
                format!("{message} {detail}")
            };
            emit_progress(app, &text);
        };
        let pnpm_cli = runtime::prepare_dsh_installer(app, config, &node_exe, &mut reporter)?;
        Ok((node_exe, pnpm_cli, target_version))
    };

    let install = |prepared: (std::path::PathBuf, std::path::PathBuf, String)| {
        let (node_exe, pnpm_cli, target_version) = prepared;
        let mut reporter = |message: &str, detail: &str| {
            let text = if detail.is_empty() {
                message.to_string()
            } else {
                format!("{message} {detail}")
            };
            emit_progress(app, &text);
        };
        runtime::install_dsh_version(
            app,
            config,
            &node_exe,
            &pnpm_cli,
            &target_version,
            &mut reporter,
        )
    };

    with_directory_transaction(
        app,
        &current,
        &backup,
        &marker,
        "dsh",
        true,
        RollbackRecoveryNote::Restore,
        prepare,
        install,
    )
}
