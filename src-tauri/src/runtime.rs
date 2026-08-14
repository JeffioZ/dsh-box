//! 运行时安装与维护：Node 检测/便携安装、dsh npm 包安装、服务启动。

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use tauri::AppHandle;

use crate::app_state::{BootPhase, Config};
use crate::processes::{self, TreeGuard};
use crate::versions::{node_satisfies, parse_node_version};
use crate::{emit_status, emit_status_progress};

const NODEJS_INDEX: &str = "https://nodejs.org/dist/index.json";
const NPM_DIST_TAGS: &str = "https://registry.npmjs.org/-/package/@deepseek-ai/dsh/dist-tags";

/// 全局共享 HTTP 客户端：rustls/TLS 配置只构建一次（高频查询路径省初始化开销）。
static AGENT: std::sync::OnceLock<ureq::Agent> = std::sync::OnceLock::new();

/// 带超时的 HTTP 客户端（rustls，不依赖系统证书存储）。
pub(crate) fn client() -> ureq::Agent {
    AGENT
        .get_or_init(|| {
            ureq::AgentBuilder::new()
                .timeout_connect(Duration::from_secs(15))
                .timeout_read(Duration::from_secs(90))
                .timeout_write(Duration::from_secs(90))
                .build()
        })
        .clone()
}

/// 读取一个小 URL 到字符串（供版本检查等使用）。
fn get_text(url: &str) -> Result<String, String> {
    let resp = client()
        .get(url)
        .call()
        .map_err(|e| format!("网络请求失败：{e}"))?;
    let mut reader = resp.into_reader();
    let mut s = String::new();
    reader
        .read_to_string(&mut s)
        .map_err(|e| format!("读取响应失败：{e}"))?;
    Ok(s)
}

/// Node 官方版本索引的进程内缓存（约 1MB JSON，检查更新高频拉取）。
/// TTL 1 小时；仅检查场景复用，安装/更新 Node 前强制刷新。
static LTS_CACHE: std::sync::Mutex<Option<(std::time::Instant, String)>> =
    std::sync::Mutex::new(None);

/// 当前最新 LTS 版本号（形如 v24.19.0；带 1 小时进程内缓存）。
pub(crate) fn latest_lts() -> Result<String, String> {
    latest_lts_cached(false)
}

/// 最新 LTS：force=true 强制刷新（安装/更新 Node 前调用）。
pub(crate) fn latest_lts_cached(force: bool) -> Result<String, String> {
    let now = std::time::Instant::now();
    {
        let cache = LTS_CACHE.lock().unwrap_or_else(|e| e.into_inner());
        if !force {
            if let Some((t, v)) = cache.as_ref() {
                if now.duration_since(*t) < Duration::from_secs(3600) {
                    return Ok(v.clone());
                }
            }
        }
    }
    let resp = client()
        .get(NODEJS_INDEX)
        .call()
        .map_err(|e| format!("获取 Node 版本信息失败：{e}"))?;
    let json: serde_json::Value = resp
        .into_json()
        .map_err(|e| format!("解析 Node 版本信息失败：{e}"))?;
    let arr = json.as_array().ok_or("Node 版本列表格式错误")?;
    for entry in arr {
        if entry
            .get("lts")
            .map(|v| !v.is_boolean() || v.as_bool().unwrap_or(false))
            == Some(true)
        {
            if let Some(v) = entry.get("version").and_then(|v| v.as_str()) {
                let s = v.to_string();
                *LTS_CACHE.lock().unwrap_or_else(|e| e.into_inner()) = Some((now, s.clone()));
                return Ok(s);
            }
        }
    }
    Err("未找到 Node LTS 版本".into())
}

/// 查询 npm 官方 `@deepseek-ai/dsh` 的最新版本。
pub(crate) fn npm_latest_dsh_version() -> Result<String, String> {
    let text = get_text(NPM_DIST_TAGS)?;
    let json: serde_json::Value =
        serde_json::from_str(&text).map_err(|e| format!("解析失败：{e}"))?;
    json.get("latest")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| "响应中没有 latest 字段".into())
}

