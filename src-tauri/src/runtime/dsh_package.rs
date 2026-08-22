//! dsh 包版本选择、安装与日志诊断。

use super::package_manager::{ensure_pnpm, run_pnpm_add, PnpmAdd};
use super::*;

const NPM_REGISTRY: &str = "https://registry.npmjs.org";
const NPM_MIRROR: &str = "https://registry.npmmirror.com";

fn dsh_installed(config: &Config) -> bool {
    config.dsh_entry().exists()
}

fn npm_cli(node_exe: &Path) -> Result<PathBuf, String> {
    npm_cli_for_node(node_exe).ok_or_else(|| {
        crate::locale::owned(
            format!("未找到与 Node.js 匹配的 npm：{}", node_exe.display()),
            format!(
                "npm matching this Node.js runtime was not found: {}",
                node_exe.display()
            ),
        )
    })
}

/// 在任何 dsh 目录替换前准备好固定版本 pnpm，避免更新事务停服后才发现
/// 安装器不可用。首次安装与更新共用这一入口。
pub(crate) fn prepare_dsh_installer(
    app: &AppHandle,
    config: &Config,
    node_exe: &Path,
    reporter: &mut dyn FnMut(&str, &str),
) -> Result<PathBuf, String> {
    ensure_pnpm(app, config, node_exe, &npm_cli(node_exe)?, reporter)
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

fn log_version(version: &str) -> String {
    version
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-') {
                ch
            } else {
                '_'
            }
        })
        .collect()
}

struct DshInstallTarget<'a> {
    target: &'a str,
    expected_version: Option<&'a str>,
    no_progress_secs: u64,
    cancellable: bool,
}

fn install_target(
    app: &AppHandle,
    config: &Config,
    node_exe: &Path,
    pnpm_cli: &Path,
    install: DshInstallTarget<'_>,
    reporter: &mut dyn FnMut(&str, &str),
) -> Result<(), String> {
    let mut last_error = String::new();
    let version_label = install.expected_version.unwrap_or("dist-tag");
    for (registry, source) in registries(config) {
        reporter(
            crate::locale::text("正在安装 dsh…", "Installing dsh…"),
            source,
        );
        let source_label = if registry == NPM_REGISTRY {
            "official"
        } else {
            "mirror"
        };
        let log_path = config.logs_dir().join(format!(
            "pnpm-install-{}-{source_label}.log",
            log_version(version_label)
        ));
        match run_pnpm_add(
            app,
            config,
            node_exe,
            pnpm_cli,
            PnpmAdd {
                target: install.target,
                registry,
                source,
                log_path: &log_path,
                no_progress_secs: install.no_progress_secs,
                cancellable: install.cancellable,
            },
            reporter,
        ) {
            Ok(()) => {
                if !dsh_installed(config) {
                    last_error = crate::locale::text(
                        "dsh 已安装，但找不到入口文件",
                        "dsh was installed, but its entry file was not found",
                    )
                    .to_string();
                    continue;
                }
                let installed = installed_dsh_version(config);
                if install
                    .expected_version
                    .is_some_and(|expected| installed.as_deref() != Some(expected))
                {
                    last_error = crate::locale::owned(
                        format!(
                            "安装版本校验失败：实际为 {}，预期为 {}",
                            installed.as_deref().unwrap_or("未知"),
                            install.expected_version.unwrap_or_default()
                        ),
                        format!(
                            "The installed version could not be verified: got {}, expected {}",
                            installed.as_deref().unwrap_or("unknown"),
                            install.expected_version.unwrap_or_default()
                        ),
                    );
                    continue;
                }
                return Ok(());
            }
            Err(error) => {
                if install.cancellable && install_cancelled(app) {
                    return Err(install_cancelled_error());
                }
                last_error = error;
                crate::logging::log(&format!(
                    "runtime: pnpm install {} ({registry}) 失败：{last_error}",
                    install.target
                ));
            }
        }
    }
    Err(last_error)
}

