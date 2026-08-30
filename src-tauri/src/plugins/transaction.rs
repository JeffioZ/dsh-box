//! 插件事务与启动收敛：落盘事务标记、原始 manifest 备份、中断后按
//! 包名/manifest 差异定向收敛（详见 mod.rs 的模块总述）。
//!
//! 并发协议（锁序 lifecycle → MARKET_PNPM_LOCK → RESTART_STATE，见
//! runner::MARKET_PNPM_LOCK 注释）：CLI 执行前写标记，成功/失败清标记；
//! 启动收敛（clear_resolved / recover_*）与 pnpm CLI 互斥。

use serde::{Deserialize, Serialize};

use super::maintenance::is_market_pkg;
use super::manifest_package_names;
use super::runner;

pub(super) const PLUGIN_INSTALL_MARKER_KEY: &str = "plugin_install_in_progress";

#[derive(Clone, Copy, Default, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(super) enum PluginMutationKind {
    #[default]
    Add,
    Remove,
}

#[derive(Clone, Deserialize, Serialize)]
pub(super) struct PluginInstallMarker {
    #[serde(default)]
    pub(super) package: Option<String>,
    pub(super) spec: String,
    #[serde(default)]
    pub(super) kind: PluginMutationKind,
    #[serde(default)]
    pub(super) user_removal: bool,
    #[serde(default)]
    pub(super) original_manifest: Option<String>,
}

/// 从 npm 依赖规格中提取 package.json dependency 名。无法可靠识别的 git、
/// URL 或本地路径规格返回 None，崩溃恢复不会据此删除 manifest 内容。
pub(super) fn spec_package_name(spec: &str) -> Option<&str> {
    let spec = spec.trim().split('#').next()?.trim();
    if spec.is_empty() || spec.contains("://") || spec.starts_with("git+") || spec.starts_with('.')
    {
        return None;
    }
    let end = if spec.starts_with('@') {
        let slash = spec.find('/')?;
        spec[slash + 1..]
            .rfind('@')
            .map(|at| slash + 1 + at)
            .unwrap_or(spec.len())
    } else {
        spec.rfind('@').filter(|at| *at > 0).unwrap_or(spec.len())
    };
    let name = &spec[..end];
    let valid_shape = if let Some(scoped) = name.strip_prefix('@') {
        scoped.split_once('/').is_some_and(|(scope, package)| {
            !scope.is_empty() && !package.is_empty() && !package.contains('/')
        })
    } else {
        !name.contains('/')
    };
    (valid_shape
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '@' | '/' | '-' | '_' | '.')))
    .then_some(name)
}

pub(super) fn install_marker(config: &crate::app_state::Config) -> Option<PluginInstallMarker> {
    crate::app_state::load_state_value(&config.root, PLUGIN_INSTALL_MARKER_KEY)
        .and_then(|value| serde_json::from_value(value).ok())
}

pub(super) fn save_install_marker(
    config: &crate::app_state::Config,
    spec: &str,
    package: Option<&str>,
    kind: PluginMutationKind,
    user_removal: bool,
    original_manifest: Option<&str>,
) -> Result<(), String> {
    if let Some(marker) = install_marker(config) {
        return Err(crate::locale::owned(
            format!(
                "上次插件操作尚未完成（{}），请先重启应用完成恢复后再试。",
                marker.spec
            ),
            format!(
                "The previous plugin operation ({}) is still unfinished. Restart the app to complete recovery before trying again.",
                marker.spec
            ),
        ));
    }
    crate::app_state::save_state_value(
        &config.root,
        PLUGIN_INSTALL_MARKER_KEY,
        serde_json::to_value(PluginInstallMarker {
            package: package.map(str::to_string),
            spec: spec.to_string(),
            kind,
            user_removal,
            original_manifest: original_manifest.map(str::to_string),
        })
        .map_err(|e| e.to_string())?,
    )
}

pub(super) fn clear_install_marker(config: &crate::app_state::Config) -> Result<(), String> {
    // 真删键而非写 Null：语义相同（Null 反序列化失败按无标记处理），
    // 但避免 state.json 永久残留无意义键
    crate::app_state::remove_state_value(&config.root, PLUGIN_INSTALL_MARKER_KEY)
}

