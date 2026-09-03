//! 运行时安装与维护入口：Node、自管 pnpm、dsh 包与服务启动。

mod download;
mod dsh_package;
mod node;
mod package_manager;
mod server;

// 消费方（updater/app.rs 的下载/校验）为 windows 门控：非 Windows 下
// 再导出无人使用，按仓库既有模式豁免（同 powershell.rs 的 latest_stable_tag）
#[cfg_attr(not(windows), allow(unused_imports))]
pub(crate) use download::{sha256_file, stream_to_file, DownloadError, StreamRequest};
#[cfg(test)]
use dsh_package::read_log_tail;
pub(crate) use dsh_package::{ensure_dsh, install_dsh_version, prepare_dsh_installer};
#[cfg(test)]
use node::parse_node_sha256;
pub(crate) use node::{
    current_node_version, ensure_node, find_system_node, install_node_from_archive,
    npm_cli_for_node, npm_version, prepare_node_archive_with, upgrade_portable_npm,
};
#[cfg(test)]
use package_manager::parse_pnpm_progress;
#[cfg(test)]
use server::version_supports_no_open;
pub(crate) use server::{base_envs, start_server};

use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use tauri::{AppHandle, Manager};

use crate::app_state::{AppState, BootPhase, Config};
use crate::processes::{self, TreeGuard};
use crate::versions::{node_satisfies, parse_node_version};
use crate::{emit_status, emit_status_progress};

const NODEJS_INDEX: &str = "https://nodejs.org/dist/index.json";
/// npmmirror 国内镜像（阿里开源镜像，302 到 CDN）：官方直连失败时兜底，
/// 缓解 nodejs.org 在国内下载慢/超时。包下载与版本索引 index.json 都会
/// 走镜像；SHASUMS256 校验清单始终优先官方（完整性锚点不与下载同源），
/// 仅 mirror 模式下官方不可达才降级镜像清单（见 node.rs 的
/// checksums_urls 与 docs/security.md）。
const NODE_MIRROR_BASE: &str = "https://npmmirror.com/mirrors/node";
const NPM_DIST_TAGS: &str = "https://registry.npmjs.org/-/package/@deepseek-ai/dsh/dist-tags";
/// npm 包自身的 dist-tags（查询 npm 最新版用于检查更新）。
const NPM_LATEST: &str = "https://registry.npmjs.org/-/package/npm/dist-tags";
/// @deepseek-ai/dsh 的完整包元数据（拿全部版本 + 发布时间，用于动态降级）。
const DSH_PACKAGE_META: &str = "https://registry.npmjs.org/@deepseek-ai%2fdsh";

/// npm registry 官方源与国内镜像（pnpm 引导 / dsh 安装 / npm 升级共用一份，
/// 避免多处定义漂移）。
pub(crate) const NPM_REGISTRY: &str = "https://registry.npmjs.org";
pub(crate) const NPM_MIRROR: &str = "https://registry.npmmirror.com";

