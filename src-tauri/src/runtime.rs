//! 运行时安装与维护：Node 检测/便携安装、dsh npm 包安装、服务启动。

use std::fs::File;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use sha2::{Digest, Sha256};
use tauri::AppHandle;

use crate::app_state::{BootPhase, Config};
use crate::processes::{self, TreeGuard};
use crate::versions::{node_satisfies, parse_node_version};
use crate::{emit_status, emit_status_progress};

const NODEJS_INDEX: &str = "https://nodejs.org/dist/index.json";
/// npmmirror 国内镜像（阿里开源镜像，302 到 CDN）：官方直连失败时兜底，
/// 缓解 nodejs.org 在国内下载慢/超时。仅包下载用，校验与 index 仍走官方。
const NODE_MIRROR_BASE: &str = "https://npmmirror.com/mirrors/node";
const NPM_DIST_TAGS: &str = "https://registry.npmjs.org/-/package/@deepseek-ai/dsh/dist-tags";
/// npm 包自身的 dist-tags（查询 npm 最新版用于检查更新）。
const NPM_LATEST: &str = "https://registry.npmjs.org/-/package/npm/dist-tags";
/// @deepseek-ai/dsh 的完整包元数据（拿全部版本 + 发布时间，用于动态降级）。
const DSH_PACKAGE_META: &str = "https://registry.npmjs.org/@deepseek-ai%2fdsh";

/// 全局共享 HTTP 客户端：TLS 配置只构建一次（高频查询路径省初始化开销）。
static AGENT: std::sync::OnceLock<ureq::Agent> = std::sync::OnceLock::new();
const MAX_NODE_ARCHIVE_BYTES: u64 = 256 * 1024 * 1024;

/// 带超时的 HTTP 客户端（TLS 后端见 Cargo.toml 平台条件依赖：
/// Windows/macOS 用系统原生实现，Linux 用 rustls）。
/// ureq 3 将读超时拆分为接收响应与接收响应体两部分，两者均设 90s。
pub(crate) fn client() -> ureq::Agent {
    AGENT
        .get_or_init(|| {
            ureq::Agent::config_builder()
                .tls_config(crate::default_tls_config())
                .timeout_connect(Some(Duration::from_secs(15)))
                .timeout_recv_response(Some(Duration::from_secs(90)))
                .timeout_recv_body(Some(Duration::from_secs(90)))
                .build()
                .new_agent()
        })
        .clone()
}

/// 版本检查专用客户端：连接 5s、响应 8s——检查类请求（npm / Node LTS /
/// GitHub）应当快速失败并显示"暂无法获取版本信息"，而不是拖满 90s
/// 拖慢整个检查流程（大文件下载仍用 download_client）。
pub(crate) fn check_client() -> ureq::Agent {
    ureq::Agent::config_builder()
        .tls_config(crate::default_tls_config())
        .timeout_connect(Some(Duration::from_secs(5)))
        .timeout_recv_response(Some(Duration::from_secs(8)))
        .timeout_recv_body(Some(Duration::from_secs(8)))
        .build()
        .new_agent()
}

/// 大文件下载专用客户端：ureq 3 的 timeout_recv_body 是「响应头收完起、
/// body 全部收完的整体时限」（timings.rs 中 RecvBody 以上一阶段记录时刻
/// 为基准），并非单次读取空闲超时。常规 90s 会掐断慢网下的大归档下载，
/// 整体预算放宽到 1 小时；挂死连接最坏 1 小时后报错，安装进度可随时取消。
pub(crate) fn download_client() -> ureq::Agent {
    ureq::Agent::config_builder()
        .tls_config(crate::default_tls_config())
        .timeout_connect(Some(Duration::from_secs(15)))
        .timeout_recv_response(Some(Duration::from_secs(90)))
        .timeout_recv_body(Some(Duration::from_secs(3600)))
        .build()
        .new_agent()
}

/// 读取一个小 URL 到字符串（供版本检查等使用；短超时快速失败）。
fn get_text(url: &str) -> Result<String, String> {
    let resp = check_client().get(url).call().map_err(|e| {
        format!(
            "{}: {e}",
            crate::locale::text("网络请求失败", "Network request failed")
        )
    })?;
    let mut reader = resp.into_body().into_reader();
    let mut s = String::new();
    reader.read_to_string(&mut s).map_err(|e| {
        format!(
            "{}: {e}",
            crate::locale::text("读取响应失败", "Failed to read the response")
        )
    })?;
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
    let resp = check_client()
        .get(NODEJS_INDEX)
        .call()
        .or_else(|official_err| {
            // 官方 index.json 失败（国内网络常见）→ 兜底 npmmirror 镜像
            crate::logging::log(&format!(
                "runtime: Node 版本索引官方失败（{official_err}），改走 npmmirror 镜像"
            ));
            check_client()
                .get(&format!("{NODE_MIRROR_BASE}/index.json"))
                .call()
                .map_err(|mirror_err| {
                    format!(
                        "{}: 官方 {official_err}；镜像 {mirror_err}",
                        crate::locale::text(
                            "获取 Node 版本信息失败",
                            "Failed to retrieve Node.js version information"
                        )
                    )
                })
        })?;
    let json: serde_json::Value = resp.into_body().read_json().map_err(|e| {
        format!(
            "{}: {e}",
            crate::locale::text(
                "解析 Node 版本信息失败",
                "Failed to parse Node.js version information"
            )
        )
    })?;
    let arr = json.as_array().ok_or_else(|| {
        crate::locale::text(
            "Node 版本列表格式错误",
            "The Node.js version list is invalid",
        )
    })?;
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
    Err(crate::locale::text(
        "未找到 Node.js LTS 版本",
        "No Node.js LTS release was found",
    )
    .into())
}

/// 查询 npm 官方 `@deepseek-ai/dsh` 的最新版本。
/// dsh 更新通道：latest（稳定推荐）/ next（预览尝鲜）。
#[derive(Clone, Copy, PartialEq)]
pub(crate) enum DshChannel {
    Latest,
    Next,
}

impl DshChannel {
    pub(crate) fn from_config(config: &crate::app_state::Config) -> Self {
        if config.dsh_update_channel == "next" {
            Self::Next
        } else {
            Self::Latest
        }
    }

