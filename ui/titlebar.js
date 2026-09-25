// 自绘标题栏：品牌区/拖拽区、共用主菜单与窗口控制按钮。
// （余额 chip 自底部状态栏迁入：状态栏已整体移除，统计交还 dsh 原生统计行）
const $ = (id) => document.getElementById(id);
const invoke = (command, args) => window.__TAURI__.core.invoke(command, args);
const listen = (event, handler) => window.__TAURI__.event.listen(event, handler);

let mainMenuOpen = false;
let mainMenu = null;
let mainMenuMotion = null;
let mainMenuRequest = 0;
let mainMenuSelectionPending = false;
let closeBehavior = 'tray';
let hoverSuspendPoint = null;
let lastPointerPoint = null;

function suspendWindowControlHover(event) {
  if (event && Number.isFinite(event.clientX) && Number.isFinite(event.clientY)) {
    hoverSuspendPoint = { x: event.clientX, y: event.clientY };
  } else if (!document.body.classList.contains('hover-suspended')) {
    hoverSuspendPoint = lastPointerPoint;
  }
  document.body.classList.add('hover-suspended');
  const active = document.activeElement;
  if (active instanceof HTMLElement && active.classList.contains('tb-btn')) active.blur();
}

function resumeWindowControlHover(event) {
  if (!document.body.classList.contains('hover-suspended')) return;
  // 最小化/失焦期间的 pointerleave 不代表用户真正移动了指针。
  // 等窗口重新激活后再由真实 pointermove 解除悬停，避免恢复时残留 hover。
  if (document.body.classList.contains('window-inactive')) return;
  // 恢复窗口时 WebView2 偶尔会在原坐标补发 pointermove。忽略这一帧；
  // 用户真正移动哪怕 1px 后立即恢复 hover，不要求先离开整组按钮。
  if (event && hoverSuspendPoint
      && event.clientX === hoverSuspendPoint.x && event.clientY === hoverSuspendPoint.y) return;
  document.body.classList.remove('hover-suspended');
  hoverSuspendPoint = null;
}

function applyCloseBehavior(value) {
  closeBehavior = value === 'quit' ? 'quit' : 'tray';
  const button = $('btn-close');
  const label = dshdT(closeBehavior === 'quit' ? 'quit' : 'closeToTray');
  button.title = label;
  button.setAttribute('aria-label', label);
}

function initPlatform() {
  const platform = (navigator.userAgentData && navigator.userAgentData.platform)
    ? navigator.userAgentData.platform
    : (navigator.platform || '');
  const isMac = platform.toLowerCase().includes('mac') || navigator.userAgent.includes('Macintosh');
  if (isMac) document.body.classList.add('macos');
  else $('win-buttons').classList.remove('hidden');
}

// 浮层高度按内容实测（+36px 标题栏 + 48px shadow-lv3 底部余量）：
// 与独立弹窗/托盘菜单使用同一阴影安全区；不足会被 webview
// 底缘硬切（阴影边缘突然截断）。Rust 端再按 36..620 收敛
function syncOverlayHeight() {
  const content = mainMenuOpen && $('main-menu-panel') ? $('main-menu-panel').offsetHeight : 0;
  const height = content > 0 ? 36 + content + 48 : 36;
  return invoke('titlebar_expand', { expand: height > 36, height }).catch(() => {});
}

async function refreshMainMenu() {
  try {
    mainMenu.setItems(await invoke('menu_get', { traySurface: false }));
  } catch (error) {
    // 刷新失败时保留上一次的菜单模型，功能不受影响
  }
}

