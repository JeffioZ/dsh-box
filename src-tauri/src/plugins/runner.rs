//! dsh plugin 子进程、串行化与 pnpm virtual-store 自愈。

use super::*;

fn text_tail(text: &str, max_chars: usize) -> String {
    let mut chars: Vec<char> = text.chars().rev().take(max_chars).collect();
    chars.reverse();
    chars.into_iter().collect::<String>().trim().to_string()
}

/// 所有 `dsh plugin`（pnpm）操作的互斥锁：引导、定时升级、手动升级、
/// 手动安装/卸载都可能并发触发 pnpm，串行化避免 pnpm 锁竞争与状态错乱。
pub(super) static MARKET_PNPM_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// 调用 dsh CLI 的 plugin 子命令（阻塞至完成，5 分钟超时）。
fn run_dsh_plugin(app: &AppHandle, args: &[&str]) -> Result<String, String> {
    let config = app.state::<AppState>().config();
    // 便携 Node 优先，其次系统 Node（与 dsh 服务启动的运行时选择一致）
    let node = if config.node_exe().exists() {
        config.node_exe()
    } else {
        crate::runtime::find_system_node().ok_or_else(|| {
            crate::locale::text(
                "Node.js 运行时未就绪。",
                "The Node.js runtime is not ready.",
            )
        })?
    };
    let mut cmd = std::process::Command::new(&node);
    cmd.arg(config.dsh_entry())
        .arg("plugin")
        .arg("--profile")
        .arg("web")
        .args(args);
    for (k, v) in crate::runtime::base_envs(&node, &config) {
        cmd.env(k, v);
    }
    // 正式版是 GUI 子系统；必须隐藏 node 控制台，否则首次插件引导会闪窗。
    crate::processes::hide_console(&mut cmd);
    // 输出重定向到临时文件：npm 输出可能远超管道缓冲（64KB），
    // 若不持续读取会让子进程写阻塞，误触 5 分钟超时。
    // 文件名做安全化 + 唯一随机后缀：scope 包名含 @ / 等字符（Windows 文件名
    // 不允许），同 pid 并发调用不共文件；长度截断防 Windows MAX_PATH 超限
    // （临时目录路径 + 长 spec 可能超过 260 字符）。
    let safe_args: String = args
        .join("_")
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.') {
                c
            } else {
                '_'
            }
        })
        .take(40)
        .collect();
    let mut nonce = [0u8; 6];
    let _ = getrandom::fill(&mut nonce);
    let nonce_hex: String = nonce.iter().map(|b| format!("{b:02x}")).collect();
    let out_path = std::env::temp_dir().join(format!(
        "dshd-plugin-{}-{}-{}.log",
        std::process::id(),
        safe_args,
        nonce_hex
    ));
    let out_file = std::fs::File::create(&out_path).map_err(|e| {
        crate::locale::error("创建输出文件失败", "Failed to create the output file", e)
    })?;
    cmd.stdout(out_file.try_clone().map_err(|e| {
        crate::locale::error(
            "复制输出句柄失败",
            "Failed to duplicate the output handle",
            e,
        )
    })?)
    .stderr(out_file);
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            let _ = std::fs::remove_file(&out_path);
            return Err(crate::locale::error(
                "启动 dsh 插件命令失败",
                "Failed to start the dsh plugin command",
                e,
            ));
        }
    };
    // 插件命令也纳入进程树守卫：应用退出或超时时一并回收包管理器后代进程。
    let _guard = crate::processes::TreeGuard::from_child(&child);
    // 5 分钟超时（插件安装可能较慢）；超时杀掉避免线程悬挂
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(300);
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {
                if std::time::Instant::now() > deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    let _ = std::fs::remove_file(&out_path);
                    return Err(crate::locale::text(
                        "插件操作超时（超过 5 分钟），已中止。",
                        "The plugin operation timed out after 5 minutes and was aborted.",
                    )
                    .into());
                }
                std::thread::sleep(std::time::Duration::from_millis(500));
            }
            Err(e) => {
                let _ = std::fs::remove_file(&out_path);
                return Err(crate::locale::error(
                    "等待插件命令失败",
                    "Failed while waiting for the plugin command",
                    e,
                ));
            }
        }
    };
    // 输出（含 stderr 尾部）作为错误详情返回（stdout/stderr 已重定向到临时文件）
    let tail = std::fs::read_to_string(&out_path).unwrap_or_default();
    let _ = std::fs::remove_file(&out_path);
    if !status.success() {
        let detail = tail.trim().to_string();
        if !detail.is_empty() {
            crate::logging::log(&format!(
                "plugins: dsh 插件命令失败，输出尾部：{}",
                text_tail(&detail, 12_000)
            ));
        }
        let mut err = if super::is_supply_chain_error(&detail) {
            // 新发布的包仍在 pnpm 供应链冷却期（minimumReleaseAge）内，
            // 任何针对该包的写操作（含卸载）都会被锁文件校验拒绝。转成
            // 友好提示，避免把原始堆栈抛给用户。
            crate::locale::owned(
                "该插件版本仍在供应链安全冷却期内，请在冷却结束后重试。".into(),
                "This package version is still within the supply-chain safety cooldown. Try again after the cooldown ends.".into(),
            )
        } else if detail.is_empty() {
            crate::locale::text("dsh 插件命令执行失败。", "The dsh plugin command failed.").into()
        } else {
            text_tail(&detail, 2_000)
        };
        // Windows 上被外部进程终止（如安全软件拦截）时取不到退出码；
        // 附注便于识别与诊断（is_environment_block_error 据此分类）
        if status.code().is_none() {
            err.push_str(crate::locale::text(
                "\n（进程被外部终止，无退出码——可能被安全软件拦截）",
                "\n(The process was terminated externally without an exit code, possibly by security software.)",
            ));
        }
        return Err(err);
    }
    Ok(tail)
}

