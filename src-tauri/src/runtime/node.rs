//! Node.js 选择、便携安装、校验与解压。

use super::*;

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

/// 定位指定 Node 对应的 npm CLI。官方 Windows 包把 npm 放在 Node 同级
/// node_modules；Unix 的发行版、Homebrew 与 nvm 通常放在相邻 lib/share 目录。
pub(crate) fn npm_cli_for_node(node_exe: &Path) -> Option<PathBuf> {
    let mut executables = vec![node_exe.to_path_buf()];
    if let Ok(canonical) = std::fs::canonicalize(node_exe) {
        if canonical != node_exe {
            executables.push(canonical);
        }
    }
    for executable in executables {
        let Some(bin_dir) = executable.parent() else {
            continue;
        };
        for candidate in [
            bin_dir.join("node_modules/npm/bin/npm-cli.js"),
            bin_dir.join("../lib/node_modules/npm/bin/npm-cli.js"),
            bin_dir.join("../share/nodejs/npm/bin/npm-cli.js"),
        ] {
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
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
    let npm_cli = npm_cli_for_node(&node_exe)?;
    let package_json = npm_cli.parent()?.parent()?.join("package.json");
    let json: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(package_json).ok()?).ok()?;
    json.get("version")?.as_str().map(str::to_string)
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

pub(crate) struct NodeRuntime {
    pub executable: PathBuf,
    pub version: String,
}

fn inspect_runtime(executable: PathBuf) -> Option<NodeRuntime> {
    let (major, minor, patch) = node_version(&executable)?;
    node_satisfies(major, minor).then(|| NodeRuntime {
        executable,
        version: format!("v{major}.{minor}.{patch}"),
    })
}

fn install_runtime(app: &AppHandle, config: &Config) -> Result<NodeRuntime, String> {
    let executable = install_portable_node(app, config)?;
    inspect_runtime(executable).ok_or_else(|| {
        crate::locale::text(
            "安装后的 Node.js 无法运行或版本不满足要求。",
            "The installed Node.js runtime cannot run or does not meet the version requirement.",
        )
        .to_string()
    })
}

enum NodeDownloadError {
    /// 远端响应、响应体或内容校验错误；auto 模式可安全尝试另一个源。
    Source(String),
    /// 本地文件系统或用户取消等错误；换源没有意义。
    Fatal(String),
}

fn download_node_archive(
    app: &AppHandle,
    version: &str,
    url: &str,
    archive_path: &Path,
    expected_sha256: &str,
    source: &str,
) -> Result<(), NodeDownloadError> {
    let resp = download_client().get(url).call().map_err(|error| {
        NodeDownloadError::Source(crate::locale::owned(
            format!("Node.js 下载失败（{source}）：{error}"),
            format!("Failed to download Node.js ({source}): {error}"),
        ))
    })?;
    let total: u64 = resp
        .headers()
        .get("content-length")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse().ok())
        .unwrap_or(0);
    if total > MAX_NODE_ARCHIVE_BYTES {
        return Err(NodeDownloadError::Source(
            crate::locale::text(
                "Node.js 下载文件超过 256 MB 安全上限",
                "The Node.js download exceeds the 256 MB safety limit",
            )
            .into(),
        ));
    }
    let mut reader = resp.into_body().into_reader();
    let mut file = std::fs::File::create(archive_path).map_err(|error| {
        NodeDownloadError::Fatal(crate::locale::owned(
            format!("写入临时文件失败：{error}"),
            format!("Failed to create the temporary download file: {error}"),
        ))
    })?;
    let mut buf = [0u8; 65536];
    let mut done = 0u64;
    let mut last_pct = -1i64;
    let mut last_emit = std::time::Instant::now() - Duration::from_secs(1);
    loop {
        if install_cancelled(app) {
            drop(file);
            let _ = std::fs::remove_file(archive_path);
            return Err(NodeDownloadError::Fatal(install_cancelled_error()));
        }
        let n = match reader.read(&mut buf) {
            Ok(n) => n,
            Err(error) => {
                drop(file);
                let _ = std::fs::remove_file(archive_path);
                return Err(NodeDownloadError::Source(crate::locale::owned(
                    format!("下载 Node.js 响应体失败：{error}"),
                    format!("Failed to read the Node.js download: {error}"),
                )));
            }
        };
        if n == 0 {
            break;
        }
        done += n as u64;
        if done > MAX_NODE_ARCHIVE_BYTES {
            drop(file);
            let _ = std::fs::remove_file(archive_path);
            return Err(NodeDownloadError::Source(
                crate::locale::text(
                    "Node.js 下载文件超过 256 MB 安全上限",
                    "The Node.js download exceeds the 256 MB safety limit",
                )
                .into(),
            ));
        }
        if let Err(error) = file.write_all(&buf[..n]) {
            drop(file);
            let _ = std::fs::remove_file(archive_path);
            return Err(NodeDownloadError::Fatal(crate::locale::owned(
                format!("写入临时文件失败：{error}"),
                format!("Failed to write the temporary download file: {error}"),
            )));
        }
        if total > 0 {
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
                        "{} · {:.1}/{:.1} MB",
                        source,
                        done as f64 / 1048576.0,
                        total as f64 / 1048576.0
                    ),
                    Some(pct as f64),
                );
            }
        }
    }
    drop(file);
    if install_cancelled(app) {
        let _ = std::fs::remove_file(archive_path);
        return Err(NodeDownloadError::Fatal(install_cancelled_error()));
    }
    let actual_sha256 = sha256_file(archive_path).map_err(NodeDownloadError::Fatal)?;
    if actual_sha256 != expected_sha256 {
        let _ = std::fs::remove_file(archive_path);
        return Err(NodeDownloadError::Source(crate::locale::owned(
            format!("Node.js 下载文件校验失败：期望 {expected_sha256}，实际 {actual_sha256}"),
            format!(
                "Node.js download verification failed: expected {expected_sha256}, got {actual_sha256}"
            ),
        )));
    }
    Ok(())
}

