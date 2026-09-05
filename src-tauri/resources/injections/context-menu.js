// 分隔线高 = 1/dpr（恰 1 设备像素）：与内置菜单 common.js 的 --dshd-hair 同
// 机制。1px 字面值在分数缩放（125%/150%）下会被设备像素 snap 成 1 或 2 行
// 不等——分隔块 4+1+4=9px 在 150% 下是 13.5 设备px，每多一条分隔线相位漂移
// 半像素，同一菜单内厚度就不一致；归一后分隔块回到整数设备高度，相位不漂移
var HAIR_PX = (1 / (window.devicePixelRatio || 1)) + 'px';
var css = [
  // 与 dsh 菜单同规格：卡片 r14/pad4（圆角为自绘浮层统一档，其余逐值同 dsh）、
  // 条目 min-h40/r10/14px、hover 8%/按压 14%；
  // 最小宽 168：比 dsh 卡宽（218）窄，短条目与快捷键列之间的留白更协调，
  // 长条目自动撑宽
  '.__dshd_cm{position:fixed;z-index:2147483000;min-width:168px;padding:4px;',
  'background:var(--dsw-specific-menu,#353638);',
  // 描边：基础档沿用 dsh inverted（浅色=透明、深色=白 6%，与改前一致）；
  // dsh 深色主题下升级 border-l2 档——深色下阴影柔光几乎不可见，由描边承担
  // 分离（dsh 上游 elevation 体系结论），浮层落在同色卡片上时白 6% 不够用。
  // 无 dsh 令牌时回退白 12%（内置页菜单同档）
  'border:1px solid var(--dsw-alias-border-inverted,rgba(255,255,255,.06));',
  // 菜单圆角 14px（偏离上游现值 20：同心圆要求 面板半径 − 内边距 = 条目
  // 半径，14 − 4 = 10 与条目 r10 严格同心，全部菜单统一取舍）+ shadow-lv3
  // + 字体跟随 dsh（--dsw-font-family）：右键菜单与 dsh 自家浮层同屏出现，
  // 字体必须与 dsh 原生一致；令牌缺失时回退 dsh 同款系统栈
  'border-radius:14px;box-shadow:var(--dsw-shadow-lv3,0 0 1px rgba(0,0,0,.2),0 0 4px rgba(0,0,0,.02),0 12px 32px rgba(0,0,0,.08));',
  'font:14px/22px var(--dsw-font-family,-apple-system,BlinkMacSystemFont,"Segoe UI","PingFang SC","Hiragino Sans GB","Microsoft YaHei","Helvetica Neue",Helvetica,Arial,sans-serif);',
  'color:var(--dsw-alias-label-primary,#f9fafb);user-select:none;',
  'animation:dshd-cm-in .11s ease-out;}',
  // 深色主题描边升级（必须是独立顶层规则：拼在基础规则块内会被 CSS 嵌套
  // 解析成 & 后代选择器而永不匹配）；body[data-ds-dark-theme] 与 dsh 自家
  // 样式表的深色钩子一致，主题切换时自动跟随
  'body[data-ds-dark-theme] .__dshd_cm{border-color:var(--dsw-alias-border-l2,rgba(255,255,255,.12));}',
  '@keyframes dshd-cm-in{from{opacity:0;transform:translateY(-4px)}to{opacity:1;transform:translateY(0)}}',
  '.__dshd_cm_i{min-height:40px;padding:8px 10px;border-radius:10px;cursor:default;white-space:nowrap;',
  'display:flex;align-items:center;gap:8px;box-sizing:border-box;',
  // hover 淡入淡出：与托盘/标题栏菜单、弹窗按钮的过渡节奏一致
  'transition:background-color .12s ease,color .12s ease;}',
  '.__dshd_cm_i:hover{background:var(--dsw-alias-interactive-bg-hover,rgba(255,255,255,.08));}',
  '.__dshd_cm_i:focus{outline:none;}',
  '.__dshd_cm.__dshd_cm_kbd .__dshd_cm_i:focus{outline:2px solid var(--dsw-brand-color-primary,#5686fe);outline-offset:-2px;}',
  // 按压底色读 dsh interactive-bg-active：深色 14% 白/浅色 10% 蓝灰，与内置
  // --dshd-pressed 两主题逐值一致；fallback 为深色值
  '.__dshd_cm_i:active{background:var(--dsw-alias-interactive-bg-active,rgba(255,255,255,.14));}',
  '.__dshd_cm_i.__dshd_cm_p{background:var(--dsw-alias-interactive-bg-active,rgba(255,255,255,.14));}',
  // 禁用透明度 .4 = dsh Menu .item:disabled 逐值（内置 .dshd-row 已同步）
  '.__dshd_cm_i.__dshd_cm_d{opacity:.4;pointer-events:none;}',
  '@media (prefers-reduced-motion:reduce){.__dshd_cm{animation:none;}',
  '.__dshd_cm_i{transition:none;}}',
  '.__dshd_cm_l{flex:1;min-width:0;overflow:hidden;text-overflow:ellipsis;}',
  // 图标/快捷键 rest = label-secondary、hover 提亮 primary：与内置 .dshd-row
  // 的 .ic/.dim 同规格（dsh 原生 itemIcon 为 tertiary 静态，此处按内置取舍）
  '.__dshd_cm_ic{width:16px;height:16px;flex:none;display:block;color:var(--dsw-alias-label-secondary,#cfd3d6);}',
  '.__dshd_cm_i:hover .__dshd_cm_ic{color:var(--dsw-alias-label-primary,#f9fafb);}',
  ' .__dshd_cm_ic svg{display:block;width:16px;height:16px;fill:none;stroke:currentColor;',
  'stroke-width:1.8;stroke-linecap:round;stroke-linejoin:round;}',
  '.__dshd_cm_k{color:var(--dsw-alias-label-secondary,#cfd3d6);font-size:12px;}',
  '.__dshd_cm_i:hover .__dshd_cm_k{color:var(--dsw-alias-label-primary,#f9fafb);}',
  '.__dshd_cm_ar{font-family:"Segoe Fluent Icons","Segoe MDL2 Assets",sans-serif;',
  'font-size:12px;color:var(--dsw-alias-label-tertiary,#adb2b8);margin-left:6px;line-height:1;}',
  '.__dshd_cm_sep{height:' + HAIR_PX + ';margin:4px 2px;background:var(--dsw-alias-border-l1,rgba(255,255,255,.06));}',
  // 退出动效：与主菜单/托盘菜单一致的淡出（90ms）
  '.__dshd_cm_out{opacity:0;transition:opacity .09s ease;}',
  // 子菜单：与 dsh 一致，最小宽 163；伪元素桥接父项与子菜单间隙，鼠标跨过不丢悬停
  '.__dshd_cm_sub{min-width:163px;}',
  '.__dshd_cm_sub::before{content:"";position:absolute;top:0;bottom:0;left:-6px;width:6px;}',
  // 复制结果：不移动菜单/页面布局，短暂显示后自行消失
  '.__dshd_cm_toast{position:fixed;z-index:2147483001;left:50%;bottom:18px;transform:translateX(-50%);',
  'padding:6px 10px;border-radius:7px;pointer-events:none;white-space:nowrap;',
  'font:12px/18px var(--dsw-font-family,-apple-system,BlinkMacSystemFont,"Segoe UI","Microsoft YaHei",sans-serif);',
  'color:var(--dsw-alias-label-primary,#f9fafb);background:var(--dsw-specific-menu,#353638);',
  'border:1px solid var(--dsw-alias-border-inverted,rgba(255,255,255,.08));',
  'box-shadow:var(--dsw-shadow-lv2,0 6px 18px rgba(0,0,0,.28));animation:dshd-toast-in .1s ease-out;}',
  '.__dshd_cm_toast.__dshd_cm_error{border-color:var(--dsw-alias-state-error,#e85c5c);}',
  '@keyframes dshd-toast-in{from{opacity:0;transform:translate(-50%,3px)}to{opacity:1;transform:translate(-50%,0)}}',
  '@media (prefers-reduced-motion:reduce){.__dshd_cm,.__dshd_cm_toast{animation:none;}}'
].join('');
// 重复注入（page-load 主路径 + navigate 兜底）时样式只挂一次
if (!document.getElementById('__dshd_cm_style')) {
  var styleEl = document.createElement('style');
  styleEl.id = '__dshd_cm_style';
  styleEl.textContent = css;
  document.documentElement.appendChild(styleEl);
}

