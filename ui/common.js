// DSHBox 共享前端工具。双语文案集中在 i18n.js。
// 各窗口页面无打包器，共享脚本以普通 <script src="common.js"> 引用。

let DSHD_LANGUAGE = (() => {
  const preferred = String(
    // 与 dsh 的兜底一致：无注入语言、浏览器语言也不是 zh/en 时落产品默认 zh
    window.__DSHD_LANG || (navigator.languages && navigator.languages[0]) || navigator.language || 'zh-CN',
  ).toLowerCase();
  return preferred.startsWith('zh') ? 'zh-CN' : 'en';
})();

// —— 双保险：window.__TAURI__ 未注入时（如 app.withGlobalTauri 被误删），
//    页面不会在解构/访问处抛 TypeError 整页白屏——IPC 调用降级为明确失败
//    的 Promise，控制台打印一次醒目错误，且不覆盖已存在的正常注入。
//    曾因误删该配置导致标题栏/托盘菜单整页失效且无任何提示。
if (!window.__TAURI__) {
  console.error(
    '[DSHD] window.__TAURI__ 未注入：请检查 tauri.conf.json 的 app.withGlobalTauri 是否为 true（IPC 将全部失败）',
  );
  window.__TAURI__ = {
    core: {
      invoke: async () => {
        throw new Error('TAURI API not injected (app.withGlobalTauri missing)');
      },
    },
    event: { listen: async () => () => {} },
    window: {
      getCurrentWindow: () => ({
        close: () => {},
        hide: () => {},
        minimize: () => {},
        toggleMaximize: () => {},
        isMaximized: async () => false,
      }),
    },
  };
}


function dshdLocale() {
  return DSHD_LANGUAGE;
}

function dshdT(key, values) {
  const pair = DSHD_MESSAGES[key];
  let text = pair ? pair[DSHD_LANGUAGE === 'zh-CN' ? 0 : 1] : key;
  for (const [name, value] of Object.entries(values || {})) {
    // 函数形式替换：值里的 $&、$' 等不会被当作特殊替换模式展开
    text = text.replaceAll('{' + name + '}', () => String(value));
  }
  return text;
}

function dshdApplyI18n(root) {
  document.documentElement.lang = DSHD_LANGUAGE;
  const scope = root || document;
  scope.querySelectorAll('[data-i18n]').forEach((el) => {
    el.textContent = dshdT(el.dataset.i18n);
  });
  scope.querySelectorAll('[data-i18n-placeholder]').forEach((el) => {
    el.setAttribute('placeholder', dshdT(el.dataset.i18nPlaceholder));
  });
  scope.querySelectorAll('[data-i18n-title]').forEach((el) => {
    el.title = dshdT(el.dataset.i18nTitle);
  });
  scope.querySelectorAll('[data-i18n-aria-label]').forEach((el) => {
    el.setAttribute('aria-label', dshdT(el.dataset.i18nAriaLabel));
  });
}

function dshdSetLanguage(language) {
  DSHD_LANGUAGE = String(language || '').toLowerCase().startsWith('zh') ? 'zh-CN' : 'en';
  window.__DSHD_LANG = DSHD_LANGUAGE;
  dshdApplyI18n();
  window.dispatchEvent(new CustomEvent('dshd-language-changed', {
    detail: { language: DSHD_LANGUAGE },
  }));
}

function dshdCssDurationMs(name, fallback) {
  const raw = getComputedStyle(document.documentElement).getPropertyValue(name).trim();
  const value = Number.parseFloat(raw);
  if (!Number.isFinite(value)) return fallback;
  return raw.endsWith('s') && !raw.endsWith('ms') ? value * 1000 : value;
}

// —— 事件签名校验：core:event 的 listen/emit 不区分来源，dsh 页可伪造
//    事件驱动内置 UI；Rust 发射的载荷带 __dshdNonce，本助手先经 origin
//    守卫的 event_nonce 命令取值（dsh 页拿不到），不匹配的事件直接丢弃。
let dshdEventNonceCache = null;
function dshdEventNonce() {
  if (!dshdEventNonceCache) {
    dshdEventNonceCache = window.__TAURI__.core.invoke('event_nonce').catch(() => null);
  }
  return dshdEventNonceCache;
}
async function dshdListen(event, handler) {
  try {
    const nonce = await dshdEventNonce();
    return await window.__TAURI__.event.listen(event, (e) => {
      if (!nonce || !e.payload || e.payload.__dshdNonce !== nonce) return;
      handler(e);
    });
  } catch {
    return null;
  }
}

