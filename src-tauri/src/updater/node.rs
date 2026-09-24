//! 托管 Node.js 更新事务。

use super::*;

// ---------- Node 更新 ----------

/// 切换到内置（便携）Node：当前运行在系统 Node 上时，从「检查更新」页
/// npm 行发起。与 update_node 不同：便携目录本不存在，无旧目录可换、
/// 无需停服（dsh 仍跑在系统 Node 上，安装互不影响），也不走目录事务——
/// 安装中断留下的半成品由 ensure_node 的既有隔离收敛自愈。下载进度走
/// 与 update_node 同一事件通道；完成后下次启动 ensure_node 即选中便携。
pub(super) fn switch_to_portable_node(
    app: &AppHandle,
    config: &crate::app_state::Config,
) -> Result<(), String> {
    // 幂等前置用真探测而非裸文件存在：解压中断留下的半成品目录（node.exe
    // 在但缺依赖）不该被当成「已在使用」——install 对已存在目录会清理重装，
    // 半成品允许直接重切收敛
    if runtime::inspect_runtime(config.node_exe()).is_some() {
        return Err(crate::locale::text(
            "已在使用内置 Node.js。",
            "Already using the built-in Node.js runtime.",
        )
        .into());
    }
    let current = runtime::current_node_version(config).unwrap_or_default();
    crate::logging::log(&format!(
        "runtime: 开始切换内置 Node（当前系统 Node {current}）"
    ));
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
    let prepared = runtime::prepare_node_archive_with(app, config, Some(&sink))?;
    runtime::install_node_from_archive(app, config, &prepared)?;
    // 终态文案统一走 done 通道（apply_update 的成功文案）：progress 是
    // 过程通道，done 写入时会清 progress，双写只会让「重启后生效」闪现
    // 一瞬即被覆盖
    crate::logging::log("runtime: 内置 Node 安装完成，重启后生效");
    Ok(())
}

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
