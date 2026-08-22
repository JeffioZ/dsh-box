//! dsh 页面注入脚本使用的受限自定义协议。

use crate::*;
use tauri::Manager;

/// 处理注入脚本发来的 dshd:// 请求 —— dsh 页面 JS → Rust 的唯一通道
/// （页面无法使用 IPC：commands 会拒绝其来源；自定义协议由 WebView 网络层拦截，
/// 处理时再次校验主 WebView、当前 dsh 来源和进程级随机令牌）。
///
/// 权限与页面既有能力对齐：dsh 页面本就可以通过自己的后端“默认程序打开”任意
/// 本地文件，这里只是补充 定位/另存为/指定应用打开/复制内容/图标提取；
/// 只接受绝对路径，相对路径的工作区解析归 dsh 后端（“打开”菜单项直接复用
/// 页面按钮自身的点击逻辑）。
/// 请求形如 `http://dshd.localhost/<动作>?token=…&path=…`（Windows）或
/// `dshd://localhost/<动作>?token=…&path=…`（macOS/Linux），动作在路径段。
pub(crate) fn handle_dshd_scheme(
    ctx: tauri::UriSchemeContext<'_, tauri::Wry>,
    request: http::Request<Vec<u8>>,
) -> http::Response<Vec<u8>> {
    let parsed = url::Url::parse(&request.uri().to_string()).ok();
    // 平台 URL 形式不同：Windows 为 http://dshd.localhost/<动作>，
    // macOS/Linux 为 dshd://localhost/<动作>；动作取首个路径段，其余主机形式兼容取 host
    let action = parsed
        .as_ref()
        .map(|u| {
            let host = u.host_str().unwrap_or("");
            if host == "dshd.localhost" || host == "localhost" || host == "dshd" {
                u.path_segments()
                    .and_then(|mut s| s.next())
                    .unwrap_or("")
                    .to_string()
            } else {
                host.to_string()
            }
        })
        .unwrap_or_default();
    let query = |key: &str| {
        parsed.as_ref().and_then(|u| {
            u.query_pairs()
                .find(|(k, _)| k == key)
                .map(|(_, v)| v.into_owned())
        })
    };
    let path = query("path");
    let app = query("app");

    let state = ctx.app_handle().state::<AppState>();
    let config = state.config();
    let allowed_origin = config.web_url();
    let respond = |status, mime, body| scheme_response(status, mime, body, &allowed_origin);
    let current_is_dsh = main_webview(ctx.app_handle())
        .and_then(|webview| webview.url().ok())
        .is_some_and(|url| is_dsh_url(&url, &config));
    let authorized = ctx.webview_label() == MAIN_WINDOW
        && current_is_dsh
        && query("token").as_deref() == Some(state.protocol_token());
    if !authorized {
        logging::log("dshd: 拒绝未授权的自定义协议请求");
        return respond(403, "text/plain; charset=utf-8", b"forbidden".to_vec());
    }

    match (action.as_str(), path.as_deref()) {
        // 探测（前端问 VS Code 是否可用）
        ("probe", _) => {
            let body = match query("what").as_deref() {
                Some("vscode") if file_actions::vscode_exe().is_some() => "1",
                _ => "0",
            };
            respond(200, "text/plain; charset=utf-8", body.as_bytes().to_vec())
        }
        // 展开 ~/ 与 Windows MSYS /c/ 路径；只返回规范化后的绝对路径。
        ("normalize", Some(p)) => match file_actions::normalize_user_path(p) {
            Some(path) if path.is_absolute() => respond(
                200,
                "text/plain; charset=utf-8",
                path.to_string_lossy().into_owned().into_bytes(),
            ),
            _ => respond(400, "text/plain; charset=utf-8", b"invalid path".to_vec()),
        },
        // 菜单图标：icon?path=<文件>（关联应用图标）或 icon?app=code|notepad|paint
        ("icon", _) => {
            let source: Option<std::path::PathBuf> = if let Some(a) = app {
                match a.as_str() {
                    "code" => file_actions::vscode_exe(),
                    "notepad" => std::env::var("SystemRoot").ok().map(|r| {
                        std::path::PathBuf::from(r)
                            .join("System32")
                            .join("notepad.exe")
                    }),
                    "paint" => std::env::var("SystemRoot").ok().map(|r| {
                        std::path::PathBuf::from(r)
                            .join("System32")
                            .join("mspaint.exe")
                    }),
                    // 文件夹图标：取文件所在目录的系统图标
                    "folder" => path.as_deref().and_then(|p| {
                        if file_actions::is_absolute(p) {
                            std::path::Path::new(p)
                                .parent()
                                .map(std::path::PathBuf::from)
                        } else {
                            None
                        }
                    }),
                    _ => None,
                }
            } else {
                path.filter(|p| file_actions::is_absolute(p))
                    .map(std::path::PathBuf::from)
            };
            match source {
                Some(s) => match file_icons::icon_png_16(&s) {
                    Some(png) => respond(200, "image/png", png),
                    None => {
                        logging::log(&format!("dshd: 图标提取失败：{}", s.display()));
                        respond(404, "", Vec::new())
                    }
                },
                None => {
                    logging::log("dshd: 图标请求无有效来源");
                    respond(404, "", Vec::new())
                }
            }
        }
        // 复制文件内容：读文本（限 2MB、拒绝二进制/非 UTF-8）
        ("content", Some(p)) if file_actions::is_absolute(p) => {
            match file_actions::read_text_file(std::path::Path::new(p), 2 * 1024 * 1024) {
                Ok(text) => respond(200, "text/plain; charset=utf-8", text.into_bytes()),
                Err(_) => respond(415, "", Vec::new()),
            }
        }
        // 在默认浏览器打开链接（仅 http/https）
        ("browse", Some(p)) => {
            if p.starts_with("http://") || p.starts_with("https://") {
                if let Err(e) = file_actions::open_browser(p) {
                    logging::log(&format!("dshd: 打开浏览器失败：{e}"));
                }
                respond(204, "", Vec::new())
            } else {
                logging::log("dshd: 仅支持 http/https 链接");
                respond(204, "", Vec::new())
            }
        }
        // 另存为：系统保存对话框 + 拷贝（异步，弹窗期间不阻塞 WebView 网络回调）
        ("saveas", Some(p)) if file_actions::is_absolute(p) => {
            let app_handle = ctx.app_handle().clone();
            let src = p.to_string();
            std::thread::spawn(move || {
                use tauri_plugin_dialog::DialogExt;
                let name = std::path::Path::new(&src)
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "file".into());
                let mut builder = app_handle.dialog().file().set_file_name(&name);
                if let Some(win) = main_window(&app_handle) {
                    if win.is_visible().unwrap_or(false) {
                        builder = builder.set_parent(&win);
                    }
                }
                if let Some(dest) = builder
                    .blocking_save_file()
                    .and_then(|d| d.into_path().ok())
                {
                    if let Err(e) = std::fs::copy(&src, &dest) {
                        logging::log(&format!("dshd: 另存为失败：{e}"));
                    }
                }
            });
            respond(204, "", Vec::new())
        }
        // 用指定应用打开（code/notepad/paint，Windows）
        ("openapp", Some(p)) if file_actions::is_absolute(p) => {
            let result = match app.as_deref() {
                Some(a) => file_actions::open_with_app(a, std::path::Path::new(p)),
                None => {
                    Err(crate::locale::text("缺少 app 参数", "The app parameter is missing").into())
                }
            };
            match result {
                Ok(()) => logging::log(&format!("dshd: 指定应用打开已触发（{p}）")),
                Err(e) => logging::log(&format!("dshd: 指定应用打开失败：{e}")),
            }
            respond(204, "", Vec::new())
        }
        ("open", Some(p)) if file_actions::is_absolute(p) => {
            if file_actions::is_potentially_executable(std::path::Path::new(p)) {
                let app_handle = ctx.app_handle().clone();
                let path = p.to_string();
                std::thread::spawn(move || {
                    use tauri_plugin_dialog::MessageDialogKind;
                    let message = crate::locale::owned(
                        format!(
                            "该文件可能执行代码或修改系统：\n{path}\n\n仅在你信任其来源时继续。"
                        ),
                        format!(
                            "This file may execute code or modify the system:\n{path}\n\nContinue only if you trust its source."
                        ),
                    );
                    if crate::native_dialog::ask(
                        &app_handle,
                        message,
                        crate::locale::text("确认打开文件", "Confirm opening file"),
                        MessageDialogKind::Warning,
                        crate::locale::text("仍要打开", "Open anyway"),
                        crate::locale::text("取消", "Cancel"),
                    ) {
                        match file_actions::open_default(std::path::Path::new(&path)) {
                            Ok(()) => {
                                logging::log(&format!("dshd: 用户确认后打开可执行文件（{path}）"))
                            }
                            Err(e) => logging::log(&format!("dshd: 默认程序打开失败：{e}")),
                        }
                    } else {
                        logging::log("dshd: 用户取消打开可执行文件");
                    }
                });
                return respond(202, "", Vec::new());
            }
            match file_actions::open_default(std::path::Path::new(p)) {
                Ok(()) => logging::log(&format!("dshd: 默认程序打开已触发（{p}）")),
                Err(e) => logging::log(&format!("dshd: 默认程序打开失败：{e}")),
            }
            respond(204, "", Vec::new())
        }
        ("reveal", Some(p)) if file_actions::is_absolute(p) => {
            match file_actions::reveal(std::path::Path::new(p)) {
                Ok(()) => logging::log(&format!("dshd: 定位文件已触发（{p}）")),
                Err(e) => logging::log(&format!("dshd: 定位文件失败：{e}")),
            }
            respond(204, "", Vec::new())
        }
        ("openwith", Some(p)) if file_actions::is_absolute(p) => {
            // 系统“打开方式”对话框（SHOpenWithDialog）：独立 STA 线程模态执行，
            // 不阻塞 WebView2 回调线程；失败弹窗告知并记录 HRESULT
            let app_handle = ctx.app_handle().clone();
            let p2 = p.to_string();
            std::thread::spawn(move || {
                #[cfg(windows)]
                let hwnd =
                    main_window(&app_handle).and_then(|w| w.hwnd().ok().map(|h| h.0 as isize));
                #[cfg(not(windows))]
                let hwnd = None;
                match file_actions::open_with_picker(std::path::Path::new(&p2), hwnd) {
                    Ok(()) => logging::log(&format!("dshd: 打开方式已触发（{p2}）")),
                    Err(e) => {
                        logging::log(&format!("dshd: 打开方式失败：{e}"));
                        use tauri_plugin_dialog::MessageDialogKind;
                        crate::native_dialog::show_message(
                            &app_handle,
                            format!(
                                "{}: {e}",
                                crate::locale::text(
                                    "无法打开系统“打开方式”对话框",
                                    "Could not open the system Open with dialog"
                                )
                            ),
                            crate::locale::text("打开方式", "Open with"),
                            MessageDialogKind::Warning,
                        );
                    }
                }
            });
            respond(204, "", Vec::new())
        }
        (act, _) => {
            logging::log(&format!("dshd: 未处理请求：{act}"));
            respond(204, "", Vec::new())
        }
    }
}

/// 构造自定义协议响应：只允许当前 dsh 来源读取图标/文本。
fn scheme_response(
    status: u16,
    mime: &str,
    body: Vec<u8>,
    allowed_origin: &str,
) -> http::Response<Vec<u8>> {
    http::Response::builder()
        .status(status)
        .header("Access-Control-Allow-Origin", allowed_origin)
        .header("Vary", "Origin")
        .header("content-type", mime)
        .body(body)
        .unwrap_or_else(|_| http::Response::new(Vec::new()))
}