// —— 共享图标：路径数据唯一定义点，页面经 dshdIcon 生成带各自属性的 svg ——
// 条目统一取自 lucide 官方路径数据（ISC，见 THIRD_PARTY_NOTICES.md），
// 24×24 描边式，与各上下文的 stroke 渲染规则天然一致。
const DSHD_ICON_PATHS = {
  download: '<path d="M12 17V3"></path><path d="m6 11 6 6 6-6"></path><path d="M19 21H5"></path>',
  // 插件：lucide「puzzle」（与 ZCode 同源的图标体系，ISC 许可）。
  // 键名沿用 puzzle：与 Rust 侧菜单模型（tray_menu.rs 的 icon 字符串）耦合，
  // 改动需两端同步。
  puzzle: '<path d="M15.39 4.39a1 1 0 0 0 1.68-.474 2.5 2.5 0 1 1 3.014 3.015 1 1 0 0 0-.474 1.68l1.683 1.682a2.414 2.414 0 0 1 0 3.414L19.61 15.39a1 1 0 0 1-1.68-.474 2.5 2.5 0 1 0-3.014 3.015 1 1 0 0 1 .474 1.68l-1.683 1.682a2.414 2.414 0 0 1-3.414 0L8.61 19.61a1 1 0 0 0-1.68.474 2.5 2.5 0 1 1-3.014-3.015 1 1 0 0 0 .474-1.68l-1.683-1.682a2.414 2.414 0 0 1 0-3.414L4.39 8.61a1 1 0 0 1 1.68.474 2.5 2.5 0 1 0 3.014-3.015 1 1 0 0 1-.474-1.68l1.683-1.682a2.414 2.414 0 0 1 3.414 0z"></path>',
  gear: '<path d="M9.671 4.136a2.34 2.34 0 0 1 4.659 0 2.34 2.34 0 0 0 3.319 1.915 2.34 2.34 0 0 1 2.33 4.033 2.34 2.34 0 0 0 0 3.831 2.34 2.34 0 0 1-2.33 4.033 2.34 2.34 0 0 0-3.319 1.915 2.34 2.34 0 0 1-4.659 0 2.34 2.34 0 0 0-3.32-1.915 2.34 2.34 0 0 1-2.33-4.033 2.34 2.34 0 0 0 0-3.831A2.34 2.34 0 0 1 6.35 6.051a2.34 2.34 0 0 0 3.319-1.915"></path><circle cx="12" cy="12" r="3"></circle>',
  info: '<circle cx="12" cy="12" r="10"></circle><path d="M12 16v-4"></path><path d="M12 8h.01"></path>',
  clock: '<circle cx="12" cy="12" r="10"></circle><path d="M12 6v6l4 2"></path>',
  chevronDown: '<path d="m6 9 6 6 6-6"></path>',
  // —— 直接换 d 的图标条目（运行时 setAttribute 切换，非 dshdIcon 标签形态）——
  // 注意：这些是裸 d 字符串，与上方给 dshdIcon 用的完整标签条目不可混用，
  // 键名故意的区分开
  winMax: 'M4 4h16v16H4z',
  // 还原图标：完整框在左下，被盖住的框在右上（只露上+右两边）
  winRestore: 'M4 8h12v12H4z M8 4h12v12',
  // 密码可见性：眼睛 ⇄ 眼睛斜杠（lucide eye/eye-off 衍生：瞳孔 r2.5，
  // 16px 下比原版 r3 圆润可辨；保持单 path 直接换 d 切换）
  eyeShow: 'M2.5 12s3.5-6.5 9.5-6.5 9.5 6.5 9.5 6.5-3.5 6.5-9.5 6.5S2.5 12 2.5 12Z M14.5 12a2.5 2.5 0 1 1-5 0 2.5 2.5 0 1 1 5 0Z',
  eyeHide: 'M3 3l18 18 M10.6 5.7A10.8 10.8 0 0 1 12 5.5c6 0 9.5 6.5 9.5 6.5a16 16 0 0 1-2.2 3.1 M6.2 6.3A16.7 16.7 0 0 0 2.5 12s3.5 6.5 9.5 6.5a10.4 10.4 0 0 0 4-.8 M10.2 10.2a2.5 2.5 0 0 0 3.6 3.6',
};

