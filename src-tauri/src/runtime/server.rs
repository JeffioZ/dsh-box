//! dsh web 服务命令与环境。

use super::*;
use std::process::Child;

pub(crate) struct StartedServer {
    pub child: Child,
    pub guard: Option<TreeGuard>,
    /// 本次启动前日志文件的字节偏移；`--port 0` 的实际端口从此处之后解析。
    pub log_offset: u64,
}

// ---------- 服务启动 ----------

/// 子进程基础环境：PATH 前置自管 Node / pnpm、设置 DSH_HOME，并把包管理器
/// 缓存收口到应用数据目录。dsh 的 plugin 子命令会调用 pnpm，因此这里不能
/// 只在首次安装时直接执行 pnpm，还要让后续 dsh 子进程稳定找到同一版本。
pub(crate) fn base_envs(node_exe: &Path, config: &Config) -> Vec<(&'static str, String)> {
    let mut envs: Vec<(&'static str, String)> = Vec::new();
    if let Some(dir) = node_exe.parent() {
        let path = std::env::var("PATH").unwrap_or_default();
        // PATH 分隔符按平台：Windows 分号，Unix 冒号
        #[cfg(windows)]
        let merged = format!(
            "{};{};{}",
            config.package_manager_bin_dir().to_string_lossy(),
            dir.to_string_lossy(),
            path
        );
        #[cfg(not(windows))]
        let merged = format!(
            "{}:{}:{}",
            config.package_manager_bin_dir().to_string_lossy(),
            dir.to_string_lossy(),
            path
        );
        envs.push(("PATH", merged));
    }
    envs.push(("DSH_HOME", config.dsh_home.to_string_lossy().into_owned()));
    // npm 缓存落在应用根目录内（避免写入用户级缓存目录带来的权限问题）。
    let cache = config.root.join("npm-cache");
    envs.push(("npm_config_cache", cache.to_string_lossy().into_owned()));
    let store = config.root.join("pnpm-store");
    envs.push((
        "PNPM_HOME",
        config
            .package_manager_bin_dir()
            .to_string_lossy()
            .into_owned(),
    ));
    envs.push((
        "pnpm_config_store_dir",
        store.to_string_lossy().into_owned(),
    ));
    // 强制 native 模块用预编译二进制：node-pty 等包的 prebuild 检查失败后会
    // 回退 node-gyp 编译，新机器无 Python/VS 工具链时卡死（node_repl 等交互
    // 进程挂起）。设为 false 让它们找不到 prebuild 就立即失败而非尝试编译。
    envs.push(("npm_config_build_from_source", "false".to_string()));
    envs
}

/// 从完整 SemVer 判断是否支持 `--no-open`。该参数从 0.1.0-rc.8 起可用；
/// 不能只比较 rc 序号，例如 0.1.1-rc.2 实际晚于 0.1.0-rc.8。
pub(super) fn version_supports_no_open(version: &str) -> bool {
    let Ok(installed) = semver::Version::parse(version.trim().trim_start_matches('v')) else {
        return false;
    };
    let minimum = semver::Version::parse("0.1.0-rc.8").expect("valid --no-open threshold");
    installed.cmp_precedence(&minimum).is_ge()
}

/// 已装 dsh 版本是否支持 `dsh web --no-open`（0.1.0-rc.8 及以上）。
/// 更早版本不认识该标志会把未知选项当错误导致启动失败，因此必须按
/// 已装版本判定；无法解析的版本号保守不加（保持旧行为）。
fn dsh_supports_no_open(config: &Config) -> bool {
    installed_dsh_version(config)
        .as_deref()
        .is_some_and(version_supports_no_open)
}

/// 隐藏窗口启动 `dsh web` 服务。保留 Child 供调用方监控早退；只保留 PID
/// 无法及时发现端口绑定失败，会让启动页误等完整超时。
pub(crate) fn start_server(
    _app: &AppHandle,
    config: &Config,
    node_exe: &Path,
) -> Result<StartedServer, String> {
    let mut args = vec![config.dsh_entry().to_string_lossy().into_owned()];
    args.push("web".into());
    // 支持时禁用浏览器弹出；桌面壳会在服务就绪后导航主 WebView。
    if dsh_supports_no_open(config) {
        args.push("--no-open".into());
    }
    args.push("--port".into());
    args.push(config.port.to_string());
    let envs = base_envs(node_exe, config);
    let log = config.dsh_log();
    let log_offset = std::fs::metadata(&log).map(|meta| meta.len()).unwrap_or(0);
    let child =
        processes::spawn_process(node_exe, &args, &envs, Some(&config.dsh_dir()), Some(&log))
            .map_err(|e| {
                crate::locale::owned(
                    format!("启动 dsh 服务失败：{e}"),
                    format!("Failed to start the dsh service: {e}"),
                )
            })?;
    // 建立进程树守卫（Windows Job / Unix 进程组）；失败不致命，退出时另有兜底。
    let guard = processes::TreeGuard::from_child(&child);
    Ok(StartedServer {
        child,
        guard,
        log_offset,
    })
}
