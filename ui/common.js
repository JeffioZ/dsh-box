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
    text = text.replaceAll('{' + name + '}', String(value));
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

const DSHD_PASSWORD_ICONS = {
  show: '<svg viewBox="0 0 24 24" focusable="false" aria-hidden="true"><path d="M2.5 12s3.5-6.5 9.5-6.5 9.5 6.5 9.5 6.5-3.5 6.5-9.5 6.5S2.5 12 2.5 12Z"></path><circle cx="12" cy="12" r="2.5"></circle></svg>',
  hide: '<svg viewBox="0 0 24 24" focusable="false" aria-hidden="true"><path d="M3 3l18 18"></path><path d="M10.6 5.7A10.8 10.8 0 0 1 12 5.5c6 0 9.5 6.5 9.5 6.5a16 16 0 0 1-2.2 3.1"></path><path d="M6.2 6.3A16.7 16.7 0 0 0 2.5 12s3.5 6.5 9.5 6.5a10.4 10.4 0 0 0 4-.8"></path><path d="M10.2 10.2a2.5 2.5 0 0 0 3.6 3.6"></path></svg>',
};

/** API Key 等密码输入框共用的可见性按钮、焦点保持与空值状态。 */
function dshdBindPasswordToggle(input, toggle) {
  if (!input || !toggle) return;
  const sync = () => {
    const hasValue = String(input.value || '').length > 0;
    if (!hasValue && input.type === 'text') input.type = 'password';
    const visible = input.type === 'text';
    const label = dshdT(visible ? 'settingsApiKeyHideAria' : 'settingsApiKeyShowAria');
    toggle.innerHTML = DSHD_PASSWORD_ICONS[visible ? 'hide' : 'show'];
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