/// 已安装 dsh 版本（读 package.json）。
pub fn installed_dsh_version(config: &Config) -> Option<String> {
    let pkg = config
        .dsh_dir()
        .join("node_modules/@deepseek-ai/dsh/package.json");
    let text = std::fs::read_to_string(pkg).ok()?;
    let json: serde_json::Value = serde_json::from_str(&text).ok()?;
    json.get("version")
        .and_then(|v| v.as_str())
        .map(String::from)
}

// ---------- Node 运行时 ----------

/// 读取指定 node 可执行文件的版本 (major, minor, patch)。
pub(crate) fn node_version(program: &Path) -> Option<(u32, u32, u32)> {
    let mut cmd = std::process::Command::new(program);
    cmd.arg("--version");
    processes::hide_console(&mut cmd);
    let out = cmd.output().ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    parse_node_version(&text)
}

/// 在系统中查找 Node：Windows 常见目录 / Unix 常见位置 / PATH 兜底。
pub(crate) fn find_system_node() -> Option<PathBuf> {
    #[cfg(windows)]
    {
        let candidates = [
            PathBuf::from(std::env::var("ProgramFiles").unwrap_or_default())
                .join("nodejs/node.exe"),
            PathBuf::from(std::env::var("ProgramFiles(x86)").unwrap_or_default())
                .join("nodejs/node.exe"),
        ];
        for c in &candidates {
            if c.exists() {
                return Some(c.clone());
            }
        }
    }
    // PATH 兜底（Windows 找 node.exe，Unix 找 node）
    if let Ok(path) = std::env::var("PATH") {
        for dir in std::env::split_paths(&path) {
            let name = if cfg!(windows) { "node.exe" } else { "node" };
            let exe = dir.join(name);
            if exe.exists() {
                return Some(exe);
            }
        }
    }
    #[cfg(not(windows))]
    {
        // Unix 常见安装位置兜底
        for p in [
            "/usr/local/bin/node",
            "/usr/bin/node",
            "/opt/homebrew/bin/node",
        ] {
            let exe = PathBuf::from(p);
            if exe.exists() {
                return Some(exe);
            }
        }
    }
    None
}

/// 当前使用的 Node 版本字符串（便携优先，其次系统）。
pub(crate) fn current_node_version(config: &Config) -> Option<String> {
    let managed = config.node_exe();
    let exe = if managed.exists() {
        managed
    } else {
        find_system_node()?
    };
    node_version(&exe).map(|(m, n, p)| format!("v{m}.{n}.{p}"))
}

/// 确保 Node 可用：优先便携 Node（我们自己安装的 LTS，跳过版本验证提速），
/// 其次系统 Node（必须验证版本），都没有则下载安装便携版。
pub(crate) fn ensure_node(app: &AppHandle, config: &Config) -> Result<PathBuf, String> {
    let managed = config.node_exe();
    if managed.exists() {
        // 便携 Node 是本应用安装的 LTS，版本必然满足；跳过 node --version 检测省一次进程启动。
        // 若被手动破坏，服务启动失败会进入错误页并提示。
        return Ok(managed);
    }
    if let Some(system) = find_system_node() {
        if let Some((maj, min, _)) = node_version(&system) {
            if node_satisfies(maj, min) {
                return Ok(system);
            }
        }
    }
    install_portable_node(app, config)
}