var menuEl = null;var items = [];
var subEl = null;
var subItems = [];
var subTimer = null;
var subParent = null;
var contextSequence = 0;
var PRESS_DELAY_MS = 70;
var SHADOW_SIDE = 36;
var SHADOW_TOP = 24;
var SHADOW_BOTTOM = 48;
var IS_MAC = /Mac/i.test(navigator.userAgent);
var IS_WIN = /Windows/i.test(navigator.userAgent);
// 语言契约：注入 wrapper（navigation.rs inject_dsh_page）与本脚本同帧内联
// window.__DSHD_LANG；navigator.language 仅是异常兜底
var UI_ZH = String(window.__DSHD_LANG || navigator.language || '').toLowerCase().indexOf('zh') === 0;
function T(zh, en) { return UI_ZH ? zh : en; }
window.__dshdSetInjectedLanguage = function (language) {
  UI_ZH = String(language || '').toLowerCase().indexOf('zh') === 0;
};
function MOD() { return IS_MAC ? '⌘' : 'Ctrl'; }

// JS → Rust 通道：自定义协议 dshd。Windows 的注册形式是 http://dshd.localhost/<动作>，
// macOS/Linux 是 dshd://localhost/<动作>（Tauri 平台差异）。
var DSH_REQ_BASE = IS_WIN ? 'http://dshd.localhost/' : 'dshd://localhost/';
// 令牌只走 X-DSHd-Token 请求头、不进 URL：URL 会进页面 resource timing
// 缓冲区，同源任意脚本可枚举提取（凭令牌可经 content 动作读本地文本文件）
var DSH_TOKEN = window.__dshdProtocolToken || '';
function dshdUrl(action, query) {
  return DSH_REQ_BASE + action + (query ? '?' + query : '');
}
function dshdFetch(action, query) {
  return fetch(dshdUrl(action, query), { headers: { 'X-DSHd-Token': DSH_TOKEN } });
}
// 探测环境能力（VS Code 是否安装）；未回包前按未安装处理
var HAS_CODE = false;
try {
  dshdFetch('probe', 'what=vscode').then(function (r) { return r.text(); })
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
  if (menuEl) {
    // 统一退出动效：与主菜单/托盘菜单一致的淡出（90ms），
    // 播放完再移除 DOM；reduced-motion 下立即移除
    var el = menuEl;
    menuEl = null;
    if (window.matchMedia && window.matchMedia('(prefers-reduced-motion: reduce)').matches) {
      el.remove();
    } else {
      el.classList.add('__dshd_cm_out');
      setTimeout(function () { el.remove(); }, 90);
    }
  }
  items = [];
  // 回收文件图标 objectURL（app: 缓存常驻复用，不在此列）；
  // 已解码的图标不受 revoke 影响，菜单淡出期间不会空白
  menuIconUrls.forEach(function (url) { URL.revokeObjectURL(url); });
  menuIconUrls = [];
  // 焦点归还有失败可能（元素已离文档）：无论是否归还都清掉引用，避免
  // 失效元素长期挂在外观变量上
  var focusTarget = focusReturn;
  focusReturn = null;
  if (focusTarget && document.contains(focusTarget)) {
    try { focusTarget.focus(); } catch (e) {}
  }
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
  if (s === '..' || s.slice(0, 2) === './' || s.slice(0, 3) === '../') return true;
  return /[\\/]/.test(s) && /\.[A-Za-z0-9]{1,8}$/.test(s);
}
function isAbsPath(s) {
  // 与 Rust Path::is_absolute 语义对齐：Windows 上 '/' 前缀不是绝对路径
  //（/c/ 等 MSYS 形式由 resolveAbsPath 走 normalize），Unix 上 '/' 才是
  return /^[A-Za-z]:[\\/]/.test(s) || s.slice(0, 2) === '\\\\'
    || (!IS_WIN && s.charAt(0) === '/');
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
function isTextLike(p) {
  // 凭据文件不提供文本动作（yaml 在白名单内，特判避免经 content/菜单复制泄出）
  var base = String(p).replace(/^.*[\/]/, '').toLowerCase();
  if (base.indexOf('.credentials') === 0) { return false; }
  return TEXT_EXTS.indexOf(extOf(p)) >= 0;
}
function isImageLike(p) { return IMG_EXTS.indexOf(extOf(p)) >= 0; }
function isExeLike(p) { return EXE_EXTS.indexOf(extOf(p)) >= 0; }

// —— 动作通道 ——
// 只发不收；自定义头只能在 CORS 模式携带（no-cors 会丢弃非安全清单外的头），
// 原来的 <img> 兜底带不了请求头、只能退回 URL 令牌，一并移除
function req(action, path, app) {
  try {
    dshdFetch(action, 'path=' + encodeURIComponent(path)
      + (app ? '&app=' + encodeURIComponent(app) : '')).catch(function () {});
  } catch (e) {}
}
function copyToast(ok) {
  var old = document.querySelector('.__dshd_cm_toast');
  if (old) old.remove();
  var el = document.createElement('div');
  el.className = '__dshd_cm_toast' + (ok ? '' : ' __dshd_cm_error');
  el.setAttribute('role', 'status');
  el.textContent = ok ? T('已复制', 'Copied') : T('复制失败', 'Copy failed');
  document.body.appendChild(el);
  setTimeout(function () { el.remove(); }, 1100);
}
function fallbackWriteClip(t) {
  var el = document.createElement('textarea');
  el.value = t;
  el.setAttribute('aria-hidden', 'true');
  el.style.cssText = 'position:fixed;left:-9999px;top:0;opacity:0';
  document.body.appendChild(el);
  el.select();
  var ok = false;
  try { ok = document.execCommand('copy'); } catch (e) {}
  el.remove();
  copyToast(ok);
}
function writeClip(t) {
  try {
    var promise = navigator.clipboard && navigator.clipboard.writeText
      ? navigator.clipboard.writeText(t) : null;
    if (promise && promise.then) {
      promise.then(function () { copyToast(true); }, function () { fallbackWriteClip(t); });
    } else {
      fallbackWriteClip(t);
    }
  } catch (e) { fallbackWriteClip(t); }
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
var pathBaseCache = { key: '', value: null, expiresAt: 0, pending: null };
function pathBaseKey() {
  return typeof dshSessionTitle === 'string' ? dshSessionTitle.trim() : '';
}
function fetchPathBase(key) {
  return rpc('session.list', {}).then(function (json) {
    var sessionItems = (rpcValue(json) || {}).items || [];
    var exact = key ? sessionItems.filter(function (it) { return it.title === key; }) : [];
    var candidates = exact.length ? exact : sessionItems;
    var best = null;
    candidates.forEach(function (it) {
      if (!it.cwd) return;
      if (it.running) {
        if (!best || !best.running || it.updatedAt > best.updatedAt) best = it;
        return;
      }
      if (!best || (!best.running && it.updatedAt > best.updatedAt)) best = it;
    });
    if (best && best.cwd) return best.cwd;
    return rpc('workspace.list', {}).then(function (json) {
      var workspaceItems = (rpcValue(json) || {}).items || [];
      return workspaceItems.length === 1 && workspaceItems[0].path ? workspaceItems[0].path : null;
    }).catch(function () { return null; });
  }).catch(function () { return null; });
}
function resolvePathBase() {
  var key = pathBaseKey();
  var now = Date.now();
  if (pathBaseCache.key === key && pathBaseCache.expiresAt > now) {
    return Promise.resolve(pathBaseCache.value);
  }
  if (pathBaseCache.key === key && pathBaseCache.pending) return pathBaseCache.pending;
  var pending = fetchPathBase(key).then(function (base) {
    if (pathBaseCache.key === key) {
      pathBaseCache.value = base;
      pathBaseCache.expiresAt = Date.now() + (base ? 5000 : 1000);
      pathBaseCache.pending = null;
    }
    return base;
  }, function () {
    if (pathBaseCache.key === key) pathBaseCache.pending = null;
    return null;
  });
  pathBaseCache = { key: key, value: null, expiresAt: 0, pending: pending };
  return pending;
}
function resolveAbsPath(rel) {
  if (rel === '~' || rel.slice(0, 2) === '~/' || rel.slice(0, 2) === '~\\'
      || (IS_WIN && /^\/[A-Za-z]\//.test(rel))) {
    return dshdFetch('normalize', 'path=' + encodeURIComponent(rel))
      .then(function (r) { if (!r.ok) return null; return r.text(); })
      .catch(function () { return null; });
  }
  return resolvePathBase().then(function (base) { return base ? joinPath(base, rel) : null; });
}
// 会话页稳定后预热 cwd；右击相对路径时通常无需再等待两次 RPC。
setTimeout(function () { resolvePathBase(); }, 300);
function copyContent(path) {
  try {
    dshdFetch('content', 'path=' + encodeURIComponent(path))
      .then(function (r) { if (!r.ok) throw 0; return r.text(); })
      .then(function (t) { writeClip(t); })
      .catch(function () { copyToast(false); });
  } catch (e) { copyToast(false); }
}
function copyImage(img) {
  try {
    var w = img.naturalWidth || img.width || 0;
    var h = img.naturalHeight || img.height || 0;
    if (!w || !h) { copyToast(false); return; }
    var c = document.createElement('canvas');
    c.width = w; c.height = h;
    var ctx = c.getContext('2d');
    if (!ctx) { copyToast(false); return; }
    ctx.drawImage(img, 0, 0, w, h);
    c.toBlob(function (b) {
      if (!b) { copyToast(false); return; }
      try {
        navigator.clipboard.write([new ClipboardItem({ 'image/png': b })])
          .then(function () { copyToast(true); }, function () { copyToast(false); });
      } catch (e2) { copyToast(false); }
    }, 'image/png');
  } catch (e) { copyToast(false); }
}

// —— 菜单渲染（支持图标异步填充 + hover 子菜单）——
// 图标首帧即占位（内联 SVG 数据 URL，零网络），真实图标加载完成后原地替换；
// 提取失败则保留占位符，不会出现空白。app: 图标在注入时预取一次预热 Rust 缓存。
var PH_FILE = "data:image/svg+xml,%3Csvg%20xmlns='http://www.w3.org/2000/svg'%20width='16'%20height='16'%20viewBox='0%200%2016%2016'%3E%3Cpath%20d='M4.5%201.5h5L12%204v10.5h-7.5z'%20fill='none'%20stroke='%238b8b94'/%3E%3Cpath%20d='M9.5%201.5V4H12'%20fill='none'%20stroke='%238b8b94'/%3E%3C/svg%3E";
var PH_APP = "data:image/svg+xml,%3Csvg%20xmlns='http://www.w3.org/2000/svg'%20width='16'%20height='16'%20viewBox='0%200%2016%2016'%3E%3Crect%20x='1.5'%20y='1.5'%20width='5.5'%20height='5.5'%20rx='1'%20fill='none'%20stroke='%238b8b94'/%3E%3Crect%20x='9'%20y='1.5'%20width='5.5'%20height='5.5'%20rx='1'%20fill='none'%20stroke='%238b8b94'/%3E%3Crect%20x='1.5'%20y='9'%20width='5.5'%20height='5.5'%20rx='1'%20fill='none'%20stroke='%238b8b94'/%3E%3Crect%20x='9'%20y='9'%20width='5.5'%20height='5.5'%20rx='1'%20fill='none'%20stroke='%238b8b94'/%3E%3C/svg%3E";
function placeholderFor(spec) {
  return spec && spec.slice(0, 5) === 'file:' ? PH_FILE : PH_APP;
}
// 内置 stroke 图标（与托盘/标题栏菜单同风格，随文字颜色 currentColor；
// 真实文件/应用图标走 img + 异步加载）。`ic:` 前缀引用。
var ICON_SVGS = {
  cut: '<svg viewBox="0 0 24 24"><circle cx="6" cy="6" r="3"></circle><path d="M8.12 8.12 12 12"></path><path d="M20 4 8.12 15.88"></path><circle cx="6" cy="18" r="3"></circle><path d="M14.8 14.8 20 20"></path></svg>',
  copy: '<svg viewBox="0 0 24 24"><rect x="9" y="9" width="13" height="13" rx="2"></rect><path d="M5 15H4a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h9a2 2 0 0 1 2 2v1"></path></svg>',
  paste: '<svg viewBox="0 0 24 24"><rect x="8" y="2" width="8" height="4" rx="1"></rect><path d="M16 4h2a2 2 0 0 1 2 2v14a2 2 0 0 1-2 2H6a2 2 0 0 1-2-2V6a2 2 0 0 1 2-2h2"></path></svg>',
  select: '<svg viewBox="0 0 24 24"><path d="M4 8V6a2 2 0 0 1 2-2h2"></path><path d="M16 4h2a2 2 0 0 1 2 2v2"></path><path d="M20 16v2a2 2 0 0 1-2 2h-2"></path><path d="M8 20H6a2 2 0 0 1-2-2v-2"></path><path d="M8.5 8.5h7v7h-7z"></path></svg>',
  undo: '<svg viewBox="0 0 24 24"><path d="M9 7 4 12l5 5"></path><path d="M4 12h10a6 6 0 0 1 6 6"></path></svg>',
  redo: '<svg viewBox="0 0 24 24"><path d="m15 7 5 5-5 5"></path><path d="M20 12H10a6 6 0 0 0-6 6"></path></svg>',
  save: '<svg viewBox="0 0 24 24"><path d="M19 21H5a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h11l5 5v11a2 2 0 0 1-2 2z"></path><path d="M17 21v-8H7v8"></path><path d="M7 3v5h8"></path></svg>',
  link: '<svg viewBox="0 0 24 24"><path d="M10 13a5 5 0 0 0 7.54.54l3-3a5 5 0 0 0-7.07-7.07l-1.72 1.71"></path><path d="M14 11a5 5 0 0 0-7.54-.54l-3 3a5 5 0 0 0 7.07 7.07l1.71-1.71"></path></svg>',
  folder: '<svg viewBox="0 0 24 24"><path d="M20 20a2 2 0 0 0 2-2V8a2 2 0 0 0-2-2h-7.9a2 2 0 0 1-1.69-.9L9.6 3.9A2 2 0 0 0 7.93 3H4a2 2 0 0 0-2 2v13a2 2 0 0 0 2 2Z"></path></svg>',
  image: '<svg viewBox="0 0 24 24"><rect x="3" y="3" width="18" height="18" rx="2"></rect><circle cx="9" cy="9" r="2"></circle><path d="m21 15-3.086-3.086a2 2 0 0 0-2.828 0L6 21"></path></svg>',
  globe: '<svg viewBox="0 0 24 24"><circle cx="12" cy="12" r="9"></circle><path d="M3 12h18"></path><path d="M12 3c3 3.4 3 14 0 18"></path><path d="M12 3c-3 3.4-3 14 0 18"></path></svg>',
  apps: '<svg viewBox="0 0 24 24"><rect x="3" y="3" width="7" height="7" rx="1"></rect><rect x="14" y="3" width="7" height="7" rx="1"></rect><rect x="14" y="14" width="7" height="7" rx="1"></rect><rect x="3" y="14" width="7" height="7" rx="1"></rect></svg>'
};
// 图标 HTML：内置 stroke 图标内联渲染（颜色随文字），文件/应用图标走
// 占位图 + 异步加载（真实图标加载完成后原地替换）
function iconHtml(spec) {
  if (!spec) return '';
  if (spec.slice(0, 3) === 'ic:') {
    var svg = ICON_SVGS[spec.slice(3)];
    return svg ? '<span class="__dshd_cm_ic" aria-hidden="true">' + svg + '</span>' : '';
  }
  return '<img class="__dshd_cm_ic" alt="" src="' + placeholderFor(spec) + '" />';
}
// <img> 直接加载带不了自定义头：图标统一 fetch（带头）→ blob → objectURL。
// 文件图标 objectURL 随菜单 hide 回收（见 hide）；app: 图标常驻缓存复用
//（仅 code/notepad/paint 三个，预热即填满）。
var menuIconUrls = [];
var appIconCache = {};
function loadIcon(img, spec) {
  var query;
  if (spec.slice(0, 5) === 'file:') {
    query = 'path=' + encodeURIComponent(spec.slice(5));
  } else if (spec.slice(0, 4) === 'app:') {
    query = 'app=' + encodeURIComponent(spec.slice(4));
  } else { return; }
  var cached = appIconCache[spec];
  if (cached) { img.src = cached; return; }
  dshdFetch('icon', query).then(function (r) {
    if (!r.ok) throw 0;
    return r.blob();
  }).then(function (blob) {
    var url = URL.createObjectURL(blob);
    if (spec.slice(0, 4) === 'app:') { appIconCache[spec] = url; }
    else { menuIconUrls.push(url); }
    img.src = url;
  }).catch(function () {}); // 提取失败保留占位图，不出现空白
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
      html += '<div class="__dshd_cm_i" role="menuitem" tabindex="-1" data-i="' + i + '">'
        + iconHtml(it.icon)
        + '<span class="__dshd_cm_l">' + it.label + '</span></div>';
    }
  });
  subEl.innerHTML = html;
  menuEl.appendChild(subEl);
  subEl.querySelectorAll('.__dshd_cm_i').forEach(function (node) {
    var it = subItems[Number(node.getAttribute('data-i'))];
    if (it && it.icon && it.icon.slice(0, 3) !== 'ic:') {
      var img = node.querySelector('img.__dshd_cm_ic');
      if (img) loadIcon(img, it.icon);
    }
  });
  // 定位：与 dsh 子菜单一致——底部对齐父项（向上生长）、右侧 6px 间隙
  // （::before 桥接间隙，鼠标跨过不丢悬停）；放不下翻到左侧
  var pr = parentNode.getBoundingClientRect();
  var sr = subEl.getBoundingClientRect();
  var left = pr.right + 6;
  if (left + sr.width > window.innerWidth - SHADOW_SIDE) left = pr.left - sr.width - 6;
  left = clampSurface(left, sr.width, window.innerWidth, SHADOW_SIDE, SHADOW_SIDE);
  var top = pr.bottom - sr.height + 4;
  top = clampSurface(top, sr.height, window.innerHeight, SHADOW_TOP, SHADOW_BOTTOM);
  subEl.style.left = left + 'px';
  subEl.style.top = top + 'px';
}

