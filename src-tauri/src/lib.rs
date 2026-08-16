//! DSHDesktop —— DeepSeek Harness (dsh) 桌面端外壳。
//!
//! 职责：管理 Node/dsh 运行时（检测、安装、更新），以隐藏窗口方式启动
//! `dsh web` 服务，用 WebView 加载 http://127.0.0.1:<port> 的官方界面，
//! 提供托盘/标题栏菜单与自绘弹窗（打开 / 检查更新 / API 余额 / 关于 / 退出），
//! 退出时清理全部子进程。

mod app_dialog;
mod app_state;
mod autostart;
mod balance;
mod commands;
mod dialog;
mod dsh;
mod file_actions;
mod heartbeat;
mod icons;
pub mod locale;
mod logging;
mod notify;
mod onboarding;
mod plugins;
mod processes;
mod runtime;
mod session_diff;
mod titlebar;
mod tray;
mod tray_menu;
mod update_txn;
mod updater;
mod util;
mod versions;
mod window;

use app_state::{AppState, BootPhase};
use tauri::{AppHandle, Emitter, Manager};

/// 主窗口 label。
pub const MAIN_WINDOW: &str = "main";

/// 按平台选择 ureq 的 TLS 配置：Windows/macOS 用系统原生实现（对应
/// Cargo.toml 的 native-tls feature），Linux 用 rustls。
/// 注意：ureq 的默认 TlsProvider 是 Rustls 且「不会随 feature 自动切换」——
/// 不显式设置时运行期握手会直接报错，所有 https 请求都会失败。
pub fn default_tls_config() -> ureq::tls::TlsConfig {
    let builder = ureq::tls::TlsConfig::builder();
    #[cfg(target_os = "linux")]
    // Linux（rustls）：用默认 WebPki 内置根；PlatformVerifier 在 rustls 后端
    // 需要额外 feature，直接 panic。
    let config = builder.provider(ureq::tls::TlsProvider::Rustls);
    #[cfg(not(target_os = "linux"))]
    // Windows/macOS（native-tls）：用系统信任库验证。附加 webpki 根会覆盖
    // schannel 的默认信任行为（实测 npm registry 证书链因此无法验证）。
    let config = builder
        .provider(ureq::tls::TlsProvider::NativeTls)
        .root_certs(ureq::tls::RootCerts::PlatformVerifier);
    config.build()
}

/// panic = "abort" 的兜底：panic 信息默认输出到 GUI 应用不可见的 stderr。
/// 由 main 里的 panic hook 调用，直接追加写入应用日志（logging 可能尚未
/// 初始化，不能走 logging::log）。
pub fn log_panic(line: &str) {
    let root = std::env::var("DSH_DESKTOP_ROOT")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| app_state::default_app_root());
    let path = root.join("logs").join("desktop.log");
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    {
        use std::io::Write;
        let _ = writeln!(f, "panic: {line}");
    }
}

/// 对外产品名（窗口标题/托盘/exe 属性等统一显示名）。
pub const APP_TITLE: &str = "DeepSeek Harness Desktop";

/// 本地启动页（生产环境 Tauri 资源源）。
pub const SPLASH_ORIGIN: &str = "tauri://localhost";

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

fn is_allowed_navigation(app: &AppHandle, url: &url::Url) -> bool {
    let dev = app_dev_origin(app);
    is_local_app_url(url, dev.as_ref()) || is_dsh_url(url, &app.state::<AppState>().config())
}

/// 注入 dsh 页面的初始化脚本（document start 执行，每次导航生效）：
/// 深色主题首帧预设：dsh 的 CSS 用 `body[data-ds-dark-theme]` 选择器，消除
/// “深 loading → 白 dsh → 深 dsh”的首帧闪（dsh 挂载后自行接管主题）。
const PAGE_INIT_SCRIPT: &str = r#"
if (window.matchMedia && window.matchMedia('(prefers-color-scheme: dark)').matches) {
  document.documentElement.dataset.dsDarkTheme = '';
}
"#;

/// 右键菜单定制脚本：dsh 已处理右键（defaultPrevented/stopPropagation）则放行其自带
/// 菜单；否则屏蔽 WebView 默认菜单（后退/刷新/保存图片等网页操作），改为弹自绘菜单：
/// 可编辑区弹 剪切/复制/粘贴/全选（禁用态+快捷键提示），文件路径按钮弹
/// 打开/打开文件所在位置/打开方式/复制路径。样式用 dsh 设计系统变量构建——深浅色、
/// DPI 均自适应。经 navigate 后的 eval 注入（initialization_script 对外部 URL 导航不可靠）。
const MENU_INJECT: &str = r#"
var css = [
  // 与 dsh 菜单同规格：卡片 r12/pad4、条目 min-h40/r10/14px、hover 8%/按压 14%；
  // 最小宽 168：比 dsh 卡宽（218）窄，短条目与快捷键列之间的留白更协调，
  // 长条目自动撑宽
  '.__dshd_cm{position:fixed;z-index:2147483000;min-width:168px;padding:4px;',
  'background:var(--dsw-specific-menu,#353638);',
  'border:1px solid var(--dsw-alias-border-inverted,rgba(255,255,255,.06));',
  'border-radius:12px;box-shadow:var(--dsw-shadow-lv3,0 12px 32px rgba(0,0,0,.4));',
  'font:14px/22px "Segoe UI","Microsoft YaHei UI",system-ui,sans-serif;',
  'color:var(--dsw-alias-label-primary,#f9fafb);user-select:none;',
  'animation:dshd-cm-in .11s ease-out;}',
  '@keyframes dshd-cm-in{from{opacity:0;transform:translateY(-3px)}to{opacity:1;transform:translateY(0)}}',
  '.__dshd_cm_i{min-height:40px;padding:8px 10px;border-radius:10px;cursor:default;white-space:nowrap;',
  'display:flex;align-items:center;gap:8px;box-sizing:border-box;',
  // hover 淡入淡出：与托盘/标题栏菜单、弹窗按钮的过渡节奏一致
  'transition:background-color .12s ease,color .12s ease;}',
  '.__dshd_cm_i:hover{background:var(--dsw-alias-interactive-bg-hover,rgba(255,255,255,.08));}',
  '.__dshd_cm_i:focus{outline:none;}',
  '.__dshd_cm.__dshd_cm_kbd .__dshd_cm_i:focus{outline:2px solid var(--dsw-brand-color-primary,#4d6bfe);outline-offset:-2px;}',
  '.__dshd_cm_i:active{background:rgba(255,255,255,.14);}',
  '.__dshd_cm_i.__dshd_cm_p{background:rgba(255,255,255,.14);}',
  '.__dshd_cm_i.__dshd_cm_d{opacity:.4;pointer-events:none;}',
  '@media (prefers-reduced-motion:reduce){.__dshd_cm{animation:none;}',
  '.__dshd_cm_i{transition:none;}}',
  '.__dshd_cm_l{flex:1;min-width:0;overflow:hidden;text-overflow:ellipsis;}',
  '.__dshd_cm_ic{width:16px;height:16px;flex:none;display:block;}',
  '.__dshd_cm_k{color:var(--dsw-alias-label-tertiary,#adb2b8);font-size:12px;}',
  '.__dshd_cm_ar{font-family:"Segoe Fluent Icons","Segoe MDL2 Assets",sans-serif;',
  'font-size:12px;color:var(--dsw-alias-label-tertiary,#adb2b8);margin-left:6px;line-height:1;}',
  '.__dshd_cm_sep{height:1px;margin:4px 2px;background:var(--dsw-alias-border-l1,rgba(255,255,255,.06));}',
  // 子菜单：与 dsh 一致，最小宽 163；伪元素桥接父项与子菜单间隙，鼠标跨过不丢悬停
  '.__dshd_cm_sub{min-width:163px;}',
  '.__dshd_cm_sub::before{content:"";position:absolute;top:0;bottom:0;left:-6px;width:6px;}'
].join('');
var styleEl = document.createElement('style');
styleEl.textContent = css;
document.documentElement.appendChild(styleEl);

