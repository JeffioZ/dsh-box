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

/** 主菜单与托盘菜单共用的可中断动效状态机。 */
function dshdCreateMenuMotion(surface) {
  const reducedMotion = window.matchMedia('(prefers-reduced-motion: reduce)');
  let openFrame = 0;
  let closeTimer = 0;
  let pressTimer = 0;

  function cancelOpenFrame() {
    cancelAnimationFrame(openFrame);
    openFrame = 0;
  }

  function clearTimers() {
    clearTimeout(closeTimer);
    clearTimeout(pressTimer);
    closeTimer = 0;
    pressTimer = 0;
  }

  function open(enterY) {
    cancelOpenFrame();
    clearTimers();
    surface.style.setProperty('--dshd-menu-enter-y', enterY || '-3px');
    surface.classList.remove('dshd-menu-open', 'dshd-menu-closing');
    void surface.offsetWidth;
    if (reducedMotion.matches) {
      surface.classList.add('dshd-menu-open');
      return;
    }
    openFrame = requestAnimationFrame(() => {
      openFrame = 0;
      surface.classList.add('dshd-menu-open');
    });
  }

  function close(onClosed) {
    cancelOpenFrame();
    clearTimers();
    surface.classList.remove('dshd-menu-open');
    surface.classList.add('dshd-menu-closing');
    const delay = reducedMotion.matches
      ? 0
      : dshdCssDurationMs('--dshd-menu-exit-duration', 90);
    closeTimer = setTimeout(() => {
      closeTimer = 0;
      surface.classList.remove('dshd-menu-closing');
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
    surface.classList.remove('dshd-menu-open', 'dshd-menu-closing');
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
