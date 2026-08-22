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
pub(crate) const PAGE_INIT_SCRIPT: &str = r#"
if (window.matchMedia && window.matchMedia('(prefers-color-scheme: dark)').matches) {
  document.documentElement.dataset.dsDarkTheme = '';
}
"#;

/// 右键菜单定制脚本：dsh 已处理右键（defaultPrevented/stopPropagation）则放行其自带
/// 菜单；否则屏蔽 WebView 默认菜单（后退/刷新/保存图片等网页操作），改为弹自绘菜单：
/// 可编辑区弹 剪切/复制/粘贴/全选（禁用态+快捷键提示），文件路径按钮弹
/// 打开/打开文件所在位置/打开方式/复制路径。样式用 dsh 设计系统变量构建——深浅色、
/// DPI 均自适应。经 navigate 后的 eval 注入（initialization_script 对外部 URL 导航不可靠）。
const MENU_INJECT: &str = include_str!("../../resources/injections/context-menu.js");

/// 隐藏工具调用开关开启时注入的样式脚本（navigate 注入与托盘切换共用）。
const HIDE_TOOLS_APPLY: &str = "var __h=document.getElementById('__dshd_hide_tools');if(!__h){var s=document.createElement('style');\
s.id='__dshd_hide_tools';s.textContent='[data-tool]{display:none!important}';\
document.documentElement.appendChild(s);}";
/// 开关关闭时移除该样式。
const HIDE_TOOLS_CLEAR: &str =
    "var __h=document.getElementById('__dshd_hide_tools');if(__h)__h.remove();";

/// 隐藏会话统计行（开关开启时注入）：CSS 按当前版本 class 隐藏
/// StatsLine，另挂文本特征 fallback + MutationObserver 补位——dsh 更新
/// 后 class 变化时 fallback 仍能隐藏；两路都失效则统计行重新出现
/// （静默降级，不影响任何功能）。
/// 隐藏 StatsLine 的 CSS（initialization_script 首帧注入与 navigate 注入共用，
/// 同一份定义避免双份拷贝；style id 与 fallback 脚本共用 guard）。
/// 注意：属性选择器必须用双引号——注入脚本以 JS 单引号字符串承载本 CSS，
/// 内含单引号会破坏脚本语法（曾因此使整段注入失效）。
const HIDE_STATS_CSS: &str =
    "[data-slot=\"conversation.composer.dock\"] .FJxK0a_root{display:none!important}";

/// 隐藏会话统计行（开关开启时注入）：CSS 按当前版本 class 隐藏
/// StatsLine，另挂文本特征 fallback + MutationObserver 补位——dsh 更新
/// 后 class 变化时 fallback 仍能隐藏；两路都失效则统计行重新出现
/// （静默降级，不影响任何功能）。
pub(crate) fn hide_stats_apply() -> String {
    format!(
        "{head}{css}{tail}",
        head = r#"window.__dshdHideStats = true;
if (!document.getElementById('__dshd_hide_stats')) {
  var s = document.createElement('style');
  s.id = '__dshd_hide_stats';
  s.textContent = '"#,
        css = HIDE_STATS_CSS,
        tail = r#"';
  document.documentElement.appendChild(s);
}
if (!window.__dshdHideStatsObs) {
  window.__dshdHideStatsObs = true;
  var dockSel = '[data-slot="conversation.composer.dock"]';
  var statsRe = /(轮|步|turns|steps)/i;
  function sweepStats() {
    var dock = document.querySelector(dockSel);
    if (!dock) return;
    var matches = [];
    dock.querySelectorAll('div').forEach(function (el) {
      var t = el.textContent || '';
      // dsh 统计行以「·」分组（本壳状态栏同款文案），「|」为兼容旧版
      if (t.length < 12 || (t.indexOf('|') < 0 && t.indexOf('·') < 0) || !statsRe.test(t)) return;
      matches.push(el);
    });
    matches.forEach(function (el) {
      // textContent 会让祖先也命中；只处理没有更小匹配后代的叶端候选，
      // 避免误隐藏整个 composer/input 容器。
      if (matches.some(function (other) { return other !== el && el.contains(other); })) return;
      if (window.__dshdHideStats) {
        if (!el.__dshdHiddenStats) { el.__dshdHiddenStats = true; el.style.display = 'none'; }
      } else if (el.__dshdHiddenStats) {
        el.__dshdHiddenStats = false; el.style.display = '';
      }
    });
  }
  var timer = null;
  var obs = new MutationObserver(function () {
    if (timer) return;
    timer = setTimeout(function () { timer = null; sweepStats(); }, 200);
  });
  obs.observe(document.documentElement, { childList: true, subtree: true });
  sweepStats();
}
"#,
    )
}

/// initialization_script 首帧注入：dsh 页面挂载前即隐藏 StatsLine，消除
/// “统计行先出现在输入框下方、注入后跳走”的闪动。与 navigate 后注入的
/// 完整脚本共用同一 style id——后者 guard 命中即跳过，开关关闭一并移除。
/// 外部 URL 导航的 initialization_script 可靠性不足，navigate 注入仍是兜底。
pub(crate) fn hide_stats_early() -> String {
    format!(
        "try{{var __hs=document.getElementById('__dshd_hide_stats');\
         if(!__hs){{var __hst=document.createElement('style');\
         __hst.id='__dshd_hide_stats';__hst.textContent='{css}';\
         document.documentElement.appendChild(__hst);}}}}catch(e){{}}",
        css = HIDE_STATS_CSS
    )
}