var menuEl = null;var items = [];
var subEl = null;
var subItems = [];
var subTimer = null;
var subParent = null;
var IS_MAC = /Mac/i.test(navigator.userAgent);
var IS_WIN = /Windows/i.test(navigator.userAgent);
var UI_ZH = String(window.__DSHD_LANG || navigator.language || '').toLowerCase().indexOf('zh') === 0;
function T(zh, en) { return UI_ZH ? zh : en; }
window.__dshdSetInjectedLanguage = function (language) {
  UI_ZH = String(language || '').toLowerCase().indexOf('zh') === 0;
};
function MOD() { return IS_MAC ? '⌘' : 'Ctrl'; }

// JS → Rust 通道：自定义协议 dshd。Windows 的注册形式是 http://dshd.localhost/<动作>，
// macOS/Linux 是 dshd://localhost/<动作>（Tauri 平台差异）。
var DSH_REQ_BASE = IS_WIN ? 'http://dshd.localhost/' : 'dshd://localhost/';
var DSH_TOKEN = window.__dshdProtocolToken || '';
function dshdUrl(action, query) {
  return DSH_REQ_BASE + action + '?token=' + encodeURIComponent(DSH_TOKEN) + (query ? '&' + query : '');
}
// 探测环境能力（VS Code 是否安装）；未回包前按未安装处理
var HAS_CODE = false;
try {
  fetch(dshdUrl('probe', 'what=vscode')).then(function (r) { return r.text(); })
    .then(function (t) { HAS_CODE = t === '1'; }).catch(function () {});
} catch (e) {}

function closeSub() {
  clearTimeout(subTimer);
  if (subEl) { subEl.remove(); subEl = null; }
  subItems = [];
  if (subParent) subParent.setAttribute('aria-expanded', 'false');
  subParent = null;
}
// 菜单打开前持有焦点的元素：菜单为键盘导航聚焦条目，收起后把焦点还给原元素
//（右击输入框后菜单消失，光标焦点不丢，行为与原生右键菜单一致）
var focusReturn = null;
function hide() {
  closeSub();
  if (menuEl) { menuEl.remove(); menuEl = null; }
  items = [];
  if (focusReturn && document.contains(focusReturn)) {
    try { focusReturn.focus(); } catch (e) {}
    focusReturn = null;
  }
}

function execOn(el, cmd) {
  el.focus();
  try { return document.execCommand(cmd); } catch (e) { return false; }
}

function pasteInto(el) {
  el.focus();
  try {
    if (document.execCommand('paste')) return;
    navigator.clipboard.readText().then(function (t) {
      el.focus();
      document.execCommand('insertText', false, t);
    }).catch(function () {});
  } catch (e) {}
}

// —— 本地文件路径识别 ——
// dsh 输出里的路径有三种形态：消息内联 file-mention（<code><button title=路径>）、
// 产物 chip（button title=路径）、工具行 path 链接（button 无 title，文本即路径）。
function looksLikePath(s) {
  if (!s || s.length > 500) return false;
  if (/^https?:/i.test(s)) return false;
  var abs = /^[A-Za-z]:[\\/]/.test(s) || s.slice(0, 2) === '\\\\'
    || s.charAt(0) === '/' || s.charAt(0) === '~';
  if (abs) return true;
  // 相对路径：含分隔符、末段带扩展名、无中文——避免把普通句子误判为路径
  if (/[\u4e00-\u9fff]/.test(s)) return false;
  if (s.charAt(0) === '.') return true; // ./ ../ 开头
  return /[\\/]/.test(s) && /\.[A-Za-z0-9]{1,8}$/.test(s);
}
function isAbsPath(s) {
  return /^[A-Za-z]:[\\/]/.test(s) || s.slice(0, 2) === '\\\\' || s.charAt(0) === '/';
}
function findPathTarget(t) {
  var btn = t && t.closest ? t.closest('button[title]') : null;
  if (btn) {
    var ti = btn.getAttribute('title') || '';
    var inCode = btn.parentElement && btn.parentElement.tagName === 'CODE';
    if (inCode || looksLikePath(ti)) return { el: btn, path: ti, viaButton: true };
  }
  var node = t && t.closest ? t.closest('button,code,a,span') : null;
  if (node) {
    var txt = (node.textContent || '').trim();
    if (looksLikePath(txt)) {
      return { el: node, path: txt, viaButton: node.tagName === 'BUTTON' };
    }
  }
  if (t && t.tagName && t.tagName !== 'BODY' && t.tagName !== 'HTML') {
    var t2 = (t.textContent || '').trim();
    if (looksLikePath(t2)) return { el: t, path: t2, viaButton: false };
  }
  return null;
}

// —— 文件类型判断（智能规则：非文本不提供 VS Code/复制内容；可执行文件不提供打开方式）——
function extOf(p) { var m = /\.([^./\\]+)$/.exec(p); return m ? m[1].toLowerCase() : ''; }
var TEXT_EXTS = ['txt','md','markdown','json','jsonc','js','mjs','cjs','ts','tsx','jsx','py','rb','rs','go','java','c','h','cpp','hpp','cc','hh','cs','css','scss','less','sass','html','htm','xml','yml','yaml','toml','ini','cfg','conf','cnf','sh','bash','zsh','bat','ps1','psm1','sql','log','csv','tsv','vue','svelte','m','mm','swift','kt','kts','php','lua','r','pl','pm','tex','sty','bib','env','gitignore','gitattributes','lock','properties','gradle','cmake','dockerfile','makefile','editorconfig','eslintrc','prettierrc','babelrc','npmrc','tsconfig','jsconfig','htaccess','svg','map','patch','diff','ipynb'];
var IMG_EXTS = ['png','jpg','jpeg','gif','webp','bmp','ico','avif','tif','tiff','heic'];
var EXE_EXTS = ['exe','msi','com','bat','cmd','lnk','scr','appx','msix','pif','cpl'];
function isTextLike(p) { return TEXT_EXTS.indexOf(extOf(p)) >= 0; }
function isImageLike(p) { return IMG_EXTS.indexOf(extOf(p)) >= 0; }
function isExeLike(p) { return EXE_EXTS.indexOf(extOf(p)) >= 0; }

