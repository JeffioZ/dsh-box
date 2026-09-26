//! WebView 来源校验、页面注入与导航。

use crate::*;
use tauri::Manager;

/// Tauri 内置资源页面的精确来源白名单。
/// `dev_origin` 为当前构建 bake 的 devUrl（仅开发构建有值；生产为 None）：
/// 开发模式下内置页面直接从 devUrl 加载，需一并放行
pub(crate) fn is_local_app_url(url: &url::Url, dev_origin: Option<&url::Url>) -> bool {
    let builtin = ((url.scheme() == "tauri" && url.host_str() == Some("localhost"))
        || (url.scheme() == "http" && url.host_str() == Some("tauri.localhost")))
        && url.port().is_none();
    let dev_ok = dev_origin.is_some_and(|dev| {
        let url_host = url.host_str().map(String::from);
        let dev_host = dev.host_str().map(String::from);
        url.scheme() == dev.scheme()
            && url_host.as_deref() == dev_host.as_deref()
            && url.port() == dev.port()
    });
    (builtin || dev_ok) && url.username().is_empty() && url.password().is_none()
}

/// 当前构建 bake 的 devUrl 来源（开发构建注入，生产为 None）。
pub(crate) fn app_dev_origin(app: &AppHandle) -> Option<url::Url> {
    app.config().build.dev_url.clone()
}

/// 生产构建的启动页入口。Windows/Android 上 Tauri 以 http://tauri.localhost
/// 作为应用来源（tauri 的 tauri_protocol_url），tauri://localhost 形式在该平台
/// 没有协议处理器，运行时直接 Navigate 会被 WebView2 静默拒绝——重启过渡页
/// 与错误页曾因此永远回不到启动页。与 titlebar.rs 子 webview 重载 fallback
/// 同款平台分支；根路径形式与窗口创建时的初始 URL 一致（Tauri 会把
/// App("index.html") 简化为来源根）。
fn splash_entry_url() -> String {
    if cfg!(windows) {
        "http://tauri.localhost/".to_string()
    } else {
        SPLASH_ORIGIN.to_string()
    }
}

fn local_app_entry_url(dev_origin: Option<&url::Url>) -> String {
    dev_origin
        .map(url::Url::to_string)
        .unwrap_or_else(splash_entry_url)
}

/// 当前配置对应的 dsh 页面来源；端口必须与应用实际持有的服务一致。
pub(crate) fn is_dsh_url(url: &url::Url, config: &app_state::Config) -> bool {
    url.scheme() == "http"
        && url.host_str() == Some("127.0.0.1")
        && url.port_or_known_default() == Some(config.port)
        && url.username().is_empty()
        && url.password().is_none()
}

pub(crate) fn is_allowed_navigation(app: &AppHandle, url: &url::Url) -> bool {
    let dev = app_dev_origin(app);
    is_local_app_url(url, dev.as_ref()) || is_dsh_url(url, &app.state::<AppState>().config())
}

/// 注入 dsh 页面的初始化脚本（document start 执行，每次导航生效）：
/// 深色主题首帧预设：dsh 的 CSS 用 `body[data-ds-dark-theme]` 选择器，消除
/// “深 loading → 白 dsh → 深 dsh”的首帧闪（dsh 挂载后自行接管主题）。
/// document-start 时 <body> 尚不存在，用 MutationObserver 等其出现后再设。
pub(crate) const PAGE_INIT_SCRIPT: &str = r#"
if (window.matchMedia && window.matchMedia('(prefers-color-scheme: dark)').matches) {
  var applyDsDark = function () { document.body.setAttribute('data-ds-dark-theme', ''); };
  if (document.body) {
    applyDsDark();
  } else {
    var dsDarkObs = new MutationObserver(function () {
      if (document.body) { applyDsDark(); dsDarkObs.disconnect(); }
    });
    // 观察目标必须是 document：本段在文档创建时刻执行，彼时
    // documentElement 还是 null（observe 抛错会中止整个初始化脚本——
    // 2026-09-25 探针实证，浅色主题下深色预设分支才走到这里）。
    // 且必须带 subtree：body 是 html 的后代，对 document 的无 subtree
    // childList 观察只能看到 html 本身的插入（彼时 body 仍为 null），
    // 回调此后不再触发、深色预设永不生效——被遮罩掩盖的静默失效
    dsDarkObs.observe(document, { childList: true, subtree: true });
  }
}
"#;

