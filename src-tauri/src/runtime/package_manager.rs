//! DSHBox 自管 pnpm 的引导、执行与安装进度解析。

use super::dsh_package::read_log_tail;
use super::*;

/// 与 dsh 上游 `packageManager` 保持一致。固定版本避免用户 PATH、Corepack
/// 缓存或全局 pnpm 的差异影响首次安装；该包无依赖、无安装脚本。
const PNPM_VERSION: &str = "11.7.0";
const NPM_REGISTRY: &str = "https://registry.npmjs.org";
const NPM_MIRROR: &str = "https://registry.npmmirror.com";

/// 来自 dsh 上游 pnpm-workspace.yaml 的发布依赖脚本策略。核心运行所需的
/// node-pty / koffi 与 dsh 自身 helper 明确放行；已确认无必要的脚本明确拒绝；
/// 未列出的新脚本由 pnpm 失败关闭，避免 dsh 更新静默扩大代码执行面。
const DSH_PNPM_POLICY: &str = r#"packages: []
strictDepBuilds: true
allowBuilds:
  node-pty: true
  koffi: true
  '@deepseek-ai/dsh-subprocess-local': true
  '@google/genai': false
  protobufjs: false
  node-addon-require-builtin: false
"#;

pub(super) struct ToolAttempt<'a> {
    pub args: &'a [String],
    pub log_path: &'a Path,
    pub no_progress_secs: u64,
    pub cancellable: bool,
}

pub(super) struct PnpmAdd<'a> {
    pub target: &'a str,
    pub registry: &'a str,
    pub source: &'a str,
    pub log_path: &'a Path,
    pub no_progress_secs: u64,
    pub cancellable: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct PnpmProgress {
    pub resolved: u64,
    pub downloaded: u64,
    pub added: u64,
}

fn package_version(package_json: &Path) -> Option<String> {
    let text = std::fs::read_to_string(package_json).ok()?;
    let json: serde_json::Value = serde_json::from_str(&text).ok()?;
    json.get("version")?.as_str().map(str::to_string)
}

fn pnpm_is_ready(config: &Config) -> bool {
    config.pnpm_cli().exists()
        && package_version(
            &config
                .package_manager_dir()
                .join("node_modules/pnpm/package.json"),
        )
        .as_deref()
            == Some(PNPM_VERSION)
}

fn registries(config: &Config) -> Vec<(&'static str, &'static str)> {
    match config.download_source.as_str() {
        "official" => vec![(
            NPM_REGISTRY,
            crate::locale::text("npm 官方源", "Official npm registry"),
        )],
        "mirror" => vec![(NPM_MIRROR, crate::locale::text("镜像源", "Mirror"))],
        _ => vec![
            (
                NPM_REGISTRY,
                crate::locale::text("npm 官方源", "Official npm registry"),
            ),
            (NPM_MIRROR, crate::locale::text("镜像源", "Mirror")),
        ],
    }
}