// —— 动作通道 ——
function req(action, path, app) {
  var u = dshdUrl(action, 'path=' + encodeURIComponent(path)
    + (app ? '&app=' + encodeURIComponent(app) : ''));
  var sent = false;
  try {
    if (window.fetch) {
      fetch(u, { mode: 'no-cors' }).catch(function () {});
      sent = true;
    }
  } catch (e) {}
  if (!sent) {
    try { var im = new Image(); im.src = u; } catch (e2) {}
  }
}
function writeClip(t) {
  try { navigator.clipboard.writeText(t); } catch (e) {}
}
// —— 相对路径 → 绝对路径：调用 dsh 后端同源 RPC（注入脚本运行在页面上下文，无跨域）——
// session.list 返回各会话 cwd（running 会话优先，其次最近更新）；
// 兜底 workspace.list：唯一工作区时用其根目录。
function rpc(method, payload) {
  return fetch('/api/' + method, {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify({
      type: 'client-request',
      rpcId: 'dshd-' + Math.random().toString(36).slice(2),
      method: method,
      payload: payload || {}
    })
  }).then(function (r) { return r.json(); });
}
function rpcValue(json) {
  return json && json.result && json.result.ok ? json.result.value : null;
}
function joinPath(base, rel) {
  var b = String(base).replace(/[\\/]+$/, '');
  var r = String(rel).replace(/^[\\/]+/, '');
  if (isAbsPath(r)) return r;
  var sep = b.indexOf('\\') >= 0 ? '\\' : '/';
  return b + sep + r;
}
function resolveAbsPath(rel) {
  return rpc('session.list', {}).then(function (json) {
    var items = (rpcValue(json) || {}).items || [];
    var best = null;
    items.forEach(function (it) {
      if (!it.cwd) return;
      if (it.running) {
        if (!best || !best.running || it.updatedAt > best.updatedAt) best = it;
        return;
      }
      if (!best || (!best.running && it.updatedAt > best.updatedAt)) best = it;
    });
    if (best && best.cwd) return joinPath(best.cwd, rel);
    return null;
  }).catch(function () { return null; }).then(function (abs) {
    if (abs) return abs;
    return rpc('workspace.list', {}).then(function (json) {
      var items = (rpcValue(json) || {}).items || [];
      if (items.length === 1 && items[0].path) return joinPath(items[0].path, rel);
      return null;
    }).catch(function () { return null; });
  });
}
function copyContent(path) {
  try {
    fetch(dshdUrl('content', 'path=' + encodeURIComponent(path)))
      .then(function (r) { if (!r.ok) throw 0; return r.text(); })
      .then(function (t) { return navigator.clipboard.writeText(t); })
      .catch(function () {});
  } catch (e) {}
}
function copyImage(img) {
  try {
    var w = img.naturalWidth || img.width || 0;
    var h = img.naturalHeight || img.height || 0;
    if (!w || !h) return;
    var c = document.createElement('canvas');
    c.width = w; c.height = h;
    var ctx = c.getContext('2d');
    if (!ctx) return;
    ctx.drawImage(img, 0, 0, w, h);
    c.toBlob(function (b) {
      if (!b) return;
      try {
        navigator.clipboard.write([new ClipboardItem({ 'image/png': b })]).catch(function () {});
      } catch (e2) {}
    }, 'image/png');
  } catch (e) {}
}

// —— 菜单渲染（支持图标异步填充 + hover 子菜单）——
// 图标首帧即占位（内联 SVG 数据 URL，零网络），真实图标加载完成后原地替换；
// 提取失败则保留占位符，不会出现空白。app: 图标在注入时预取一次预热 Rust 缓存。
var PH_FILE = "data:image/svg+xml,%3Csvg%20xmlns='http://www.w3.org/2000/svg'%20width='16'%20height='16'%20viewBox='0%200%2016%2016'%3E%3Cpath%20d='M4.5%201.5h5L12%204v10.5h-7.5z'%20fill='none'%20stroke='%238b8b94'/%3E%3Cpath%20d='M9.5%201.5V4H12'%20fill='none'%20stroke='%238b8b94'/%3E%3C/svg%3E";
var PH_APP = "data:image/svg+xml,%3Csvg%20xmlns='http://www.w3.org/2000/svg'%20width='16'%20height='16'%20viewBox='0%200%2016%2016'%3E%3Crect%20x='1.5'%20y='1.5'%20width='5.5'%20height='5.5'%20rx='1'%20fill='none'%20stroke='%238b8b94'/%3E%3Crect%20x='9'%20y='1.5'%20width='5.5'%20height='5.5'%20rx='1'%20fill='none'%20stroke='%238b8b94'/%3E%3Crect%20x='1.5'%20y='9'%20width='5.5'%20height='5.5'%20rx='1'%20fill='none'%20stroke='%238b8b94'/%3E%3Crect%20x='9'%20y='9'%20width='5.5'%20height='5.5'%20rx='1'%20fill='none'%20stroke='%238b8b94'/%3E%3C/svg%3E";
function placeholderFor(spec) {
  return spec && spec.slice(0, 5) === 'file:' ? PH_FILE : PH_APP;
}
function loadIcon(img, spec) {
  var url;
  if (spec.slice(0, 5) === 'file:') {
    url = dshdUrl('icon', 'path=' + encodeURIComponent(spec.slice(5)));
  } else if (spec.slice(0, 4) === 'app:') {
    url = dshdUrl('icon', 'app=' + encodeURIComponent(spec.slice(4)));
  } else { return; }
  img.src = url;
}
// 注入时预热固定应用图标（VS Code/记事本/画图）：提取+缓存提前完成，子菜单首开即出图
loadIcon(new Image(), 'app:code');
loadIcon(new Image(), 'app:notepad');
loadIcon(new Image(), 'app:paint');

function openSub(parentNode, list) {
  closeSub();
  subItems = list;
  subParent = parentNode;
  subParent.setAttribute('aria-expanded', 'true');
  subEl = document.createElement('div');
  subEl.className = '__dshd_cm __dshd_cm_sub';
  subEl.setAttribute('role', 'menu');
  subEl.setAttribute('aria-label', T('打开方式', 'Open with'));
  var html = '';
  list.forEach(function (it, i) {
    if (it.sep) { html += '<div class="__dshd_cm_sep" role="separator"></div>'; }
    else {
      var ic = it.icon
        ? '<img class="__dshd_cm_ic" alt="" src="' + placeholderFor(it.icon) + '" />'
        : '';
      html += '<div class="__dshd_cm_i" role="menuitem" tabindex="-1" data-i="' + i + '">' + ic
        + '<span class="__dshd_cm_l">' + it.label + '</span></div>';
    }
  });
  subEl.innerHTML = html;
  menuEl.appendChild(subEl);
  subEl.querySelectorAll('.__dshd_cm_i').forEach(function (node) {
    var it = subItems[Number(node.getAttribute('data-i'))];
    if (it && it.icon) {
      var img = node.querySelector('.__dshd_cm_ic');
      if (img) loadIcon(img, it.icon);
    }
  });
  // 定位：与 dsh 子菜单一致——底部对齐父项（向上生长）、右侧 6px 间隙
  // （::before 桥接间隙，鼠标跨过不丢悬停）；放不下翻到左侧
  var pr = parentNode.getBoundingClientRect();
  var sr = subEl.getBoundingClientRect();
  var left = pr.right + 6;
  if (left + sr.width > window.innerWidth - 4) left = pr.left - sr.width - 6;
  var top = pr.bottom - sr.height + 4;
  top = Math.max(4, Math.min(top, window.innerHeight - sr.height - 4));
  subEl.style.left = left + 'px';
  subEl.style.top = top + 'px';
}