function clampSurface(value, surfaceSize, viewportSize, leadingShadow, trailingShadow) {
  // 正常视口完整保留 shadow-lv3 的透明扩散区；极窄/极矮时退回 6px 内容
  // 安全边距，优先保证菜单本体可用。
  if (viewportSize >= surfaceSize + leadingShadow + trailingShadow) {
    return Math.max(leadingShadow, Math.min(value, viewportSize - surfaceSize - trailingShadow));
  }
  return Math.max(6, Math.min(value, Math.max(6, viewportSize - surfaceSize - 6)));
}

function closeSubSoon() {
  clearTimeout(subTimer);
  // 容差：慢速移动鼠标跨过父项与子菜单间隙时不至于误关。
  // 150ms 兼顾跟手（移走后快速收起）与防误关
  subTimer = setTimeout(closeSub, 150);
}

function renderItems(list) {
  closeSub();
  items = list;
  var html = '';
  list.forEach(function (it, i) {
    if (it.sep) { html += '<div class="__dshd_cm_sep" role="separator"></div>'; }
    else {
      var dis = it.enabled === false ? ' __dshd_cm_d' : '';
      var ariaDisabled = it.enabled === false ? ' aria-disabled="true"' : '';
      var subAttrs = it.sub ? ' aria-haspopup="menu" aria-expanded="false"' : '';
      var tail = it.sub ? '<span class="__dshd_cm_ar">&#xE76C;</span>'
        : (it.key ? '<span class="__dshd_cm_k">' + it.key + '</span>' : '');
      html += '<div class="__dshd_cm_i' + dis + '" role="menuitem" tabindex="-1"' + ariaDisabled + subAttrs
        + ' data-i="' + i + '">' + iconHtml(it.icon)
        + '<span class="__dshd_cm_l">' + it.label + '</span>' + tail + '</div>';
    }
  });
  menuEl.innerHTML = html;
  menuEl.querySelectorAll('.__dshd_cm_i').forEach(function (node) {
    var it = items[Number(node.getAttribute('data-i'))];
    if (it && it.icon && it.icon.slice(0, 3) !== 'ic:') {
      var img = node.querySelector('img.__dshd_cm_ic');
      if (img) loadIcon(img, it.icon);
    }
  });
}