    fn dist_tag(self) -> &'static str {
        match self {
            Self::Latest => "latest",
            Self::Next => "next",
        }
    }
}

pub(crate) fn npm_latest_dsh_version(channel: DshChannel) -> Result<String, String> {
    let text = get_text(NPM_DIST_TAGS)?;
    let json: serde_json::Value = serde_json::from_str(&text).map_err(|e| {
        format!(
            "{}: {e}",
            crate::locale::text("解析失败", "Failed to parse the response")
        )
    })?;
    let tag = channel.dist_tag();
    json.get(tag)
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| {
            let zh = format!("响应中没有 {tag} 字段");
            let en = format!("The response has no {tag} field");
            if crate::locale::is_chinese() {
                zh
            } else {
                en
            }
        })
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

/// 查询 npm 包自身最新版（registry dist-tags 的 latest），用于检查更新。
pub(crate) fn npm_latest_version() -> Result<String, String> {
    let text = get_text(NPM_LATEST)?;
    let json: serde_json::Value = serde_json::from_str(&text).map_err(|e| {
        format!(
            "{}: {e}",
            crate::locale::text("解析失败", "Failed to parse the response")
        )
    })?;
    json.get("latest")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| {
            let zh = "响应中没有 latest 字段".to_string();
            let en = "The response has no latest field".to_string();
            if crate::locale::is_chinese() {
                zh
            } else {
                en
            }
        })
}