/// 下载并安装便携版 Node 到应用数据目录。
pub(crate) fn install_portable_node(app: &AppHandle, config: &Config) -> Result<PathBuf, String> {
    emit_status(app, BootPhase::InstallingNode, "正在下载 Node.js…", "");
    let version = latest_lts_cached(true)?; // 形如 v24.19.0；安装前强制刷新缓存

    // 按平台选择官方包（Windows zip / macOS、Linux tar.gz）
    let (dir_name, url, is_zip) = node_package(&version);
    let node_dir = config.node_dir();
    let archive_name = if is_zip {
        "node-download.zip"
    } else {
        "node-download.tar.gz"
    };
    let archive_path = config.root.join(archive_name);
    std::fs::create_dir_all(&node_dir).map_err(|e| e.to_string())?;

    emit_status(
        app,
        BootPhase::InstallingNode,
        &format!("正在下载 Node.js {version}…"),
        "",
    );
    let resp = client()
        .get(&url)
        .call()
        .map_err(|e| format!("下载 Node.js 失败：{e}"))?;
    let total: u64 = resp
        .header("Content-Length")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let mut reader = resp.into_reader();
    let mut file =
        std::fs::File::create(&archive_path).map_err(|e| format!("写入临时文件失败：{e}"))?;
    let mut buf = [0u8; 65536];
    let mut done: u64 = 0;
    let mut last_pct: i64 = -1;
    let mut last_emit = std::time::Instant::now() - Duration::from_secs(1);
    loop {
        let n = reader
            .read(&mut buf)
            .map_err(|e| format!("下载 Node.js 失败：{e}"))?;
        if n == 0 {
            break;
        }
        file.write_all(&buf[..n])
            .map_err(|e| format!("写入临时文件失败：{e}"))?;
        done += n as u64;
        if total > 0 {
            // 节流：仅跨整数百分点且距上次广播 ≥200ms 才发 IPC，避免数百次高频事件
            let pct = ((done as f64 / total as f64) * 100.0) as i64;
            if pct > last_pct && last_emit.elapsed() >= Duration::from_millis(200) {
                last_pct = pct;
                last_emit = std::time::Instant::now();
                emit_status_progress(
                    app,
                    BootPhase::InstallingNode,
                    &format!("正在下载 Node.js {version}… {pct}%",),
                    &format!(
                        "{:.1} MB / {:.1} MB",
                        done as f64 / 1048576.0,
                        total as f64 / 1048576.0
                    ),
                    Some(pct as f64),
                );
            }
        }
    }
    drop(file);

    emit_status(app, BootPhase::InstallingNode, "正在解压 Node.js…", "");
    #[cfg(windows)]
    extract_zip(&archive_path, &node_dir)?;
    #[cfg(not(windows))]
    extract_tar(&archive_path, &node_dir)?;
    let _ = std::fs::remove_file(&archive_path);

    // 包内是 node-<ver>-<platform>/ 单层目录，拍平（对文件/目录残留分别处理，保证幂等）
    let inner = node_dir.join(&dir_name);
    if inner.exists() {
        for entry in std::fs::read_dir(&inner).map_err(|e| e.to_string())? {
            let entry = entry.map_err(|e| e.to_string())?;
            let target = node_dir.join(entry.file_name());
            if target.is_dir() {
                let _ = std::fs::remove_dir_all(&target);
            } else {
                let _ = std::fs::remove_file(&target);
            }
            std::fs::rename(entry.path(), &target).map_err(|e| e.to_string())?;
        }
        let _ = std::fs::remove_dir_all(&inner);
    }

    if !config.node_exe().exists() {
        return Err("Node.js 解压后未找到 node 可执行文件".into());
    }
    Ok(config.node_exe())
}

