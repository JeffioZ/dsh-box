//! PowerShell 7 检测与 winget 更新。

#[cfg(windows)]
use super::{emit_progress, truncate};
#[cfg(windows)]
use crate::app_state::AppState;
#[cfg(windows)]
use crate::{processes, runtime};
#[cfg(windows)]
use std::io::Read;
use tauri::AppHandle;
#[cfg(windows)]
use tauri::Manager;

// ---------- PowerShell 7（可选增强，仅 Windows） ----------

/// 检测已安装的 PowerShell 7 版本（未安装返回 None）。
#[cfg(windows)]
pub(super) fn pwsh_version() -> Option<String> {
    // pwsh 用绝对路径优先：应用启动后才安装的 pwsh 不在 PATH 快照里
    let mut cmd = processes::pwsh_command();
    cmd.args([
        "-NoProfile",
        "-Command",
        "$PSVersionTable.PSVersion.ToString()",
    ]);
    processes::hide_console(&mut cmd);
    let out = cmd.output().ok()?;
    if !out.status.success() {
        return None;
    }
    let v = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if v.is_empty() {
        None
    } else {
        Some(v)
    }
}

/// 从 GitHub 官方 metadata 解析稳定版本号（仅 Windows 的 PowerShell 检测使用；
/// 单测跨平台引用，故非 Windows 下仅抑制 dead_code）。
#[cfg_attr(not(windows), allow(dead_code))]
pub(super) fn parse_pwsh_metadata(json: &serde_json::Value) -> Result<String, String> {
    json.get("StableReleaseTag")
        .or_else(|| json.get("ReleaseTag"))
        .and_then(|value| value.as_str())
        .map(|value| value.trim_start_matches('v').to_string())
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "metadata has no stable release tag".into())
}

/// 查询 PowerShell 官方最新稳定版。
///
/// 主用 GitHub Releases 列表（按 atom 顺序取首个稳定 tag）：官方 metadata.json 的
/// StableReleaseTag 更新滞后于发布（实测 7.6.5 发布后仍停留在 7.6.4），
/// 只在其上兜底会在补丁发布后漏报。GitHub API 失败时回退 metadata。
#[cfg(windows)]
pub(super) fn latest_pwsh_version() -> Result<String, String> {
    match github_latest_stable() {
        Ok(version) => return Ok(version),
        Err(github_error) => {
            crate::logging::log(&format!(
                "updater: PowerShell GitHub Releases 查询失败，回退官方 metadata：{github_error}"
            ));
        }
    }

    let metadata_result = runtime::check_client()
        .get("https://raw.githubusercontent.com/PowerShell/PowerShell/master/tools/metadata.json")
        .header("User-Agent", "DSHBox")
        .call()
        .map_err(|e| e.to_string())
        .and_then(|response| {
            response
                .into_body()
                .read_json::<serde_json::Value>()
                .map_err(|e| e.to_string())
        })
        .and_then(|json| parse_pwsh_metadata(&json));
    match metadata_result {
        Ok(version) => Ok(version),
        Err(metadata_error) => Err(format!(
            "{}: {metadata_error}",
            crate::locale::text(
                "获取 PowerShell 版本信息失败",
                "Failed to retrieve PowerShell version information"
            )
        )),
    }
}

/// 从 GitHub Releases 列表取按 atom 顺序的首个稳定 tag。
#[cfg(windows)]
fn github_latest_stable() -> Result<String, String> {
    // 用 releases.atom 页面绕开 GitHub API 限流；预发布 tag 按文本过滤即可
    let response = runtime::check_client()
        .get("https://github.com/PowerShell/PowerShell/releases.atom")
        .header("User-Agent", "DSHBox")
        .call()
        .map_err(|e| format!("GitHub Releases: {e}"))?;
    let mut text = String::new();
    response
        .into_body()
        .into_reader()
        .read_to_string(&mut text)
        .map_err(|e| format!("GitHub Releases: {e}"))?;
    latest_stable_tag(&parse_releases_atom(&text)).ok_or_else(|| {
        crate::locale::text(
            "GitHub Releases 中未找到稳定版本。",
            "No stable version was found in GitHub Releases.",
        )
        .to_string()
    })
}

