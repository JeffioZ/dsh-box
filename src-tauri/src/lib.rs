//! DSHDesktop —— DeepSeek Harness (dsh) 桌面端外壳。
//!
//! 职责：管理 Node/dsh 运行时（检测、安装、更新），以隐藏窗口方式启动
//! `dsh web` 服务，用 WebView 加载 http://127.0.0.1:<port> 的官方界面，
//! 提供托盘菜单（打开 / 检查更新 / API 余额 / 退出），退出时清理全部子进程。

// MSVC link.exe 会向 stdout 输出“正在创建库 …”（/NOLOGO 无法抑制），
// 被 rustc 报告为 linker_messages 警告——按预期允许，保持构建输出干净。
#![allow(linker_messages)]

mod app_dialog;
mod app_state;
mod autostart;
mod balance;
mod commands;
mod dialog;
mod dsh;
mod file_actions;
mod icons;
mod logging;
mod processes;
mod runtime;
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

/// 对外产品名（窗口标题/托盘/exe 属性等统一显示名）。
pub const APP_TITLE: &str = "DeepSeek Harness Desktop";

/// 本地启动页（生产环境 Tauri 资源源）。
pub const SPLASH_ORIGIN: &str = "tauri://localhost";

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
  'display:flex;align-items:center;gap:8px;box-sizing:border-box;}',
  '.__dshd_cm_i:hover{background:var(--dsw-alias-interactive-bg-hover,rgba(255,255,255,.08));}',
  '.__dshd_cm_i:active{background:rgba(255,255,255,.14);}',
  '.__dshd_cm_i.__dshd_cm_p{background:rgba(255,255,255,.14);}',
  '.__dshd_cm_i.__dshd_cm_d{opacity:.4;pointer-events:none;}',
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

var menuEl = null;
var items = [];
var subEl = null;
var subItems = [];
var subTimer = null;
var IS_MAC = /Mac/i.test(navigator.userAgent);
var IS_WIN = /Windows/i.test(navigator.userAgent);
function MOD() { return IS_MAC ? '⌘' : 'Ctrl'; }

// JS → Rust 通道：自定义协议 dshd。Windows 的注册形式是 http://dshd.localhost/<动作>，
// macOS/Linux 是 dshd://localhost/<动作>（Tauri 平台差异）。
var DSH_REQ_BASE = IS_WIN ? 'http://dshd.localhost/' : 'dshd://localhost/';
// 探测环境能力（VS Code 是否安装）；未回包前按未安装处理
var HAS_CODE = false;
try {
  fetch(DSH_REQ_BASE + 'probe?what=vscode').then(function (r) { return r.text(); })
    .then(function (t) { HAS_CODE = t === '1'; }).catch(function () {});
} catch (e) {}