/// 执行 dsh plugin 命令；若失败原因是 pnpm virtual store 错位（DSH_HOME
/// 被整体迁移/复制后 node_modules 元数据里的绝对路径失效），自动备份并
/// 重建 node_modules 后重试一次。安装/升级/卸载共用，遇错自愈。
/// 所有 pnpm 操作在此串行化（互斥锁），避免与定时同步/手动操作并发。
pub(super) fn run_dsh_plugin_auto(app: &AppHandle, args: &[&str]) -> Result<String, String> {
    run_dsh_plugin_auto_with_intent(app, args, false)
}

pub(super) fn run_dsh_plugin_auto_user_remove(
    app: &AppHandle,
    args: &[&str],
) -> Result<String, String> {
    run_dsh_plugin_auto_with_intent(app, args, true)
}

fn run_dsh_plugin_auto_with_intent(
    app: &AppHandle,
    args: &[&str],
    user_removal: bool,
) -> Result<String, String> {
    let _guard = MARKET_PNPM_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let config = app.state::<AppState>().config();
    let mutation = args.first().and_then(|command| match *command {
        "add" => Some(super::PluginMutationKind::Add),
        "remove" => Some(super::PluginMutationKind::Remove),
        _ => None,
    });
    let mutation_spec = mutation.and_then(|_| args.get(1)).copied();
    let mutation_package = mutation_spec.and_then(super::spec_package_name);
    let manifest_path = config.dsh_home().join("profiles/web/package.json");
    let original_manifest =
        mutation_spec.and_then(|_| std::fs::read_to_string(&manifest_path).ok());
    if let (Some(kind), Some(spec)) = (mutation, mutation_spec) {
        super::save_install_marker(
            &config,
            spec,
            mutation_package,
            kind,
            user_removal && matches!(kind, super::PluginMutationKind::Remove),
            original_manifest.as_deref(),
        )?;
    }

    let result = run_dsh_plugin_auto_locked(app, &config, args);
    if mutation_spec.is_none() {
        return result;
    }
    match result {
        Ok(output) => {
            if user_removal && matches!(mutation, Some(super::PluginMutationKind::Remove)) {
                if let Some(package) =
                    mutation_package.filter(|package| super::is_market_pkg(package))
                {
                    if let Err(e) = super::try_mark_user_removed(&config, package) {
                        return Err(crate::locale::owned(
                            format!(
                                "插件已卸载，但保存用户管理状态失败：{e}。已保留恢复信息，请重启应用后再试。"
                            ),
                            format!(
                                "The plugin was removed, but its user-managed state could not be saved: {e}. Recovery information was kept; restart the app before trying again."
                            ),
                        ));
                    }
                }
            }
            if let Err(e) = super::clear_install_marker(&config) {
                crate::logging::log(&format!(
                    "plugins: 命令成功，但清理插件事务标记失败（下次服务就绪后重试）：{e}"
                ));
            }
            Ok(output)
        }
        Err(error) => {
            let rollback = original_manifest.as_deref().map(|text| {
                crate::app_state::atomic_write(&manifest_path, text).map_err(|e| {
                    crate::locale::owned(
                        format!("回滚 package.json 失败：{e}"),
                        format!("Failed to restore package.json: {e}"),
                    )
                })
            });
            match rollback {
                Some(Ok(())) => {
                    if let Err(e) = super::clear_install_marker(&config) {
                        crate::logging::log(&format!(
                            "plugins: manifest 已回滚，但清理插件事务标记失败：{e}"
                        ));
                    }
                    crate::logging::log("plugins: 插件命令失败，已原子恢复 package.json");
                    Err(error)
                }
                Some(Err(rollback_error)) => Err(crate::locale::owned(
                        format!(
                            "{error}\n（{rollback_error}；已保留恢复信息，下次启动会自动尝试修复）"
                        ),
                        format!(
                            "{error}\n({rollback_error}; recovery information was kept so the next launch can try to repair it automatically)"
                        ),
                    )),
                None => {
                    crate::logging::log("plugins: 插件命令前未读到 package.json，无法回滚");
                    Err(error)
                }
            }
        }
    }
}