function setMainMenuOpen(open, focusMenu = false) {
  const request = ++mainMenuRequest;
  mainMenuOpen = Boolean(open);
  if (!mainMenuOpen) mainMenuSelectionPending = false;
  $('main-menu-wrap').classList.toggle('open', mainMenuOpen);
  $('btn-menu').setAttribute('aria-expanded', String(mainMenuOpen));
  $('main-menu-panel').setAttribute('aria-hidden', String(!mainMenuOpen));
  $('main-menu-panel').inert = !mainMenuOpen;
  if (mainMenuOpen) {
    // 打开时不立即同步高度：等条目渲染完成后再按真实高度一次性扩展。
    // 先扩展再渲染会造成两次快速 resize（空面板高度→真实高度），
    // 双重重绘正是标题栏文案偶发闪烁的来源。
    // 鼠标打开菜单时不转移焦点（会打断按钮的 :active 态引起闪烁，
    // 原生菜单鼠标点开也不转移）；键盘激活由 focusMenu 在渲染完成后聚焦。
    refreshMainMenu().then(async () => {
      if (request !== mainMenuRequest || !mainMenuOpen) return;
      await syncOverlayHeight();
      if (request !== mainMenuRequest || !mainMenuOpen) return;
      mainMenuMotion.open('-4px');
      if (focusMenu) mainMenu.focusFirst();
    });
    return;
  }
  // 先完成共用的 90ms 纯淡出，再收缩标题栏 WebView，避免硬裁切阴影。
  mainMenuMotion.close(() => {
    if (request === mainMenuRequest && !mainMenuOpen) syncOverlayHeight();
  });
}

function bindMainMenu() {
  mainMenuMotion = dshdCreateMenuMotion($('main-menu-panel'));
  mainMenu = dshdCreateMenu($('main-menu-list'), {
    onChoose: (id) => {
      // 语义动作立即分发，不依赖动画完成；仅视觉关闭统一保留 70ms 按压反馈。
      mainMenuSelectionPending = true;
      invoke('menu_choose', { id }).catch(() => {});
      mainMenuMotion.afterPress(() => {
        mainMenuSelectionPending = false;
        setMainMenuOpen(false);
      });
    },
    onEscape: () => {
      setMainMenuOpen(false);
      $('btn-menu').focus();
    },
  });
  // 打开提前到 mousedown：mouseup 后 :active 样式结束、click 才加 .open 类，
  // 中间隔一帧背景「先亮后暗」——正是主菜单按钮闪烁的来源。
  // 关闭仍在 click（打开状态下点击 = 关闭）；键盘激活无 mousedown，click 兜底。
  let openedByMouseDown = false;
  $('btn-menu').addEventListener('mousedown', (event) => {
    if (event.button !== 0) return; // 仅主键参与开关
    if (!mainMenuOpen) {
      setMainMenuOpen(true);
      openedByMouseDown = true;
    } else {
      // 已打开时的按下不预打开；若上次标志因拖走释放等路径残留，在此复位
      openedByMouseDown = false;
    }
  });
  $('btn-menu').addEventListener('click', (event) => {
    event.stopPropagation();
    if (openedByMouseDown) {
      openedByMouseDown = false;
      return; // 本次点击已由 mousedown 处理（打开），避免立即再关闭
    }
    const willOpen = !mainMenuOpen;
    setMainMenuOpen(willOpen, willOpen && event.detail === 0);
  });
  $('main-menu-panel').addEventListener('click', (event) => event.stopPropagation());
  document.addEventListener('pointerdown', (event) => {
    if (mainMenuOpen && !$('main-menu-wrap').contains(event.target)) setMainMenuOpen(false);
  });
}

function bindWindowControls() {
  $('btn-min').addEventListener('click', (event) => {
    suspendWindowControlHover(event);
    invoke('titlebar_minimize').catch(() => resumeWindowControlHover());
  });
  $('btn-max').addEventListener('click', async () => {
    try {
      applyMaxState(await invoke('titlebar_toggle_maximize'));
    } catch (error) {
      // 忽略偶发的 IPC 失败：后续 resize 刷新会重试
    }
  });
  $('btn-close').addEventListener('click', () => invoke('titlebar_close').catch(() => {}));
  document.addEventListener('pointermove', (event) => {
    resumeWindowControlHover(event);
    lastPointerPoint = { x: event.clientX, y: event.clientY };
  }, { passive: true });
  $('win-buttons').addEventListener('pointerleave', resumeWindowControlHover);
}

function applyMaxState(maximized) {
  $('btn-max').classList.toggle('maximized', maximized);
  $('btn-max').title = dshdT(maximized ? 'restore' : 'maximize');
  $('btn-max').setAttribute('aria-label', dshdT(maximized ? 'restoreWindow' : 'maximize'));
  const glyph = $('btn-max').querySelector('path');
  if (glyph) glyph.setAttribute('d', maximized ? DSHD_ICON_PATHS.winRestore : DSHD_ICON_PATHS.winMax);
}