/// 关闭隐藏：移除样式并恢复 fallback 隐藏的元素。
const HIDE_STATS_CLEAR: &str = r#"
window.__dshdHideStats = false;
var s = document.getElementById('__dshd_hide_stats');
if (s) s.remove();
var dock = document.querySelector('[data-slot="conversation.composer.dock"]');
if (dock) {
  dock.querySelectorAll('div').forEach(function (el) {
    if (el.__dshdHiddenStats) { el.__dshdHiddenStats = false; el.style.display = ''; }
  });
}
"#;

/// 应用“隐藏会话统计行”开关到 dsh 页面（设置页切换与导航注入共用）。
pub fn apply_hide_stats(app: &AppHandle) {
    let hide = app.state::<AppState>().config().hide_stats_line;
    let script = if hide {
        hide_stats_apply()
    } else {
        HIDE_STATS_CLEAR.to_string()
    };
    if let Some(wv) = main_webview(app) {
        let _ = wv.eval(script);
    }
}

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

/// 页面心跳注入：dsh 页面每 10s 上报一次存活标记。
/// 页面主线程挂起/崩溃时 setInterval 停摆，Rust 侧据此重载自愈（见 heartbeat.rs）。
const HEARTBEAT_INJECT: &str = r#"
if (!window.__dshdHeartbeat) {
  window.__dshdHeartbeat = true;
  setInterval(function () {
    try {
      window.__TAURI__.core.invoke('page_heartbeat').catch(function () {});
    } catch (e) {}
  }, 10000);
}
"#;

/// 构造并注入 dsh 页面完整增强脚本。由 page-load 主路径与 navigate 定时
/// 兜底共用；脚本内部以 __dshdInit 保证同一 document 只安装一次监听器。
pub(crate) fn inject_dsh_page(app: &AppHandle, webview: &tauri::Webview) -> Result<(), String> {
    let config = app.state::<AppState>().config();
    let url = webview.url().map_err(|error| error.to_string())?;
    if !is_dsh_url(&url, &config) {
        return Ok(());
    }
    let title = serde_json::to_string(APP_TITLE).unwrap_or_default();
    let protocol_token =
        serde_json::to_string(app.state::<AppState>().protocol_token()).unwrap_or_default();
    let hide_tools = if config.hide_tool_calls {
        HIDE_TOOLS_APPLY
    } else {
        ""
    };
    let hide_stats = if config.hide_stats_line {
        hide_stats_apply()
    } else {
        String::new()
    };
    let script = format!(
        "(() => {{ \
         if (window.__dshdInit === 'loading' || window.__dshdInit === 'ready') return; \
         window.__dshdInit = 'loading'; \
         window.__dshdProtocolToken = {protocol_token}; \
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
           const el = document.querySelector('head > title'); \
           if (el) new MutationObserver(fix).observe(el, {{ childList: true }}); \
           {menu} {heartbeat} {hide_tools} {hide_stats} \
           window.__dshdInit = 'ready'; \
         }} catch (error) {{ \
           delete window.__dshdInit; \
           throw error; \
         }} finally {{ \
           delete window.__dshdProtocolToken; \
         }} \
         }})();",
        menu = MENU_INJECT,
        heartbeat = HEARTBEAT_INJECT,
    );
    webview.eval(script).map_err(|error| error.to_string())
}

/// 让 WebView 跳到 dsh 界面（或返回本地启动页）。
pub fn navigate(app: &AppHandle, url: &str) {
    let Some(wv) = main_webview(app) else {
        logging::log("navigate: 未找到主 webview");
        return;
    };
    if let Ok(u) = url::Url::parse(url) {
        if !is_allowed_navigation(app, &u) {
            logging::log(&format!("navigate: 已拒绝非白名单地址 {url}"));
            return;
        }
        logging::log(&format!("navigate: {url}"));
        if is_dsh_url(&u, &app.state::<AppState>().config()) {
            // 即使首次注入完全失败，也让心跳监视在超时后触发一次 reload 自愈，
            // 避免 last_heartbeat=None 导致永久不检查。
            app.state::<AppState>().set_heartbeat();
        }
        let _ = wv.navigate(u);
        // 状态栏统计立即刷新：dsh 就绪后不必等下一个 5s 轮询周期
        stats::refresh_once(app.clone());
        // dsh 页面挂载时会用自带 document.title 覆盖窗口标题。
        // 两层保障：立即 set_title（窗口级，立刻生效）；
        // 页面加载完成后注入常驻脚本，任意时刻的 title 变化都会被拉回产品名。
        if let Some(win) = main_window(app) {
            let _ = win.set_title(APP_TITLE);
        }
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
                    && wv
                        .url()
                        .ok()
                        .is_some_and(|url| is_dsh_url(&url, &handle.state::<AppState>().config()))
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