/// dsh 的版本降级链：按发布时间从新到旧取最新 N 个版本（不含 latest 重复）。
/// latest 装不上时依次尝试这些版本，实现“动态往下降”而非写死两个版本。
/// 查询失败时返回空列表（安装流程会退化为仅试 latest）。
pub(crate) fn dsh_version_chain(limit: usize) -> Vec<String> {
    let text = match get_text(DSH_PACKAGE_META) {
        Ok(t) => t,
        Err(_) => return Vec::new(),
    };
    let json: serde_json::Value = match serde_json::from_str(&text) {
        Ok(j) => j,
        Err(_) => return Vec::new(),
    };
    // time 字段：{ "0.1.0-rc.1": "...", "created": "...", "modified": "..." }
    let time = match json.get("time").and_then(|t| t.as_object()) {
        Some(t) => t,
        None => return Vec::new(),
    };
    let mut versions: Vec<(&String, &serde_json::Value)> = time.iter().collect();
    // 排除 created/modified 等元数据键（版本键无时间语义，其余键是字符串时间）
    versions.retain(|(k, v)| v.is_string() && *k != "created" && *k != "modified");
    // 按时间字符串倒序（ISO8601 字典序 == 时间序）
    versions.sort_by(|a, b| b.1.as_str().unwrap_or("").cmp(a.1.as_str().unwrap_or("")));
    versions
        .iter()
        .take(limit)
        .map(|(v, _)| v.to_string())
        .collect()
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

/// 取当前使用 Node 自带 npm 的裸版本号（如 12.0.2）。
/// 便携优先、系统 Node 兜底（与 current_node_version 同一选择逻辑），
/// 返回 None 表示拿不到（无 Node / npm-cli 缺失 / 运行失败）。
pub(crate) fn npm_version(config: &Config) -> Option<String> {
    let managed = config.node_exe();
    let node_exe = if managed.exists() {
        managed
    } else {
        find_system_node()?
    };
    let npm_cli = node_exe.parent()?.join("node_modules/npm/bin/npm-cli.js");
    if !npm_cli.exists() {
        return None;
    }
    let mut cmd = std::process::Command::new(&node_exe);
    cmd.arg(&npm_cli).arg("--version");
    processes::hide_console(&mut cmd);
    let out = cmd.output().ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if text.is_empty() {
        None
    } else {
        Some(text)
    }
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

/// 确保 Node 可用：便携 Node 与系统 Node 都先执行版本探测；
/// 便携运行时损坏或版本不满足要求时自动清理，再选择合格的系统 Node 或重新安装。
pub(crate) fn ensure_node(app: &AppHandle, config: &Config) -> Result<PathBuf, String> {
    let managed = config.node_exe();
    let node_exe = if managed.exists() {
        if node_version(&managed).is_some_and(|(maj, min, _)| node_satisfies(maj, min)) {
            managed
        } else {
            crate::logging::log("runtime: 便携 Node 损坏或版本过旧，准备重新选择运行时");
            std::fs::remove_dir_all(config.node_dir()).map_err(|e| {
                crate::locale::owned(
                    format!("清理损坏的 Node.js 运行时失败：{e}"),
                    format!("Failed to remove the damaged Node.js runtime: {e}"),
                )
            })?;
            if let Some(system) = find_system_node() {
                if let Some((maj, min, _)) = node_version(&system) {
                    if node_satisfies(maj, min) {
                        system
                    } else {
                        install_portable_node(app, config)?
                    }
                } else {
                    install_portable_node(app, config)?
                }
            } else {
                install_portable_node(app, config)?
            }
        }
    } else if let Some(system) = find_system_node() {
        if let Some((maj, min, _)) = node_version(&system) {
            if node_satisfies(maj, min) {
                system
            } else {
                install_portable_node(app, config)?
            }
        } else {
            install_portable_node(app, config)?
        }
    } else {
        install_portable_node(app, config)?
    };

    // Node v24 官方自带 npm 11，其 idealTree 解析 dsh 的 528 包依赖树时会在
    // Windows 卡死（实测 placeDep ~550 行后停滞、零 tarball、reify 不开始）。
    // npm 12 无此问题。这里对「任何拿到 Node 的路径」统一升级——此前只在
    // install_portable_node 内部做，导致已装好便携 Node 的机器（ensure_node
    // 直接返回）永远不升级，dsh 照旧卡死。
    // 仅升级便携 Node 的 npm；系统 Node 由 upgrade 内部跳过（归系统管理）。
    upgrade_portable_npm(app, config, false)?;

    Ok(node_exe)
}

/// 下载并安装便携版 Node 到应用数据目录。
pub(crate) fn install_portable_node(app: &AppHandle, config: &Config) -> Result<PathBuf, String> {
    emit_status(
        app,
        BootPhase::InstallingNode,
        crate::locale::text("正在下载 Node.js…", "Downloading Node.js…"),
        "",
    );
    let version = latest_lts_cached(true)?; // 形如 v24.19.0；安装前强制刷新缓存

    // 按平台选择官方包（Windows zip / macOS、Linux tar.gz）
    let (dir_name, url, mirror_url, is_zip) = node_package(&version);
    let node_dir = config.node_dir();
    if node_dir.exists() {
        std::fs::remove_dir_all(&node_dir).map_err(|e| {
            crate::locale::owned(
                format!("清理旧 Node.js 安装目录失败：{e}"),
                format!("Failed to remove the previous Node.js directory: {e}"),
            )
        })?;
    }
    let archive_name = if is_zip {
        "node-download.zip"
    } else {
        "node-download.tar.gz"
    };
    let official_archive_name = url.rsplit('/').next().ok_or_else(|| {
        crate::locale::text(
            "Node.js 下载地址格式错误",
            "The Node.js download URL is invalid",
        )
    })?;
    let expected_sha256 = node_archive_sha256(&version, official_archive_name)?;
    let archive_path = config.root.join(archive_name);
    std::fs::create_dir_all(&node_dir).map_err(|e| e.to_string())?;

    let download_message = if crate::locale::is_chinese() {
        format!("正在下载 Node.js {version}…")
    } else {
        format!("Downloading Node.js {version}…")
    };
    emit_status(app, BootPhase::InstallingNode, &download_message, "");
    let resp = download_client().get(&url).call().or_else(|official_err| {
        // 官方直连失败（国内网络常见）→ 兜底 npmmirror 国内镜像
        crate::logging::log(&format!(
            "runtime: Node 官方下载失败（{official_err}），改走 npmmirror 国内镜像"
        ));
        download_client()
            .get(&mirror_url)
            .call()
            .map_err(|mirror_err| {
                crate::locale::owned(
                    format!("下载 Node.js 失败：官方 {official_err}；镜像 {mirror_err}"),
                    format!(
                        "Failed to download Node.js: official {official_err}; mirror {mirror_err}"
                    ),
                )
            })
    })?;
    let total: u64 = resp
        .headers()
        .get("content-length")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    if total > MAX_NODE_ARCHIVE_BYTES {
        return Err(crate::locale::text(
            "Node.js 下载文件超过 256 MB 安全上限",
            "The Node.js download exceeds the 256 MB safety limit",
        )
        .into());
    }
    let mut reader = resp.into_body().into_reader();
    let mut file = std::fs::File::create(&archive_path).map_err(|e| {
        crate::locale::owned(
            format!("写入临时文件失败：{e}"),
            format!("Failed to create the temporary download file: {e}"),
        )
    })?;
    let mut buf = [0u8; 65536];
    let mut done: u64 = 0;
    let mut last_pct: i64 = -1;
    let mut last_emit = std::time::Instant::now() - Duration::from_secs(1);
    loop {
        let n = match reader.read(&mut buf) {
            Ok(n) => n,
            Err(e) => {
                // 清理半截下载文件（文件句柄先释放，Windows 下才能删除）
                drop(file);
                let _ = std::fs::remove_file(&archive_path);
                return Err(crate::locale::owned(
                    format!("下载 Node.js 失败：{e}"),
                    format!("Failed to download Node.js: {e}"),
                ));
            }
        };
        if n == 0 {
            break;
        }
        done += n as u64;
        if done > MAX_NODE_ARCHIVE_BYTES {
            drop(file);
            let _ = std::fs::remove_file(&archive_path);
            return Err(crate::locale::text(
                "Node.js 下载文件超过 256 MB 安全上限",
                "The Node.js download exceeds the 256 MB safety limit",
            )
            .into());
        }
        if let Err(e) = file.write_all(&buf[..n]) {
            drop(file);
            let _ = std::fs::remove_file(&archive_path);
            return Err(crate::locale::owned(
                format!("写入临时文件失败：{e}"),
                format!("Failed to write the temporary download file: {e}"),
            ));
        }
        if total > 0 {
            // 节流：仅跨整数百分点且距上次广播 ≥200ms 才发 IPC，避免数百次高频事件
            let pct = (((done as f64 / total as f64) * 100.0) as i64).min(100);
            if pct > last_pct && last_emit.elapsed() >= Duration::from_millis(200) {
                last_pct = pct;
                last_emit = std::time::Instant::now();
                let message = if crate::locale::is_chinese() {
                    format!("正在下载 Node.js {version}… {pct}%")
                } else {
                    format!("Downloading Node.js {version}… {pct}%")
                };
                emit_status_progress(
                    app,
                    BootPhase::InstallingNode,
                    &message,
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

    let actual_sha256 = sha256_file(&archive_path)?;
    if actual_sha256 != expected_sha256 {
        let _ = std::fs::remove_file(&archive_path);
        return Err(crate::locale::owned(
            format!("Node.js 下载文件校验失败：期望 {expected_sha256}，实际 {actual_sha256}"),
            format!(
                "Node.js download verification failed: expected {expected_sha256}, got {actual_sha256}"
            ),
        ));
    }

    emit_status(
        app,
        BootPhase::InstallingNode,
        crate::locale::text("正在解压 Node.js…", "Extracting Node.js…"),
        "",
    );
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
        return Err(crate::locale::text(
            "Node.js 解压完成，但找不到 node 可执行文件",
            "The Node.js executable was not found after extraction",
        )
        .into());
    }

    Ok(config.node_exe())
}

/// 升级便携 Node 自带的 npm 到 12（Node v24 官方包自带 npm 11，其 idealTree
/// 解析 dsh 依赖树会卡死）。失败一律降级沿用自带版，不阻断 Node 安装——
/// dsh 安装阶段会因 npm 11 卡死而在超时后报出明确错误。
/// 整体限时 150s：升级这步走网络，run_capture 无限阻塞会卡死整个 boot，
/// 必须给 npm 自身的 fetch-timeout + 外层超时兜底。
/// 升级 Node 自带 npm 到 12。strict=false（启动自动升级）：失败降级沿用
/// 自带版、不阻断；strict=true（检查更新手动触发）：失败返回具体错误展示给
/// 用户。走官方 registry，失败切 npmmirror 兜底；外层 150s 超时。
pub(crate) fn upgrade_portable_npm(
    app: &AppHandle,
    config: &Config,
    strict: bool,
) -> Result<(), String> {
    let node_exe = config.node_exe();
    // 仅升级便携 Node 的 npm。系统 Node 的 npm 归系统管理（Program Files
    // 写入需管理员权限、且不该由便携外壳污染系统环境），strict 模式明确
    // 拒绝而非静默降级——启动自动升级（非 strict）对系统 Node 本就不触发
    // （install_portable_node 只在装便携 Node 后调用）。
    if !node_exe.exists() {
        if strict {
            return Err(crate::locale::text(
                "当前使用系统安装的 Node.js，npm 由其管理，请在系统环境升级",
                "The system-installed Node.js manages npm; upgrade it in the system environment",
            )
            .into());
        }
        return Ok(());
    }
    let node_dir = node_exe.parent().ok_or_else(|| {
        crate::locale::text(
            "Node.js 可执行文件路径无父目录",
            "The Node.js executable path has no parent directory",
        )
    })?;
    let npm_cli = node_dir.join("node_modules/npm/bin/npm-cli.js");
    if !npm_cli.exists() {
        let msg = crate::locale::text("未找到 npm", "npm was not found");
        if strict {
            return Err(msg.into());
        }
        crate::logging::log(&format!("runtime: {msg}，沿用自带版"));
        return Ok(());
    }
    // 已是 12+ 则跳过（ensure_node 每次启动都会调用，不能重复升级联网）
    let mut version_cmd = std::process::Command::new(&node_exe);
    version_cmd.arg(&npm_cli).arg("--version");
    processes::hide_console(&mut version_cmd);
    let cur = version_cmd
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string());
    if let Some(v) = cur {
        if let Some(major) = v.split('.').next().and_then(|s| s.parse::<u32>().ok()) {
            if major >= 12 {
                return Ok(());
            }
        }
    }
    emit_status(
        app,
        BootPhase::InstallingNode,
        crate::locale::text("正在升级 npm…", "Upgrading npm…"),
        "",
    );
    let prefix = node_dir.to_string_lossy().into_owned();
    // 官方 registry 优先，失败再 npmmirror（与 dsh 安装一致，国内网络兜底）
    for registry in [
        "https://registry.npmjs.org",
        "https://registry.npmmirror.com",
    ] {
        let args = vec![
            npm_cli.to_string_lossy().into_owned(),
            "install".to_string(),
            "--global".to_string(),
            "--prefix".to_string(),
            prefix.clone(),
            "npm@12".to_string(),
            "--no-audit".to_string(),
            "--no-fund".to_string(),
            // 单请求 60s 超时——升级只有 1 个包，60s 足够，挂起时快速失败
            "--fetch-timeout=60000".to_string(),
            "--registry".to_string(),
            registry.to_string(),
        ];
        let envs = base_envs(&node_exe, config);
        let mut child =
            match processes::spawn_process(&node_exe, &args, &envs, Some(&config.root), None) {
                Ok(c) => c,
                Err(e) => {
                    let msg = format!("运行 npm 失败：{e}");
                    if strict {
                        return Err(msg);
                    }
                    crate::logging::log(&format!("runtime: 升级 npm 启动失败：{e}，沿用自带版"));
                    return Ok(());
                }
            };
        let _guard = processes::TreeGuard::from_child(&child);
        let deadline = std::time::Instant::now() + Duration::from_secs(150);
        let code = loop {
            match child.try_wait() {
                Ok(Some(status)) => break status.code().unwrap_or(-1),
                Ok(None) => {
                    if std::time::Instant::now() > deadline {
                        // 超时先杀进程树（strict/非 strict 都杀，避免泄漏）
                        processes::kill_tree(child.id());
                        if strict {
                            return Err(crate::locale::text(
                                "升级 npm 超时（150s）",
                                "npm upgrade timed out (150s)",
                            )
                            .into());
                        }
                        crate::logging::log("runtime: 升级 npm 超时（150s），沿用自带版");
                        return Ok(());
                    }
                    std::thread::sleep(Duration::from_millis(200));
                }
                Err(e) => {
                    let msg = format!("等待 npm 失败：{e}");
                    if strict {
                        return Err(msg);
                    }
                    crate::logging::log(&format!("runtime: {msg}，沿用自带版"));
                    return Ok(());
                }
            }
        };
        drop(child);
        if code == 0 {
            crate::logging::log("runtime: npm 已升级到 12");
            return Ok(());
        }
        crate::logging::log(&format!(
            "runtime: 升级 npm 到 12 失败（{registry} 退出码 {code}），尝试兜底源"
        ));
    }
    if strict {
        return Err(crate::locale::text(
            "升级 npm 失败（官方源与镜像均失败）",
            "Failed to upgrade npm (both the default registry and mirror failed)",
        )
        .into());
    }
    crate::logging::log("runtime: 升级 npm 到 12 失败，沿用自带版");
    Ok(())
}

/// 当前平台的 Node 官方包：返回（包目录名、官方下载 URL、国内镜像 URL、是否 zip）。
/// 未覆盖的平台会因函数无返回值而在编译期报错，避免静默下载错误包。
fn node_package(version: &str) -> (String, String, String, bool) {
    #[cfg(target_os = "windows")]
    {
        let dir = format!("node-{version}-win-x64");
        (
            dir.clone(),
            format!("https://nodejs.org/dist/{version}/{dir}.zip"),
            format!("{NODE_MIRROR_BASE}/{version}/{dir}.zip"),
            true,
        )
    }
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    {
        let dir = format!("node-{version}-darwin-arm64");
        (
            dir.clone(),
            format!("https://nodejs.org/dist/{version}/{dir}.tar.gz"),
            format!("{NODE_MIRROR_BASE}/{version}/{dir}.tar.gz"),
            false,
        )
    }
    #[cfg(all(target_os = "macos", target_arch = "x86_64"))]
    {
        let dir = format!("node-{version}-darwin-x64");
        (
            dir.clone(),
            format!("https://nodejs.org/dist/{version}/{dir}.tar.gz"),
            format!("{NODE_MIRROR_BASE}/{version}/{dir}.tar.gz"),
            false,
        )
    }
    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    {
        let dir = format!("node-{version}-linux-x64");
        (
            dir.clone(),
            format!("https://nodejs.org/dist/{version}/{dir}.tar.gz"),
            format!("{NODE_MIRROR_BASE}/{version}/{dir}.tar.gz"),
            false,
        )
    }
    #[cfg(all(target_os = "linux", target_arch = "aarch64"))]
    {
        let dir = format!("node-{version}-linux-arm64");
        (
            dir.clone(),
            format!("https://nodejs.org/dist/{version}/{dir}.tar.gz"),
            format!("{NODE_MIRROR_BASE}/{version}/{dir}.tar.gz"),
            false,
        )
    }
}

/// 从 Node.js 官方校验清单中取得目标归档的 SHA-256。
fn node_archive_sha256(version: &str, archive_name: &str) -> Result<String, String> {
    let checksums_url = format!("https://nodejs.org/dist/{version}/SHASUMS256.txt");
    let checksums = get_text(&checksums_url).map_err(|e| {
        crate::locale::owned(
            format!("获取 Node.js 校验信息失败：{e}"),
            format!("Failed to retrieve the Node.js checksum list: {e}"),
        )
    })?;
    parse_node_sha256(&checksums, archive_name).ok_or_else(|| {
        crate::locale::owned(
            format!("Node.js 官方校验列表中未找到 {archive_name}"),
            format!("{archive_name} was not found in the official Node.js checksum list"),
        )
    })
}

fn parse_node_sha256(checksums: &str, archive_name: &str) -> Option<String> {
    checksums.lines().find_map(|line| {
        let mut parts = line.split_whitespace();
        let hash = parts.next()?;
        let file_name = parts.next()?.trim_start_matches('*');
        (file_name == archive_name
            && hash.len() == 64
            && hash.bytes().all(|byte| byte.is_ascii_hexdigit()))
        .then(|| hash.to_ascii_lowercase())
    })
}

fn sha256_file(path: &Path) -> Result<String, String> {
    let mut file = File::open(path).map_err(|e| {
        crate::locale::owned(
            format!("读取下载文件失败：{e}"),
            format!("Failed to read the downloaded file: {e}"),
        )
    })?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let count = file.read(&mut buffer).map_err(|e| {
            crate::locale::owned(
                format!("校验下载文件失败：{e}"),
                format!("Failed to verify the downloaded file: {e}"),
            )
        })?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
    }
    Ok(hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect())
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
    let ps_script = "Expand-Archive -LiteralPath $env:DSHD_NODE_ARCHIVE -DestinationPath $env:DSHD_NODE_DESTINATION -Force";
    // PowerShell 回退：优先 pwsh（PowerShell 7，若已安装），否则系统自带 5.1。
    // 仅检测使用，不强制安装或更新——两者对 Expand-Archive 能力相同。
    let mut last_err = String::new();
    // pwsh 用绝对路径优先（应用启动后才安装的 pwsh 不在 PATH 快照里）
    for (name, mut cmd) in [
        ("pwsh", processes::pwsh_command()),
        (
            "powershell.exe",
            std::process::Command::new("powershell.exe"),
        ),
    ] {
        cmd.args([
            "-NoLogo",
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            ps_script,
        ]);
        cmd.env("DSHD_NODE_ARCHIVE", zip);
        cmd.env("DSHD_NODE_DESTINATION", dest);
        processes::hide_console(&mut cmd);
        match cmd.status() {
            Ok(s) if s.success() => return Ok(()),
            Ok(_) => {
                last_err = crate::locale::owned(
                    format!("{name} 解压失败"),
                    format!("{name} could not extract the archive"),
                )
            }
            Err(e) => {
                last_err = crate::locale::owned(
                    format!("启动 {name} 失败：{e}"),
                    format!("Failed to start {name}: {e}"),
                )
            }
        }
    }
    Err(if last_err.is_empty() {
        crate::locale::text("解压 Node.js 失败", "Failed to extract Node.js").into()
    } else {
        crate::locale::owned(
            format!("解压 Node.js 失败（{last_err}）"),
            format!("Failed to extract Node.js ({last_err})"),
        )
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
        .map_err(|e| {
            crate::locale::owned(format!("解压失败：{e}"), format!("Extraction failed: {e}"))
        })?;
    if status.success() {
        Ok(())
    } else {
        Err(crate::locale::text("解压 Node.js 失败", "Failed to extract Node.js").into())
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
    // 清理前几轮失败残留的半截安装：不完整的 node_modules / package-lock.json
    // 会让 npm reify 阶段的树对比在坏状态上卡死（实测：manifest 全部 fetch 完
    // 成后、tarball 未开始前卡住，--fetch-timeout 不生效）。每次干净重装。
    let node_modules = config.dsh_dir().join("node_modules");
    let pkg_lock = config.dsh_dir().join("package-lock.json");
    if node_modules.exists() && std::fs::remove_dir_all(&node_modules).is_err() {
        crate::logging::log("runtime: 清理残留 node_modules 失败，继续尝试安装");
    }
    if pkg_lock.exists() {
        let _ = std::fs::remove_file(&pkg_lock);
    }
    emit_status(
        app,
        BootPhase::InstallingDsh,
        crate::locale::text(
            "正在安装 dsh（需要联网）…",
            "Installing dsh (internet required)…",
        ),
        "",
    );
    std::fs::create_dir_all(config.dsh_dir()).map_err(|e| e.to_string())?;

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
        return Err(crate::locale::owned(
            format!("未找到 npm：{}", npm_cli.display()),
            format!("npm was not found: {}", npm_cli.display()),
        ));
    }

    // 先官方 registry；失败（含 npm 自身超时）再走 npmmirror 国内镜像兜底——
    // 新设备 + 国内网络下 registry.npmjs.org 直连经常卡在 fetch 阶段，是
    // “dsh 数分钟装不回来”的主因。checksum/版本查询仍走官方语义不变。
    //
    // 版本降级：latest 可能处于上游发版过渡期（实测 0.1.1-rc.1/rc.2 的 62 个
    // 依赖全声明 ^0.1.1-rc.1 会互相匹配到新 rc，npm placeDep 解析组合爆炸卡
    // 死，npm 11/12 都中招）。latest 装不上时按发布时间倒序自动降级（动态
    // 取 registry 历史版本，最多试 5 个），并广播降级状态给启动页。
    let mut install_targets: Vec<String> = vec!["@deepseek-ai/dsh".to_string()];
    // 动态降级链：registry 按发布时间倒序的历史版本（跳过第 1 个 = latest
    // 本身，它已作为 install_targets[0] 试过），最多再试 4 个旧版本
    for ver in dsh_version_chain(5).into_iter().skip(1).take(4) {
        install_targets.push(format!("@deepseek-ai/dsh@{ver}"));
    }
    let base_args = vec![
        "install".into(),
        "--prefix".into(),
        config.dsh_dir().to_string_lossy().into_owned(),
        // target 由内层循环填充（占位）
        String::new(),
        "--dangerously-allow-all-scripts".into(),
        "--no-audit".into(),
        "--no-fund".into(),
        // --foreground-scripts：install scripts 的输出直接进日志（默认被 npm
        // 吞掉，卡死在哪个 script 完全看不到）；也是定位 node_repl 卡点的
        // 唯一可靠手段
        "--foreground-scripts".into(),
        // 单请求 2 分钟超时：npm 默认 fetch timeout 5 分钟，卡死时等太久
        "--fetch-timeout=120000".into(),
        "--fetch-retries=2".into(),
        // --prefer-online：不用缓存里的旧/半截包体（前几轮失败可能留下损坏
        // tarball），强制重新拉取；manifest 仍允许 revalidate 快速通过
        "--prefer-online".into(),
        // --verbose：非 TTY 下也会把 fetch/reify 各步写进日志，
        // 否则卡死时拿不到任何定位信息（日志只有最终 added 行）
        "--verbose".into(),
    ];
    let mut last_error = String::new();
    'versions: for (vi, target) in install_targets.iter().enumerate() {
        // 降级时广播状态：让启动页看得到在尝试哪个版本，而非一直“安装 dsh”
        if vi > 0 {
            let msg = crate::locale::owned(
                format!("最新版装不上，自动尝试 {target}…"),
                format!("The latest version failed; trying {target}…"),
            );
            emit_status(app, BootPhase::InstallingDsh, &msg, "");
        }
        for (attempt, registry) in [None, Some("https://registry.npmmirror.com")]
            .into_iter()
            .enumerate()
        {
            let mut args = base_args.clone();
            // base_args[3] 是 target 占位，填入当前版本
            args[3] = (*target).to_string();
            // 每轮独立日志文件：append 模式会让两轮输出混在同一个文件、
            // 尾部误读上一轮内容；分开写才能拿到本轮真实的失败尾部
            let attempt_log = if vi == 0 {
                if attempt == 0 {
                    "npm-install.log"
                } else {
                    "npm-install-mirror.log"
                }
            } else {
                if attempt == 0 {
                    "npm-install-v2.log"
                } else {
                    "npm-install-v2-mirror.log"
                }
            };
            if let Some(reg) = registry {
                args.push("--registry".into());
                args.push(reg.into());
            }
            let result = run_npm_install_with_progress(
                app,
                config,
                node_exe,
                &npm_cli,
                &args,
                &config.logs_dir().join(attempt_log),
                if vi == 0 { 180 } else { 90 },
            );
            match result {
                Ok(()) => {
                    // 装的是降级版本时，明确记日志便于排查
                    if vi > 0 {
                        crate::logging::log(&format!(
                            "runtime: dsh latest 装不上，已自动降级到 {target}"
                        ));
                    }
                    return Ok(());
                }
                Err(e) => {
                    last_error = e.clone();
                    crate::logging::log(&format!(
                        "runtime: install {target} ({registry:?}) 失败：{e}"
                    ));
                    if attempt == 0 {
                        continue; // 官方失败，同版本切镜像再试
                    }
                    // 镜像也失败：降级到下一个版本
                    continue 'versions;
                }
            }
        }
    }
    // 所有版本 + 两个 registry 都试遍仍失败：报最终错误
    Err(crate::locale::owned(
        format!("安装 dsh 失败（latest 与降级版本均无法安装）：{last_error}",),
        format!("Failed to install dsh (latest and fallback versions all failed): {last_error}",),
    ))
}

/// 跑一次 npm install，轮询 npm 缓存目录（_cacache）累计字节数作为真实下载
/// 进度（npm 非 TTY 无中间输出，缓存增长就是包下载量的真实反映），每秒汇报
/// “已下载 X MB”。返回 Ok(()) 或带日志尾部的错误信息。
fn run_npm_install_with_progress(
    app: &AppHandle,
    config: &Config,
    node_exe: &Path,
    npm_cli: &Path,
    args: &[String],
    log_path: &Path,
    no_progress_secs: u64,
) -> Result<(), String> {
    let envs = base_envs(node_exe, config);
    // node 跑 npm-cli.js：首参必须是 npm-cli 路径
    let mut spawn_args = vec![npm_cli.to_string_lossy().into_owned()];
    spawn_args.extend(args.iter().cloned());
    let mut child = processes::spawn_process(
        node_exe,
        &spawn_args,
        &envs,
        Some(&config.root),
        Some(log_path),
    )
    .map_err(|e| {
        crate::locale::owned(
            format!("运行 npm 失败：{e}"),
            format!("Failed to run npm: {e}"),
        )
    })?;
    // 安装进程也纳入守卫，应用退出时不会遗留 npm/node 后台进程。
    let _install_guard = processes::TreeGuard::from_child(&child);

    let cache_dir = config.root.join("npm-cache").join("_cacache");
    // 基线：安装开始前的缓存总量。之前直接显示缓存总量，导致“已下载 250MB”
    // 起步（历史安装累积的包），进度严重误导——应显示本次安装的增量。
    let cache_base_mb = dir_size_mb(&cache_dir);
    let start = Instant::now();
    // 无进展超时：npm 卡在非 fetch 阶段（native build / 解压 / 解析）时
    // 不退出也不长缓存，3 分钟毫无进展即判定卡死并 kill。
    let no_progress_timeout = Duration::from_secs(no_progress_secs);
    let mut last_progress = Instant::now();
    let mut last_progress_mb: u64 = 0;
    let mut last_reported_sec: u64 = u64::MAX;
    let code = loop {
        match child.try_wait().map_err(|e| {
            crate::locale::owned(
                format!("等待 npm 失败：{e}"),
                format!("Failed while waiting for npm: {e}"),
            )
        })? {
            Some(status) => break status.code().unwrap_or(-1),
            None => {
                let secs = start.elapsed().as_secs();
                let mb = if secs != last_reported_sec {
                    // 每秒只做一次全量缓存目录扫描（数万小文件递归开销不小，
                    // 不能每 0.5s 都扫）
                    dir_size_mb(&cache_dir)
                } else {
                    last_progress_mb
                };
                // 缓存有增长 = 有进展：刷新无进展计时器
                if mb != last_progress_mb {
                    last_progress_mb = mb;
                    last_progress = Instant::now();
                } else if last_progress.elapsed() > no_progress_timeout {
                    // 卡死：先杀进程树并等其真正退出（taskkill 是异步的，
                    // 不等待的话立读日志会拿到空文件/半截内容），再读日志报错。
                    crate::logging::log(&format!(
                        "runtime: npm install 超过 {}s 无进展，判定卡死并终止",
                        no_progress_timeout.as_secs()
                    ));
                    processes::kill_tree(child.id());
                    // taskkill 是异步 fire-and-forget，必须等进程树真正退出才能
                    // 读到完整日志；带 10s 兜底，防止 taskkill 失败导致永久阻塞
                    let mut waited = Duration::from_secs(0);
                    loop {
                        if child.try_wait().ok().flatten().is_some() {
                            break;
                        }
                        if waited >= Duration::from_secs(10) {
                            crate::logging::log("runtime: npm install kill 后 10s 未退出");
                            break;
                        }
                        std::thread::sleep(Duration::from_millis(200));
                        waited += Duration::from_millis(200);
                    }
                    let tail = npm_failure_tail(config, log_path, 2000);
                    return Err(crate::locale::owned(
                        format!("安装超时（{secs}s 无进展）：\n{}", tail),
                        format!("Install timed out (no progress for {secs}s):\n{}", tail),
                    ));
                }
                // 进度按秒固定刷新（已用时持续跳动），不依赖 mb 是否变化——
                // 否则 native build / 解压等“不下载”阶段 UI 会冻结在旧秒数。
                if secs != last_reported_sec {
                    last_reported_sec = secs;
                    // 本次安装增量（缓存总量减去基线），避免把历史缓存的
                    // 250MB 误报成“本次已下载”
                    let downloaded = mb.saturating_sub(cache_base_mb);
                    let detail = if downloaded > 0 {
                        if crate::locale::is_chinese() {
                            format!("已下载约 {downloaded} MB · 已用时 {secs}s")
                        } else {
                            format!("~{downloaded} MB downloaded · {secs}s elapsed")
                        }
                    } else if crate::locale::is_chinese() {
                        format!("正在下载依赖… 已用时 {secs}s")
                    } else {
                        format!("Fetching dependencies… {secs}s elapsed")
                    };
                    emit_status(
                        app,
                        BootPhase::InstallingDsh,
                        crate::locale::text("正在安装 dsh…", "Installing dsh…"),
                        &detail,
                    );
                }
                std::thread::sleep(Duration::from_millis(500));
            }
        }
    };
    drop(child);
    if code != 0 {
        let tail = npm_failure_tail(config, log_path, 2000);
        return Err(crate::locale::owned(
            format!("npm 退出码 {code}：\n{}", tail),
            format!("npm exit code {code}:\n{}", tail),
        ));
    }
    if !dsh_installed(config) {
        return Err(crate::locale::text(
            "dsh 已安装，但找不到入口文件",
            "dsh was installed, but its entry file was not found",
        )
        .into());
    }
    Ok(())
}

/// 目录累计大小（MB，整体向上取整到 1MB，避免 0/1 抖动）。
fn dir_size_mb(dir: &Path) -> u64 {
    dir_size_bytes(dir).div_ceil(1024 * 1024)
}

/// 目录累计字节数（递归，含子目录；不跟随符号链接）。
fn dir_size_bytes(dir: &Path) -> u64 {
    let mut total: u64 = 0;
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                total += dir_size_bytes(&path);
            } else if let Ok(meta) = entry.metadata() {
                total += meta.len();
            }
        }
    }
    total
}

/// 读取日志文件尾部（供错误提示）。真正从文件末尾往前读，而不是把
/// 整个文件 load 进内存——npm --verbose 日志可达几十 MB，且卡死时关键
/// 信息在末尾（最后卡在哪个包/fetch），读头部没有诊断价值。
fn read_log_tail(path: &Path, max_chars: usize) -> String {
    use std::io::{Read, Seek, SeekFrom};
    let mut file = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(_) => return String::new(),
    };
    let file_len = match file.metadata() {
        Ok(m) => m.len(),
        Err(_) => return String::new(),
    };
    if file_len == 0 {
        return String::new();
    }
    // UTF-8 最坏 4 字节/字符，留 4×max_chars 再往前多读 3 字节避免切断多字节字符
    let read_start = file_len.saturating_sub((max_chars as u64) * 4 + 3);
    if file.seek(SeekFrom::Start(read_start)).is_err() {
        return String::new();
    }
    let mut buf = Vec::new();
    if file.read_to_end(&mut buf).is_err() {
        return String::new();
    }
    let text = String::from_utf8_lossy(&buf);
    // 可能从多字节字符中间切，丢弃首个不完整字符（from_utf8_lossy 已用 � 占位，
    // 这里按字符数截取末尾 max_chars 即可，开头残尾自然被跳过）
    let chars: Vec<char> = text.chars().collect();
    if chars.len() > max_chars {
        // 保留末尾 max_chars 个字符，开头加省略号
        let from = chars.len() - max_chars;
        format!("…{}", chars[from..].iter().collect::<String>())
    } else {
        text.into_owned()
    }
}