async function refreshMaxState() {
  try {
    applyMaxState(await invoke('titlebar_is_maximized'));
  } catch (error) {
    // 后端就绪后，下一次 resize 刷新会重试
  }
}

// ---------- 余额 chip（自底部状态栏迁入：点击打开用量页，系统默认悬停提示） ----------

const WALLET_ICON = dshdIcon('wallet', 'class="c-ic" aria-hidden="true"');
const esc = dshdEsc;
let lastBalance = null;
let hideBalance = false;
// 版本信息的实测宽度（可见时更新）：宽度分级恢复判定的防震荡输入
let versionWidth = 0;
let serviceMode = 'none';

// 余额预警：remaining/total ≤30% warning、≤10% critical；total 未知不加色
function chipLowLevel(entry) {
  const remaining = entry ? Number(entry.remaining) : NaN;
  const total = entry ? Number(entry.total) : NaN;
  if (!Number.isFinite(remaining) || !Number.isFinite(total) || total <= 0) return 'none';
  const ratio = remaining / total;
  return ratio <= 0.1 ? 'critical' : ratio <= 0.3 ? 'warning' : 'none';
}

function balanceChipState() {
  const b = lastBalance;
  if (!b) return { text: '--', dot: 'err', kind: 'unavailable' };
  if (b.error_kind === 'no_key') return { text: dshdT('balanceNoKey'), dot: 'neutral', kind: 'no_key' };
  if (b.error_kind === 'invalid_key') return { text: dshdT('balanceInvalidKey'), dot: 'warn', kind: 'invalid_key' };
  if (!b.ok) return { text: '--', dot: 'err', kind: 'unavailable' };
  if (b.error) return { text: dshdT('balanceUnavailable'), dot: 'warn', kind: 'unavailable' };
  if (!b.balances || !b.balances.length) return { text: dshdT('balanceUnavailable'), dot: 'warn', kind: 'unavailable' };
  const first = b.balances[0];
  const cur = dshdCurrency(first.currency);
  const curText = cur.length === 1 ? cur : cur + ' ';
  const low = chipLowLevel(first);
  return {
    text: curText + dshdBalanceValue(first.total_balance),
    dot: low === 'critical' ? 'err' : low === 'warning' ? 'warn' : b.stale ? 'warn' : b.is_available ? 'ok' : 'warn',
    kind: 'ok',
    low,
  };
}

/** 标题栏常驻版本号（DSHBox x.y.z · dsh a.b.c）：dsh 未安装时整段隐藏；
    悬停 title 只放**补充信息**（Node/npm/端口/外部服务）——已可见的版本号
    不重复进 tooltip。缓存 payload 供语言切换时重译 title。 */
let lastVersionsPayload = null;
function renderVersions(payload) {
  const el = $('brand-ver');
  if (!el) return;
  // 迟滞：更新/安装事务中途换目录，package.json 可能瞬间读不到——非终态
  // （ready/error/cancelled）下保持上次已知版本，避免标题栏版本号在
  // 隐藏/淡入间抖动（用户报告的局部闪烁）；终态如实清空
  if (payload && !payload.dsh_version && lastVersionsPayload && lastVersionsPayload.dsh_version
    && !['ready', 'error', 'cancelled'].includes(payload.phase)) {
    payload = { ...payload, dsh_version: lastVersionsPayload.dsh_version };
  }
  lastVersionsPayload = payload;
  const parts = [];
  if (payload && payload.app_version) parts.push(payload.app_version);
  if (payload && payload.dsh_version) parts.push('dsh ' + payload.dsh_version);
  const wasHidden = el.hidden;
  el.hidden = parts.length === 0;
  // 首启编排：只在 隐藏→显示 跃迁时播淡入；周期刷新不重播
  if (wasHidden && !el.hidden) el.classList.add('ver-in');
  if (el.hidden) el.classList.remove('ver-in');
  el.textContent = parts.join(' · ');
  // 宽度缓存：分级恢复判定「放回来之后是否仍舒适」用（display:none 时量不到）
  if (!el.hidden) versionWidth = el.offsetWidth || versionWidth;
  const tip = [];
  if (payload && payload.node_version) tip.push('Node ' + payload.node_version);
  if (payload && payload.npm_version) tip.push('npm ' + payload.npm_version);
  if (payload && payload.port) tip.push(dshdT('port', { port: payload.port }));
  const external = payload && (payload.service_mode === 'external'
    || payload.service_mode === 'external-disconnected');
  if (external) tip.push(dshdT('externalService'));
  el.title = tip.join(' · ');
}