/// 右键菜单定制脚本：dsh 已处理右键（defaultPrevented/stopPropagation）则放行其自带
/// 菜单；否则屏蔽 WebView 默认菜单（后退/刷新/保存图片等网页操作），改为弹自绘菜单：
/// 可编辑区弹 剪切/复制/粘贴/全选（禁用态+快捷键提示），文件路径按钮弹
/// 打开/打开文件所在位置/打开方式/复制路径。样式用 dsh 设计系统变量构建——深浅色、
/// DPI 均自适应。经 navigate 后的 eval 注入（initialization_script 对外部 URL 导航不可靠）。
const EDIT_CONTEXT_INJECT: &str = include_str!("../../../ui/edit-context.js");
const MENU_INJECT: &str = include_str!("../../resources/injections/context-menu.js");
/// 引导期续接遮罩（见文件头注释）：图标占位在编译期替换为与启动页同源的
/// ui/assets/app-icon.svg 内联内容——注入目标是 dsh 远程文档，取不到本地资源。
/// SVG 的换行必须剥掉：占位符落在 JS 单引号字符串里，多行 SVG 会让字符串
/// 跨行、整段注入脚本语法解析失败（菜单/心跳/标题修正等一并全灭——
/// 2026-09-19 引入后静默回归至 2026-09-23 才定位）。
pub(crate) fn boot_continue_inject() -> &'static str {
    static CACHE: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    CACHE.get_or_init(|| {
        let icon: String = include_str!("../../../ui/assets/app-icon.svg")
            .chars()
            .filter(|c| *c != '\n' && *c != '\r')
            .collect();
        include_str!("../../resources/injections/boot-continue.js").replace("__DSHD_ICON__", &icon)
    })
}

/// 隐藏工具调用开关开启时注入的样式脚本（navigate 注入与托盘切换共用）。
const HIDE_TOOLS_APPLY: &str = "var __h=document.getElementById('__dshd_hide_tools');if(!__h){var s=document.createElement('style');\
s.id='__dshd_hide_tools';s.textContent='[data-tool]{display:none!important}';\
document.documentElement.appendChild(s);}";
/// 开关关闭时移除该样式。
const HIDE_TOOLS_CLEAR: &str =
    "var __h=document.getElementById('__dshd_hide_tools');if(__h)__h.remove();";

/// 应用“隐藏工具调用”开关到 dsh 页面：开启注入隐藏样式，关闭移除。
/// 导航注入与菜单切换共用同一逻辑。
pub fn apply_hide_tools(app: &AppHandle) {
    let hide = app.state::<AppState>().config().hide_tool_calls;
    let script = if hide {
        HIDE_TOOLS_APPLY
    } else {
        HIDE_TOOLS_CLEAR
    };
    if let Some(wv) = main_webview(app) {
        let _ = wv.eval(script.to_string());
    }
}

/// 页面心跳注入：dsh 页面定期上报存活标记与当前选中会话。
/// 页面主线程挂起/崩溃时 setInterval 停摆，Rust 侧据此重载自愈（见 heartbeat.rs）。
/// 上报走 dshd:// 自定义协议（dshdFetch，令牌经 X-DSHd-Token 头）：dsh 页
/// 是远程来源，window.__TAURI__.core.invoke 会被 IPC 层拒绝、永远送不到。
/// 依赖 MENU_INJECT（context-menu.js）先注入的 dshdFetch/dshSelectedSession，
/// 只在下方组合脚本的 {menu} 之后使用。
const HEARTBEAT_INJECT: &str = r#"
if (!window.__dshdHeartbeat) {
  window.__dshdHeartbeat = true;
  var lastSelection = '', lastBeat = 0;
  function reportPage() {
    try {
      var selected = dshSelectedSession();
      var key = JSON.stringify(selected);
      // 1s 检测、选择不变时至少间隔 10s 才上报：会话切换立即同步，
      // 稳态心跳节奏不随检测频率上涨（协议请求量与看门狗判死口径不变）。
      if (key === lastSelection && Date.now() - lastBeat < 10000) return;
      lastSelection = key; lastBeat = Date.now();
      dshdFetch('heartbeat', 'known=' + (selected.known ? 1 : 0)
        + (selected.id ? '&sid=' + encodeURIComponent(selected.id) : '')).catch(function () {});
    } catch (e) {}
  }
  reportPage();
  setInterval(reportPage, 1000);
}
"#;