/**
 * 生成线性图标 svg。attrs 传元素级属性串（如
 * 'focusable="false" aria-hidden="true"'）；路径数据见 DSHD_ICON_PATHS，
 * 各页面不得再复制图标 path 字面量。条目统一为 24×24 描边式（与各上下文
 * 的 stroke 渲染规则天然一致，不再需要对象形态/内联样式特例）。
 */
function dshdIcon(name, attrs) {
  const def = DSHD_ICON_PATHS[name];
  return '<svg viewBox="0 0 24 24"' + (attrs ? ' ' + attrs : '') + '>'
    + (def || '') + '</svg>';
}

// —— 分隔线物理像素对齐 ——
// 1px 高的分隔线在 125%/150% 缩放下落在半像素相位，同屏多条线因起点
// 不同被抗锯齿成不同深浅（肉眼可见的不一致）。统一为恰好 1 物理像素的
// CSS 高度后，线的覆盖不再依赖起点相位，所有分隔线渲染一致。导航分隔线
// （.nav-sep）与菜单分隔线（.dshd-sep）都引用 --dshd-hair。
(function installHairlineScale() {
  const apply = () => {
    const dpr = Math.max(1, window.devicePixelRatio || 1);
    document.documentElement.style.setProperty('--dshd-hair', (1 / dpr) + 'px');
  };
  const onDprChange = () => {
    mq.removeEventListener('change', onDprChange);
    apply();
    watch();
  };
  let mq = null;
  const watch = () => {
    mq = window.matchMedia('(resolution: ' + (window.devicePixelRatio || 1) + 'dppx)');
    mq.addEventListener('change', onDprChange);
  };
  apply();
  watch();
})();

/** API Key 等密码输入框共用的可见性按钮、焦点保持与空值状态。 */
function dshdBindPasswordToggle(input, toggle) {
  if (!input || !toggle) return;
  const sync = () => {
    const hasValue = String(input.value || '').length > 0;
    if (!hasValue && input.type === 'text') input.type = 'password';
    const visible = input.type === 'text';
    const label = dshdT(visible ? 'settingsApiKeyHideAria' : 'settingsApiKeyShowAria');
    if (!toggle.firstElementChild) {
      toggle.innerHTML = '<svg viewBox="0 0 24 24" focusable="false" aria-hidden="true"><path d="'
        + DSHD_ICON_PATHS.eyeShow + '"></path></svg>';
    }
    const pathEl = toggle.querySelector('path');
    if (pathEl) pathEl.setAttribute('d', visible ? DSHD_ICON_PATHS.eyeHide : DSHD_ICON_PATHS.eyeShow);
    toggle.hidden = !hasValue;
    toggle.setAttribute('aria-label', label);
    toggle.title = label;
    toggle.setAttribute('aria-pressed', String(visible));
  };
  toggle.__dshdPasswordSync = sync;
  if (!toggle.hasAttribute('data-dshd-password-toggle')) {
    toggle.setAttribute('data-dshd-password-toggle', '');
    // 鼠标操作不抢走输入框的焦点和光标；键盘 Tab 仍能正常聚焦按钮。
    toggle.addEventListener('mousedown', (event) => {
      if (event.button === 0) event.preventDefault();
    });
    toggle.addEventListener('click', () => {
      if (!input.value) return;
      const start = input.selectionStart;
      const end = input.selectionEnd;
      const direction = input.selectionDirection;
      input.type = input.type === 'password' ? 'text' : 'password';
      if (document.activeElement === input && Number.isInteger(start) && Number.isInteger(end)) {
        try { input.setSelectionRange(start, end, direction || 'none'); } catch (_) {}
      }
      sync();
    });
    input.addEventListener('input', sync);
  }
  sync();
}

let dshdTextContextSurface = null;
let dshdTextContextList = null;
let dshdTextContextMenu = null;
let dshdTextContextMotion = null;
let dshdTextContextItems = [];
let dshdTextContextTarget = null;
let dshdTextContextOpen = false;