/// 从 releases.atom 的 tag 列表取最新稳定版：过滤 -rc/-preview/-beta/-alpha
/// 四类预发布 tag，按 atom 顺序（最新发布在前）取首个稳定 tag，不再按
/// semver 取最大。与 check.rs 应用更新检查同一口径。
/// （仅 Windows 的生产代码引用；单测跨平台引用，故非 Windows 下仅抑制 dead_code。）
#[cfg_attr(not(windows), allow(dead_code))]
pub(super) fn latest_stable_tag(tags: &[String]) -> Option<String> {
    tags.iter()
        .map(|tag| tag.trim_start_matches('v').to_string())
        .find(|tag| {
            !tag.contains("-rc")
                && !tag.contains("-preview")
                && !tag.contains("-beta")
                && !tag.contains("-alpha")
        })
}

/// 解析 GitHub releases.atom 页面的 tag 列表（按发布顺序，最新在前）。
/// tag 取自每个 entry 的 `<link rel="alternate">` href 末段
/// （形如 .../releases/tag/v0.5.2）；title 可能是自定义发布名，不可靠。
pub(super) fn parse_releases_atom(xml: &str) -> Vec<String> {
    let mut out = Vec::new();
    for entry in xml.split("<entry>").skip(1) {
        let end = entry.find("</entry>").unwrap_or(entry.len());
        let entry = &entry[..end];
        let mut tag = None;
        for link in entry.split("<link").skip(1) {
            let seg = &link[..link.find('>').unwrap_or(link.len())];
            if !seg.contains("releases/tag/") {
                continue;
            }
            let Some(start) = seg.find("href=\"") else {
                continue;
            };
            let rest = &seg[start + 6..];
            let Some(quote) = rest.find('\"') else {
                continue;
            };
            let href = &rest[..quote];
            if let Some(last) = href.rsplit('/').next() {
                tag = Some(last.to_string());
                break;
            }
        }
        if let Some(t) = tag {
            out.push(t);
        }
    }
    out
}

/// 安装或更新 PowerShell 7（仅 Windows 有意义，其他平台给出明确提示）。
pub(super) fn update_pwsh(app: &AppHandle) -> Result<(), String> {
    #[cfg(windows)]
    {
        update_pwsh_windows(app)
    }
    #[cfg(not(windows))]
    {
        let _ = app;
        Err(crate::locale::text(
            "PowerShell 更新仅支持 Windows。",
            "PowerShell updates are supported only on Windows.",
        )
        .into())
    }
}