/// 当前平台的 Node 官方包：返回（包目录名、下载 URL、是否 zip）。
/// 未覆盖的平台会因函数无返回值而在编译期报错，避免静默下载错误包。
fn node_package(version: &str) -> (String, String, bool) {
    #[cfg(target_os = "windows")]
    {
        let dir = format!("node-{version}-win-x64");
        (
            dir.clone(),
            format!("https://nodejs.org/dist/{version}/{dir}.zip"),
            true,
        )
    }
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    {
        let dir = format!("node-{version}-darwin-arm64");
        (
            dir.clone(),
            format!("https://nodejs.org/dist/{version}/{dir}.tar.gz"),
            false,
        )
    }
    #[cfg(all(target_os = "macos", target_arch = "x86_64"))]
    {
        let dir = format!("node-{version}-darwin-x64");
        (
            dir.clone(),
            format!("https://nodejs.org/dist/{version}/{dir}.tar.gz"),
            false,
        )
    }
    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    {
        let dir = format!("node-{version}-linux-x64");
        (
            dir.clone(),
            format!("https://nodejs.org/dist/{version}/{dir}.tar.gz"),
            false,
        )
    }
    #[cfg(all(target_os = "linux", target_arch = "aarch64"))]
    {
        let dir = format!("node-{version}-linux-arm64");
        (
            dir.clone(),
            format!("https://nodejs.org/dist/{version}/{dir}.tar.gz"),
            false,
        )
    }
}

/// Windows：zip 解压（bsdtar 优先，PowerShell 回退）。
#[cfg(windows)]
fn extract_zip(zip: &Path, dest: &Path) -> Result<(), String> {
    let args = vec![
        "-xf".to_string(),
        zip.to_string_lossy().into_owned(),
        "-C".to_string(),
        dest.to_string_lossy().into_owned(),
    ];
    let mut tar_cmd = std::process::Command::new("tar");
    tar_cmd.args(&args);
    processes::hide_console(&mut tar_cmd);
    let status = tar_cmd.status().map(|s| s.success());
    if matches!(status, Ok(true)) {
        return Ok(());
    }
    let ps_args = format!(
        "-NoProfile -Command \"Expand-Archive -LiteralPath '{}' -DestinationPath '{}' -Force\"",
        zip.to_string_lossy(),
        dest.to_string_lossy()
    );
    // PowerShell 回退：优先 pwsh（PowerShell 7，若已安装），否则系统自带 5.1。
    // 只做"有就用"，不强制安装/更新——两者对 Expand-Archive 能力相同。
    let mut last_err = String::new();
    for ps in ["pwsh", "powershell.exe"] {
        let mut cmd = std::process::Command::new(ps);
        cmd.args(["-NoProfile", "-Command", &ps_args]);
        processes::hide_console(&mut cmd);
        match cmd.status() {
            Ok(s) if s.success() => return Ok(()),
            Ok(_) => last_err = format!("{ps} 解压失败"),
            Err(e) => last_err = format!("启动 {ps} 失败：{e}"),
        }
    }
    Err(if last_err.is_empty() {
        "解压 Node.js 失败".into()
    } else {
        format!("解压 Node.js 失败（{last_err}）")
    })
}