/// 安装固定版本 pnpm。这里只让 npm 解析一个无依赖、无 install script 的包，
/// 避开 dsh 数百包依赖图触发的 Arborist placeDep 卡死。
pub(super) fn ensure_pnpm(
    app: &AppHandle,
    config: &Config,
    node_exe: &Path,
    npm_cli: &Path,
    reporter: &mut dyn FnMut(&str, &str),
) -> Result<PathBuf, String> {
    if pnpm_is_ready(config) {
        return Ok(config.pnpm_cli());
    }
    if !npm_cli.exists() {
        return Err(crate::locale::owned(
            format!("未找到 npm：{}", npm_cli.display()),
            format!("npm was not found: {}", npm_cli.display()),
        ));
    }

    let dir = config.package_manager_dir();
    if dir.exists() {
        std::fs::remove_dir_all(&dir).map_err(|e| {
            crate::locale::owned(
                format!("清理包管理器残留失败：{e}"),
                format!("Failed to clear the package-manager residue: {e}"),
            )
        })?;
    }
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;

    let mut last_error = String::new();
    for (registry, source) in registries(config) {
        reporter(
            crate::locale::text("正在准备 dsh 安装环境…", "Preparing the dsh installer…"),
            source,
        );
        let args = vec![
            npm_cli.to_string_lossy().into_owned(),
            "install".into(),
            "--prefix".into(),
            dir.to_string_lossy().into_owned(),
            format!("pnpm@{PNPM_VERSION}"),
            "--ignore-scripts".into(),
            "--no-audit".into(),
            "--no-fund".into(),
            "--package-lock=false".into(),
            "--fetch-timeout=60000".into(),
            "--fetch-retries=2".into(),
            "--verbose".into(),
            "--registry".into(),
            registry.into(),
        ];
        let log_path = config.logs_dir().join(if registry == NPM_REGISTRY {
            "pnpm-bootstrap.log"
        } else {
            "pnpm-bootstrap-mirror.log"
        });
        let result = run_node_tool(
            app,
            config,
            node_exe,
            ToolAttempt {
                args: &args,
                log_path: &log_path,
                no_progress_secs: 75,
                cancellable: true,
            },
            |secs, _| {
                let detail = format!("{source} · {secs}s");
                reporter(
                    crate::locale::text("正在准备 dsh 安装环境…", "Preparing the dsh installer…"),
                    &detail,
                );
            },
        );
        match result {
            Ok(()) if pnpm_is_ready(config) => return Ok(config.pnpm_cli()),
            Ok(()) => {
                last_error = crate::locale::text(
                    "pnpm 安装完成，但入口或版本校验失败",
                    "pnpm was installed, but its entry point or version could not be verified",
                )
                .to_string();
            }
            Err(error) => {
                if install_cancelled(app) {
                    return Err(install_cancelled_error());
                }
                last_error = error;
            }
        }
    }
    Err(crate::locale::owned(
        format!("准备 dsh 安装环境失败：{last_error}"),
        format!("Failed to prepare the dsh installer: {last_error}"),
    ))
}

/// 每次安装前重建干净项目骨架；镜像切换和版本降级不会继承半截虚拟仓库。
pub(super) fn reset_dsh_project(config: &Config) -> Result<(), String> {
    let dir = config.dsh_dir();
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    for path in [
        dir.join("node_modules"),
        dir.join("package.json"),
        dir.join("package-lock.json"),
        dir.join("pnpm-lock.yaml"),
    ] {
        if !path.exists() {
            continue;
        }
        let result = if path.is_dir() {
            std::fs::remove_dir_all(&path)
        } else {
            std::fs::remove_file(&path)
        };
        result.map_err(|e| {
            crate::locale::owned(
                format!("清理安装残留失败（{}）：{e}", path.display()),
                format!("Failed to clear install residue ({}): {e}", path.display()),
            )
        })?;
    }
    crate::app_state::atomic_write(&dir.join("pnpm-workspace.yaml"), DSH_PNPM_POLICY)
}

pub(super) fn run_pnpm_add(
    app: &AppHandle,
    config: &Config,
    node_exe: &Path,
    pnpm_cli: &Path,
    add: PnpmAdd<'_>,
    reporter: &mut dyn FnMut(&str, &str),
) -> Result<(), String> {
    reset_dsh_project(config)?;
    let args = vec![
        pnpm_cli.to_string_lossy().into_owned(),
        "add".into(),
        "--dir".into(),
        config.dsh_dir().to_string_lossy().into_owned(),
        add.target.into(),
        "--save-prod".into(),
        "--reporter=append-only".into(),
        "--registry".into(),
        add.registry.into(),
    ];
    run_node_tool(
        app,
        config,
        node_exe,
        ToolAttempt {
            args: &args,
            log_path: add.log_path,
            no_progress_secs: add.no_progress_secs,
            cancellable: add.cancellable,
        },
        |secs, log| {
            let progress = parse_pnpm_progress(log);
            let detail = if let Some(progress) = progress.filter(|item| item.downloaded > 0) {
                if crate::locale::is_chinese() {
                    format!("{} · {} 个包 · {secs}s", add.source, progress.downloaded)
                } else {
                    format!(
                        "{} · {} packages · {secs}s",
                        add.source, progress.downloaded
                    )
                }
            } else if crate::locale::is_chinese() {
                format!("{} · 解析依赖 · {secs}s", add.source)
            } else {
                format!("{} · Resolving · {secs}s", add.source)
            };
            reporter(
                crate::locale::text("正在安装 dsh…", "Installing dsh…"),
                &detail,
            );
        },
    )
}