/// 安装或更新 PowerShell 7（通过 winget；机器级安装会弹出 UAC 授权）。
#[cfg(windows)]
fn update_pwsh_windows(app: &AppHandle) -> Result<(), String> {
    // UAC 预告在检查更新弹窗内展示并等待确认（不再用原生消息框——
    // 原生框无法可靠锚定到自绘弹窗上，位置/层级不可控）。
    // 弹窗关闭视为取消。
    let state = app.state::<AppState>();
    state.set_pwsh_confirmed(false);
    state.set_pwsh_pending(true);
    loop {
        if app.state::<AppState>().is_quitting() {
            state.set_pwsh_pending(false);
            return Err(crate::locale::text("应用已退出", "The app has quit").into());
        }
        if state.pwsh_confirmed() {
            break;
        }
        if !state.pwsh_pending() {
            return Err(crate::locale::text("已取消", "Cancelled").into());
        }
        std::thread::sleep(std::time::Duration::from_millis(200));
    }

    // 前置：确认 winget（微软应用安装程序）可用
    let (code, _out, err) = processes::run_capture(
        std::path::Path::new("winget"),
        &["--version".to_string()],
        &[],
        None,
    )
    .map_err(|e| {
        crate::locale::owned(
            format!("运行 winget 失败：{e}"),
            format!("Failed to run winget: {e}"),
        )
    })?;
    if code != 0 {
        let detail = truncate(&err, 300);
        return Err(crate::locale::owned(
            format!(
                "未找到 winget（微软应用安装程序）。\n请到微软官网下载 PowerShell 7 安装包手动安装。\n{detail}"
            ),
            format!(
                "winget (App Installer) was not found.\nDownload and install PowerShell 7 manually from Microsoft.\n{detail}"
            ),
        ));
    }

    let installed = pwsh_version().is_some();
    let action = if installed {
        crate::locale::text("更新", "Update")
    } else {
        crate::locale::text("安装", "Install")
    };
    let verb = if installed { "upgrade" } else { "install" };
    let progress = if installed {
        crate::locale::text("正在更新 PowerShell…", "Updating PowerShell…")
    } else {
        crate::locale::text("正在安装 PowerShell…", "Installing PowerShell…")
    };
    emit_progress(app, progress);
    let args = vec![
        verb.into(),
        "--id".into(),
        "Microsoft.PowerShell".into(),
        "--exact".into(),
        "--silent".into(),
        "--accept-package-agreements".into(),
        "--accept-source-agreements".into(),
    ];
    let (code, _out, err) =
        processes::run_capture(std::path::Path::new("winget"), &args, &[], None).map_err(|e| {
            crate::locale::owned(
                format!("运行 winget 失败：{e}"),
                format!("Failed to run winget: {e}"),
            )
        })?;
    if code != 0 {
        let detail = truncate(&err, 400);
        return Err(crate::locale::owned(
            format!("{action} PowerShell 失败（winget 退出码 {code}）：\n{detail}"),
            format!("PowerShell {action} failed (winget exit code {code}):\n{detail}"),
        ));
    }
    match pwsh_version() {
        Some(v) => {
            crate::logging::log(&format!("updater: PowerShell 就绪 v{v}"));
            Ok(())
        }
        None => Err(crate::locale::text(
            "winget 报告成功，但尚未检测到 pwsh，请稍后重试或重新打开 PowerShell 确认。",
            "winget reported success, but pwsh was not detected. Please retry later or reopen PowerShell to confirm.",
        )
        .into()),
    }
}

#[cfg(test)]
mod tests {
    use super::latest_stable_tag;

    fn tags(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| (*s).to_string()).collect()
    }

    #[test]
    fn latest_stable_tag_skips_all_prerelease_kinds() {
        // -beta tag 必须跳过（旧口径只滤 preview/-rc，会把 7.6.0-beta 当成最新）
        assert_eq!(
            latest_stable_tag(&tags(&["v7.6.0-beta.1", "v7.5.3", "v7.5.2"])),
            Some("7.5.3".to_string())
        );
        // 四类预发布（-rc/-preview/-beta/-alpha）全部过滤
        assert_eq!(
            latest_stable_tag(&tags(&[
                "v7.6.0-rc.1",
                "v7.6.0-preview.2",
                "v7.6.0-beta.1",
                "v7.5.0-alpha.3",
                "v7.5.3"
            ])),
            Some("7.5.3".to_string())
        );
        // 全是预发布时无稳定版
        assert_eq!(latest_stable_tag(&tags(&["v7.6.0-beta.1"])), None);
    }

    #[test]
    fn latest_stable_tag_takes_first_stable_in_atom_order() {
        // 旧补丁线版本列在稳定版之后时不误判为最新（按 atom 顺序取首个稳定 tag，
        // 不再扫描 semver 最大）
        assert_eq!(
            latest_stable_tag(&tags(&["v7.5.3", "v7.4.9"])),
            Some("7.5.3".to_string())
        );
    }
}