/// 安装 dsh 官方包到应用数据目录。pnpm 与 dsh 上游一致，并能线性解析其
/// 数百包依赖图；npm 11/12 会在 Arborist placeDep 阶段长时间无进展。
pub(crate) fn ensure_dsh(app: &AppHandle, config: &Config, node_exe: &Path) -> Result<(), String> {
    if dsh_installed(config) {
        return Ok(());
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
    let mut reporter = |message: &str, detail: &str| {
        emit_status(app, BootPhase::InstallingDsh, message, detail);
    };
    let pnpm_cli = prepare_dsh_installer(app, config, node_exe, &mut reporter)?;

    let channel = DshChannel::from_config(config);
    let mut versions = dsh_version_chain(channel, 5);
    if versions.is_empty() {
        versions = npm_latest_dsh_version(channel).into_iter().collect();
    }
    let targets: Vec<(String, Option<String>)> = if versions.is_empty() {
        vec![(format!("@deepseek-ai/dsh@{}", channel.dist_tag()), None)]
    } else {
        versions
            .into_iter()
            .map(|version| (format!("@deepseek-ai/dsh@{version}"), Some(version)))
            .collect()
    };

    let mut last_error = String::new();
    for (index, (target, expected)) in targets.iter().enumerate() {
        if install_cancelled(app) {
            return Err(install_cancelled_error());
        }
        if index > 0 {
            let message = crate::locale::owned(
                format!("目标版本装不上，自动尝试 {target}…"),
                format!("The target version could not be installed. Trying {target}…"),
            );
            reporter(&message, "");
        }
        match install_target(
            app,
            config,
            node_exe,
            &pnpm_cli,
            DshInstallTarget {
                target,
                expected_version: expected.as_deref(),
                no_progress_secs: if index == 0 { 90 } else { 60 },
                cancellable: true,
            },
            &mut reporter,
        ) {
            Ok(()) => {
                if index > 0 {
                    crate::logging::log(&format!(
                        "runtime: dsh 目标版本装不上，已自动降级到 {target}"
                    ));
                }
                return Ok(());
            }
            Err(error) => last_error = error,
        }
    }
    Err(crate::locale::owned(
        format!("安装 dsh 失败（目标版本与降级版本均无法安装）：{last_error}"),
        format!(
            "Failed to install dsh (neither the target nor fallback versions could be installed): {last_error}"
        ),
    ))
}

/// 更新事务使用的精确版本安装；版本由停服前的检查结果锁定，不解析移动 tag。
pub(crate) fn install_dsh_version(
    app: &AppHandle,
    config: &Config,
    node_exe: &Path,
    pnpm_cli: &Path,
    version: &str,
    reporter: &mut dyn FnMut(&str, &str),
) -> Result<(), String> {
    install_target(
        app,
        config,
        node_exe,
        pnpm_cli,
        DshInstallTarget {
            target: &format!("@deepseek-ai/dsh@{version}"),
            expected_version: Some(version),
            no_progress_secs: 90,
            cancellable: false,
        },
        reporter,
    )
}

/// 从大日志末尾读取有限字符，避免失败诊断把几十 MB npm/pnpm 日志全部载入。
pub(super) fn read_log_tail(path: &Path, max_chars: usize) -> String {
    use std::io::{Read, Seek, SeekFrom};
    let mut file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(_) => return String::new(),
    };
    let file_len = match file.metadata() {
        Ok(metadata) => metadata.len(),
        Err(_) => return String::new(),
    };
    if file_len == 0 {
        return String::new();
    }
    let read_start = file_len.saturating_sub((max_chars as u64) * 4 + 3);
    if file.seek(SeekFrom::Start(read_start)).is_err() {
        return String::new();
    }
    let mut buffer = Vec::new();
    if file.read_to_end(&mut buffer).is_err() {
        return String::new();
    }
    let text = String::from_utf8_lossy(&buffer);
    let chars: Vec<char> = text.chars().collect();
    if chars.len() > max_chars {
        format!(
            "…{}",
            chars[chars.len() - max_chars..].iter().collect::<String>()
        )
    } else {
        text.into_owned()
    }
}