/// macOS/Linux：tar.gz 解压（系统自带 tar）。
#[cfg(not(windows))]
fn extract_tar(archive: &Path, dest: &Path) -> Result<(), String> {
    let args = vec![
        "-xzf".to_string(),
        archive.to_string_lossy().into_owned(),
        "-C".to_string(),
        dest.to_string_lossy().into_owned(),
    ];
    let status = std::process::Command::new("tar")
        .args(&args)
        .status()
        .map_err(|e| format!("解压失败：{e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err("解压 Node.js 失败".into())
    }
}

// ---------- dsh 包安装 ----------

fn dsh_installed(config: &Config) -> bool {
    config.dsh_entry().exists()
}

/// 安装 dsh 官方 npm 包到应用数据目录（首次运行）。
pub(crate) fn ensure_dsh(app: &AppHandle, config: &Config, node_exe: &Path) -> Result<(), String> {
    if dsh_installed(config) {
        return Ok(());
    }
    emit_status(
        app,
        BootPhase::InstallingDsh,
        "正在安装 dsh（首次运行，需要联网）…",
        "",
    );
    std::fs::create_dir_all(config.dsh_dir()).map_err(|e| e.to_string())?;

    let npm_cli = node_exe
        .parent()
        .unwrap()
        .join("node_modules/npm/bin/npm-cli.js");
    if !npm_cli.exists() {
        return Err(format!("未找到 npm：{}", npm_cli.display()));
    }
    let args = vec![
        npm_cli.to_string_lossy().into_owned(),
        "install".into(),
        "--prefix".into(),
        config.dsh_dir().to_string_lossy().into_owned(),
        "@deepseek-ai/dsh".into(),
        "--dangerously-allow-all-scripts".into(),
        "--no-audit".into(),
        "--no-fund".into(),
    ];
    let envs = base_envs(node_exe, config);
    let npm_log = config.logs_dir().join("npm-install.log");
    let mut child =
        processes::spawn_process(node_exe, &args, &envs, Some(&config.root), Some(&npm_log))
            .map_err(|e| format!("运行 npm 失败：{e}"))?;
    // 安装进程也纳入守卫，应用退出时不会遗留 npm/node 后台进程。
    let _install_guard = processes::TreeGuard::from_child(&child);
    // 安装期间每秒汇报已用时（npm 非 TTY 时不输出进度，用计时替代）。
    let start = Instant::now();
    let code = loop {
        match child
            .try_wait()
            .map_err(|e| format!("等待 npm 失败：{e}"))?
        {
            Some(status) => break status.code().unwrap_or(-1),
            None => {
                let secs = start.elapsed().as_secs();
                emit_status(
                    app,
                    BootPhase::InstallingDsh,
                    &format!("正在安装 dsh（首次运行，需要联网）… 已用时 {secs}s"),
                    "",
                );
                std::thread::sleep(Duration::from_secs(1));
            }
        }
    };
    drop(child);
    if code != 0 {
        let tail = read_log_tail(&npm_log, 600);
        return Err(format!("安装 dsh 失败（npm 退出码 {code}）：\n{}", tail));
    }
    if !dsh_installed(config) {
        return Err("dsh 已安装，但未找到入口文件".into());
    }
    Ok(())
}

/// 读取日志文件尾部（供错误提示）。
fn read_log_tail(path: &Path, max_chars: usize) -> String {
    match std::fs::read_to_string(path) {
        Ok(text) => crate::util::truncate(&text, max_chars),
        Err(_) => String::new(),
    }
}

// ---------- 服务启动 ----------

/// 子进程基础环境：PATH 前置 node 目录、可选 DSH_HOME、npm 缓存落在应用根目录。
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
    if let Some(home) = &config.dsh_home {
        envs.push(("DSH_HOME", home.to_string_lossy().into_owned()));
    }
    // npm 缓存落在应用根目录内（避免写入用户级缓存目录带来的权限问题）。
    let cache = config.root.join("npm-cache");
    envs.push(("npm_config_cache", cache.to_string_lossy().into_owned()));
    envs
}

/// 隐藏窗口启动 `dsh web` 服务，返回 (pid, 进程树守卫)。
pub(crate) fn start_server(
    _app: &AppHandle,
    config: &Config,
    node_exe: &Path,
) -> Result<(u32, Option<TreeGuard>), String> {
    let args = vec![
        config.dsh_entry().to_string_lossy().into_owned(),
        "web".into(),
        "--port".into(),
        config.port.to_string(),
    ];
    let envs = base_envs(node_exe, config);
    let log = config.dsh_log();
    let child =
        processes::spawn_process(node_exe, &args, &envs, Some(&config.dsh_dir()), Some(&log))
            .map_err(|e| format!("启动 dsh 服务失败：{e}"))?;
    let pid = child.id();

    // 建立进程树守卫（Windows Job / Unix 进程组）；失败不致命，退出时另有兜底。
    let guard = processes::TreeGuard::from_child(&child);
    // child 句柄随函数结束关闭；进程由守卫/taskkill 管理。
    drop(child);
    Ok((pid, guard))
}