function dshdEnsureTextContextMenu() {
  if (dshdTextContextSurface || typeof dshdCreateMenu !== 'function') return Boolean(dshdTextContextSurface);
  dshdTextContextSurface = document.createElement('div');
  dshdTextContextSurface.className = 'dshd-menu-surface dshd-menu-motion dshd-text-context-menu';
  dshdTextContextSurface.hidden = true;
  dshdTextContextList = document.createElement('div');
  dshdTextContextList.className = 'dshd-text-context-list';
  dshdTextContextList.setAttribute('role', 'menu');
  dshdTextContextSurface.append(dshdTextContextList);
  document.body.append(dshdTextContextSurface);
  dshdTextContextMotion = dshdCreateMenuMotion(dshdTextContextSurface);
  dshdTextContextMenu = dshdCreateMenu(dshdTextContextList, {
    onChoose(id) {
      const item = dshdTextContextItems.find((candidate) => candidate.id === id);
      if (!item || item.enabled === false || !item.act) return;
      dshdTextContextMotion.afterPress(() => {
        dshdCloseTextContextMenu(true);
        item.act();
      });
    },
    onEscape() { dshdCloseTextContextMenu(true); },
  });
  return true;
}

function dshdCloseTextContextMenu(restoreFocus) {
  if (!dshdTextContextOpen || !dshdTextContextSurface) return;
  dshdTextContextOpen = false;
  const target = dshdTextContextTarget;
  dshdTextContextTarget = null;
  dshdTextContextItems = [];
  dshdTextContextMotion.close(() => {
    if (!dshdTextContextOpen && dshdTextContextSurface) dshdTextContextSurface.hidden = true;
  });
  if (restoreFocus && target && document.contains(target)) {
    try { target.focus({ preventScroll: true }); } catch (_) {
      try { target.focus(); } catch (_) {}
    }
  }
}

function dshdPlaceTextContextMenu(x, y, target) {
  const keyboardOpen = x === 0 && y === 0;
  if (keyboardOpen) {
    const targetRect = target.getBoundingClientRect();
    x = targetRect.left + 8;
    y = targetRect.bottom + 4;
  }
  dshdTextContextSurface.style.left = '0px';
  dshdTextContextSurface.style.top = '0px';
  const rect = dshdTextContextSurface.getBoundingClientRect();
  const side = window.innerWidth >= rect.width + 48 ? 24 : 8;
  const top = window.innerHeight >= rect.height + 56 ? 16 : 8;
  const bottom = window.innerHeight >= rect.height + 56 ? 40 : 8;
  const left = Math.min(Math.max(x, side), Math.max(side, window.innerWidth - rect.width - side));
  const placedTop = Math.min(Math.max(y, top), Math.max(top, window.innerHeight - rect.height - bottom));
  dshdTextContextSurface.style.left = Math.round(left) + 'px';
  dshdTextContextSurface.style.top = Math.round(placedTop) + 'px';
  return keyboardOpen;
}

function dshdShowTextContextMenu(event, editable) {
  if (!dshdEnsureTextContextMenu()) return false;
  if (dshdTextContextOpen) dshdCloseTextContextMenu(false);
  dshdTextContextMotion.reset();
  dshdTextContextTarget = editable;
  dshdTextContextItems = window.__DSHD_EDIT_CONTEXT.createMenuItems(editable, {
    undo: dshdT('editUndo'),
    redo: dshdT('editRedo'),
    cut: dshdT('editCut'),
    copy: dshdT('editCopy'),
    paste: dshdT('editPaste'),
    selectAll: dshdT('editSelectAll'),
  });
  dshdTextContextMenu.setItems(dshdTextContextItems, true);
  dshdTextContextSurface.hidden = false;
  const keyboardOpen = dshdPlaceTextContextMenu(event.clientX, event.clientY, editable);
  dshdTextContextOpen = true;
  dshdTextContextMotion.open('-4px');
  dshdTextContextMenu.focusFirst();
  if (keyboardOpen) dshdTextContextList.classList.add('dshd-menu-keyboard');
  return true;
}

document.addEventListener('contextmenu', (event) => {
  const editContext = window.__DSHD_EDIT_CONTEXT;
  const editable = editContext && editContext.findEditable(event.target);
  if (!editable) {
    dshdCloseTextContextMenu(false);
    event.preventDefault();
    return;
  }
  if (dshdShowTextContextMenu(event, editable)) event.preventDefault();
});