fn run_dsh_plugin_auto_locked(
    app: &AppHandle,
    config: &crate::app_state::Config,
    args: &[&str],
) -> Result<String, String> {
    restore_interrupted_virtual_store(config)?;
    match run_dsh_plugin(app, args) {
        Ok(out) => Ok(out),
        Err(e) if is_virtual_store_error(&e) || virtual_store_stale(config) => {
            crate::logging::log(
                "plugins: 检测到 pnpm virtual store 错位，备份并重建 node_modules 后重试",
            );
            if let Err(re) = recover_virtual_store(config) {
                return Err(crate::locale::owned(
                    format!("{e}\n（自愈失败：{re}）"),
                    format!("{e}\n(Automatic recovery failed: {re})"),
                ));
            }
            match run_dsh_plugin(app, args) {
                Ok(out) => {
                    crate::logging::log("plugins: virtual store 自愈成功，node_modules 已重建");
                    finish_virtual_store_recovery(config);
                    Ok(out)
                }
                Err(e2) => match rollback_virtual_store(config) {
                    Ok(()) => Err(crate::locale::owned(
                        format!("{e2}\n（重建 virtual store 仍失败，已恢复原 node_modules）"),
                        format!("{e2}\n(Rebuilding the virtual store failed again; the original node_modules was restored.)"),
                    )),
                    Err(rollback_error) => Err(crate::locale::owned(
                        format!("{e2}\n（重建 virtual store 仍失败；恢复原 node_modules 也失败：{rollback_error}）"),
                        format!("{e2}\n(Rebuilding the virtual store failed again, and restoring the original node_modules also failed: {rollback_error})"),
                    )),
                },
            }
        }
        Err(e) => Err(e),
    }
}

fn virtual_store_paths(
    config: &crate::app_state::Config,
) -> (
    std::path::PathBuf,
    std::path::PathBuf,
    std::path::PathBuf,
    std::path::PathBuf,
    std::path::PathBuf,
) {
    let dir = config.dsh_home().join("profiles/web");
    (
        dir.join("node_modules"),
        dir.join("pnpm-lock.yaml"),
        dir.join("node_modules.vstore-bak"),
        dir.join("pnpm-lock.yaml.vstore-bak"),
        dir.join(".vstore-recovery"),
    )
}