pub(super) fn try_mark_user_removed(
    config: &crate::app_state::Config,
    package: &str,
) -> Result<(), String> {
    crate::app_state::save_state_value(
        &config.root,
        &format!("market_user_removed_{package}"),
        serde_json::json!(true),
    )
}

pub(super) fn dependency_installed(config: &crate::app_state::Config, package: &str) -> bool {
    let path = config.dsh_home().join("profiles/web/package.json");
    std::fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
        .and_then(|json| {
            json.get("dependencies")
                .and_then(|deps| deps.get(package))
                .cloned()
        })
        .is_some()
}

/// pnpm 互斥锁的跨模块获取入口（供服务重启/启动收敛与 CLI 串行互斥）。
///
/// 锁序约定（违反会死锁）：`lifecycle_guard` → `MARKET_PNPM_LOCK` →
/// `RESTART_STATE`。boot 流程持 lifecycle 后经 recover_*/clear_resolved 取
/// pnpm，因此任何需要两把锁的路径必须先拿 lifecycle 再拿 pnpm——
/// 绝不能先取 pnpm 再等 lifecycle。
pub(crate) type PnpmGuard = std::sync::MutexGuard<'static, ()>;

/// 非阻塞获取 pnpm 互斥锁；`None` 表示有 `dsh plugin` CLI 在途。
pub(crate) fn try_acquire_pnpm_lock() -> Option<PnpmGuard> {
    runner::MARKET_PNPM_LOCK.try_lock().ok()
}

/// 服务能够启动说明 profile 已完整解析；完成可能在命令提交后、事务标记
/// 清理前被中断的附属状态，再清除旧事务记录。
pub(crate) fn clear_resolved_install_marker(config: &crate::app_state::Config) {
    let Some(marker) = install_marker(config) else {
        return;
    };
    // 与在途 pnpm CLI 互斥（与 recover_interrupted_plugin_mutation 的加锁
    // 对称）：服务健康不证明此前在途/崩溃的命令已写完 profile。此刻清掉
    // 事务标记，随后进程再中断就没有恢复记录。拿不到锁说明有 CLI 在途，
    // 跳过本轮收敛，下次启动会再次调用。
    let Some(_pnpm) = try_acquire_pnpm_lock() else {
        crate::logging::log("plugins: pnpm 操作在途，跳过本轮启动收敛（下次启动重试）");
        return;
    };
    clear_resolved_install_marker_locked(config, marker);
}

/// 决策本体（调用方已持有 pnpm 锁；单测直接调用以避开并行用例间的锁竞态）。
pub(super) fn clear_resolved_install_marker_locked(
    config: &crate::app_state::Config,
    marker: PluginInstallMarker,
) {
    match marker.kind {
        PluginMutationKind::Remove if marker.user_removal => match marker.package.as_deref() {
            Some(package) if !dependency_installed(config, package) => {
                if let Err(e) = try_mark_user_removed(config, package) {
                    crate::logging::log(&format!(
                        "plugins: 完成中断卸载的用户状态记录失败，保留事务标记：{e}"
                    ));
                    return;
                }
            }
            // 卸载命令未生效（依赖仍在）或包名不可识别：不能记录用户卸载状态
            // （否则仍在安装的包会失去内置维护身份），仅记日志后按完成收敛。
            _ => {
                crate::logging::log(&format!(
                    "plugins: 中断的卸载事务未生效（{} 仍在安装或包名不可识别），不记录用户卸载状态",
                    marker.package.as_deref().unwrap_or(marker.spec.as_str())
                ));
            }
        },
        // 安装命令未生效（依赖未写入）：事务按完成收敛但插件并未装上，记日志
        // 便于诊断；不在此重跑 CLI（服务已就绪，重跑不属于启动收敛职责）。
        PluginMutationKind::Add => {
            if let Some(package) = marker.package.as_deref() {
                if !dependency_installed(config, package) {
                    crate::logging::log(&format!(
                        "plugins: 中断的安装事务未生效（{package} 未写入依赖），按未完成丢弃"
                    ));
                }
            }
        }
        _ => {}
    }
    if let Err(e) = clear_install_marker(config) {
        crate::logging::log(&format!("plugins: 清理已完成的插件事务标记失败：{e}"));
    }
}