document.addEventListener('pointerdown', (event) => {
  if (dshdTextContextOpen && !dshdTextContextSurface.contains(event.target)) {
    dshdCloseTextContextMenu(false);
  }
});
window.addEventListener('blur', () => dshdCloseTextContextMenu(false));
window.addEventListener('resize', () => dshdCloseTextContextMenu(false));
window.addEventListener('wheel', () => dshdCloseTextContextMenu(false), true);
window.addEventListener('touchmove', () => dshdCloseTextContextMenu(false), true);

window.addEventListener('dshd-language-changed', () => {
  dshdCloseTextContextMenu(true);
  document.querySelectorAll('[data-dshd-password-toggle]').forEach((toggle) => {
    if (toggle.__dshdPasswordSync) toggle.__dshdPasswordSync();
  });
});

/** 主菜单与托盘菜单共用的可中断动效状态机。 */
function dshdCreateMenuMotion(surface) {
  const reducedMotion = window.matchMedia('(prefers-reduced-motion: reduce)');
  let openFrame = 0;
  let closeTimer = 0;
  let pressTimer = 0;
  let settleTimer = 0;

  function cancelOpenFrame() {
    cancelAnimationFrame(openFrame);
    openFrame = 0;
  }

  function clearTimers() {
    clearTimeout(closeTimer);
    clearTimeout(pressTimer);
    clearTimeout(settleTimer);
    closeTimer = 0;
    pressTimer = 0;
    settleTimer = 0;
  }

  function open(enterY) {
    cancelOpenFrame();
    clearTimers();
    surface.style.setProperty('--dshd-menu-enter-y', enterY || '-4px');
    surface.classList.remove('dshd-menu-open', 'dshd-menu-closing');
    surface.classList.add('dshd-menu-animating');
    void surface.offsetWidth;
    if (reducedMotion.matches) {
      surface.classList.add('dshd-menu-open');
      surface.classList.remove('dshd-menu-animating');
      return;
    }
    openFrame = requestAnimationFrame(() => {
      openFrame = 0;
      surface.classList.add('dshd-menu-open');
      settleTimer = setTimeout(() => {
        settleTimer = 0;
        surface.classList.remove('dshd-menu-animating');
      }, dshdCssDurationMs('--dshd-menu-enter-duration', 140));
    });
  }

  function close(onClosed) {
    cancelOpenFrame();
    clearTimers();
    surface.classList.remove('dshd-menu-open');
    surface.classList.add('dshd-menu-closing');
    surface.classList.toggle('dshd-menu-animating', !reducedMotion.matches);
    const delay = reducedMotion.matches
      ? 0
      : dshdCssDurationMs('--dshd-menu-exit-duration', 90);
    closeTimer = setTimeout(() => {
      closeTimer = 0;
      surface.classList.remove('dshd-menu-closing', 'dshd-menu-animating');
      if (onClosed) onClosed();
    }, delay);
  }

  function afterPress(callback) {
    clearTimeout(pressTimer);
    const delay = reducedMotion.matches
      ? 0
      : dshdCssDurationMs('--dshd-menu-press-delay', 70);
    pressTimer = setTimeout(() => {
      pressTimer = 0;
      callback();
    }, delay);
  }

  function reset() {
    cancelOpenFrame();
    clearTimers();
    surface.classList.remove('dshd-menu-open', 'dshd-menu-closing', 'dshd-menu-animating');
  }

  return { open, close, afterPress, reset };
}

/** HTML 转义（文本插入 innerHTML 前调用）。 */
function dshdEsc(s) {
  return String(s).replace(/[&<>"']/g, (c) => ({
    '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;',
  }[c]));
}

/** 币种显示符号（CNY → ¥，其余原样）。 */
function dshdCurrency(c) {
  return c === 'CNY' ? '¥' : c;
}

/** 余额字段格式化：空值兜底为 0.00，纯数字加千分位，其他原样。 */
function dshdBalanceValue(v) {
  const s = v != null && v !== '' ? String(v) : '0.00';
  if (!/^\d+(\.\d+)?$/.test(s)) return s;
  const [int, dec] = s.split('.');
  const grouped = int.replace(/\B(?=(\d{3})+(?!\d))/g, ',');
  // 无小数部分补 .00，与空值兜底格式一致（如 0 → 0.00、110 → 110.00）
  return grouped + '.' + (dec !== undefined ? dec : '00');
}