/// 取 npm 安装失败时的诊断尾部。npm 非 TTY 下 stdout/stderr 走 block
/// buffering，卡死时可能一字节都没 flush 到重定向文件（表现为 0 字节）；
/// 而 npm 内部 logger 会直接把 verbose 步骤写进 `npm-cache/_logs/*-debug-0.log`
/// （每次 install 一个新文件），不受 stdout buffering 影响，才是定位卡死的
/// 可靠来源。优先返回最新一份 debug log 尾部，缺失时回退 stdout 日志。
fn npm_failure_tail(config: &Config, log_path: &Path, max_chars: usize) -> String {
    let logs_dir = config.root.join("npm-cache").join("_logs");
    let mut latest: Option<(std::time::SystemTime, PathBuf)> = None;
    if let Ok(entries) = std::fs::read_dir(&logs_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name();
            let name = name.to_string_lossy();
            // 只认 debug-0.log（npm 每次 install 的主诊断文件）
            if !name.ends_with("-debug-0.log") {
                continue;
            }
            if let Ok(meta) = entry.metadata() {
                if let Ok(modified) = meta.modified() {
                    let newer = latest.as_ref().map(|(t, _)| modified > *t).unwrap_or(true);
                    if newer {
                        latest = Some((modified, path));
                    }
                }
            }
        }
    }
    if let Some((_, debug_log)) = latest {
        let tail = read_log_tail(&debug_log, max_chars);
        if !tail.trim().is_empty() {
            return format!("[npm debug log: {}]\n{}", debug_log.display(), tail);
        }
    }
    read_log_tail(log_path, max_chars)
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
    // 强制 native 模块用预编译二进制：node-pty 等包的 prebuild 检查失败后会
    // 回退 node-gyp 编译，新机器无 Python/VS 工具链时卡死（node_repl 等交互
    // 进程挂起）。设为 false 让它们找不到 prebuild 就立即失败而非尝试编译。
    envs.push(("npm_config_build_from_source", "false".to_string()));
    envs
}