pub(super) fn marker_targets_package(
    marker: &PluginInstallMarker,
    current_manifest: &str,
    package: &str,
) -> bool {
    if let Some(recorded) = marker.package.as_deref() {
        return recorded == package;
    }
    let Some(original) = marker.original_manifest.as_deref() else {
        return false;
    };
    let Some((before_dependencies, before_bundles)) = manifest_package_names(original) else {
        return false;
    };
    let Some((after_dependencies, after_bundles)) = manifest_package_names(current_manifest) else {
        return false;
    };
    match marker.kind {
        PluginMutationKind::Add => {
            (!before_dependencies.contains(package) && after_dependencies.contains(package))
                || (!before_bundles.contains(package) && after_bundles.contains(package))
        }
        PluginMutationKind::Remove => {
            (before_dependencies.contains(package) && !after_dependencies.contains(package))
                || (before_bundles.contains(package) && !after_bundles.contains(package))
        }
    }
}

/// 仅修复由 DSHBox 记录、且包名与启动错误完全一致的中断插件写操作。
/// add 的半成品会回退，remove 的半成品会完成卸载，二者都以可启动的
/// manifest 为收敛目标；没有事务记录或目标不匹配时保持用户配置不动。
pub(crate) fn recover_interrupted_plugin_mutation(
    config: &crate::app_state::Config,
    name: &str,
) -> Result<bool, String> {
    let Some(marker) = install_marker(config) else {
        return Ok(false);
    };
    let path = config.dsh_home().join("profiles/web/package.json");
    let current_manifest = std::fs::read_to_string(&path).map_err(|e| {
        crate::locale::owned(
            format!("读取 package.json 失败：{e}"),
            format!("Failed to read package.json: {e}"),
        )
    })?;
    if !marker_targets_package(&marker, &current_manifest, name) {
        crate::logging::log(&format!(
            "plugins: 启动错误指向 {name}，但中断事务记录的是 {}，不自动修改 manifest",
            marker.package.as_deref().unwrap_or(marker.spec.as_str())
        ));
        return Ok(false);
    }
    let _guard = runner::MARKET_PNPM_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let removed = prune_manifest_package_locked(config, name)?;
    if matches!(marker.kind, PluginMutationKind::Remove)
        && marker.user_removal
        && is_market_pkg(name)
    {
        try_mark_user_removed(config, name)?;
    }
    clear_install_marker(config)?;
    Ok(removed)
}

pub(super) fn prune_manifest_package_locked(
    config: &crate::app_state::Config,
    name: &str,
) -> Result<bool, String> {
    let path = config.dsh_home().join("profiles/web/package.json");
    let text = std::fs::read_to_string(&path).map_err(|e| {
        crate::locale::owned(
            format!("读取 package.json 失败：{e}"),
            format!("Failed to read package.json: {e}"),
        )
    })?;
    let mut json: serde_json::Value = serde_json::from_str(&text).map_err(|e| {
        crate::locale::owned(
            format!("解析 package.json 失败：{e}"),
            format!("Failed to parse package.json: {e}"),
        )
    })?;
    let mut removed = false;
    if let Some(deps) = json.get_mut("dependencies").and_then(|d| d.as_object_mut()) {
        removed |= deps.remove(name).is_some();
    }
    if let Some(bundles) = json
        .get_mut("dsh")
        .and_then(|d| d.get_mut("profile"))
        .and_then(|p| p.get_mut("bundles"))
        .and_then(|b| b.as_array_mut())
    {
        let before = bundles.len();
        bundles.retain(|item| item.as_str() != Some(name));
        removed |= bundles.len() < before;
    }
    if !removed {
        return Ok(false);
    }
    crate::app_state::atomic_write(
        &path,
        &serde_json::to_string_pretty(&json).map_err(|e| e.to_string())?,
    )
    .map_err(|e| {
        crate::locale::owned(
            format!("写回 package.json 失败：{e}"),
            format!("Failed to write package.json back: {e}"),
        )
    })?;
    Ok(true)
}