function closeSubSoon() {
  clearTimeout(subTimer);
  // 容差放宽：慢速移动鼠标跨过间隙时不至于误关
  subTimer = setTimeout(closeSub, 350);
}

function show(x, y, list) {
  hide();
  focusReturn = (document.activeElement && document.activeElement !== document.body)
    ? document.activeElement : null;
  items = list;
  menuEl = document.createElement('div');
  menuEl.className = '__dshd_cm';
  menuEl.setAttribute('role', 'menu');
  menuEl.setAttribute('aria-label', T('上下文菜单', 'Context menu'));
  var html = '';
  list.forEach(function (it, i) {
    if (it.sep) { html += '<div class="__dshd_cm_sep" role="separator"></div>'; }
    else {
      var dis = it.enabled === false ? ' __dshd_cm_d' : '';
      var ariaDisabled = it.enabled === false ? ' aria-disabled="true"' : '';
      var subAttrs = it.sub ? ' aria-haspopup="menu" aria-expanded="false"' : '';
      var ic = it.icon
        ? '<img class="__dshd_cm_ic" alt="" src="' + placeholderFor(it.icon) + '" />'
        : '';
      var tail = it.sub ? '<span class="__dshd_cm_ar">&#xE76C;</span>'
        : (it.key ? '<span class="__dshd_cm_k">' + it.key + '</span>' : '');
      html += '<div class="__dshd_cm_i' + dis + '" role="menuitem" tabindex="-1"' + ariaDisabled + subAttrs
        + ' data-i="' + i + '">' + ic
        + '<span class="__dshd_cm_l">' + it.label + '</span>' + tail + '</div>';
    }
  });
  menuEl.innerHTML = html;
  document.body.appendChild(menuEl);
  menuEl.querySelectorAll('.__dshd_cm_i').forEach(function (node) {
    var it = items[Number(node.getAttribute('data-i'))];
    if (it && it.icon) {
      var img = node.querySelector('.__dshd_cm_ic');
      if (img) loadIcon(img, it.icon);
    }
  });
  menuEl.addEventListener('mouseover', function (e) {
    // 指针进入子菜单（含分隔线/内边距）：取消待关闭计时，保持展开
    if (subEl && e.target && subEl.contains(e.target)) {
      clearTimeout(subTimer);
      return;
    }
    var node = e.target && e.target.closest ? e.target.closest('.__dshd_cm_i') : null;
    if (!node) { closeSubSoon(); return; }
    var it = items[Number(node.getAttribute('data-i'))];
    if (it && it.sub) {
      // 同一父项热区内移动时，指针跨过行内子元素（图标/文字/箭头）会逐次
      // 触发 mouseover：已为该父项展开时不再重建子菜单——重建会重播入场
      // 动画，正是“已展示的子菜单在热区内移动时闪烁”的来源；仅取消待关闭计时
      if (subParent !== node) {
        openSub(node, it.sub);
      } else {
        clearTimeout(subTimer);
      }
    } else {
      closeSubSoon();
    }
  });
  menuEl.addEventListener('mouseleave', closeSubSoon);
  menuEl.addEventListener('pointerdown', function () { menuEl.classList.remove('__dshd_cm_kbd'); });
  // 点击（mouseup）执行：按下时 :active 可见，松开后驻留 180ms 按压高亮再收起
  // —— 让按压反馈清晰可感知（原生菜单的选中节奏）
  var busy = false;
  menuEl.addEventListener('click', function (e) {
    if (busy) return;
    var node = e.target && e.target.closest ? e.target.closest('.__dshd_cm_i') : null;
    if (!node) { hide(); return; }
    var it;
    if (subEl && subEl.contains(node)) {
      it = subItems[Number(node.getAttribute('data-i'))];
    } else {
      it = items[Number(node.getAttribute('data-i'))];
    }
    if (!it || it.enabled === false) { hide(); return; }
    busy = true;
    node.classList.add('__dshd_cm_p');
    setTimeout(function () {
      busy = false;
      hide();
      if (it.act) it.act();
    }, 180);
  });
  // 边界处理：菜单不能超出主 webview 视口，按视口尺寸翻转并贴边 6px；
  // clientX/Y 与 fixed 定位同为 CSS 逻辑像素，任意 DPI 一致
  var r = menuEl.getBoundingClientRect();
  var mx = Math.max(6, Math.min(x, Math.max(6, window.innerWidth - r.width - 6)));
  var my = Math.max(6, Math.min(y, Math.max(6, window.innerHeight - r.height - 6)));
  menuEl.style.left = mx + 'px';
  menuEl.style.top = my + 'px';
  var first = menuEl.querySelector('.__dshd_cm_i:not(.__dshd_cm_d)');
  if (first) first.focus();
}

// —— Codex 式文件菜单 ——
// p 为展示用路径或已解析的绝对路径（解析成功时全部动作/图标/复制路径都用绝对路径）
function fileMenu(f, p) {
  var abs = isAbsPath(p);
  var items = [{
    label: T('打开文件', 'Open file'), icon: 'file:' + p,
    act: function () {
      if (f.viaButton) { f.el.click(); } // dsh 后端解析相对路径
      else if (abs) { req('open', p); }
    }
  }];
  if (abs && isTextLike(p) && HAS_CODE) {
    items.push({
      label: T('在 VS Code 中打开', 'Open in VS Code'), icon: 'app:code',
      act: function () { req('openapp', p, 'code'); }
    });
  }
  // “打开方式”子菜单仅 Windows 提供：记事本/画图/系统选择器
  // 均依赖 Windows 实现，macOS/Linux 上显示会点了无反应或直接报错；
  // VS Code 已有菜单顶部的独立快捷项，子菜单内不重复
  if (abs && !isExeLike(p) && IS_WIN) {
    var subs = [];
    if (isTextLike(p)) {
      subs.push({ label: T('记事本', 'Notepad'), icon: 'app:notepad', act: function () { req('openapp', p, 'notepad'); } });
    }
    if (isImageLike(p)) {
      subs.push({ label: T('画图', 'Paint'), icon: 'app:paint', act: function () { req('openapp', p, 'paint'); } });
    }
    if (subs.length) {
      subs.push({ sep: true });
      subs.push({ label: T('选择其他应用…', 'Choose another app…'), act: function () { req('openwith', p, ''); } });
      items.push({ label: T('打开方式', 'Open with'), sub: subs });
    }
  }
  items.push({ sep: true });
  if (abs) {
    items.push({ label: T('另存为…', 'Save as…'), act: function () { req('saveas', p); } });
  }
  items.push({ label: T('复制路径', 'Copy path'), act: function () { writeClip(p); } });
  if (abs && isTextLike(p)) {
    items.push({ label: T('复制文件内容', 'Copy file contents'), act: function () { copyContent(p); } });
  }
  if (abs) {
    // 文件管理器名称随平台：macOS 为 Finder，Windows 为资源管理器，其余为文件管理器
    var fmLabel = IS_MAC
      ? T('在 Finder 中显示', 'Show in Finder')
      : (IS_WIN ? T('在资源管理器中打开', 'Show in File Explorer') : T('在文件管理器中打开', 'Show in file manager'));
    items.push({ label: fmLabel, act: function () { req('reveal', p); } });
  }
  return items;
}