/// 任何插件写操作前恢复上次被进程退出打断的自愈事务。无 marker 的备份是
/// 已提交后的清理残留：有新目录时只删旧备份；新目录缺失时保守恢复旧副本。
fn restore_interrupted_virtual_store(config: &crate::app_state::Config) -> Result<(), String> {
    let (nm, lock, bak, lock_bak, marker) = virtual_store_paths(config);
    if marker.exists() {
        if std::fs::read_to_string(&marker).is_ok_and(|state| state.trim() == "committed") {
            cleanup_committed_virtual_store(&nm, &lock, &bak, &lock_bak)?;
            std::fs::remove_file(&marker).map_err(|e| {
                crate::locale::error(
                    "清理自愈标记失败",
                    "Failed to remove the recovery marker",
                    e,
                )
            })?;
            return Ok(());
        }
        if bak.exists() {
            return rollback_virtual_store(config);
        }
        if lock_bak.exists() {
            if lock.exists() {
                std::fs::remove_file(&lock).map_err(|e| {
                    crate::locale::error(
                        "清理中断的 lock 文件失败",
                        "Failed to remove the interrupted lock file",
                        e,
                    )
                })?;
            }
            std::fs::rename(&lock_bak, &lock).map_err(|e| {
                crate::locale::error(
                    "恢复中断的 pnpm-lock.yaml 失败",
                    "Failed to restore the interrupted pnpm-lock.yaml",
                    e,
                )
            })?;
        }
        if !nm.exists() {
            return Err(crate::locale::text(
                "virtual store 自愈中断，且原 node_modules 备份不存在",
                "Virtual-store recovery was interrupted, and the original node_modules backup is missing",
            ).into());
        }
        std::fs::remove_file(&marker).map_err(|e| {
            crate::locale::error(
                "清理自愈标记失败",
                "Failed to remove the recovery marker",
                e,
            )
        })?;
        return Ok(());
    }
    cleanup_committed_virtual_store(&nm, &lock, &bak, &lock_bak)
}

fn cleanup_committed_virtual_store(
    nm: &std::path::Path,
    lock: &std::path::Path,
    bak: &std::path::Path,
    lock_bak: &std::path::Path,
) -> Result<(), String> {
    if bak.exists() {
        if nm.exists() {
            std::fs::remove_dir_all(bak).map_err(|e| {
                crate::locale::error(
                    "清理已提交的旧备份失败",
                    "Failed to remove the committed backup",
                    e,
                )
            })?;
        } else {
            std::fs::rename(bak, nm).map_err(|e| {
                crate::locale::error(
                    "恢复孤立的 node_modules 备份失败",
                    "Failed to restore the orphaned node_modules backup",
                    e,
                )
            })?;
        }
    }
    if lock_bak.exists() {
        if lock.exists() {
            std::fs::remove_file(lock_bak).map_err(|e| {
                crate::locale::error(
                    "清理已提交的 lock 备份失败",
                    "Failed to remove the committed lock backup",
                    e,
                )
            })?;
        } else {
            std::fs::rename(lock_bak, lock).map_err(|e| {
                crate::locale::error(
                    "恢复孤立的 pnpm-lock.yaml 备份失败",
                    "Failed to restore the orphaned pnpm-lock.yaml backup",
                    e,
                )
            })?;
        }
    }
    Ok(())
}

/// virtual store 自愈：写入事务标记后再把 node_modules（与 pnpm-lock.yaml）
/// 改名备份，让 pnpm 视其为全新目录重建。进程在任意一步退出时，下次插件
/// 操作会先恢复旧副本，不会把唯一完整备份当成“残留”删除。
fn recover_virtual_store(config: &crate::app_state::Config) -> Result<(), String> {
    use std::io::Write;
    restore_interrupted_virtual_store(config)?;
    let (nm, lock, bak, lock_bak, marker) = virtual_store_paths(config);
    let mut marker_file = std::fs::File::create(&marker).map_err(|e| {
        crate::locale::error(
            "创建 virtual store 自愈标记失败",
            "Failed to create the virtual-store recovery marker",
            e,
        )
    })?;
    marker_file
        .write_all(b"in-progress\n")
        .and_then(|_| marker_file.sync_all())
        .map_err(|e| {
            crate::locale::error(
                "写入 virtual store 自愈标记失败",
                "Failed to write the virtual-store recovery marker",
                e,
            )
        })?;
    if let Err(error) = std::fs::rename(&nm, &bak) {
        // rename 未发生：清掉刚写入的 in-progress 标记（对齐下方 lock 备份
        // 失败分支），避免遗留标记让下次操作误判存在未完成的自愈事务
        let _ = std::fs::remove_file(&marker);
        return Err(crate::locale::error(
            "备份 node_modules 失败",
            "Failed to back up node_modules",
            error,
        ));
    }
    if lock.exists() {
        if let Err(error) = std::fs::rename(&lock, &lock_bak) {
            let _ = std::fs::rename(&bak, &nm);
            let _ = std::fs::remove_file(&marker);
            return Err(crate::locale::error(
                "备份 pnpm-lock.yaml 失败",
                "Failed to back up pnpm-lock.yaml",
                error,
            ));
        }
    }
    Ok(())
}

