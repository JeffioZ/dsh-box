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

/// 子进程基础环境：PATH 前置 node 目录、设置 DSH_HOME、npm 缓存落在应用根目录。
pub(crate) fn base_envs(node_exe: &Path, config: &Config) -> Vec<(&'static str, String)> {
    let mut envs: Vec<(&'static str, String)> = Vec::new();
    if let Some(dir) = node_exe.parent() {
        let path = std::env::var("PATH").unwrap_or_default();
        // PATH 分隔符按平台：Windows 分号，Unix 冒号
        #[cfg(windows)]
        let merged = format!("{};{}", dir.to_string_lossy(), path);
        #[cfg(not(windows))]
        let merged = format!("{}:{}", dir.to_string_lossy(), path);
        envs.push(("PATH", merged));
    }
    envs.push(("DSH_HOME", config.dsh_home.to_string_lossy().into_owned()));
    // npm 缓存落在应用根目录内（避免写入用户级缓存目录带来的权限问题）。
    let cache = config.root.join("npm-cache");
    envs.push(("npm_config_cache", cache.to_string_lossy().into_owned()));
    // 强制 native 模块用预编译二进制：node-pty 等包的 prebuild 检查失败后会
    // 回退 node-gyp 编译，新机器无 Python/VS 工具链时卡死（node_repl 等交互
    // 进程挂起）。设为 false 让它们找不到 prebuild 就立即失败而非尝试编译。
    envs.push(("npm_config_build_from_source", "false".to_string()));
    envs
}

/// 从版本字符串判断是否支持 `--no-open`（rc.8 及以上）。
/// 纯函数便于测试。
pub(super) fn version_supports_no_open(version: &str) -> bool {
    if let Some((_, rc_part)) = version.split_once("-rc") {
        // 形如 "0.1.0-rc.8"：split 得 ".8"，去前导点后取 rc 号数值比较
        rc_part
            .trim_start_matches('.')
            .parse::<u32>()
            .map(|n| n >= 8)
            .unwrap_or(false)
    } else {
        // 无 rc 后缀的稳定版（如 0.1.0）视为支持
        !version.is_empty()
    }
}

/// 已装 dsh 版本是否支持 `dsh web --no-open`（rc.8 及以上）。
/// rc.7 及更早不认识该标志会把未知选项当错误导致启动失败，因此必须按
/// 已装版本判定；无法解析的版本号保守不加（保持旧行为）。
fn dsh_supports_no_open(config: &Config) -> bool {
    let pkg = config
        .dsh_dir()
        .join("node_modules")
        .join("@deepseek-ai")
        .join("dsh")
        .join("package.json");
    let Ok(text) = std::fs::read_to_string(&pkg) else {
        return false;
    };
    let Ok(json) = serde_json::from_str::<serde_json::Value>(&text) else {
        return false;
    };
    let Some(version) = json.get("version").and_then(|v| v.as_str()) else {
        return false;
    };
    version_supports_no_open(version)
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
    // rc.8+ 支持 --no-open（桌面壳自己导航，不需要 dsh 自动弹浏览器）
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