function renderBalance() {
  const chip = $('balance-chip');
  // 首个余额结果到达前整体隐藏：loading 期整窗已自解释；结果到达即显示
  if (hideBalance || !lastBalance) {
    chip.style.display = 'none';
    chip.classList.remove('chip-in');
    return;
  }
  const wasHidden = chip.style.display === 'none';
  chip.style.display = '';
  // 首启编排：网络晚到的「蹦出」改淡入；周期刷新不重播
  if (wasHidden) chip.classList.add('chip-in');
  if (serviceMode === 'external' || serviceMode === 'external-disconnected') {
    chip.disabled = true;
    chip.classList.remove('low-warning', 'low-critical');
    chip.innerHTML = WALLET_ICON + '<span id="balance-text">--</span>';
    chip.title = dshdT('balanceExternalHint');
    chip.setAttribute('aria-label', dshdT('balanceExternalHint'));
    return;
  }
  chip.disabled = false;
  const state = balanceChipState();
  const dotClass = state.dot === 'ok' ? 'dot' : 'dot ' + state.dot;
  chip.classList.toggle('low-warning', state.low === 'warning');
  chip.classList.toggle('low-critical', state.low === 'critical');
  chip.innerHTML =
    WALLET_ICON +
    '<span class="' + dotClass + '" aria-hidden="true"></span>' +
    '<span id="balance-text">' + esc(state.text) + '</span>';
  const stale = !!(lastBalance && lastBalance.stale);
  const depleted = state.kind === 'ok' && state.low === 'none' && !stale
    && !!lastBalance && !lastBalance.is_available;
  let hints;
  if (state.kind === 'no_key' || state.kind === 'invalid_key') {
    hints = [dshdT(state.kind === 'no_key' ? 'balanceNoKeyHint' : 'balanceInvalidKeyHint')];
  } else {
    hints = [];
    if (state.kind === 'unavailable') hints.push(dshdT('balanceUnavailable'));
    if (state.low === 'critical') hints.push(dshdT('usageWarnCritical'));
    else if (state.low === 'warning') hints.push(dshdT('usageWarnLow'));
    if (stale) hints.push(dshdT('staleBalance'));
    if (depleted) hints.push(dshdT('balanceDepleted'));
    hints.push(dshdT('balanceChipHint'));
  }
  chip.title = hints.join('\n');
  const credentialIssue = state.kind === 'no_key' || state.kind === 'invalid_key';
  const actionHint = state.kind === 'invalid_key' ? dshdT('balanceInvalidKeyHint') : dshdT('balanceNoKeyHint');
  const ariaLead = state.kind === 'unavailable' && state.text === '--'
    ? dshdT('balanceUnavailable') : state.text;
  const ariaStatus = credentialIssue ? [] : hints.slice(0, -1).filter((line) => line !== ariaLead);
  chip.setAttribute('aria-label',
    [ariaLead].concat(ariaStatus, [credentialIssue ? actionHint : dshdT('balanceDetailsAria')]).join(' — '));
}