/// 自愈成功后提交：新依赖已被 dsh CLI 验证可用，删除旧目录避免长期占用双份空间。
fn finish_virtual_store_recovery(config: &crate::app_state::Config) {
    use std::io::Write;
    let (nm, lock, bak, lock_bak, marker) = virtual_store_paths(config);
    let commit_result = (|| -> Result<(), String> {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .truncate(true)
            .open(&marker)
            .map_err(|error| error.to_string())?;
        file.write_all(b"committed\n")
            .and_then(|_| file.sync_all())
            .map_err(|error| error.to_string())
    })();
    if let Err(error) = commit_result {
        crate::logging::log(&format!(
            "plugins: 提交 virtual store 自愈标记失败，保留备份等待下次恢复：{error}"
        ));
        return;
    }
    if let Err(error) = cleanup_committed_virtual_store(&nm, &lock, &bak, &lock_bak) {
        crate::logging::log(&format!(
            "plugins: 清理已提交的 virtual store 备份失败（下次继续）：{error}"
        ));
        return;
    }
    if let Err(error) = std::fs::remove_file(&marker) {
        crate::logging::log(&format!(
            "plugins: 清理已提交的 virtual store 标记失败（下次继续）：{error}"
        ));
    }
}

/// 自愈重试失败时回滚，保证原 profile 仍可继续使用。
fn rollback_virtual_store(config: &crate::app_state::Config) -> Result<(), String> {
    let (nm, lock, bak, lock_bak, marker) = virtual_store_paths(config);
    if nm.exists() {
        std::fs::remove_dir_all(&nm).map_err(|e| {
            crate::locale::error(
                "清理失败的新 node_modules 失败",
                "Failed to remove the unsuccessful new node_modules",
                e,
            )
        })?;
    }
    std::fs::rename(&bak, &nm).map_err(|e| {
        crate::locale::error(
            "恢复原 node_modules 失败",
            "Failed to restore the original node_modules",
            e,
        )
    })?;
    if lock_bak.exists() {
        if lock.exists() {
            std::fs::remove_file(&lock).map_err(|e| {
                crate::locale::error(
                    "清理失败的新 lock 文件失败",
                    "Failed to remove the unsuccessful new lock file",
                    e,
                )
            })?;
        }
        std::fs::rename(&lock_bak, &lock).map_err(|e| {
            crate::locale::error(
                "恢复原 pnpm-lock.yaml 失败",
                "Failed to restore the original pnpm-lock.yaml",
                e,
            )
        })?;
    }
    if marker.exists() {
        std::fs::remove_file(&marker).map_err(|e| {
            crate::locale::error(
                "清理自愈标记失败",
                "Failed to remove the recovery marker",
                e,
            )
        })?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        finish_virtual_store_recovery, recover_virtual_store, restore_interrupted_virtual_store,
        rollback_virtual_store, text_tail,
    };

    #[test]
    fn user_error_tail_is_bounded_without_splitting_unicode() {
        assert_eq!(text_tail("前文\n错误甲乙丙", 4), "误甲乙丙");
        assert_eq!(text_tail("  short  ", 20), "short");
    }

    fn fixture(name: &str) -> (crate::app_state::Config, std::path::PathBuf) {
        use std::time::{SystemTime, UNIX_EPOCH};
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "dshbox-vstore-{name}-{}-{nonce}",
            std::process::id()
        ));
        let mut config = crate::app_state::Config::load();
        config.root = root.clone();
        config.dsh_home = root.join("home");
        (config, root)
    }

    #[test]
    fn failed_virtual_store_rebuild_restores_profile() {
        let (config, root) = fixture("rollback");
        let profile = config.dsh_home().join("profiles/web");
        std::fs::create_dir_all(profile.join("node_modules")).unwrap();
        std::fs::write(profile.join("node_modules/original"), "old").unwrap();
        std::fs::write(profile.join("pnpm-lock.yaml"), "old-lock").unwrap();

        recover_virtual_store(&config).unwrap();
        std::fs::create_dir_all(profile.join("node_modules")).unwrap();
        std::fs::write(profile.join("node_modules/partial"), "new").unwrap();
        std::fs::write(profile.join("pnpm-lock.yaml"), "new-lock").unwrap();
        rollback_virtual_store(&config).unwrap();

        assert_eq!(
            std::fs::read_to_string(profile.join("node_modules/original")).unwrap(),
            "old"
        );
        assert!(!profile.join("node_modules/partial").exists());
        assert_eq!(
            std::fs::read_to_string(profile.join("pnpm-lock.yaml")).unwrap(),
            "old-lock"
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn successful_virtual_store_rebuild_removes_backup() {
        let (config, root) = fixture("commit");
        let profile = config.dsh_home().join("profiles/web");
        std::fs::create_dir_all(profile.join("node_modules")).unwrap();
        std::fs::write(profile.join("node_modules/original"), "old").unwrap();
        std::fs::write(profile.join("pnpm-lock.yaml"), "old-lock").unwrap();

        recover_virtual_store(&config).unwrap();
        std::fs::create_dir_all(profile.join("node_modules")).unwrap();
        finish_virtual_store_recovery(&config);

        assert!(!profile.join("node_modules.vstore-bak").exists());
        assert!(!profile.join("pnpm-lock.yaml.vstore-bak").exists());
        assert!(!profile.join(".vstore-recovery").exists());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn interrupted_rebuild_restores_backup_before_the_next_operation() {
        let (config, root) = fixture("interrupted");
        let profile = config.dsh_home().join("profiles/web");
        std::fs::create_dir_all(profile.join("node_modules.vstore-bak")).unwrap();
        std::fs::write(profile.join("node_modules.vstore-bak/original"), "old").unwrap();
        std::fs::create_dir_all(profile.join("node_modules")).unwrap();
        std::fs::write(profile.join("node_modules/partial"), "new").unwrap();
        std::fs::write(profile.join("pnpm-lock.yaml.vstore-bak"), "old-lock").unwrap();
        std::fs::write(profile.join("pnpm-lock.yaml"), "new-lock").unwrap();
        std::fs::write(profile.join(".vstore-recovery"), "v1\n").unwrap();

        restore_interrupted_virtual_store(&config).unwrap();

        assert!(profile.join("node_modules/original").exists());
        assert!(!profile.join("node_modules/partial").exists());
        assert_eq!(
            std::fs::read_to_string(profile.join("pnpm-lock.yaml")).unwrap(),
            "old-lock"
        );
        assert!(!profile.join(".vstore-recovery").exists());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn committed_rebuild_keeps_new_profile_after_cleanup_interruption() {
        let (config, root) = fixture("committed-interruption");
        let profile = config.dsh_home().join("profiles/web");
        std::fs::create_dir_all(profile.join("node_modules.vstore-bak")).unwrap();
        std::fs::write(profile.join("node_modules.vstore-bak/original"), "old").unwrap();
        std::fs::create_dir_all(profile.join("node_modules")).unwrap();
        std::fs::write(profile.join("node_modules/current"), "new").unwrap();
        std::fs::write(profile.join("pnpm-lock.yaml.vstore-bak"), "old-lock").unwrap();
        std::fs::write(profile.join("pnpm-lock.yaml"), "new-lock").unwrap();
        std::fs::write(profile.join(".vstore-recovery"), "committed\n").unwrap();

        restore_interrupted_virtual_store(&config).unwrap();

        assert!(profile.join("node_modules/current").exists());
        assert!(!profile.join("node_modules/original").exists());
        assert_eq!(
            std::fs::read_to_string(profile.join("pnpm-lock.yaml")).unwrap(),
            "new-lock"
        );
        assert!(!profile.join(".vstore-recovery").exists());
        let _ = std::fs::remove_dir_all(root);
    }
}