function onCtx(e) {
  if (e.defaultPrevented) return; // dsh 自带右键菜单：放行
  var t = e.target;
  var f = findPathTarget(t);
  if (f) {
    e.preventDefault();
    if (isAbsPath(f.path)) {
      show(e.clientX, e.clientY, fileMenu(f, f.path));
    } else {
      // 相对路径：先解析成绝对路径（复制路径/图标/定位都需要完整路径），
      // 本地 RPC 通常几十毫秒；失败降级为 打开文件+复制路径
      resolveAbsPath(f.path).then(function (abs) {
        if (menuEl) return;
        show(e.clientX, e.clientY, fileMenu(f, abs || f.path));
      }).catch(function () {
        if (menuEl) return;
        show(e.clientX, e.clientY, fileMenu(f, f.path));
      });
    }
    return;
  }
  var el = t && t.closest
    ? t.closest('input,textarea,[contenteditable="true"],[contenteditable=""],[role="textbox"]')
    : null;
  if (el) {
    e.preventDefault();
    var isInput = el.tagName === 'INPUT' || el.tagName === 'TEXTAREA';
    var hasSel = isInput
      ? (el.selectionStart !== el.selectionEnd)
      : (function () {
          var s = window.getSelection();
          return !!(s && s.toString() && s.anchorNode && el.contains(s.anchorNode));
        })();
    var hasContent = !!(isInput
      ? el.value.length > 0
      : el.textContent.trim().length > 0);
    var m = MOD();
    show(e.clientX, e.clientY, [
      { label: T('剪切', 'Cut'), key: m + '+X', enabled: hasSel, act: function () { execOn(el, 'cut'); } },
      { label: T('复制', 'Copy'), key: m + '+C', enabled: hasSel, act: function () { execOn(el, 'copy'); } },
      { label: T('粘贴', 'Paste'), key: m + '+V', act: function () { pasteInto(el); } },
      { sep: true },
      { label: T('全选', 'Select all'), key: m + '+A', enabled: hasContent, act: function () {
        el.focus();
        if (el.select) { el.select(); } else { document.execCommand('selectAll'); }
      } }
    ]);
    return;
  }
  var img = t && t.tagName === 'IMG' ? t : (t && t.closest ? t.closest('img') : null);
  if (img) {
    e.preventDefault();
    show(e.clientX, e.clientY, [
      { label: T('复制图片', 'Copy image'), act: function () { copyImage(img); } }
    ]);
    return;
  }
  var a = t && t.closest ? t.closest('a[href]') : null;
  if (a) {
    var href = a.getAttribute('href') || '';
    if (/^https?:/i.test(href)) {
      e.preventDefault();
      show(e.clientX, e.clientY, [
        { label: T('复制链接', 'Copy link'), act: function () { writeClip(href); } },
        { label: T('在浏览器中打开', 'Open in browser'), act: function () { req('browse', href); } }
      ]);
      return;
    }
  }
  var sel = window.getSelection();
  if (sel && sel.toString()) {
    e.preventDefault();
    show(e.clientX, e.clientY, [
      { label: T('复制', 'Copy'), key: MOD() + '+C', act: function () { document.execCommand('copy'); } }
    ]);
  } else {
    e.preventDefault(); // 无选区：静默屏蔽默认菜单
  }
}

document.addEventListener('contextmenu', onCtx);
document.addEventListener('mousedown', function (e) {
  if (menuEl && !menuEl.contains(e.target)) hide();
});
window.addEventListener('blur', hide);
window.addEventListener('resize', hide);
// 只在用户真实滚动时收起（wheel/touchmove）；不监听 scroll：
// 思考流式输出会程序化自动滚动聊天区，之前导致菜单被误关
window.addEventListener('wheel', hide, true);
window.addEventListener('touchmove', hide, true);
document.addEventListener('keydown', function (e) {
  if (!menuEl) return;
  if (e.key === 'Escape') { e.preventDefault(); hide(); return; }
  if (['ArrowDown', 'ArrowUp', 'ArrowLeft', 'ArrowRight', 'Home', 'End', 'Enter', ' '].indexOf(e.key) >= 0) {
    menuEl.classList.add('__dshd_cm_kbd');
  }
  var active = document.activeElement;
  var scope = subEl && active && subEl.contains(active) ? subEl : menuEl;
  var nodes = Array.prototype.slice.call(
    scope.querySelectorAll('.__dshd_cm_i:not(.__dshd_cm_d)')
  ).filter(function (node) { return node.parentElement === scope; });
  if (!nodes.length) return;
  var index = nodes.indexOf(active);
  if (e.key === 'ArrowDown' || e.key === 'ArrowUp' || e.key === 'Home' || e.key === 'End') {
    e.preventDefault();
    if (e.key === 'Home') index = 0;
    else if (e.key === 'End') index = nodes.length - 1;
    else if (e.key === 'ArrowDown') index = (index + 1 + nodes.length) % nodes.length;
    else index = (index - 1 + nodes.length) % nodes.length;
    nodes[index].focus();
    return;
  }
  if (e.key === 'ArrowLeft' && subEl && subEl.contains(active)) {
    e.preventDefault();
    var parent = subParent;
    closeSub();
    if (parent) parent.focus();
    return;
  }
  if (e.key === 'ArrowRight' || e.key === 'Enter' || e.key === ' ') {
    var isSub = subEl && subEl.contains(active);
    var source = isSub ? subItems : items;
    var it = active && active.matches('.__dshd_cm_i')
      ? source[Number(active.getAttribute('data-i'))]
      : null;
    if (!it) return;
    e.preventDefault();
    if (it.sub) {
      openSub(active, it.sub);
      var subFirst = subEl.querySelector('.__dshd_cm_i:not(.__dshd_cm_d)');
      if (subFirst) subFirst.focus();
    } else if (e.key === 'Enter' || e.key === ' ') {
      active.click();
    }
  }
});
"#;

/// 深色主题的统一底色（与 dsh 深色主题 body 背景 #151517 一致，衔接无缝）。
pub(crate) const DARK_BG: tauri::window::Color = tauri::window::Color(0x15, 0x15, 0x17, 0xFF);
/// 浅色主题的统一底色（与 dsh 浅色主题 body 背景纯白一致）。
pub(crate) const LIGHT_BG: tauri::window::Color = tauri::window::Color(0xFF, 0xFF, 0xFF, 0xFF);
/// 自绘托盘菜单/弹窗的统一深色卡片底色（与 dsh 菜单卡片 bluish-800 一致）。
pub(crate) const CARD_BG_DARK: tauri::window::Color = tauri::window::Color(0x35, 0x36, 0x38, 0xFF);
/// 自绘托盘菜单/弹窗的统一浅色卡片底色（纯白）。
pub(crate) const CARD_BG_LIGHT: tauri::window::Color = tauri::window::Color(0xFF, 0xFF, 0xFF, 0xFF);

/// 向启动页广播状态（不确定进度）。
pub fn emit_status(app: &AppHandle, phase: BootPhase, message: &str, detail: &str) {
    emit_status_progress(app, phase, message, detail, None);
}