/// 从版本字符串判断是否支持 `--no-open`（rc.8 及以上）。
/// 纯函数便于测试。
fn version_supports_no_open(version: &str) -> bool {
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

/// 隐藏窗口启动 `dsh web` 服务，返回 (pid, 进程树守卫)。
pub(crate) fn start_server(
    _app: &AppHandle,
    config: &Config,
    node_exe: &Path,
) -> Result<(u32, Option<TreeGuard>), String> {
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
    let child =
        processes::spawn_process(node_exe, &args, &envs, Some(&config.dsh_dir()), Some(&log))
            .map_err(|e| {
                crate::locale::owned(
                    format!("启动 dsh 服务失败：{e}"),
                    format!("Failed to start the dsh service: {e}"),
                )
            })?;
    let pid = child.id();

    // 建立进程树守卫（Windows Job / Unix 进程组）；失败不致命，退出时另有兜底。
    let guard = processes::TreeGuard::from_child(&child);
    // child 句柄随函数结束关闭；进程由守卫/taskkill 管理。
    drop(child);
    Ok((pid, guard))
}

#[cfg(test)]
mod checksum_tests {
    use super::dir_size_mb;
    use super::parse_node_sha256;
    use super::read_log_tail;
    use super::version_supports_no_open;

    #[test]
    fn parses_only_the_exact_archive_name() {
        let hash = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
        let checksums = format!("{hash}  node-v24.1.0-win-x64.zip\n{hash}  other.zip\n");
        assert_eq!(
            parse_node_sha256(&checksums, "node-v24.1.0-win-x64.zip").as_deref(),
            Some(hash)
        );
        assert!(parse_node_sha256(&checksums, "missing.zip").is_none());
    }

    #[test]
    fn no_open_support_by_rc_version() {
        // rc.8 及以上支持 --no-open
        assert!(version_supports_no_open("0.1.0-rc.8"));
        assert!(version_supports_no_open("0.2.0-rc.11"));
        // rc.7 及更早不支持
        assert!(!version_supports_no_open("0.1.0-rc.7"));
        // 稳定版（无 rc 后缀）支持
        assert!(version_supports_no_open("0.1.0"));
        // 空版本保守不支持；非 rc 的任意非空串按稳定版处理（版本来自
        // package.json，形如 x.y.z 或 x.y.z-rc.N，不会出现垃圾串）
        assert!(!version_supports_no_open(""));
    }

    #[test]
    fn dir_size_rounds_up_to_mb_and_recurses() {
        // 唯一目录名：避免上次失败运行残留文件干扰（进程 id 会复用）
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let tmp =
            std::env::temp_dir().join(format!("dshbox-dirsize-{}-{nanos}", std::process::id()));
        std::fs::create_dir_all(tmp.join("sub")).unwrap();
        // 5 KB 文件：不足 1MB，应向上取整为 1MB
        std::fs::write(tmp.join("sub").join("a.bin"), vec![0u8; 5 * 1024]).unwrap();
        assert_eq!(dir_size_mb(&tmp), 1);
        // 1MB + 1 字节：应向上取整为 2MB
        std::fs::write(tmp.join("sub").join("b.bin"), vec![0u8; 1024 * 1024 + 1]).unwrap();
        assert_eq!(dir_size_mb(&tmp), 2);
        std::fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn read_log_tail_returns_end_not_head() {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let file =
            std::env::temp_dir().join(format!("dshbox-taillog-{}-{nanos}", std::process::id()));
        // 头部是长噪声，尾部只有标记行：读尾必须拿到标记而非头部
        let head = "x".repeat(100_000);
        std::fs::write(&file, format!("{head}\nTAIL-MARKER-END\n")).unwrap();
        let tail = read_log_tail(&file, 50);
        assert!(tail.contains("TAIL-MARKER-END"), "got: {tail}");
        assert!(!tail.starts_with("xxx"), "不应返回头部：{tail}");
        std::fs::remove_file(&file).ok();
    }
}
