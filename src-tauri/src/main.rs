// 发布版不弹出附加的控制台窗口。
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
// MSVC link.exe 会向 stdout 输出“正在创建库 …”（/NOLOGO 无法抑制），
// 被 rustc 报告为 linker_messages 警告——按预期允许，保持构建输出干净。
// 该 lint 只能在被链接的 crate 根（bin）控制，不能放在 lib 根。
// Rust 1.85 尚不认识 linker_messages；允许未知 lint 以兑现 manifest 的最低版本。
#![allow(unknown_lints)]
#![allow(linker_messages)]

//! DSHBox 入口。
//!
//! Windows：启动前检查 WebView2 Runtime，缺失时自动安装；
//! macOS/Linux：使用系统内置渲染（WKWebView/WebKitGTK），无需预检。

#[cfg(windows)]
#[path = "platform/windows/webview2.rs"]
mod webview2;

fn main() {
    // panic = "abort"：panic 信息默认输出到 GUI 应用不可见的 stderr。
    // 挂接 hook 把 panic 信息尽力写入应用日志（dshbox.log），
    // 写入失败时静默忽略；随后交还默认 hook，保留 stderr 输出与
    // RUST_BACKTRACE（dev 构建 panic=unwind 下依赖它），默认 abort 行为不变。
    let previous_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let payload = if let Some(s) = info.payload().downcast_ref::<&str>() {
            (*s).to_string()
        } else if let Some(s) = info.payload().downcast_ref::<String>() {
            s.clone()
        } else {
            String::from("（非字符串 panic 载荷）")
        };
        let location = info
            .location()
            .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()))
            .unwrap_or_default();
        dsh_box_lib::log_panic(&format!("{payload}（{location}）"));
        previous_hook(info);
    }));

    #[cfg(windows)]
    if !webview2::ensure_webview2() {
        return;
    }
    dsh_box_lib::run()
}