/// 向启动页广播状态（可带 0-100 确定进度）。
pub fn emit_status_progress(
    app: &AppHandle,
    phase: BootPhase,
    message: &str,
    detail: &str,
    progress: Option<f64>,
) {
    // 事件载荷带完整版本信息：此前这里固定 None，前端每次收到事件都会
    // 重算 footer（版本/端口行）并将其清空——启动过程中 footer 短暂出现
    // 后即“消失”。snapshot 的版本检测有缓存，高频事件无额外开销。
    let snapshot = app.state::<AppState>().snapshot();
    let payload = app_state::StatusPayload {
        phase: phase.as_str().to_string(),
        message: message.to_string(),
        detail: detail.to_string(),
        progress,
        dsh_version: snapshot.dsh_version,
        node_version: snapshot.node_version,
        port: snapshot.port,
    };
    let _ = app.emit("dsh-status", payload);
}

/// 主窗口（Window 级操作：show/icon/title/scale 等）。
/// 不能用 get_webview_window：窗口存在子 webview（自绘标题栏）时它会返回 None。
pub fn main_window(app: &AppHandle) -> Option<tauri::Window> {
    app.get_window(MAIN_WINDOW)
}

/// 主 webview（Webview 级操作：navigate/eval）。
pub fn main_webview(app: &AppHandle) -> Option<tauri::Webview> {
    main_window(app)?
        .webviews()
        .into_iter()
        .find(|w| w.label() == MAIN_WINDOW)
}

/// 主窗口当前所在显示器的逻辑工作区 (x, y, w, h)。
pub(crate) fn logical_work_area(app: &AppHandle) -> Option<(f64, f64, f64, f64)> {
    let win = main_window(app)?;
    let mon = win.current_monitor().ok()??;
    let scale = mon.scale_factor();
    let wa = mon.work_area();
    Some((
        wa.position.x as f64 / scale,
        wa.position.y as f64 / scale,
        wa.size.width as f64 / scale,
        wa.size.height as f64 / scale,
    ))
}

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
fn handle_dshd_scheme(
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
                Some(s) => match icons::icon_png_16(&s) {
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
                        crate::dialog::show_message(
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

/// 显示主窗口并聚焦（托盘“打开”）。
pub fn show_main(app: &AppHandle) {
    if let Some(w) = main_window(app) {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
    }
}

/// 应用“隐藏工具调用”开关到 dsh 页面：开启注入隐藏样式，关闭移除。
/// 导航注入与菜单切换共用同一逻辑。
pub fn apply_hide_tools(app: &AppHandle) {
    let hide = app.state::<AppState>().config().hide_tool_calls;
    let script = if hide {
        "var __h=document.getElementById('__dshd_hide_tools');if(!__h){var s=document.createElement('style');\
         s.id='__dshd_hide_tools';s.textContent='[data-tool]{display:none!important}';\
         document.documentElement.appendChild(s);}"
    } else {
        "var __h=document.getElementById('__dshd_hide_tools');if(__h)__h.remove();"
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
        let _ = wv.navigate(u);
        // dsh 页面挂载时会用自带 document.title 覆盖窗口标题。
        // 两层保障：立即 set_title（窗口级，立刻生效）；
        // 页面加载完成后注入常驻脚本，任意时刻的 title 变化都会被拉回产品名。
        if let Some(win) = main_window(app) {
            let _ = win.set_title(APP_TITLE);
        }
        let handle = app.clone();
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(1500));
            if let Some(wv) = main_webview(&handle) {
                let config = handle.state::<AppState>().config();
                if !wv.url().ok().is_some_and(|url| is_dsh_url(&url, &config)) {
                    return;
                }
                let title = serde_json::to_string(APP_TITLE).unwrap_or_default();
                let protocol_token =
                    serde_json::to_string(handle.state::<AppState>().protocol_token())
                        .unwrap_or_default();
                // 单实例：重复 navigate 不叠加观察器/菜单监听；
                // MutationObserver 只在 title 真正变化时拉回（无常驻轮询开销）；
                // 右键菜单定制一并注入（此通道在 dsh 页面生效）
                let hide_tools = if handle.state::<AppState>().config().hide_tool_calls {
                    "var __h=document.getElementById('__dshd_hide_tools');if(!__h){var s=document.createElement('style');\
                     s.id='__dshd_hide_tools';s.textContent='[data-tool]{display:none!important}';\
                     document.documentElement.appendChild(s);}"
                } else {
                    ""
                };
                let _ = wv.eval(format!(
                    "(() => {{ if (window.__dshdInit) return; window.__dshdInit = true; \
                     window.__dshdProtocolToken = {protocol_token}; \
                     const t = {title}; \
                     const fix = () => {{ if (document.title !== t) document.title = t; }}; \
                     fix(); \
                     const el = document.querySelector('head > title'); \
                     if (el) new MutationObserver(fix).observe(el, {{ childList: true }}); \
                     {menu} {heartbeat} {hide_tools} }})();",
                    menu = MENU_INJECT,
                    heartbeat = HEARTBEAT_INJECT,
                    hide_tools = hide_tools,
                    protocol_token = protocol_token,
                ));
            }
        });
    }
}