function closeSub() {
  clearTimeout(subTimer);
  if (subEl) { subEl.remove(); subEl = null; }
  subItems = [];
}
function hide() { closeSub(); if (menuEl) { menuEl.remove(); menuEl = null; } items = []; }

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
  var u = DSH_REQ_BASE + action + '?path=' + encodeURIComponent(path)
    + (app ? '&app=' + encodeURIComponent(app) : '');
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
    fetch(DSH_REQ_BASE + 'content?path=' + encodeURIComponent(path))
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
    url = DSH_REQ_BASE + 'icon?path=' + encodeURIComponent(spec.slice(5));
  } else if (spec.slice(0, 4) === 'app:') {
    url = DSH_REQ_BASE + 'icon?app=' + encodeURIComponent(spec.slice(4));
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
  subEl = document.createElement('div');
  subEl.className = '__dshd_cm __dshd_cm_sub';
  var html = '';
  list.forEach(function (it, i) {
    if (it.sep) { html += '<div class="__dshd_cm_sep"></div>'; }
    else {
      var ic = it.icon
        ? '<img class="__dshd_cm_ic" alt="" src="' + placeholderFor(it.icon) + '" />'
        : '';
      html += '<div class="__dshd_cm_i" data-i="' + i + '">' + ic
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
  items = list;
  menuEl = document.createElement('div');
  menuEl.className = '__dshd_cm';
  var html = '';
  list.forEach(function (it, i) {
    if (it.sep) { html += '<div class="__dshd_cm_sep"></div>'; }
    else {
      var dis = it.enabled === false ? ' __dshd_cm_d' : '';
      var ic = it.icon
        ? '<img class="__dshd_cm_ic" alt="" src="' + placeholderFor(it.icon) + '" />'
        : '';
      var tail = it.sub ? '<span class="__dshd_cm_ar">&#xE76C;</span>'
        : (it.key ? '<span class="__dshd_cm_k">' + it.key + '</span>' : '');
      html += '<div class="__dshd_cm_i' + dis + '" data-i="' + i + '">' + ic
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
    if (it && it.sub) openSub(node, it.sub);
    else closeSubSoon();
  });
  menuEl.addEventListener('mouseleave', closeSubSoon);
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
}

// —— Codex 式文件菜单 ——
// p 为展示用路径或已解析的绝对路径（解析成功时全部动作/图标/复制路径都用绝对路径）
function fileMenu(f, p) {
  var abs = isAbsPath(p);
  var items = [{
    label: '打开文件', icon: 'file:' + p,
    act: function () {
      if (f.viaButton) { f.el.click(); } // dsh 后端解析相对路径
      else if (abs) { req('open', p); }
    }
  }];
  if (abs && isTextLike(p) && HAS_CODE) {
    items.push({
      label: '在 VS Code 中打开', icon: 'app:code',
      act: function () { req('openapp', p, 'code'); }
    });
  }
  if (abs && !isExeLike(p)) {
    var subs = [];
    if (isTextLike(p)) {
      if (HAS_CODE) {
        subs.push({ label: 'VS Code', icon: 'app:code', act: function () { req('openapp', p, 'code'); } });
      }
      subs.push({ label: '记事本', icon: 'app:notepad', act: function () { req('openapp', p, 'notepad'); } });
    }
    if (isImageLike(p)) {
      subs.push({ label: '画图', icon: 'app:paint', act: function () { req('openapp', p, 'paint'); } });
    }
    if (subs.length) {
      subs.push({ sep: true });
      subs.push({ label: '选择其他应用…', act: function () { req('openwith', p, ''); } });
      items.push({ label: '打开方式', sub: subs });
    }
  }
  items.push({ sep: true });
  if (abs) {
    items.push({ label: '另存为…', act: function () { req('saveas', p); } });
  }
  items.push({ label: '复制路径', act: function () { writeClip(p); } });
  if (abs && isTextLike(p)) {
    items.push({ label: '复制文件内容', act: function () { copyContent(p); } });
  }
  if (abs) {
    // 文件管理器名称随平台：macOS 为 Finder，Windows 为资源管理器，其余为文件管理器
    var fmLabel = IS_MAC ? '在 Finder 中显示' : (IS_WIN ? '在资源管理器中打开' : '在文件管理器中打开');
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
      { label: '剪切', key: m + '+X', enabled: hasSel, act: function () { execOn(el, 'cut'); } },
      { label: '复制', key: m + '+C', enabled: hasSel, act: function () { execOn(el, 'copy'); } },
      { label: '粘贴', key: m + '+V', act: function () { pasteInto(el); } },
      { sep: true },
      { label: '全选', key: m + '+A', enabled: hasContent, act: function () {
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
      { label: '复制图片', act: function () { copyImage(img); } }
    ]);
    return;
  }
  var a = t && t.closest ? t.closest('a[href]') : null;
  if (a) {
    var href = a.getAttribute('href') || '';
    if (/^https?:/i.test(href)) {
      e.preventDefault();
      show(e.clientX, e.clientY, [
        { label: '复制链接', act: function () { writeClip(href); } },
        { label: '在浏览器中打开', act: function () { req('browse', href); } }
      ]);
      return;
    }
  }
  var sel = window.getSelection();
  if (sel && sel.toString()) {
    e.preventDefault();
    show(e.clientX, e.clientY, [
      { label: '复制', key: MOD() + '+C', act: function () { document.execCommand('copy'); } }
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
document.addEventListener('keydown', function (e) { if (e.key === 'Escape') hide(); });
"#;

/// 深色主题的统一底色（与 dsh 深色主题 body 背景 #151517 一致，衔接无缝）。
const DARK_BG: tauri::window::Color = tauri::window::Color(0x15, 0x15, 0x17, 0xFF);
/// 浅色主题的统一底色（与 dsh 浅色主题 body 背景纯白一致）。
const LIGHT_BG: tauri::window::Color = tauri::window::Color(0xFF, 0xFF, 0xFF, 0xFF);

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
    let payload = app_state::StatusPayload {
        phase: phase.as_str().to_string(),
        message: message.to_string(),
        detail: detail.to_string(),
        progress,
        dsh_version: None,
        node_version: None,
        port: None,
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
/// 对任意 URL 加载的页面均生效）。
///
/// 权限与页面既有能力对齐：dsh 页面本就可以通过自己的后端“默认程序打开”任意
/// 本地文件，这里只是补充 定位/另存为/指定应用打开/复制内容/图标提取；
/// 只接受绝对路径，相对路径的工作区解析归 dsh 后端（“打开”菜单项直接复用
/// 页面按钮自身的点击逻辑）。
/// 请求形如 `http://dshd.localhost/<动作>?path=…`（Windows）或
/// `dshd://localhost/<动作>?path=…`（macOS/Linux），动作在路径段。
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

    match (action.as_str(), path.as_deref()) {
        // 探测（前端问 VS Code 是否可用）
        ("probe", _) => {
            let body = match query("what").as_deref() {
                Some("vscode") if file_actions::vscode_exe().is_some() => "1",
                _ => "0",
            };
            scheme_response(200, "text/plain; charset=utf-8", body.as_bytes().to_vec())
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
                    Some(png) => scheme_response(200, "image/png", png),
                    None => {
                        logging::log(&format!("dshd: 图标提取失败：{}", s.display()));
                        scheme_response(404, "", Vec::new())
                    }
                },
                None => {
                    logging::log("dshd: 图标请求无有效来源");
                    scheme_response(404, "", Vec::new())
                }
            }
        }
        // 复制文件内容：读文本（限 2MB、拒绝二进制/非 UTF-8）
        ("content", Some(p)) if file_actions::is_absolute(p) => {
            match file_actions::read_text_file(std::path::Path::new(p), 2 * 1024 * 1024) {
                Ok(text) => scheme_response(200, "text/plain; charset=utf-8", text.into_bytes()),
                Err(_) => scheme_response(415, "", Vec::new()),
            }
        }
        // 在默认浏览器打开链接（仅 http/https）
        ("browse", Some(p)) => {
            if p.starts_with("http://") || p.starts_with("https://") {
                if let Err(e) = file_actions::open_browser(p) {
                    logging::log(&format!("dshd: 打开浏览器失败：{e}"));
                }
                scheme_response(204, "", Vec::new())
            } else {
                logging::log("dshd: 仅支持 http/https 链接");
                scheme_response(204, "", Vec::new())
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
            scheme_response(204, "", Vec::new())
        }
        // 用指定应用打开（code/notepad/paint，Windows）
        ("openapp", Some(p)) if file_actions::is_absolute(p) => {
            let result = match app.as_deref() {
                Some(a) => file_actions::open_with_app(a, std::path::Path::new(p)),
                None => Err("缺少 app 参数".into()),
            };
            match result {
                Ok(()) => logging::log(&format!("dshd: 指定应用打开已触发（{p}）")),
                Err(e) => logging::log(&format!("dshd: 指定应用打开失败：{e}")),
            }
            scheme_response(204, "", Vec::new())
        }
        ("open", Some(p)) if file_actions::is_absolute(p) => {
            match file_actions::open_default(std::path::Path::new(p)) {
                Ok(()) => logging::log(&format!("dshd: 默认程序打开已触发（{p}）")),
                Err(e) => logging::log(&format!("dshd: 默认程序打开失败：{e}")),
            }
            scheme_response(204, "", Vec::new())
        }
        ("reveal", Some(p)) if file_actions::is_absolute(p) => {
            match file_actions::reveal(std::path::Path::new(p)) {
                Ok(()) => logging::log(&format!("dshd: 定位文件已触发（{p}）")),
                Err(e) => logging::log(&format!("dshd: 定位文件失败：{e}")),
            }
            scheme_response(204, "", Vec::new())
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
                            format!("无法打开系统“打开方式”对话框：{e}"),
                            "打开方式",
                            MessageDialogKind::Warning,
                        );
                    }
                }
            });
            scheme_response(204, "", Vec::new())
        }
        (act, _) => {
            logging::log(&format!("dshd: 未处理请求：{act}"));
            scheme_response(204, "", Vec::new())
        }
    }
}

/// 构造自定义协议响应：统一加 CORS 头（页面 fetch 读取图标/文本需要）。
fn scheme_response(status: u16, mime: &str, body: Vec<u8>) -> http::Response<Vec<u8>> {
    http::Response::builder()
        .status(status)
        .header("Access-Control-Allow-Origin", "*")
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

/// 让 WebView 跳到 dsh 界面（或返回本地启动页）。
pub fn navigate(app: &AppHandle, url: &str) {
    let Some(wv) = main_webview(app) else {
        logging::log("navigate: 未找到主 webview");
        return;
    };
    if let Ok(u) = url::Url::parse(url) {
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
                let title = serde_json::to_string(APP_TITLE).unwrap_or_default();
                // 单实例：重复 navigate 不叠加观察器/菜单监听；
                // MutationObserver 只在 title 真正变化时拉回（无常驻轮询开销）；
                // 右键菜单定制一并注入（此通道在 dsh 页面生效）
                let _ = wv.eval(format!(
                    "(() => {{ if (window.__dshdInit) return; window.__dshdInit = true; \
                     const t = {title}; \
                     const fix = () => {{ if (document.title !== t) document.title = t; }}; \
                     fix(); \
                     const el = document.querySelector('head > title'); \
                     if (el) new MutationObserver(fix).observe(el, {{ childList: true }}); \
                     {menu} }})();",
                    menu = MENU_INJECT
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
        .manage(AppState::new())
        .invoke_handler(commands::invoke_handler())
        .setup(|app| {
            // 手建主窗口（conf windows 为空）：带初始化脚本预设 dsh 深色主题，
            // 背景色跟随系统主题，与 dsh/loading 底色统一，消除启动与导航的明暗闪烁
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
            .initialization_script(PAGE_INIT_SCRIPT)
            .build()
            .expect("主窗口创建失败");
            #[cfg(target_os = "macos")]
            let _ = win.set_title_bar_style(tauri::TitleBarStyle::Overlay);
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
                    }
                    // set_size 后重新居中（创建时的 center 基于初始尺寸）
                    let _ = win.center();
                }
            }
            // 启动后二次修正：show 会触发系统对窗口几何的协商（约 +14w/+37h），
            // 0.6s 后按保存值重新应用一次（覆盖协商漂移），1.5s 后再做一次
            // 越界兜底收敛；期间保存静默，避免漂移值被持久化造成逐次变大
            {
                let handle = app.handle().clone();
                let cfg = app.state::<AppState>().config();
                std::thread::spawn(move || {
                    std::thread::sleep(std::time::Duration::from_millis(600));
                    // 阶段一：按保存矩形（裁剪后）重新应用
                    if let Some((lx, ly, lw, lh)) = cfg.load_window_rect() {
                        if lw >= 400.0 && lh >= 300.0 {
                            if let Some(win) = main_window(&handle) {
                                if !win.is_maximized().unwrap_or(false) {
                                    if let Some((px, py, pw, ph)) = logical_work_area(&handle) {
                                        let wc = lw.min(pw);
                                        let hc = lh.min(ph);
                                        let xc = lx.clamp(px, px + pw - wc);
                                        let yc = ly.clamp(py, py + ph - hc);
                                        logging::log(&format!(
                                            "窗口: 重新应用 ({lx:.0},{ly:.0},{lw:.0}x{lh:.0}) -> ({xc:.0},{yc:.0},{wc:.0}x{hc:.0})"
                                        ));
                                        let _ = win.set_position(tauri::Position::Logical(
                                            tauri::LogicalPosition::new(xc, yc),
                                        ));
                                        let _ = win.set_size(tauri::Size::Logical(
                                            tauri::LogicalSize::new(wc, hc),
                                        ));
                                    }
                                }
                            }
                        }
                    }
                    std::thread::sleep(std::time::Duration::from_millis(900));
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
                    if let Some((px, py, pw, ph)) = logical_work_area(&handle) {
                        let wc = lw.min(pw);
                        let hc = lh.min(ph);
                        let xc = lx.clamp(px, px + pw - wc);
                        let yc = ly.clamp(py, py + ph - hc);
                        logging::log(&format!(
                            "窗口: 终态 逻辑=({lx:.0},{ly:.0},{lw:.0}x{lh:.0}) 工作区=({pw:.0}x{ph:.0})"
                        ));
                        if (xc - lx).abs() > 1.0
                            || (yc - ly).abs() > 1.0
                            || (wc - lw).abs() > 1.0
                            || (hc - lh).abs() > 1.0
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
            // 自绘弹窗与（Windows）托盘菜单窗口：启动时预创建（隐藏），
            // 此后只定位/显示/隐藏——绝不在事件回调里新建/销毁 WebView 窗口
            app_dialog::precreate(app.handle());
            #[cfg(windows)]
            tray_menu::precreate(app.handle());
            // 标题栏余额常驻显示：后台每 5 分钟刷新一次
            balance::start_periodic_refresh(app.handle().clone());
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
                if !window
                    .app_handle()
                    .state::<AppState>()
                    .inner()
                    .is_quitting()
                {
                    window::save_window_state_now(window.app_handle());
                    api.prevent_close();
                    let _ = window.hide();
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
        if let tauri::RunEvent::ExitRequested { .. } = event {
            window::save_window_state_now(app_handle);
            dsh::shutdown(app_handle);
        }
    });
}