function placeMenu(x, y) {
  // 边界处理：常规视口给 shadow-lv3 留出完整扩散区；极小视口退回 6px；
  // clientX/Y 与 fixed 定位同为 CSS 逻辑像素，任意 DPI 一致
  var r = menuEl.getBoundingClientRect();
  var mx = clampSurface(x, r.width, window.innerWidth, SHADOW_SIDE, SHADOW_SIDE);
  var my = clampSurface(y, r.height, window.innerHeight, SHADOW_TOP, SHADOW_BOTTOM);
  menuEl.style.left = mx + 'px';
  menuEl.style.top = my + 'px';
}

function updateVisibleMenu(x, y, list) {
  if (!menuEl) return;
  renderItems(list);
  placeMenu(x, y);
  var first = menuEl.querySelector('.__dshd_cm_i:not(.__dshd_cm_d)');
  if (first) first.focus();
}

function show(x, y, list) {
  hide();
  // 清理尚在淡出的旧菜单（hide 的延迟 remove 未到期时立即移除）
  var stale = document.querySelector('.__dshd_cm_out');
  if (stale) stale.remove();
  focusReturn = (document.activeElement && document.activeElement !== document.body)
    ? document.activeElement : null;
  menuEl = document.createElement('div');
  menuEl.className = '__dshd_cm';
  menuEl.setAttribute('role', 'menu');
  menuEl.setAttribute('aria-label', T('上下文菜单', 'Context menu'));
  renderItems(list);
  document.body.appendChild(menuEl);
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
  // 点击（mouseup）后保留一帧可见按压态，再执行动作；与内置菜单的节奏一致。
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
    }, PRESS_DELAY_MS);
  });
  placeMenu(x, y);
  var first = menuEl.querySelector('.__dshd_cm_i:not(.__dshd_cm_d)');
  if (first) first.focus();
}