pub fn run() {
    let app = tauri::Builder::default()
        .register_uri_scheme_protocol("dshd", handle_dshd_scheme)
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            show_main(app);
        }))
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_notification::init())
        .manage(AppState::new())
        .invoke_handler(commands::invoke_handler())
        .setup(|app| {
            // 手建主窗口（conf windows 为空）：带初始化脚本预设 dsh 深色主题，
            // 背景色跟随系统主题，与 dsh/loading 底色统一，消除启动与导航的明暗闪烁
            let navigation_app = app.handle().clone();
            let page_init_script = format!("{}\n{}", locale::init_script(), PAGE_INIT_SCRIPT);
            let win = tauri::WebviewWindowBuilder::new(
                app,
                MAIN_WINDOW,
                tauri::WebviewUrl::App("index.html".into()),
            )
            .title(APP_TITLE)
            .inner_size(1280.0, 820.0)
            .min_inner_size(960.0, 620.0)
            .resizable(true)
            .center()
            .visible(false)
            .background_color(DARK_BG)
            .initialization_script(page_init_script)
            .on_navigation(move |url| {
                let allowed = is_allowed_navigation(&navigation_app, url);
                if !allowed {
                    logging::log(&format!("navigation: 已拦截非白名单地址 {url}"));
                }
                allowed
            })
            .build()
            .expect("主窗口创建失败");
            // 不使用 set_shadow：tao 的无边框阴影实现带隐藏 insets（窗口
            // 外矩形比可见区域大一圈），保存/恢复 outer_size 时 insets 逐次
            // 累积——正是“每次启动窗口大一圈”的来源；且它会附加 1px 白边。
            // 主窗口保持无装饰直角窗口，尺寸记忆由窗口状态逻辑独立负责。
            #[cfg(target_os = "macos")]
            let _ = win.set_title_bar_style(tauri::TitleBarStyle::Overlay);
            // dsh 主题优先：启动时即读取 settings.yaml 的 ui-theme.preference，
            // light/dark 直接固定窗口主题，system 或未设置则跟随系统。
            // 这样加载页从第一帧起就与 dsh 的主题一致，而不是先按系统主题
            // 显示、加载完成后再切换（win.theme() 随后取到的是固定后的主题，
            // 背景色也随之对齐，避免启动闪烁）。
            if let Some(theme) = app.state::<AppState>().config().resolve_dsh_theme() {
                let _ = win.set_theme(Some(theme));
            }
            if let Ok(theme) = win.theme() {
                let color = if theme == tauri::Theme::Dark {
                    DARK_BG
                } else {
                    LIGHT_BG
                };
                let _ = win.set_background_color(Some(color));
            }

            let cfg = app.state::<AppState>().config();
            logging::init(cfg.logs_dir().join("desktop.log"));
            logging::log(&format!(
                "启动: port={} root={}",
                cfg.port,
                cfg.root.display()
            ));

            // 记忆窗口位置/大小：全程逻辑坐标——保存的就是逻辑值，恢复也直接用
            // 逻辑坐标设置，交给系统做 DPI 换算（物理坐标设置在高 DPI 下会被
            // 系统二次协商撑大尺寸，导致底部再次越过任务栏）。
            // 目标显示器选择 + 裁剪都在逻辑空间完成，且必须“同一台显示器”与
            // 窗口相交；恢复时硬性收敛进该显示器工作区。
            // 本次启动实际应用的尺寸（恢复值或自适应值），供终态线程
            // 测量系统协商增量（见 window.rs 的 NEGOTIATION_DELTA 说明）
            let mut applied_size: (f64, f64) = (0.0, 0.0);
            let restored = cfg
                .load_window_rect()
                .map(|(lx, ly, lw, lh)| {
                    if lw < 400.0 || lh < 300.0 {
                        return false;
                    }
                    let target = app
                        .available_monitors()
                        .unwrap_or_default()
                        .iter()
                        .find_map(|m| {
                            let scale = m.scale_factor();
                            let wa = m.work_area();
                            // 逻辑工作区
                            let (px, py) = (wa.position.x as f64 / scale, wa.position.y as f64 / scale);
                            let (pw, ph) = (wa.size.width as f64 / scale, wa.size.height as f64 / scale);
                            // 仅要求与工作区相交（留最小可见区），尺寸不合则裁剪
                            let ok = lx < px + pw - 40.0
                                && lx + lw > px + 40.0
                                && ly < py + ph - 40.0
                                && ly + lh > py + 40.0;
                            ok.then_some((px, py, pw, ph))
                        });
                    if let Some((px, py, pw, ph)) = target {
                        // 硬性收敛进工作区：尺寸不超工作区，位置完整可见
                        let wc = lw.min(pw);
                        let hc = lh.min(ph);
                        let xc = lx.clamp(px, px + pw - wc);
                        let yc = ly.clamp(py, py + ph - hc);
                        logging::log(&format!(
                            "窗口: 恢复 原始=({lx:.0},{ly:.0},{lw:.0}x{lh:.0}) 裁剪=({xc:.0},{yc:.0},{wc:.0}x{hc:.0}) 工作区=({pw:.0}x{ph:.0})"
                        ));
                        if let Some(win) = main_window(app.handle()) {
                            let _ = win.set_position(tauri::Position::Logical(
                                tauri::LogicalPosition::new(xc, yc),
                            ));
                            let _ = win.set_size(tauri::Size::Logical(
                                tauri::LogicalSize::new(wc, hc),
                            ));
                            applied_size = (wc, hc);
                        }
                        true
                    } else {
                        false
                    }
                })
                .unwrap_or(false);
            if !restored {
                if let Some(win) = main_window(app.handle()) {
                    // 无有效记忆：按当前显示器工作区自适应（约 80%，受最小尺寸与设计上限约束），
                    // 适配小屏/高 DPI 显示器
                    if let Ok(Some(monitor)) = win.current_monitor() {
                        let scale = monitor.scale_factor();
                        let wa = monitor.work_area();
                        let ww = wa.size.width as f64 / scale;
                        let wh = wa.size.height as f64 / scale;
                        let w = (ww * 0.8).clamp(960.0, 1280.0);
                        let h = (wh * 0.82).clamp(620.0, 820.0);
                        let _ =
                            win.set_size(tauri::Size::Logical(tauri::LogicalSize::new(w, h)));
                        applied_size = (w, h);
                    }
                    // set_size 后重新居中（创建时的 center 基于初始尺寸）
                    let _ = win.center();
                }
            }
            // 启动后越界兜底收敛：show 时系统会对窗口几何做一次协商（本机观察
            // 约 +14w/+9h，随后稳定），协商后的尺寸即最终值——不再按保存值
            // “重新应用”：中途再 set 一次会被系统再次协商拉回，形成 loading
            // 期间肉眼可见的尺寸跳动（正是启动时窗口变一下的来源）。1.5s 后
            // 仅做越界收敛（阈值 4 逻辑像素，跳过无害的亚像素噪声）；启动
            // 静默期内不落盘，协商漂移不会被持久化，逐次启动大小保持稳定。
            {
                let handle = app.handle().clone();
                let applied = applied_size;
                std::thread::spawn(move || {
                    std::thread::sleep(std::time::Duration::from_millis(1500));
                    // 阶段二：终态兜底收敛（位置/尺寸硬性收进工作区）
                    let Some(win) = main_window(&handle) else {
                        return;
                    };
                    if win.is_maximized().unwrap_or(false) {
                        return;
                    }
                    let (Ok(pos), Ok(size)) = (win.outer_position(), win.outer_size()) else {
                        return;
                    };
                    let scale = win.scale_factor().unwrap_or(1.0);
                    let (lx, ly) = (pos.x as f64 / scale, pos.y as f64 / scale);
                    let (lw, lh) = (size.width as f64 / scale, size.height as f64 / scale);
                    // 测量本次设置的系统协商增量，供后续保存时扣除
                    crate::window::record_negotiation_delta(lw - applied.0, lh - applied.1);
                    if let Some((px, py, pw, ph)) = logical_work_area(&handle) {
                        let wc = lw.min(pw);
                        let hc = lh.min(ph);
                        let xc = lx.clamp(px, px + pw - wc);
                        let yc = ly.clamp(py, py + ph - hc);
                        logging::log(&format!(
                            "窗口: 终态 逻辑=({lx:.0},{ly:.0},{lw:.0}x{lh:.0}) 工作区=({pw:.0}x{ph:.0})"
                        ));
                        if (xc - lx).abs() > 4.0
                            || (yc - ly).abs() > 4.0
                            || (wc - lw).abs() > 4.0
                            || (hc - lh).abs() > 4.0
                        {
                            logging::log(&format!(
                                "窗口: 二次收敛 ({lx:.0},{ly:.0},{lw:.0}x{lh:.0}) -> ({xc:.0},{yc:.0},{wc:.0}x{hc:.0})"
                            ));
                            let _ = win.set_position(tauri::Position::Logical(
                                tauri::LogicalPosition::new(xc, yc),
                            ));
                            let _ = win.set_size(tauri::Size::Logical(
                                tauri::LogicalSize::new(wc, hc),
                            ));
                        }
                    }
                });
            }
            // 按 DPI 设置窗口图标（标题栏/任务栏 1:1 像素，避免系统缩放糊化）
            window::set_window_icon(app.handle());
            // 自绘标题栏：去掉系统标题栏（macOS 除外）、创建顶条子 webview、主 webview 让位
            if let Err(e) = titlebar::init(app.handle()) {
                logging::log(&format!("标题栏: 初始化失败：{e}"));
            }
            // 标题栏加载自愈：页面初始化完成会回报 titlebar_ready；
            // 3s 内未回报（页面加载失败/被跳过）则重新导航一次——
            // 偶发的“启动后标题栏空白”由此兜底
            {
                let handle = app.handle().clone();
                std::thread::spawn(move || {
                    std::thread::sleep(std::time::Duration::from_secs(3));
                    if !titlebar::is_ready() {
                        logging::log("titlebar: 页面未就绪，重试加载");
                        titlebar::reload(&handle);
                    }
                });
            }
            // 标题栏渲染自愈：合成层失效（间歇空白、DOM 正常）无法探测，
            // 周期发送重绘脉冲兜底恢复
            {
                let handle = app.handle().clone();
                std::thread::spawn(move || loop {
                    std::thread::sleep(std::time::Duration::from_secs(30));
                    if handle.state::<AppState>().is_quitting() {
                        return;
                    }
                    titlebar::repaint_pulse(&handle);
                });
            }
            // 自绘弹窗与（Windows）托盘菜单窗口：启动时预创建（隐藏），
            // 此后只定位/显示/隐藏——绝不在事件回调里新建/销毁 WebView 窗口
            app_dialog::precreate(app.handle());
            #[cfg(windows)]
            tray_menu::precreate(app.handle());
            // 标题栏余额常驻显示：后台每 5 分钟刷新一次
            balance::start_periodic_refresh(app.handle().clone());
            // 运行期每 6 小时自动检查一次 dsh 更新（发现新版弹提示，不自动安装）
            updater::start_periodic_check(app.handle().clone());
            // 任务完成系统通知（主窗口不可见时；只读轮询 dsh 会话日志）
            notify::start_task_watch(app.handle().clone());
            // dsh 页面心跳监控：页面挂起/崩溃时重载自愈（指数退避）
            heartbeat::start_page_watch(app.handle().clone());
            // 跟随 dsh 的设置（语言/主题）：后台每 15s 读取 settings.yaml
            tray::start_follow_dsh_settings(app.handle().clone());
            // 窗口以隐藏状态创建，图标就绪后再显示 —— 任务栏/标题栏第一帧即是清晰图标
            let minimized = std::env::args().any(|a| a == "--minimized");
            if minimized {
                logging::log("启动: --minimized 静默进托盘");
            } else if let Some(win) = main_window(app.handle()) {
                let _ = win.show();
            }
            // 启动静默期：恢复/协商产生的几何事件不落盘（3s 内），
            // 避免系统微调后的尺寸被持久化、逐次启动累积变大
            window::start_save_settle(3000);

            match tray::create(app.handle()) {
                Ok(()) => logging::log("托盘: 已创建"),
                Err(e) => logging::log(&format!("托盘: 创建失败：{e}")),
            }
            let handle = app.handle().clone();
            std::thread::spawn(move || dsh::boot_loop(handle));
            Ok(())
        })
        .on_window_event(|window, event| match event {
            tauri::WindowEvent::CloseRequested { api, .. } => {
                // 关窗 = 最小化到托盘；真正退出走托盘“退出”或 quit 命令。
                // 无论哪条路径，先保存一次窗口状态：退出路径（is_quitting）
                // 下窗口即将销毁，等 ExitRequested 再读时窗口句柄已不存在，
                // 最后一次位置会丢失。
                window::save_window_state_now(window.app_handle());
                if !window
                    .app_handle()
                    .state::<AppState>()
                    .inner()
                    .is_quitting()
                {
                    api.prevent_close();
                    let _ = window.hide();
                }
            }
            tauri::WindowEvent::Focused(focused) => {
                // 标题栏失焦样式跟随主窗口焦点：子 webview 的 window
                // focus/blur 事件与主窗口焦点并不同步（WebView2 行为），
                // 由 Rust 侧统一广播，titlebar.js 按此切换样式
                if let Some(wv) = window
                    .webviews()
                    .into_iter()
                    .find(|w| w.label() == crate::titlebar::TITLEBAR_LABEL)
                {
                    let _ = wv.eval(format!(
                        "window.__dshdSetWindowActive && window.__dshdSetWindowActive({focused})"
                    ));
                }
                if *focused {
                    // 获焦时触发重绘脉冲：合成层失效导致的标题栏空白
                    // 在窗口重新激活时自愈
                    crate::titlebar::repaint_pulse(window.app_handle());
                }
            }
            tauri::WindowEvent::Resized(_) => {
                // 标题栏/主 webview 边界跟随窗口尺寸（必须先于带 guard 的臂执行，
                // 否则 resize 时 sync_bounds 永不触发，标题栏被主 webview 覆盖）
                titlebar::sync_bounds(window.app_handle());
                if !window
                    .app_handle()
                    .state::<AppState>()
                    .inner()
                    .is_quitting()
                {
                    // 拖动/缩放时节流保存（500ms 内最多落盘一次）
                    window::save_window_state(window.app_handle());
                }
            }
            tauri::WindowEvent::Moved(_)
                if !window
                    .app_handle()
                    .state::<AppState>()
                    .inner()
                    .is_quitting() =>
            {
                window::save_window_state(window.app_handle());
            }
            _ => {}
        })
        .build(tauri::generate_context!())
        .expect("failed to build tauri application");

    app.run(|app_handle, event| {
        match event {
            tauri::RunEvent::ExitRequested { .. } => {
                window::save_window_state_now(app_handle);
                dsh::shutdown(app_handle);
            }
            // macOS：点击系统通知/从后台恢复时恢复隐藏窗口（Windows 的通知
            // 点击走系统激活 + 单实例回调 show_main，此处兜底 macOS/Linux）
            tauri::RunEvent::Resumed => show_main(app_handle),
            _ => {}
        }
    });
}