/// 注入脚本的语言内联值：与 title/token 同帧写入 window.__DSHD_LANG，
/// 右键菜单（context-menu.js）首帧即按应用语言渲染，不等托盘语言跟随的 eval。
fn injected_language() -> String {
    serde_json::to_string(crate::locale::code()).unwrap_or_default()
}

/// 组装 dsh 页面完整增强脚本（各段注入 + 独立 try 隔离）。独立成函数是
/// 为了让语法防御测试能对**组装产物**做 node --check——逐段校验测不出
/// 组装层的破坏（2026-09-26：`//` 注释进入 `\` 续行模板后被拼成单行，
/// 注释吞掉 try{ 与首段首行，整段 eval 语法错误——恰是 09-19「所有注入
/// 全灭」故障模式的复刻）。**模板内只能用 `/* */` 块注释**：Rust 的行续行
/// 不产生换行，`//` 注释会吞到下一段内容为止。
fn combined_injection_script(
    protocol_token: &str,
    language: &str,
    title: &str,
    hide_tools: &str,
) -> String {
    format!(
        "(() => {{ \
         if (window.__dshdInit === 'loading' || window.__dshdInit === 'ready') return; \
         window.__dshdInit = 'loading'; \
         window.__dshdProtocolToken = {protocol_token}; \
         window.__DSHD_LANG = {language}; \
         try {{ \
           const t = {title}; \
           let dshSessionTitle = ''; \
           const fix = () => {{ \
             const current = document.title; \
             const split = current.lastIndexOf(' — '); \
             if (split > 0) dshSessionTitle = current.slice(0, split); \
             if (current !== t) document.title = t; \
           }}; \
           fix(); \
           try {{ \
             const el = document.querySelector('head > title'); \
             if (el) new MutationObserver(fix).observe(el, {{ childList: true }}); \
           }} catch (e) {{}} \
           /* 每段独立隔离：一段的运行时抛错不得中止同块后续段（09-19 教训 \
           的运行时补全——node --check 只防语法；外层 catch 会删 \
           __dshdInit 触发整段重试，对确定性运行时错误同样无效） */ \
           try {{ {boot_continue} }} catch (e) {{}} \
           try {{ {edit_context} }} catch (e) {{}} \
           try {{ {menu} }} catch (e) {{}} \
           try {{ {heartbeat} }} catch (e) {{}} \
           try {{ {hide_tools} }} catch (e) {{}} \
           window.__dshdInit = 'ready'; \
         }} catch (error) {{ \
           delete window.__dshdInit; \
           throw error; \
         }} finally {{ \
           delete window.__dshdProtocolToken; \
         }} \
         }})();",
        boot_continue = boot_continue_inject(),
        edit_context = EDIT_CONTEXT_INJECT,
        menu = MENU_INJECT,
        heartbeat = HEARTBEAT_INJECT,
    )
}

/// 构造并注入 dsh 页面完整增强脚本。由 page-load 主路径与 navigate 定时
/// 兜底共用；脚本内部以 __dshdInit 保证同一 document 只安装一次监听器，
/// 各注入段独立 try 隔离——一段运行时抛错不灭后续段。
pub(crate) fn inject_dsh_page(app: &AppHandle, webview: &tauri::Webview) -> Result<(), String> {
    let config = app.state::<AppState>().config();
    let url = webview.url().map_err(|error| error.to_string())?;
    if !is_dsh_url(&url, &config) {
        return Ok(());
    }
    let title = serde_json::to_string(APP_TITLE).unwrap_or_default();
    // 令牌一帧内交给注入脚本后即从全局对象删除；注入端只以 X-DSHd-Token
    // 请求头回传——任何 URL 都不得携带令牌（resource timing 缓冲区同源可见）
    let protocol_token =
        serde_json::to_string(app.state::<AppState>().protocol_token()).unwrap_or_default();
    let language = injected_language();
    let hide_tools = if config.hide_tool_calls {
        HIDE_TOOLS_APPLY
    } else {
        ""
    };
    let script = combined_injection_script(&protocol_token, &language, &title, hide_tools);
    webview.eval(script).map_err(|error| error.to_string())
}