/// 确保 Node 可用：便携 Node 与系统 Node 都先执行版本探测；
/// 便携运行时损坏或版本不满足要求时自动清理，再选择合格的系统 Node 或重新安装。
/// 返回探测时已经取得的版本，启动链路无需再次创建 node 子进程。
pub(crate) fn ensure_node(app: &AppHandle, config: &Config) -> Result<NodeRuntime, String> {
    let runtime = ensure_node_inner(app, config)?;
    // 对「任何拿到 Node 的路径」统一升级便携 npm 到 12：Node v24 官方自带
    // npm 11，其 idealTree 在解析 dsh 数百包依赖树时会卡死。此调用必须保留
    // 在 wrapper（而非 inner）末尾——inner 的多个早退点会跳过升级，使已装好
    // 便携 Node 的机器永远停留在 npm 11（历史回归）。非 strict：失败静默沿用
    // 自带版，不阻断启动；系统 Node 由升级函数内部跳过（归系统管理）。
    upgrade_portable_npm(app, config, false)?;
    Ok(runtime)
}

/// 选择或安装一个满足版本要求的 Node 运行时（不涉及 npm 升级，见 `ensure_node`）。
fn ensure_node_inner(app: &AppHandle, config: &Config) -> Result<NodeRuntime, String> {
    let managed = config.node_exe();
    if managed.exists() {
        if let Some(runtime) = inspect_runtime(managed) {
            return Ok(runtime);
        } else {
            crate::logging::log("runtime: 便携 Node 损坏或版本过旧，准备重新选择运行时");
            std::fs::remove_dir_all(config.node_dir()).map_err(|e| {
                crate::locale::owned(
                    format!("清理损坏的 Node.js 运行时失败：{e}"),
                    format!("Failed to remove the damaged Node.js runtime: {e}"),
                )
            })?;
            return find_system_node()
                .and_then(inspect_runtime)
                .map(Ok)
                .unwrap_or_else(|| install_runtime(app, config));
        }
    }
    find_system_node()
        .and_then(inspect_runtime)
        .map(Ok)
        .unwrap_or_else(|| install_runtime(app, config))
}