/// 按下载源配置给出（registry, 展示名）候选列表：official 仅官方、mirror 仅
/// 镜像、auto 官方优先镜像兜底。
pub(crate) fn registries(config: &Config) -> Vec<(&'static str, &'static str)> {
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

/// 版本检查客户端进程内缓存（见 check_client）。
static CHECK_AGENT: std::sync::OnceLock<ureq::Agent> = std::sync::OnceLock::new();
const MAX_NODE_ARCHIVE_BYTES: u64 = 256 * 1024 * 1024;

fn install_cancelled(app: &AppHandle) -> bool {
    app.state::<AppState>().install_cancelled()
}

fn install_cancelled_error() -> String {
    crate::locale::text("安装已取消。", "Installation cancelled.").to_string()
}

/// 版本检查专用客户端：连接 5s、响应 8s——检查类请求（npm / Node LTS /
/// GitHub）应当快速失败并显示"暂无法获取版本信息"，而不是拖满 90s
/// 拖慢整个检查流程（大文件下载仍用 download_client）。
/// 进程内缓存复用：TLS 配置只构建一次。
pub(crate) fn check_client() -> ureq::Agent {
    CHECK_AGENT
        .get_or_init(|| {
            ureq::Agent::config_builder()
                .tls_config(crate::default_tls_config())
                .timeout_connect(Some(Duration::from_secs(5)))
                .timeout_recv_response(Some(Duration::from_secs(8)))
                .timeout_recv_body(Some(Duration::from_secs(8)))
                .build()
                .new_agent()
        })
        .clone()
}

/// 大文件下载专用客户端：ureq 3 的 timeout_recv_body 是「响应头收完起、
/// body 全部收完的整体时限」（timings.rs 中 RecvBody 以上一阶段记录时刻
/// 为基准），并非单次读取空闲超时。常规 90s 会掐断慢网下的大归档下载，
/// 整体预算放宽到 1 小时；响应头最多等 30s，避免用户取消后仍长时间卡在
/// 同步建连阶段。进入响应体后每个读取周期都会检查取消状态。
pub(crate) fn download_client() -> ureq::Agent {
    ureq::Agent::config_builder()
        .tls_config(crate::default_tls_config())
        .timeout_connect(Some(Duration::from_secs(15)))
        .timeout_recv_response(Some(Duration::from_secs(30)))
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
    latest_lts_cached(false, "auto")
}

/// 最新 LTS：force=true 强制刷新（安装/更新 Node 前调用）；source 与首次安装
/// 下载源一致，避免用户选择“仅镜像”后版本索引仍先访问官方。
pub(crate) fn latest_lts_cached(force: bool, source: &str) -> Result<String, String> {
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
    let mirror_index = format!("{NODE_MIRROR_BASE}/index.json");
    let sources: Vec<(&str, &str)> = match source {
        "official" => vec![(NODEJS_INDEX, "official")],
        "mirror" => vec![(&mirror_index, "mirror")],
        _ => vec![(NODEJS_INDEX, "official"), (&mirror_index, "mirror")],
    };
    let mut errors = Vec::new();
    let mut version = None;
    for (index, (url, label)) in sources.iter().enumerate() {
        match fetch_latest_lts(url) {
            Ok(value) => {
                version = Some(value);
                break;
            }
            Err(error) => {
                errors.push(format!("{label}: {error}"));
                if index + 1 < sources.len() {
                    crate::logging::log(&format!(
                        "runtime: Node 版本索引官方响应无效（{error}），改走 npmmirror 镜像"
                    ));
                }
            }
        }
    }
    let s = version.ok_or_else(|| errors.join("; "))?;
    *LTS_CACHE.lock().unwrap_or_else(|e| e.into_inner()) = Some((now, s.clone()));
    Ok(s)
}

fn fetch_latest_lts(url: &str) -> Result<String, String> {
    let resp = check_client().get(url).call().map_err(|error| {
        format!(
            "{}: {error}",
            crate::locale::text("网络请求失败", "Network request failed")
        )
    })?;
    let json: serde_json::Value = resp.into_body().read_json().map_err(|error| {
        format!(
            "{}: {error}",
            crate::locale::text(
                "解析 Node 版本信息失败",
                "Failed to parse Node.js version information"
            )
        )
    })?;
    parse_latest_lts(&json)
}

fn parse_latest_lts(json: &serde_json::Value) -> Result<String, String> {
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
                return Ok(v.to_string());
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
/// dsh 更新通道：latest（稳定推荐）/ next（预览）/ alpha（尝鲜，
/// 上游 2026-09 起将最新预发布置于 alpha tag，next 与 latest 常同版）。
#[derive(Clone, Copy, PartialEq)]
pub(crate) enum DshChannel {
    Latest,
    Next,
    Alpha,
}

impl DshChannel {
    pub(crate) fn from_config(config: &crate::app_state::Config) -> Self {
        match config.dsh_update_channel.as_str() {
            "next" => Self::Next,
            "alpha" => Self::Alpha,
            _ => Self::Latest,
        }
    }

    pub(crate) fn dist_tag(self) -> &'static str {
        match self {
            Self::Latest => "latest",
            Self::Next => "next",
            Self::Alpha => "alpha",
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

/// dsh 的版本降级链：以当前通道的 dist-tag 为上界，按发布时间向旧版本回退。
/// 稳定通道的目标为稳定版时不会混入预览版；目标本身为预览版时仍允许同通道
/// 的旧预览版，以兼容上游曾将 rc 置于 latest 的发布方式。
pub(crate) fn dsh_version_chain(channel: DshChannel, limit: usize) -> Vec<String> {
    let text = match get_text(DSH_PACKAGE_META) {
        Ok(t) => t,
        Err(_) => return Vec::new(),
    };
    parse_dsh_version_chain(&text, channel, limit)
}

fn parse_dsh_version_chain(text: &str, channel: DshChannel, limit: usize) -> Vec<String> {
    let json: serde_json::Value = match serde_json::from_str(text) {
        Ok(j) => j,
        Err(_) => return Vec::new(),
    };
    let selected = match json
        .get("dist-tags")
        .and_then(|tags| tags.get(channel.dist_tag()))
        .and_then(|value| value.as_str())
    {
        Some(version) => version,
        None => return Vec::new(),
    };
    let selected_semver = match semver::Version::parse(selected) {
        Ok(version) => version,
        Err(_) => return vec![selected.to_string()].into_iter().take(limit).collect(),
    };
    let stable_only = channel == DshChannel::Latest && selected_semver.pre.is_empty();
    // time 字段：{ "0.1.0-rc.1": "...", "created": "...", "modified": "..." }
    let time = match json.get("time").and_then(|t| t.as_object()) {
        Some(t) => t,
        None => return vec![selected.to_string()].into_iter().take(limit).collect(),
    };
    let mut versions: Vec<(&String, &serde_json::Value)> = time
        .iter()
        .filter(|(name, published)| {
            published.is_string()
                && semver::Version::parse(name).is_ok_and(|version| {
                    version <= selected_semver && (!stable_only || version.pre.is_empty())
                })
        })
        .collect();
    // 按时间字符串倒序（ISO8601 字典序 == 时间序）
    versions.sort_by(|a, b| b.1.as_str().unwrap_or("").cmp(a.1.as_str().unwrap_or("")));
    let mut chain = vec![selected.to_string()];
    chain.extend(
        versions
            .iter()
            .filter(|(version, _)| version.as_str() != selected)
            .map(|(v, _)| v.to_string()),
    );
    chain.truncate(limit);
    chain
}

#[cfg(test)]
mod checksum_tests {
    use super::parse_node_sha256;
    use super::parse_pnpm_progress;
    use super::read_log_tail;
    use super::version_supports_no_open;
    use super::{parse_dsh_version_chain, parse_latest_lts, DshChannel};
    use crate::app_state::Config;

    #[test]
    fn dsh_channel_from_config_and_tag() {
        // Config::load 起始 dsh_update_channel 恒为 latest，不受本机 config.json 影响
        let mut config = Config::load();
        config.dsh_update_channel = "latest".into();
        assert_eq!(DshChannel::from_config(&config).dist_tag(), "latest");
        config.dsh_update_channel = "next".into();
        assert_eq!(DshChannel::from_config(&config).dist_tag(), "next");
        config.dsh_update_channel = "alpha".into();
        assert_eq!(DshChannel::from_config(&config).dist_tag(), "alpha");
        // 未知值回退稳定通道，不会因手改 config.json 失效
        config.dsh_update_channel = "beta".into();
        assert_eq!(DshChannel::from_config(&config).dist_tag(), "latest");
    }

    #[test]
    fn node_lts_parser_skips_current_releases() {
        let json = serde_json::json!([
            {"version":"v26.0.0","lts":false},
            {"version":"v24.12.0","lts":"Krypton"},
            {"version":"v22.1.0","lts":"Jod"}
        ]);
        assert_eq!(parse_latest_lts(&json).unwrap(), "v24.12.0");
        assert!(parse_latest_lts(&serde_json::json!({})).is_err());
    }

    #[test]
    fn dsh_fallback_chain_is_anchored_to_the_selected_channel() {
        let metadata = r#"{
          "dist-tags": {"latest":"1.2.0","next":"2.0.0-rc.2"},
          "time": {
            "created":"2026-01-01T00:00:00Z",
            "1.1.0":"2026-02-01T00:00:00Z",
            "1.2.0-rc.1":"2026-03-01T00:00:00Z",
            "1.2.0":"2026-04-01T00:00:00Z",
            "2.0.0-rc.1":"2026-05-01T00:00:00Z",
            "2.0.0-rc.2":"2026-06-01T00:00:00Z",
            "modified":"2026-06-02T00:00:00Z"
          }
        }"#;
        assert_eq!(
            parse_dsh_version_chain(metadata, DshChannel::Latest, 5),
            vec!["1.2.0", "1.1.0"]
        );
        assert_eq!(
            parse_dsh_version_chain(metadata, DshChannel::Next, 3),
            vec!["2.0.0-rc.2", "2.0.0-rc.1", "1.2.0"]
        );
    }

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
    fn no_open_support_uses_the_full_semver() {
        // 引入该参数的边界。
        assert!(version_supports_no_open("0.1.0-rc.8"));
        assert!(!version_supports_no_open("0.1.0-rc.7"));
        // 后续补丁版本重新从 rc.1 计数，仍然晚于 0.1.0-rc.8。
        assert!(version_supports_no_open("0.1.1-rc.2"));
        assert!(version_supports_no_open("0.2.0-rc.1"));
        assert!(version_supports_no_open("0.1.0"));
        // 无法解析时保守不添加参数。
        assert!(!version_supports_no_open(""));
        assert!(!version_supports_no_open("unknown"));
    }

    #[test]
    fn pnpm_progress_parser_uses_the_latest_complete_line() {
        let log = "Progress: resolved 59, reused 0, downloaded 0, added 0\n\
                   warning text\n\
                   Progress: resolved 445, reused 2, downloaded 443, added 419\n";
        let progress = parse_pnpm_progress(log).unwrap();
        assert_eq!(progress.resolved, 445);
        assert_eq!(progress.downloaded, 443);
        assert_eq!(progress.added, 419);
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
