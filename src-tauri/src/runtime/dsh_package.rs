//! dsh npm 包安装、日志诊断与进度。

use super::*;

// ---------- dsh 包安装 ----------

fn dsh_installed(config: &Config) -> bool {
    config.dsh_entry().exists()
}

/// 安装 dsh 官方 npm 包到应用数据目录（首次运行）。
pub(crate) fn ensure_dsh(app: &AppHandle, config: &Config, node_exe: &Path) -> Result<(), String> {
    if dsh_installed(config) {
        return Ok(());
    }
    // npm 12 只在首次安装 dsh 时是硬需求；热启动已安装的 dsh 不应联网升级 npm。
    // 系统 Node 由 upgrade_portable_npm 内部识别并跳过，不改系统环境。
    upgrade_portable_npm(app, config, false)?;
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
    // 版本降级：目标版本可能处于上游发版过渡期（实测 0.1.1-rc.1/rc.2 的 62 个
    // 依赖全声明 ^0.1.1-rc.1 会互相匹配到新 rc，npm placeDep 解析组合爆炸卡
    // 死，npm 11/12 都中招）。目标装不上时只在所选通道的版本上界内向下
    // 回退，避免 stable/next 检查结果与真正安装版本不一致。
    let channel = DshChannel::from_config(config);
    let mut versions = dsh_version_chain(channel, 5);
    if versions.is_empty() {
        // 完整元数据失败时再查轻量 dist-tags；两者都不可用才让 npm 解析 tag。
        versions = npm_latest_dsh_version(channel).into_iter().collect();
    }
    let install_targets: Vec<(String, Option<String>)> = if versions.is_empty() {
        vec![(format!("@deepseek-ai/dsh@{}", channel.dist_tag()), None)]
    } else {
        versions
            .into_iter()
            .map(|version| (format!("@deepseek-ai/dsh@{version}"), Some(version)))
            .collect()
    };
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
    'versions: for (vi, (target, expected_version)) in install_targets.iter().enumerate() {
        if install_cancelled(app) {
            return Err(install_cancelled_error());
        }
        // 降级时广播状态：让启动页看得到在尝试哪个版本，而非一直“安装 dsh”
        if vi > 0 {
            let msg = crate::locale::owned(
                format!("目标版本装不上，自动尝试 {target}…"),
                format!("The target version could not be installed. Trying {target}…"),
            );
            emit_status(app, BootPhase::InstallingDsh, &msg, "");
        }
        let registries: Vec<Option<&str>> = match config.download_source.as_str() {
            "official" => vec![None],
            "mirror" => vec![Some("https://registry.npmmirror.com")],
            _ => vec![None, Some("https://registry.npmmirror.com")],
        };
        let last_attempt = registries.len().saturating_sub(1);
        for (attempt, registry) in registries.into_iter().enumerate() {
            let source = if registry.is_some() {
                crate::locale::text("镜像源", "Mirror")
            } else {
                crate::locale::text("npm 官方源", "Official npm registry")
            };
            emit_status(
                app,
                BootPhase::InstallingDsh,
                crate::locale::text("正在安装 dsh…", "Installing dsh…"),
                source,
            );
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
            let log_path = config.logs_dir().join(attempt_log);
            let result = run_npm_install_with_progress(
                app,
                config,
                node_exe,
                &npm_cli,
                NpmInstallAttempt {
                    args: &args,
                    log_path: &log_path,
                    no_progress_secs: if vi == 0 { 180 } else { 90 },
                    source,
                },
            );
            match result {
                Ok(()) => {
                    let installed = installed_dsh_version(config);
                    if expected_version
                        .as_ref()
                        .is_some_and(|expected| installed.as_deref() != Some(expected.as_str()))
                    {
                        last_error = crate::locale::owned(
                            format!(
                                "npm 返回成功，但实际安装版本为 {}，预期为 {}",
                                installed.as_deref().unwrap_or("未知"),
                                expected_version.as_deref().unwrap_or_default()
                            ),
                            format!(
                                "npm reported success, but installed {} instead of {}",
                                installed.as_deref().unwrap_or("unknown"),
                                expected_version.as_deref().unwrap_or_default()
                            ),
                        );
                        crate::logging::log(&format!(
                            "runtime: install {target} 版本校验失败：{last_error}"
                        ));
                        continue 'versions;
                    }
                    // 装的是降级版本时，明确记日志便于排查
                    if vi > 0 {
                        crate::logging::log(&format!(
                            "runtime: dsh 目标版本装不上，已自动降级到 {target}"
                        ));
                    }
                    return Ok(());
                }
                Err(e) => {
                    if install_cancelled(app) {
                        return Err(install_cancelled_error());
                    }
                    last_error = e.clone();
                    crate::logging::log(&format!(
                        "runtime: install {target} ({registry:?}) 失败：{e}"
                    ));
                    if attempt < last_attempt {
                        continue;
                    }
                    // 镜像也失败：降级到下一个版本
                    continue 'versions;
                }
            }
        }
    }
    // 所有版本 + 两个 registry 都试遍仍失败：报最终错误
    Err(crate::locale::owned(
        format!("安装 dsh 失败（目标版本与降级版本均无法安装）：{last_error}",),
        format!("Failed to install dsh (neither the target nor fallback versions could be installed): {last_error}",),
    ))
}

/// 跑一次 npm install，轮询 npm 缓存目录（_cacache）累计字节数作为真实下载
/// 进度（npm 非 TTY 无中间输出，缓存增长就是包下载量的真实反映），每秒汇报
/// “已下载 X MB”。返回 Ok(()) 或带日志尾部的错误信息。
struct NpmInstallAttempt<'a> {
    args: &'a [String],
    log_path: &'a Path,
    no_progress_secs: u64,
    source: &'a str,
}

fn run_npm_install_with_progress(
    app: &AppHandle,
    config: &Config,
    node_exe: &Path,
    npm_cli: &Path,
    attempt: NpmInstallAttempt<'_>,
) -> Result<(), String> {
    let envs = base_envs(node_exe, config);
    // node 跑 npm-cli.js：首参必须是 npm-cli 路径
    let mut spawn_args = vec![npm_cli.to_string_lossy().into_owned()];
    spawn_args.extend(attempt.args.iter().cloned());
    let mut child = processes::spawn_process(
        node_exe,
        &spawn_args,
        &envs,
        Some(&config.root),
        Some(attempt.log_path),
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
    let no_progress_timeout = Duration::from_secs(attempt.no_progress_secs);
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
                if install_cancelled(app) {
                    processes::kill_tree(child.id());
                    return Err(install_cancelled_error());
                }
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
                    let tail = npm_failure_tail(config, attempt.log_path, 2000);
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
                            format!("{} · 约 {downloaded} MB · {secs}s", attempt.source)
                        } else {
                            format!("{} · ~{downloaded} MB · {secs}s", attempt.source)
                        }
                    } else if crate::locale::is_chinese() {
                        format!("{} · 下载依赖 · {secs}s", attempt.source)
                    } else {
                        format!("{} · Fetching · {secs}s", attempt.source)
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
        let tail = npm_failure_tail(config, attempt.log_path, 2000);
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
pub(super) fn dir_size_mb(dir: &Path) -> u64 {
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
pub(super) fn read_log_tail(path: &Path, max_chars: usize) -> String {
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
    // 可能从多字节字符中间切，丢弃首个不完整字符（from_utf8_lossy 已用 U+FFFD 占位，
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