// —— Codex 式文件菜单 ——
// p 为展示用路径或已解析的绝对路径（解析成功时全部动作/图标/复制路径都用绝对路径）
function fileMenu(f, p) {
  var abs = isAbsPath(p);
  var items = [{
    label: T('打开文件', 'Open file'), icon: 'file:' + p,
    // 相对路径解析失败且非 dsh 按钮时无法打开（Rust open 动作要求绝对路径），
    // 禁用而非静默无反应
    enabled: f.viaButton || abs,
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
      subs.push({ label: T('选择其他应用…', 'Choose another app…'), icon: 'ic:apps', act: function () { req('openwith', p); } });
      items.push({ label: T('打开方式', 'Open with'), sub: subs });
    }
  }
  items.push({ sep: true });
  if (abs) {
    items.push({ label: T('另存为…', 'Save as…'), icon: 'ic:save', act: function () { req('saveas', p); } });
  }
  items.push({ label: T('复制路径', 'Copy path'), icon: 'ic:link', act: function () { writeClip(p); } });
  if (abs && isTextLike(p)) {
    items.push({ label: T('复制文件内容', 'Copy file contents'), icon: 'ic:copy', act: function () { copyContent(p); } });
  }
  if (abs) {
    // 文件管理器名称随平台：macOS 为 Finder，Windows 为资源管理器，其余为文件管理器
    var fmLabel = IS_MAC
      ? T('在 Finder 中显示', 'Show in Finder')
      : (IS_WIN ? T('在资源管理器中打开', 'Show in File Explorer') : T('在文件管理器中打开', 'Show in file manager'));
    items.push({ label: fmLabel, icon: 'ic:folder', act: function () { req('reveal', p); } });
  }
  return items;
}