/// 让 WebView 跳到 dsh 界面（或返回本地启动页）。
pub fn navigate(app: &AppHandle, url: &str) {
    let Some(wv) = main_webview(app) else {
        // boot 线程早于窗口创建启动时的快路径（外部服务接入/并发就绪可
        // 在窗口建完前到达此处）：直接丢弃导航会永久停在启动页。延迟重试
        // 直到 webview 出现；窗口创建失败由 fatal_boot_exit 收尾，轮询以
        // is_quitting 与 15s 截止兜底，不会悬挂。重试路径 webview 已存在，
        // 不会再进本分支（无递归链）。
        logging::log(&format!("navigate: 主 webview 未就绪，延迟重试 {url}"));
        let handle = app.clone();
        let target = url.to_string();
        std::thread::spawn(move || {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
            while std::time::Instant::now() < deadline {
                if handle.state::<AppState>().is_quitting() {
                    return;
                }
                if crate::main_webview(&handle).is_some() {
                    navigate(&handle, &target);
                    return;
                }
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            logging::log("navigate: 等待主 webview 超时，放弃本次导航");
        });
        return;
    };
    if let Ok(u) = url::Url::parse(url) {
        if !is_allowed_navigation(app, &u) {
            logging::log(&format!("navigate: 已拒绝非白名单地址 {url}"));
            return;
        }
        logging::log(&format!("navigate: {url}"));
        let navigating_to_dsh = is_dsh_url(&u, &app.state::<AppState>().config());
        if navigating_to_dsh {
            // 即使首次注入完全失败，也让心跳监视在超时后触发一次 reload 自愈，
            // 避免 last_heartbeat=None 导致永久不检查。（版本信息常驻标题栏；
            // 进度条不做页面交接——window.name 跨站点被清空，机制已移除）
            app.state::<AppState>().set_heartbeat();
        }
        if let Err(error) = wv.navigate(u) {
            logging::log(&format!("navigate: 导航 {url} 失败：{error}"));
        }
        // dsh 页面挂载时会用自带 document.title 覆盖窗口标题。
        // 两层保障：立即 set_title（窗口级，立刻生效）；
        // 页面加载完成后注入常驻脚本，任意时刻的 title 变化都会被拉回产品名。
        if let Some(win) = main_window(app) {
            let _ = win.set_title(APP_TITLE);
        }
        if navigating_to_dsh {
            let handle = app.clone();
            std::thread::spawn(move || {
                std::thread::sleep(std::time::Duration::from_millis(1500));
                // 注入失败自愈：eval 返回错误时（页面加载中、导航瞬间执行被拒）
                // 补注——eval 成功即注入完成（guard 在 IIFE 首行置位，重复注入
                // 幂等），最多 3 次尝试（1.5s/3.5s/5.5s）。
                for attempt in 0..3 {
                    let Some(wv) = main_webview(&handle) else {
                        return;
                    };
                    if inject_dsh_page(&handle, &wv).is_ok()
                        && wv.url().ok().is_some_and(|url| {
                            is_dsh_url(&url, &handle.state::<AppState>().config())
                        })
                    {
                        return;
                    }
                    if attempt < 2 {
                        std::thread::sleep(std::time::Duration::from_secs(2));
                    }
                }
            });
        }
    }
}

/// 返回内置启动页。开发构建的资源来自 devUrl，正式构建才使用 Tauri 协议。
pub fn navigate_to_splash(app: &AppHandle) {
    let dev_origin = app_dev_origin(app);
    navigate(app, &local_app_entry_url(dev_origin.as_ref()));
}

#[cfg(test)]
mod tests {
    use super::{
        boot_continue_inject, combined_injection_script, injected_language, is_local_app_url,
        local_app_entry_url, EDIT_CONTEXT_INJECT, HEARTBEAT_INJECT, HIDE_TOOLS_APPLY, MENU_INJECT,
        PAGE_INIT_SCRIPT,
    };

    /// 注入脚本的语法防御：任一段落语法坏会让整段 eval 解析失败——
    /// 右键菜单、心跳、标题修正等全部静默丢失（2026-09-19 内联多行 SVG
    /// 进单引号字符串即此类回归，静默存活四天才定位）。有 node 可用时对
    /// 每段生成物与注入资源做 --check（含内联 r#"…"# 模板段——同属该
    /// 故障模式）；无 node 的环境跳过（CI 与开发机都有 node）。
    #[test]
    fn injection_scripts_pass_node_syntax_check() {
        let Some(node) = find_node() else {
            eprintln!("node 不可用，跳过注入脚本语法校验");
            return;
        };
        let segments: [(&str, String); 5] = [
            ("boot-continue", boot_continue_inject().to_string()),
            ("menu", MENU_INJECT.to_string()),
            ("edit-context", EDIT_CONTEXT_INJECT.to_string()),
            ("page-init", PAGE_INIT_SCRIPT.to_string()),
            ("heartbeat", HEARTBEAT_INJECT.to_string()),
        ];
        for (name, script) in segments {
            let file = std::env::temp_dir().join(format!("dshbox-inject-check-{name}.js"));
            std::fs::write(&file, &script).unwrap();
            let status = std::process::Command::new(&node)
                .arg("--check")
                .arg(&file)
                .status()
                .unwrap();
            let _ = std::fs::remove_file(&file);
            assert!(
                status.success(),
                "注入段 {name} 语法错误：整段 eval 会静默失效"
            );
        }
    }

    fn find_node() -> Option<std::path::PathBuf> {
        // PATH 探测：spawn 失败（找不到可执行）即视为无 node。
        std::process::Command::new("node")
            .arg("--version")
            .status()
            .ok()
            .filter(|status| status.success())
            .map(|_| std::path::PathBuf::from("node"))
    }

    /// 组装产物的语法防御：逐段校验测不出组装层的破坏（2026-09-26 现场：
    /// `//` 注释进入 `\` 续行模板，Rust 行续行不产生换行，注释吞掉 try{ 与
    /// 首段首行，整段 eval 语法错误——菜单/心跳/遮罩/标题修正全部静默
    /// 全灭）。对真实组装函数的产物做 --check，模板今后任何拼接层回归
    /// （注释、括号、占位符错位）都在此处拦截。
    #[test]
    fn combined_injection_passes_node_syntax_check() {
        let Some(node) = find_node() else {
            eprintln!("node 不可用，跳过注入脚本语法校验");
            return;
        };
        // 两个变体都过：hide_tools 关（空串）/开（样式段进入 try 块）
        for (name, script) in [
            (
                "combined-hide-off",
                combined_injection_script("\"t\"", "\"zh\"", "\"DSHBox\"", ""),
            ),
            (
                "combined-hide-on",
                combined_injection_script("\"t\"", "\"zh\"", "\"DSHBox\"", HIDE_TOOLS_APPLY),
            ),
        ] {
            let file = std::env::temp_dir().join(format!("dshbox-inject-check-{name}.js"));
            std::fs::write(&file, &script).unwrap();
            let status = std::process::Command::new(&node)
                .arg("--check")
                .arg(&file)
                .status()
                .unwrap();
            let _ = std::fs::remove_file(&file);
            assert!(
                status.success(),
                "组装脚本 {name} 语法错误：整段 eval 会静默失效"
            );
        }
    }

    /// 内联 SVG 必须是单行：占位符落在 JS 单引号字符串里，跨行即语法
    /// 错误（node --check 防御的第一道快断言，无 node 也生效）。
    #[test]
    fn inlined_svg_stays_single_line() {
        let script = boot_continue_inject();
        let start = script.find("<svg").expect("svg inlined");
        let end = start + script[start..].find("</svg>").expect("svg closed");
        assert!(
            !script[start..end].contains('\n'),
            "内联 SVG 跨行会破坏承载它的单引号字符串"
        );
    }

    // ---------- 启动页/引导遮罩 同源守卫 ----------

    const COMMON_CSS: &str = include_str!("../../../ui/common.css");
    const STARTUP_CSS: &str = include_str!("../../../ui/startup.css");
    const I18N_JS: &str = include_str!("../../../ui/i18n.js");

    fn nospace(text: &str) -> String {
        text.chars().filter(|c| !c.is_whitespace()).collect()
    }

    /// 比色归一：去空白 + 压掉逗号后的前导零（`rgba(0,0,0,0.1)` 与
    /// `rgba(0,0,0,.1)` 是同一颜色，两侧书写不同档）。
    fn css_norm(text: &str) -> String {
        nospace(text).replace(",0.", ",.")
    }

    /// 从 CSS 文本解析 `--token: value;`（取首个匹配）。
    fn css_var(css: &str, token: &str) -> String {
        let marker = format!("--{token}:");
        let start = css
            .find(&marker)
            .unwrap_or_else(|| panic!("缺少令牌 {token}"));
        let value = &css[start + marker.len()..];
        value[..value.find(';').expect("令牌值未终止")]
            .trim()
            .to_string()
    }

    /// 引导遮罩（boot-continue.js 内联字面量，注入跨源文档无法引用本壳 CSS）
    /// 与启动页视觉必须逐值一致：帧级衔接靠「同一段 loading」成立，任一处
    /// 漂移都会在切换瞬间可见。此测试把 common.css 令牌与两侧规格对账——
    /// 改任一侧都必须同步另一侧（遮罩侧是手抄字面量，本测试即其防漂移源）。
    #[test]
    fn mask_splash_parity_holds() {
        let mask = boot_continue_inject();
        let mask_n = nospace(mask);
        let startup_n = nospace(STARTUP_CSS);
        let common_n = nospace(COMMON_CSS);

        // 颜色：common.css 深色（默认块）+ 浅色（prefers-color-scheme: light 块）
        // 的令牌值必须作为字面量出现在遮罩里
        let light_at = COMMON_CSS
            .find("@media (prefers-color-scheme: light)")
            .expect("浅色令牌块");
        for css in [&COMMON_CSS[..light_at], &COMMON_CSS[light_at..]] {
            for token in ["dshd-bg", "dshd-text", "dshd-text-dim"] {
                let value = css_var(css, token);
                assert!(
                    mask_n.contains(&nospace(&value)),
                    "遮罩缺少令牌 --{token} 的字面量 {value}"
                );
            }
        }

        // 字号：遮罩内联值 ↔ common.css 令牌值
        for (mask_size, token) in [("20px", "2xl"), ("14px", "base"), ("12px", "sm")] {
            assert!(mask_n.contains(&format!("font-size:{mask_size}")));
            assert!(
                common_n.contains(&format!("--dshd-fs-{token}:{mask_size}")),
                "common.css 令牌 --dshd-fs-{token} 不再是 {mask_size}，需同步遮罩"
            );
        }

        // 承重几何：卡片行高/占位、进度条圆角——双侧同值（footer 已移除，
        // 版本信息常驻标题栏；占位行在遮罩是空 div 定高、在启动页是
        // min-height，书写不同值相同）
        for (mask_spec, startup_spec) in [
            ("min-height:20px", "min-height:20px"),
            ("height:18px", "min-height:18px"),
            ("height:28px", "min-height:28px"),
            ("border-radius:999px", "border-radius:999px"),
        ] {
            assert!(mask_n.contains(mask_spec), "遮罩缺少规格 {mask_spec}");
            assert!(
                startup_n.contains(startup_spec),
                "启动页缺少规格 {startup_spec}"
            );
        }
        // 行高：遮罩 line-height 内联，启动页经 body 的 font 简写斜杠书写
        assert!(mask_n.contains("line-height:1.5"));
        assert!(
            startup_n.contains("var(--dshd-fs-md)/1.5"),
            "启动页 body 行高不再是 1.5，需同步遮罩"
        );

        // 背景径向渐变逐值同源（startup.css body ↔ 遮罩 el + 调色板 g1）
        assert!(
            startup_n.contains("radial-gradient(1200px600pxat50%-10%,#191a1f0%,var(--dshd-bg)60%)"),
            "启动页深色渐变规格漂移"
        );
        assert!(startup_n.contains("#edeef40%"), "启动页浅色渐变起点漂移");
        assert!(
            mask_n.contains("radial-gradient(1200px600pxat50%-10%,'+c.g1+'0%,'+c.bg+'60%)"),
            "遮罩渐变代码漂移"
        );
        assert!(mask.contains("'#191a1f'") && mask.contains("'#edeef4'"));

        // 进度条配色逐值对账：两主题的 accent/accent-hover/border-strong
        // 令牌 ↔ 遮罩调色板 a1/a2/track（浅色 a2 曾漂移成深色档 #679efe，
        // 切换瞬间进度条变色，本对账即其拦截器）
        for css in [&COMMON_CSS[..light_at], &COMMON_CSS[light_at..]] {
            let accent = css_norm(&css_var(css, "dshd-accent"));
            let hover = css_norm(&css_var(css, "dshd-accent-hover"));
            assert!(
                mask_n.contains(&format!("a1:'{accent}',a2:'{hover}'")),
                "遮罩进度条渐变端点与令牌漂移：a1 {accent} / a2 {hover}"
            );
            let track = css_norm(&css_var(css, "dshd-border-strong"));
            assert!(
                mask_n.contains(&format!("track:'{track}'")),
                "遮罩进度条轨道色与 --dshd-border-strong 漂移：{track}"
            );
        }
        // 启动页侧引用同名令牌（.bar 轨道 / .bar-fill 渐变）——令牌改动须
        // 双侧联动，这里钉住引用本身
        assert!(
            startup_n.contains("background:var(--dshd-border-strong)"),
            "启动页 .bar 轨道不再引用 --dshd-border-strong"
        );
        assert!(
            startup_n
                .contains("linear-gradient(90deg,var(--dshd-accent),var(--dshd-accent-hover))"),
            "启动页 .bar-fill 渐变令牌引用漂移"
        );

        // 进度条动画逐值：关键帧四停靠点（nospace 后同形）与 1.4s 周期
        // 双侧对齐（进度条无相位交接——window.name 跨站点被清空，机制已
        // 移除；两侧周期不一致才是要拦的漂移）。遮罩侧是 JS 源码里两个字面量
        // 拼接，中缝为 `'+`，按源码形态匹配
        let css_stops = "0%{transform:translateX(-167%);opacity:0;}".to_string()
            + "14%{opacity:1;}86%{opacity:1;}"
            + "100%{transform:translateX(917%);opacity:0;}";
        let mask_stops = "0%{transform:translateX(-167%);opacity:0;}'+'".to_string()
            + "14%{opacity:1;}86%{opacity:1;}"
            + "100%{transform:translateX(917%);opacity:0;}}";
        assert!(mask_n.contains(&mask_stops), "遮罩 slide 关键帧漂移");
        assert!(startup_n.contains(&css_stops), "启动页 slide 关键帧漂移");
        assert!(mask_n.contains("1.4slinearinfinite"), "遮罩进度条周期漂移");
        assert!(
            startup_n.contains("slide1.4slinearinfinite"),
            "启动页进度条周期漂移"
        );

        // 进度条几何：轨道高 5px / 行程宽 12%
        assert!(
            mask_n.contains("height:5px;width:100%"),
            "遮罩进度条轨道几何漂移"
        );
        assert!(mask_n.contains("width:12%"), "遮罩进度条行程宽度漂移");
        assert!(startup_n.contains("height:5px"), "启动页进度条轨道高度漂移");
        assert!(startup_n.contains("width:12%"), "启动页进度条行程宽度漂移");

        // 末帧/首帧文案逐字一致（i18n.js loading 键 ↔ 遮罩字面量）
        assert!(
            I18N_JS.contains("['正在加载…', 'Loading…']"),
            "i18n loading 键与遮罩文案需逐字一致"
        );
        assert!(mask.contains("'正在加载…'") && mask.contains("'Loading…'"));
    }

    #[test]
    fn local_app_entry_uses_dev_origin_when_configured() {
        let dev_origin = "http://localhost:4321".parse().unwrap();
        assert_eq!(
            local_app_entry_url(Some(&dev_origin)),
            "http://localhost:4321/"
        );
    }

    #[test]
    fn local_app_entry_uses_navigable_form_in_production() {
        let entry = local_app_entry_url(None);
        // 生产入口必须是白名单内的本地页面，且在当前平台可被运行时导航
        // （Windows 的 tauri:// 形式会被 WebView2 静默拒绝，见 splash_entry_url）。
        let url = url::Url::parse(&entry).unwrap();
        assert!(is_local_app_url(&url, None));
        if cfg!(windows) {
            assert_eq!(entry, "http://tauri.localhost/");
        } else {
            assert_eq!(entry, crate::SPLASH_ORIGIN);
        }
    }

    #[test]
    fn injected_language_is_a_quoted_supported_locale() {
        // 与 locale::code() 的输出契约一致：右键菜单据此判断中英文
        assert!(matches!(
            injected_language().as_str(),
            "\"zh-CN\"" | "\"en\""
        ));
    }
}