/// 运行 Node 工具并监控其日志活动。npm 的 fetch timeout 管不到依赖解析，
/// 因此必须在进程外按“日志是否继续推进”判定卡死，同时持续响应用户取消。
fn run_node_tool(
    app: &AppHandle,
    config: &Config,
    node_exe: &Path,
    attempt: ToolAttempt<'_>,
    mut on_tick: impl FnMut(u64, &str),
) -> Result<(), String> {
    if let Some(parent) = attempt.log_path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    // spawn_process 使用 append；先截断，确保活动检测和错误尾部只属于本轮。
    File::create(attempt.log_path).map_err(|e| e.to_string())?;
    let envs = base_envs(node_exe, config);
    let mut child = processes::spawn_process(
        node_exe,
        attempt.args,
        &envs,
        Some(&config.root),
        Some(attempt.log_path),
    )
    .map_err(|e| {
        crate::locale::owned(
            format!("运行包管理器失败：{e}"),
            format!("Failed to run the package manager: {e}"),
        )
    })?;
    let _guard = processes::TreeGuard::from_child(&child);
    let start = Instant::now();
    let timeout = Duration::from_secs(attempt.no_progress_secs);
    let mut last_activity = Instant::now();
    let mut last_len = 0;
    let mut last_reported = u64::MAX;

    let code = loop {
        match child.try_wait().map_err(|e| e.to_string())? {
            Some(status) => break status.code().unwrap_or(-1),
            None => {
                if attempt.cancellable && install_cancelled(app) {
                    processes::kill_tree(child.id());
                    // 切源会立即开启下一轮安装；先确认旧进程退出，避免两轮 pnpm
                    // 短暂并发争用同一 store / node_modules。
                    wait_after_kill(&mut child);
                    return Err(install_cancelled_error());
                }
                let secs = start.elapsed().as_secs();
                if secs != last_reported {
                    last_reported = secs;
                    let len = std::fs::metadata(attempt.log_path)
                        .map(|meta| meta.len())
                        .unwrap_or(0);
                    if len != last_len {
                        last_len = len;
                        last_activity = Instant::now();
                    }
                    let log = read_log_tail(attempt.log_path, 12_000);
                    on_tick(secs, &log);
                }
                if last_activity.elapsed() > timeout {
                    crate::logging::log(&format!(
                        "runtime: 包管理器超过 {}s 无日志进展，终止本轮",
                        timeout.as_secs()
                    ));
                    processes::kill_tree(child.id());
                    wait_after_kill(&mut child);
                    let tail = read_log_tail(attempt.log_path, 2400);
                    return Err(crate::locale::owned(
                        format!("安装超时（{}s 无进展）：\n{tail}", timeout.as_secs()),
                        format!(
                            "Install timed out (no progress for {}s):\n{tail}",
                            timeout.as_secs()
                        ),
                    ));
                }
                std::thread::sleep(Duration::from_millis(500));
            }
        }
    };
    drop(child);
    if code == 0 {
        return Ok(());
    }
    let tail = read_log_tail(attempt.log_path, 2400);
    Err(crate::locale::owned(
        format!("包管理器退出码 {code}：\n{tail}"),
        format!("Package manager exit code {code}:\n{tail}"),
    ))
}

fn wait_after_kill(child: &mut std::process::Child) {
    let mut waited = Duration::ZERO;
    while child.try_wait().ok().flatten().is_none() && waited < Duration::from_secs(10) {
        std::thread::sleep(Duration::from_millis(200));
        waited += Duration::from_millis(200);
    }
}

/// 从 append-only 输出中取最后一条进度。字段将来增减时，只读取认识的键。
pub(super) fn parse_pnpm_progress(text: &str) -> Option<PnpmProgress> {
    let line = text.lines().rev().find(|line| line.contains("Progress:"))?;
    let normalized = line.replace(',', " ");
    let fields: Vec<&str> = normalized.split_whitespace().collect();
    let value_after = |name: &str| -> u64 {
        fields
            .windows(2)
            .find(|pair| pair[0] == name)
            .and_then(|pair| pair[1].parse().ok())
            .unwrap_or(0)
    };
    Some(PnpmProgress {
        resolved: value_after("resolved"),
        downloaded: value_after("downloaded"),
        added: value_after("added"),
    })
}