function onCtx(e) {
  var sequence = ++contextSequence;
  if (e.defaultPrevented) return; // dsh 自带右键菜单：放行
  var t = e.target;
  // 可编辑区优先：输入框内即便文本长得像路径，也应提供标准编辑菜单。
  var editContext = window.__DSHD_EDIT_CONTEXT;
  var el = editContext ? editContext.findEditable(t) : null;
  if (el) {
    e.preventDefault();
    show(e.clientX, e.clientY, editContext.createMenuItems(el, {
      undo: T('撤销', 'Undo'),
      redo: T('重做', 'Redo'),
      cut: T('剪切', 'Cut'),
      copy: T('复制', 'Copy'),
      paste: T('粘贴', 'Paste'),
      selectAll: T('全选', 'Select all')
    }, 'ic:'));
    return;
  }
  var f = findPathTarget(t);
  if (f) {
    e.preventDefault();
    var needsNormalize = f.path === '~' || f.path.slice(0, 2) === '~/'
      || f.path.slice(0, 2) === '~\\' || (IS_WIN && /^\/[A-Za-z]\//.test(f.path));
    if (isAbsPath(f.path) && !needsNormalize) {
      show(e.clientX, e.clientY, fileMenu(f, f.path));
    } else {
      // 相对路径先用可用动作即时出菜单；绝对路径解析完成后原位补齐图标、
      // 另存为、复制内容、资源管理器等动作，不因 RPC 延迟阻塞右键反馈或丢能力。
      show(e.clientX, e.clientY, fileMenu(f, f.path));
      resolveAbsPath(f.path).then(function (abs) {
        if (sequence !== contextSequence || !abs || !menuEl) return;
        updateVisibleMenu(e.clientX, e.clientY, fileMenu(f, abs));
      }).catch(function () {});
    }
    return;
  }
  var img = t && t.tagName === 'IMG' ? t : (t && t.closest ? t.closest('img') : null);
  if (img) {
    e.preventDefault();
    show(e.clientX, e.clientY, [
      { label: T('复制图片', 'Copy image'), icon: 'ic:image', act: function () { copyImage(img); } }
    ]);
    return;
  }
  var a = t && t.closest ? t.closest('a[href]') : null;
  if (a) {
    var href = a.getAttribute('href') || '';
    if (/^https?:/i.test(href)) {
      e.preventDefault();
      show(e.clientX, e.clientY, [
        { label: T('复制链接', 'Copy link'), icon: 'ic:link', act: function () { writeClip(href); } },
        { label: T('在浏览器中打开', 'Open in browser'), icon: 'ic:globe', act: function () { req('browse', href); } }
      ]);
      return;
    }
  }
  var sel = window.getSelection();
  if (sel && sel.toString()) {
    e.preventDefault();
    show(e.clientX, e.clientY, [
      { label: T('复制', 'Copy'), icon: 'ic:copy', key: MOD() + '+C', act: function () { document.execCommand('copy'); } }
    ]);
  } else {
    e.preventDefault(); // 无选区：静默屏蔽默认菜单
  }
}

document.addEventListener('contextmenu', onCtx);
document.addEventListener('mousedown', function (e) {
  if (menuEl && !menuEl.contains(e.target)) { ++contextSequence; hide(); }
  else if (!menuEl) { ++contextSequence; }
});
function cancelContext() { ++contextSequence; hide(); }
window.addEventListener('blur', cancelContext);
window.addEventListener('resize', cancelContext);
// 只在用户真实滚动时收起（wheel/touchmove）；不监听 scroll：
// 思考流式输出会程序化自动滚动聊天区，之前导致菜单被误关
window.addEventListener('wheel', cancelContext, true);
window.addEventListener('touchmove', cancelContext, true);
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