/// 下载并安装便携版 Node 到应用数据目录。
pub(crate) fn install_portable_node(app: &AppHandle, config: &Config) -> Result<PathBuf, String> {
    if install_cancelled(app) {
        return Err(install_cancelled_error());
    }
    emit_status(
        app,
        BootPhase::InstallingNode,
        crate::locale::text("正在下载 Node.js…", "Downloading Node.js…"),
        "",
    );
    let version = latest_lts_cached(true, &config.download_source)?; // 形如 v24.19.0

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
    let official_source = crate::locale::text("官方源", "Official");
    let mirror_source = crate::locale::text("镜像源", "Mirror");
    let sources: Vec<(&str, &str)> = match config.download_source.as_str() {
        "official" => vec![(&url, official_source)],
        "mirror" => vec![(&mirror_url, mirror_source)],
        _ => vec![(&url, official_source), (&mirror_url, mirror_source)],
    };
    // 清掉进程异常退出遗留的旧归档；每个源都从全新文件开始，绝不让失败
    // 响应与上次下载内容拼接后再进入校验。
    if archive_path.exists() {
        std::fs::remove_file(&archive_path).map_err(|error| {
            crate::locale::owned(
                format!("清理旧 Node.js 下载文件失败：{error}"),
                format!("Failed to remove the previous Node.js download: {error}"),
            )
        })?;
    }
    let mut source_errors = Vec::new();
    let mut source = official_source;
    for (index, (download_url, download_source)) in sources.iter().enumerate() {
        emit_status(
            app,
            BootPhase::InstallingNode,
            &download_message,
            download_source,
        );
        match download_node_archive(
            app,
            &version,
            download_url,
            &archive_path,
            &expected_sha256,
            download_source,
        ) {
            Ok(()) => {
                source = download_source;
                break;
            }
            Err(NodeDownloadError::Fatal(error)) => return Err(error),
            Err(NodeDownloadError::Source(error)) => {
                source_errors.push(error.clone());
                if index + 1 < sources.len() {
                    crate::logging::log(&format!(
                        "runtime: Node 下载或校验失败（{error}），改试下一个下载源"
                    ));
                    continue;
                }
                return Err(source_errors.join("; "));
            }
        }
    }

    emit_status(
        app,
        BootPhase::InstallingNode,
        crate::locale::text("正在解压 Node.js…", "Extracting Node.js…"),
        source,
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
    // 已是 12+ 则跳过（首次 dsh 安装与手动检查都可能进入，不能重复升级联网）。
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
    let registries: &[&str] = match config.download_source.as_str() {
        "official" => &["https://registry.npmjs.org"],
        "mirror" => &["https://registry.npmmirror.com"],
        _ => &[
            "https://registry.npmjs.org",
            "https://registry.npmmirror.com",
        ],
    };
    for registry in registries {
        let source = if registry.contains("npmmirror") {
            crate::locale::text("镜像源", "Mirror")
        } else {
            crate::locale::text("npm 官方源", "Official npm registry")
        };
        emit_status(
            app,
            BootPhase::InstallingNode,
            crate::locale::text("正在升级 npm…", "Upgrading npm…"),
            source,
        );
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
            (*registry).to_string(),
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
                    if install_cancelled(app) {
                        processes::kill_tree(child.id());
                        return Err(install_cancelled_error());
                    }
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

pub(super) fn parse_node_sha256(checksums: &str, archive_name: &str) -> Option<String> {
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
                    format!("Archive extraction with {name} failed"),
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

#[cfg(test)]
mod npm_cli_tests {
    use super::npm_cli_for_node;

    #[test]
    fn finds_npm_in_unix_style_sibling_lib_directory() {
        let root = std::env::temp_dir().join(format!("dsh-box-npm-cli-{}", std::process::id()));
        let node = root.join("bin/node");
        let npm_cli = root.join("lib/node_modules/npm/bin/npm-cli.js");
        std::fs::create_dir_all(node.parent().unwrap()).unwrap();
        std::fs::create_dir_all(npm_cli.parent().unwrap()).unwrap();
        std::fs::write(&node, b"").unwrap();
        std::fs::write(&npm_cli, b"").unwrap();

        assert_eq!(
            npm_cli_for_node(&node),
            Some(root.join("bin/../lib/node_modules/npm/bin/npm-cli.js"))
        );

        std::fs::remove_dir_all(root).unwrap();
    }
}
