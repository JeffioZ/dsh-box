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
const DSHD_ICON_PATHS = {
  download: '<path d="M12 3v12"></path><path d="m7 10 5 5 5-5"></path><path d="M4 21h16"></path>',
  // 插件：复用 dsh 官方设置界面的“个性化”滑杆图标（IconPersonalizationOutline16，
  // 16×16 填充式，与官方设置导航同款）。键名沿用 puzzle：与 Rust 侧菜单模型
  // （tray_menu.rs 的 icon 字符串）耦合，改动需两端同步。
  puzzle: {
    vb: '0 0 16 16',
    body: '<path transform="translate(1.292 1.3)" style="fill:currentColor;stroke:none" d="M10.3232 9.18164C11.2868 9.18164 12.0985 9.82833 12.3506 10.7109L13.415 10.7109L13.415 11.8711L12.3496 11.8711C12.0971 12.7532 11.2864 13.3994 10.3232 13.3994C9.36031 13.3992 8.55012 12.7531 8.29785 11.8711L0 11.8711L0 10.7109L8.29688 10.7109C8.54876 9.82845 9.35988 9.18186 10.3232 9.18164ZM10.3232 10.3418C9.7999 10.3421 9.37534 10.7667 9.375 11.29C9.375 11.8137 9.79969 12.239 10.3232 12.2393C10.847 12.2393 11.2725 11.8138 11.2725 11.29C11.2721 10.7666 10.8468 10.3418 10.3232 10.3418ZM12.4326 11.291C12.4326 11.3549 12.4284 11.418 12.4229 11.4805C12.4287 11.4181 12.4326 11.355 12.4326 11.291ZM8.21484 11.2832C8.21484 11.2856 8.21484 11.2886 8.21484 11.291L8.21484 11.29C8.21484 11.2878 8.21484 11.2855 8.21484 11.2832ZM3.08301 4.59082C4.04605 4.59095 4.85696 5.23717 5.10938 6.11914L13.415 6.11914L13.415 7.2793L5.11035 7.2793C4.85833 8.16202 4.04648 8.80846 3.08301 8.80859C2.11972 8.80843 1.30963 8.16179 1.05762 7.2793L0 7.2793L0 6.11914L1.05762 6.11914C1.30994 5.23728 2.12006 4.59098 3.08301 4.59082ZM3.08301 5.75098C2.55962 5.75117 2.13512 6.17587 2.13477 6.69922C2.13477 7.22287 2.5594 7.64824 3.08301 7.64844C3.60665 7.64828 4.03223 7.2229 4.03223 6.69922C4.03187 6.17585 3.60643 5.75113 3.08301 5.75098ZM5.19238 6.69922C5.19238 6.763 5.18816 6.82633 5.18262 6.88867C5.18846 6.82629 5.19238 6.76313 5.19238 6.69922C5.19236 6.63495 5.18853 6.57152 5.18262 6.50879C5.18826 6.57154 5.19236 6.635 5.19238 6.69922ZM0.982422 6.52344C0.977382 6.58136 0.97463 6.63999 0.974609 6.69922C0.974609 6.75775 0.977496 6.81579 0.982422 6.87305C0.977758 6.81579 0.974609 6.75767 0.974609 6.69922C0.974628 6.64 0.977618 6.58142 0.982422 6.52344ZM10.3232 0C11.2869 0 12.0986 0.646596 12.3506 1.5293L13.415 1.5293L13.415 2.68945L12.3496 2.68945C12.363 2.64266 12.3754 2.59488 12.3857 2.54688C12.1838 3.50118 11.3376 4.21777 10.3232 4.21777C9.36037 4.21756 8.55018 3.57139 8.29785 2.68945L0 2.68945L0 1.5293L8.29688 1.5293C8.5487 0.646717 9.35981 0.00021854 10.3232 0ZM10.3232 1.16016C9.79984 1.16042 9.37524 1.58499 9.375 2.1084C9.375 2.63201 9.79969 3.05735 10.3232 3.05762C10.847 3.05762 11.2725 2.63217 11.2725 2.1084C11.2722 1.58483 10.8469 1.16016 10.3232 1.16016ZM12.4229 2.29883C12.4287 2.23641 12.4326 2.17331 12.4326 2.10938C12.4326 2.17327 12.4284 2.23638 12.4229 2.29883ZM8.21484 2.10938L8.21484 2.1084L8.21484 2.10938ZM8.22266 1.93359C8.21785 1.98897 8.21506 2.04499 8.21484 2.10156C8.21503 2.04501 8.2181 1.98902 8.22266 1.93359ZM8.22266 11.1162C8.2179 11.1713 8.21507 11.227 8.21484 11.2832C8.21504 11.227 8.21814 11.1713 8.22266 11.1162Z"></path>',
  },
  gear: '<circle cx="12" cy="12" r="3"></circle><path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 1 1-2.83 2.83l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 1 1-4 0v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 1 1-2.83-2.83l.06-.06a1.65 1.65 0 0 0 .33-1.82 1.65 1.65 0 0 0-1.51-1H3a2 2 0 1 1 0-4h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 1 1 2.83-2.83l.06.06a1.65 1.65 0 0 0 1.82.33H9a1.65 1.65 0 0 0 1-1.51V3a2 2 0 1 1 4 0v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 1 1 2.83 2.83l-.06.06a1.65 1.65 0 0 0-.33 1.82V9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 1 1 0 4h-.09a1.65 1.65 0 0 0-1.51 1z"></path>',
  info: '<circle cx="12" cy="12" r="9"></circle><path d="M12 11v6"></path><path d="M12 7.5v.01"></path>',
  clock: '<circle cx="12" cy="12" r="9"></circle><path d="M12 7v5l3 2"></path>',
  chevronDown: '<path d="m6 9 6 6 6-6"></path>',
  // —— morph 图标对（同 24×24 网格、描边式，经 morphicons 弹簧变形）——
  // 注意：这里必须是裸 d 字符串（morphTo 的解析输入），与上方给 dshdIcon 用的
  // 完整 <path> 标签条目（如 chevronDown）不可混用；键名故意的区分开
  winMax: 'M5 5h14v14H5Z',
  winRestore: 'M5 9h10v10H5Z M9 9V5h10v10h-4',
  menuArrowDown: 'M6 9L12 15L18 9',
  menuArrowUp: 'M6 15L12 9L18 15',
  // 密码可见性：眼睛 ⇄ 眼睛斜杠（瞳孔圆转 path 子路径以便整对变形）
  eyeShow: 'M2.5 12s3.5-6.5 9.5-6.5 9.5 6.5 9.5 6.5-3.5 6.5-9.5 6.5S2.5 12 2.5 12Z M14.5 12a2.5 2.5 0 1 1-5 0 2.5 2.5 0 1 1 5 0Z',
  eyeHide: 'M3 3l18 18 M10.6 5.7A10.8 10.8 0 0 1 12 5.5c6 0 9.5 6.5 9.5 6.5a16 16 0 0 1-2.2 3.1 M6.2 6.3A16.7 16.7 0 0 0 2.5 12s3.5 6.5 9.5 6.5a10.4 10.4 0 0 0 4-.8 M10.2 10.2a2.5 2.5 0 0 0 3.6 3.6',
};