#[cfg(test)]
mod url_tests {
    use super::is_local_app_url;

    #[test]
    fn local_app_origin_is_an_exact_pair() {
        assert!(is_local_app_url(
            &"tauri://localhost/index.html".parse().unwrap(),
            None
        ));
        assert!(is_local_app_url(
            &"http://tauri.localhost/dialog.html".parse().unwrap(),
            None
        ));
        assert!(!is_local_app_url(
            &"http://localhost/index.html".parse().unwrap(),
            None
        ));
        assert!(!is_local_app_url(
            &"http://tauri.localhost:3080/index.html".parse().unwrap(),
            None
        ));
    }

    #[test]
    fn dev_origin_allows_only_the_exact_dev_url() {
        let dev: url::Url = "http://localhost:4321".parse().unwrap();
        assert!(is_local_app_url(
            &"http://localhost:4321/titlebar.html".parse().unwrap(),
            Some(&dev)
        ));
        assert!(!is_local_app_url(
            &"http://localhost:9999/titlebar.html".parse().unwrap(),
            Some(&dev)
        ));
        // 用户名伪装不构成同一来源
        assert!(!is_local_app_url(
            &"http://localhost:4321@evil.com/titlebar.html"
                .parse()
                .unwrap(),
            Some(&dev)
        ));
        // 生产构建（无 devUrl）不放行 dev 来源
        assert!(!is_local_app_url(
            &"http://localhost:4321/titlebar.html".parse().unwrap(),
            None
        ));
    }
}