// 数值文本与 12px 图标/状态点的垂直光学补偿：数字无下降部，其墨迹中心
// 相对行盒中心的偏移只取决于字体度量且逐平台不同（Windows Segoe UI
// ≈0.7px、macOS SF Pro 近似 0）——用 canvas 实测当前字体栈渲染 '0' 的
// 度量来计算补偿，量化 0.1px；不足 0.2px 视为已对齐不引入亚像素偏移；
// 度量不可用则不设变量，CSS 回退 0px（自移除前的状态栏实现移植）。
function applyTextOpticalShift() {
  try {
    const ctx = document.createElement('canvas').getContext('2d');
    if (!ctx) return;
    ctx.font = '500 12px ' + getComputedStyle(document.body).fontFamily;
    if (ctx.font.indexOf('12px') === -1) return;
    const m = ctx.measureText('0');
    const metrics = [m.fontBoundingBoxAscent, m.fontBoundingBoxDescent, m.actualBoundingBoxAscent];
    if (!metrics.every(Number.isFinite)) return;
    const offset = Math.max(-1, Math.min(1, (metrics[0] - metrics[1] - metrics[2]) / 2));
    const shift = Math.round(offset * 10) / 10;
    if (Math.abs(shift) >= 0.2) {
      document.documentElement.style.setProperty('--dshd-text-opt-shift', (-shift).toFixed(1) + 'px');
    }
  } catch (e) { /* 度量失败维持 0 回退 */ }
}

function onBalance(payload) {
  // stale 保留：刷新失败但已有成功数据时保留上次金额（标记过期）
  if (payload && !payload.ok && !payload.error_kind && lastBalance && lastBalance.ok) {
    lastBalance = Object.assign({}, lastBalance, { stale: true });
    renderBalance();
    return;
  }
  lastBalance = payload;
  renderBalance();
}

async function init() {
  dshdApplyI18n();
  applyTextOpticalShift();
  initPlatform();
  bindMainMenu();
  bindWindowControls();

  let maxCheckTimer = null;
  window.addEventListener('resize', () => {
    clearTimeout(maxCheckTimer);
    maxCheckTimer = setTimeout(refreshMaxState, 150);
  });
  window.addEventListener('blur', () => {
    // 菜单动作可能立即打开弹窗并令标题栏失焦；此时仍保留完整按压反馈。
    if (mainMenuOpen && !mainMenuSelectionPending) setMainMenuOpen(false);
  });
  document.addEventListener('keydown', (event) => {
    if (event.key === 'Escape' && mainMenuOpen) {
      event.preventDefault();
      setMainMenuOpen(false);
      $('btn-menu').focus();
    }
  });
  window.addEventListener('dshd-language-changed', () => {
    applyMaxState($('btn-max').classList.contains('maximized'));
    applyCloseBehavior(closeBehavior);
    refreshMainMenu();
    renderBalance();
  });
  dshdListen('settings-changed', (event) => {
    applyCloseBehavior(event.payload && event.payload.close_behavior);
    hideBalance = !!(event.payload && event.payload.hide_balance);
    renderBalance();
  }).catch(() => {});

  // 渲染自愈脉冲（Rust 侧周期/获焦时直呼）：WebView2 合成层失效会导致
  // 标题栏间歇空白（DOM 正常仅画面空白），强制创建再销毁合成层恢复渲染
  window.__dshdRepaint = () => {
    const tb = document.querySelector('.titlebar');
    if (!tb) return;
    tb.style.transform = 'translateZ(0)';
    requestAnimationFrame(() => {
      requestAnimationFrame(() => { tb.style.transform = ''; });
    });
  };
  $('balance-chip').addEventListener('click', () => {
    invoke('app_dialog_open_usage').catch((e) => console.warn('titlebar: 打开用量弹窗失败', e));
  });
  dshdListen('balance-updated', (e) => onBalance(e.payload)).catch(() => {});
  dshdListen('dsh-status', (e) => {
    const payload = e.payload || {};
    const previousMode = serviceMode;
    serviceMode = payload.service_mode || 'none';
    renderVersions(payload);
    renderBalance();
    const ready = payload.phase === 'ready' && serviceMode === 'managed';
    if (ready && previousMode !== 'managed') {
      invoke('api_balance').then(onBalance).catch(() => {});
    }
  }).catch(() => {});
  refreshMaxState();
  invoke('settings_get').then((settings) => {
    applyCloseBehavior(settings && settings.close_behavior);
    hideBalance = !!(settings && settings.hide_balance);
    renderBalance();
  }).catch(() => {});
  invoke('get_status').then((payload) => {
    serviceMode = (payload && payload.service_mode) || 'none';
    renderVersions(payload);
    renderBalance();
    if (serviceMode !== 'external' && serviceMode !== 'external-disconnected') {
      invoke('api_balance').then(onBalance).catch(() => {});
    }
  }).catch(() => {});
  // 语言切换：命令式设置的 tooltip（版本信息/余额分层提示/最大化按钮）
  // 不走 data-i18n-title 通道，必须在此重译，否则残留旧语言直到下次事件
  window.addEventListener('dshd-language-changed', () => {
    if (lastVersionsPayload) renderVersions(lastVersionsPayload);
    renderBalance();
    refreshMaxState();
  });
  // 窗口焦点状态由 Rust 侧广播（WebView2 子窗口的 window focus/blur
  // 与主窗口焦点不同步），挂载全局函数供 Rust eval 直呼
  window.__dshdSetWindowActive = (active) => {
    // 样式切换走 common.js 的去抖实现（启动期焦点往返不渲染）；悬停
    // 冻结保持原时序
    window.__dshdApplyWindowFocus(active);
    if (!active) suspendWindowControlHover();
  };
  refreshMainMenu();
  // Win11 贴边浮层：上报最大化按钮矩形（本视口 CSS 像素），Rust 侧叠加
  // webview 偏移后在窗口上创建 HTMAXBUTTON 原生覆盖层——悬停弹系统浮层、
  // 点击走原生最大化/还原。覆盖层挡住 webview 的真实 hover，悬停视觉由
  // Rust 的 dshd-snap-hover 事件镜像 .is-hovered（见下方监听与 CSS）
  const syncSnapOverlay = () => {
    const btn = $('btn-max');
    if (!btn) return;
    const rect = btn.getBoundingClientRect();
    invoke('snap_overlay_update', {
      x: Math.round(rect.left), y: Math.round(rect.top),
      width: Math.round(rect.width), height: Math.round(rect.height),
    }).catch(() => {});
  };
  syncSnapOverlay();
  new ResizeObserver(syncSnapOverlay).observe(document.body);
  dshdListen('dshd-snap-hover', (event) => {
    $('btn-max').classList.toggle('is-hovered', !!(event && event.payload && event.payload.on));
  }).catch(() => {});
  dshdListen('dshd-snap-press', (event) => {
    $('btn-max').classList.toggle('is-pressed', !!(event && event.payload && event.payload.on));
  }).catch(() => {});
  window.addEventListener('pagehide', () => {
    invoke('snap_overlay_detach').catch(() => {});
  });
  // 初始化完成回报：Rust 启动自愈看门狗据此判断本页面是否加载成功
  invoke('titlebar_ready').catch(() => {});
}