/**
 * 生成线性图标 svg。attrs 传元素级属性串（如
 * 'focusable="false" aria-hidden="true"'）；路径数据见 DSHD_ICON_PATHS，
 * 各页面不得再复制图标 path 字面量。字符串条目为 24×24 描边式；对象条目
 * 自带 viewBox 与完整 body（官方 16×16 填充式图标，body 内以内联 style
 * 覆盖各上下文的 stroke 渲染规则）。
 */
function dshdIcon(name, attrs) {
  const def = DSHD_ICON_PATHS[name];
  if (def && typeof def === 'object') {
    return '<svg viewBox="' + def.vb + '"' + (attrs ? ' ' + attrs : '') + '>'
      + def.body + '</svg>';
  }
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

// —— 图标 morph（vendored morphicons，见 ui/vendor/morphicons/README.md）——
// 模块懒加载：未就绪、加载失败或身处无动态 import 的环境（vm 检查等）时，
// 调用方拿到的 setD 退化为直接替换 path 的 d，交互不因动画缺失而降级。
let dshdMorphCreate = null;
let dshdMorphLoading = null;
function dshdEnsureMorph() {
  if (dshdMorphLoading) return dshdMorphLoading;
  try {
    dshdMorphLoading = import('vendor/morphicons/dom.js')
      .then((mod) => { dshdMorphCreate = mod.createMorph; })
      .catch(() => {});
  } catch (e) {
    dshdMorphLoading = Promise.resolve();
  }
  return dshdMorphLoading;
}

/**
 * 把单个 <path> 绑定为可变形图标，返回 setD(d)。morph 就绪后带弹簧动画切换，
 * 否则直接换 d。reducedMotion 固定 'user'：跟随系统减弱动效偏好（本仓库
 * 强制约定；该库默认不尊重系统设置）。描边/网格要求见 vendor README。
 */
function dshdMorphIcon(pathEl) {
  let handle = null;
  let currentD = pathEl.getAttribute('d') || '';
  dshdEnsureMorph().then(() => {
    if (!dshdMorphCreate || !pathEl.isConnected) return;
    handle = dshdMorphCreate(pathEl, currentD, { reducedMotion: 'user' });
  });
  return (nextD) => {
    currentD = nextD;
    if (handle) handle.morphTo(nextD);
    else pathEl.setAttribute('d', nextD);
  };
}

/** API Key 等密码输入框共用的可见性按钮、焦点保持与空值状态。 */
function dshdBindPasswordToggle(input, toggle) {
  if (!input || !toggle) return;
  let setGlyph = null;
  const ensureGlyph = () => {
    // 首次渲染眼睛图标（单 path，后续经 morph 变形切换）
    if (toggle.firstElementChild) return;
    toggle.innerHTML = '<svg viewBox="0 0 24 24" focusable="false" aria-hidden="true"><path d="'
      + DSHD_ICON_PATHS.eyeShow + '"></path></svg>';
    const pathEl = toggle.querySelector('path');
    if (pathEl) setGlyph = dshdMorphIcon(pathEl);
  };
  const sync = () => {
    const hasValue = String(input.value || '').length > 0;
    if (!hasValue && input.type === 'text') input.type = 'password';
    const visible = input.type === 'text';
    const label = dshdT(visible ? 'settingsApiKeyHideAria' : 'settingsApiKeyShowAria');
    ensureGlyph();
    if (setGlyph) setGlyph(visible ? DSHD_ICON_PATHS.eyeHide : DSHD_ICON_PATHS.eyeShow);
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