init();

// 宽度分级：拖拽区实际宽度是挤压力信号。三级优先级——品牌核心+右侧四钮
// 永不让位；tb-compact-1 余额 chip 让位（覆盖层：释放拖拽区面积与视觉密度，
// 不释放布局宽度，阈值独立带迟滞）；tb-compact-2 版本信息让位（释放布局
// 宽度）。版本恢复用「放回来之后仍舒适」判定（拖拽区宽 − 版本宽 > 恢复线）
// ——简单阈值会被「隐藏释放宽度→立即可恢复」的反馈环拖入无限抖动。
// 放顶层而非 init()：不依赖任何 IPC/await 成功；脚本在 body 末尾执行，
// .drag-space 已就绪。窄窗冷启接受首帧全员闪现后收敛（避免首帧前测量）
(() => {
  const applyCompact = () => {
    const space = document.querySelector('.drag-space');
    if (!space) return;
    const w = space.clientWidth;
    const compacted1 = document.body.classList.contains('tb-compact-1');
    // chip：让位 @32 / 找回 @40（迟滞防边缘闪烁；让位不改布局，无反馈环）
    document.body.classList.toggle('tb-compact-1', w < 32 || (compacted1 && w < 40));
    // 版本：让位 @24；找回需 w − versionWidth > 40（防震荡数学）
    if (w < 24) document.body.classList.add('tb-compact-2');
    else if (w - versionWidth > 40) document.body.classList.remove('tb-compact-2');
  };
  const dragSpace = document.querySelector('.drag-space');
  if (dragSpace && window.ResizeObserver) {
    new ResizeObserver(applyCompact).observe(dragSpace);
  }
  window.addEventListener('resize', applyCompact);
  applyCompact();
})();
