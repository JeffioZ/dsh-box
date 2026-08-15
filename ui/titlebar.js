// 自绘标题栏：品牌区/拖拽区、API 余额、共用主菜单与窗口控制按钮。
const $ = (id) => document.getElementById(id);
const invoke = (command, args) => window.__TAURI__.core.invoke(command, args);
const esc = dshdEsc;

const MANUAL_REFRESH_COOLDOWN_MS = 1600;
const MIN_REFRESH_SPIN_MS = 900;

let lastBalance = null;
let balanceExpanded = false;
let balanceRefreshing = false;
let refreshCooldownUntil = 0;
let mainMenuOpen = false;
let mainMenu = null;

function initPlatform() {
  const platform = (navigator.userAgentData && navigator.userAgentData.platform)
    ? navigator.userAgentData.platform
    : (navigator.platform || '');
  const isMac = platform.toLowerCase().includes('mac') || navigator.userAgent.includes('Macintosh');
  if (isMac) document.body.classList.add('macos');
  else {
    const isWindows = platform.toLowerCase().includes('win') || navigator.userAgent.includes('Windows');
    document.body.classList.add(isWindows ? 'windows' : 'linux');
    $('win-buttons').classList.remove('hidden');
  }
}

// 浮层高度按内容实测（+36px 标题栏 + 24px 阴影余量）：
// 当前阴影 blur 12px + y 偏移 4px，24px 余量足够；不足会被 webview
// 底缘硬切（阴影边缘突然截断）。Rust 端再按 36..512 收敛
function syncOverlayHeight() {
  let content = 0;
  if (mainMenuOpen) content = $('main-menu-panel') ? $('main-menu-panel').offsetHeight : 0;
  else if (balanceExpanded) content = $('balance-pop') ? $('balance-pop').offsetHeight : 0;
  const height = content > 0 ? 36 + content + 24 : 36;
  invoke('titlebar_expand', { expand: height > 36, height }).catch(() => {});
}

// 收起时推迟收缩 webview：淡出动画（140/160ms）期间立即收缩会把
// 正在淡出的面板硬裁切，裁切边贴着标题栏分隔线，产生“衔接处闪烁”
let overlaySyncTimer = null;

function setBalanceExpanded(expanded) {
  balanceExpanded = Boolean(expanded) && !mainMenuOpen;
  $('balance-wrap').classList.toggle('open', balanceExpanded);
  $('balance-chip').setAttribute('aria-expanded', String(balanceExpanded));
  $('balance-pop').setAttribute('aria-hidden', String(!balanceExpanded));
  $('balance-pop').inert = !balanceExpanded;
  clearTimeout(overlaySyncTimer);
  if (balanceExpanded) syncOverlayHeight();
  else overlaySyncTimer = setTimeout(syncOverlayHeight, 200);
}

function popHeader() {
  return '<div class="pop-head">' +
    '<span class="pop-head-label">' + dshdT('balanceTitle') + '</span>' +
    '<button type="button" class="pop-refresh" id="balance-refresh" title="' + dshdT('refreshBalance') + '" aria-label="' + dshdT('refreshBalance') + '">' +
    '<span class="refresh-glyph" aria-hidden="true"><svg viewBox="0 0 24 24" focusable="false"><path d="M21 12a9 9 0 1 1-2.64-6.36L21 8"></path><path d="M21 3v5h-5"></path></svg></span></button></div>';
}

function updateRefreshButton() {
  const button = $('balance-refresh');
  if (!button) return;
  // 冷却期只静默忽略重复点击，不禁用按钮：禁用态会让人误以为“卡住不可点”
  button.disabled = balanceRefreshing;
  button.classList.toggle('refreshing', balanceRefreshing);
  const label = balanceRefreshing ? dshdT('refreshingBalance') : dshdT('refreshBalance');
  button.title = label;
  button.setAttribute('aria-label', label);
  $('balance-pop').setAttribute('aria-busy', String(balanceRefreshing));
}

function bindRefreshButton() {
  const button = $('balance-refresh');
  if (!button) return;
  button.addEventListener('click', (event) => {
    event.stopPropagation();
    refreshBalance(true);
  });
  updateRefreshButton();
}

function setPopContent(content) {
  $('balance-pop').innerHTML = popHeader() + content;
  bindRefreshButton();
  // 内容变化（如错误文案换行、数据到达）可能改变浮层高度，展开时同步实测高度
  if (balanceExpanded) syncOverlayHeight();
}

function renderPop(data) {
  if (!data) {
    setPopContent('<div class="pop-error">' + dshdT('queryingBalance') + '</div>');
    return;
  }
  if (!data.ok) {
    const kind = data.error_kind;
    const head = kind === 'no_key' ? dshdT('noApiKey')
      : kind === 'invalid_key' ? dshdT('invalidApiKey')
      : dshdT('balanceQueryFailed');
    setPopContent('<div class="pop-error-title">' + head + '</div><div class="pop-error">' + esc(data.error || '') + '</div>');
    return;
  }
  if (!data.balances || !data.balances.length) {
    setPopContent('<div class="pop-error">' + dshdT('noBalanceInfo') + '</div>');
    return;
  }

  const balance = data.balances[0];
  const currency = esc(dshdCurrency(balance.currency)) + ' ';
  const format = (value) => esc(dshdBalanceValue(value));
  const row = (label, value) =>
    `<div class="pop-row"><span>${label}</span><b title="${format(value)}">${currency}${format(value)}</b></div>`;
  // 明细仅在赠送 > 0 时展示：此时"总余额 = 已充值 + 赠送"的拆分才有信息量；
  // 赠送为 0 时两行与总数重复，不展示（用户此前反馈）
  const granted = parseFloat(balance.granted_balance || '0') || 0;
  const rows = granted > 0
    ? `<div class="pop-rows">${row(dshdT('toppedUpBalance'), balance.topped_up_balance)}${row(dshdT('grantedBalance'), balance.granted_balance)}</div>`
    : '';
  const updated = data.updated_at
    ? '<span class="pop-upd">' + dshdT('updatedAt', {
      time: new Date(data.updated_at * 1000).toLocaleTimeString(dshdLocale(), { hour: '2-digit', minute: '2-digit' }),
    }) + '</span>'
    : '';
  const status = data.is_available
    ? `<div class="pop-status"><span class="dot"></span>${dshdT('accountStatusAvailable')}${updated}</div>`
    : `<div class="pop-status"><span class="dot warn"></span>${dshdT('accountStatusUnavailable')}${updated}</div>`;
  setPopContent(
    `<div class="pop-total">${currency}${format(balance.total_balance)}<span class="cur">${esc(balance.currency)}</span></div>` +
    rows + status,
  );
}

function updateChipAccessibility() {
  const value = $('balance-text').textContent;
  $('balance-chip').setAttribute('aria-label', value + ' — ' + dshdT('balanceDetailsAria'));
}

function renderChip(data) {
  const chip = $('balance-chip');
  const text = $('balance-text');
  const dot = chip.querySelector('.dot');
  if (!data || !data.ok) {
    chip.classList.remove('hidden');
    const kind = data && data.error_kind;
    if (kind === 'no_key') {
      dot.className = 'dot warn';
      text.textContent = dshdT('noApiKey');
    } else if (kind === 'invalid_key') {
      dot.className = 'dot err';
      text.textContent = dshdT('invalidApiKey');
    } else {
      dot.className = 'dot warn';
      text.textContent = dshdT('balanceQueryFailed');
    }
    updateChipAccessibility();
    return;
  }
  if (data.balances && data.balances.length) {
    const balance = data.balances[0];
    const currency = dshdCurrency(balance.currency);
    chip.classList.remove('hidden');
    dot.className = 'dot' + (data.is_available ? '' : ' warn');
    text.textContent = currency + (balance.currency === 'CNY' ? '' : ' ') + dshdBalanceValue(balance.total_balance);
  } else {
    chip.classList.remove('hidden');
    dot.className = 'dot err';
    text.textContent = dshdT('noBalance');
  }
  updateChipAccessibility();
}

function renderBalance(data) {
  lastBalance = data;
  renderChip(data);
  renderPop(data);
}

async function refreshBalance(manual = false) {
  if (balanceRefreshing) return;
  if (manual && Date.now() < refreshCooldownUntil) return;

  balanceRefreshing = true;
  const spinStarted = Date.now();
  // 转圈期间只在现有按钮上更新状态，不重建浮层内容：
  // 重建会替换按钮元素，CSS 旋转动画在新元素上从 0 度重来 → 闪烁
  updateRefreshButton();
  let next = null;
  try {
    next = await invoke('api_balance');
  } catch (error) {
    next = { ok: false, error: String(error) };
  } finally {
    // 旋转动画至少转满一圈：本地查询很快（百余毫秒），
    // 若立即停转会呈现“刚动就停”的错觉
    const elapsed = Date.now() - spinStarted;
    if (elapsed < MIN_REFRESH_SPIN_MS) {
      await new Promise((resolve) => setTimeout(resolve, MIN_REFRESH_SPIN_MS - elapsed));
    }
    balanceRefreshing = false;
    // 转圈结束才一次性渲染新数据（含按钮重建，此刻无旋转状态）
    renderBalance(next);
    if (manual) refreshCooldownUntil = Date.now() + MANUAL_REFRESH_COOLDOWN_MS;
    updateRefreshButton();
  }
}

async function refreshMainMenu() {
  try {
    mainMenu.setItems(await invoke('menu_get', { traySurface: false }));
  } catch (error) {
    // 刷新失败时保留上一次的菜单模型，功能不受影响
  }
}

function setMainMenuOpen(open, focusMenu = false) {
  mainMenuOpen = Boolean(open);
  if (mainMenuOpen) setBalanceExpanded(false);
  $('main-menu-wrap').classList.toggle('open', mainMenuOpen);
  $('btn-menu').setAttribute('aria-expanded', String(mainMenuOpen));
  $('main-menu-panel').setAttribute('aria-hidden', String(!mainMenuOpen));
  $('main-menu-panel').inert = !mainMenuOpen;
  if (mainMenuOpen) {
    mainMenu.collapseSubmenus(false);
    // 打开时不立即同步高度：等条目渲染完成后再按真实高度一次性扩展。
    // 先扩展再渲染会造成两次快速 resize（空面板高度→真实高度），
    // 双重重绘正是标题栏文案偶发闪烁的来源。
    // 鼠标打开菜单时不转移焦点（会打断按钮的 :active 态引起闪烁，
    // 原生菜单鼠标点开也不转移）；键盘激活由 focusMenu 在渲染完成后聚焦。
    refreshMainMenu().then(() => {
      if (focusMenu) mainMenu.focusFirst();
      syncOverlayHeight();
    });
  }
  clearTimeout(overlaySyncTimer);
  if (!mainMenuOpen) overlaySyncTimer = setTimeout(syncOverlayHeight, 200);
}

function bindBalance() {
  const wrap = $('balance-wrap');
  const chip = $('balance-chip');
  const pop = $('balance-pop');
  let hoverTimer = null;
  let leaveTimer = null;

  // 指针是否仍在“入口区或浮层区”内：浮层展开需要 IPC 往返，
  // 期间指针可能已经离开入口区（webview 加高前该区域尚未属于本页面），
  // 用几何包含判定代替 mouseleave，避免途经间隙时误收起
  const inSurface = (x, y) => {
    const wr = wrap.getBoundingClientRect();
    if (x >= wr.left && x <= wr.right && y >= wr.top && y <= wr.bottom) return true;
    if (!balanceExpanded) return false;
    const pr = pop.getBoundingClientRect();
    return x >= pr.left && x <= pr.right && y >= pr.top && y <= pr.bottom;
  };

  chip.addEventListener('click', (event) => {
    event.stopPropagation();
    setBalanceExpanded(false);
    invoke('app_dialog_open_balance').catch(() => {});
  });
  wrap.addEventListener('mouseenter', () => {
    clearTimeout(hoverTimer);
    hoverTimer = setTimeout(() => setBalanceExpanded(true), 100);
  });
  wrap.addEventListener('mouseleave', () => {
    // 离开芯片：取消未到时的展开；已展开时补收起定时器——
    // 无后续 mousemove 也能收起（mousemove 路径会正常接管/取消它）
    clearTimeout(hoverTimer);
    if (!balanceExpanded || leaveTimer) return;
    leaveTimer = setTimeout(() => { leaveTimer = null; setBalanceExpanded(false); }, 240);
  });
  document.addEventListener('mousemove', (event) => {
    if (!balanceExpanded) return;
    if (inSurface(event.clientX, event.clientY)) {
      if (leaveTimer) { clearTimeout(leaveTimer); leaveTimer = null; }
    } else if (!leaveTimer) {
      // 离开两区持续 240ms 才收起：给快速扫过的移动与浮层淡入留余量
      leaveTimer = setTimeout(() => { leaveTimer = null; setBalanceExpanded(false); }, 240);
    }
  });
  chip.addEventListener('focus', () => {
    clearTimeout(hoverTimer);
    setBalanceExpanded(true);
  });
  chip.addEventListener('blur', () => {
    clearTimeout(hoverTimer);
    setTimeout(() => {
      if (!wrap.matches(':hover') && !wrap.matches(':focus-within')) setBalanceExpanded(false);
    }, 0);
  });
}

function bindMainMenu() {
  mainMenu = dshdCreateMenu($('main-menu-list'), {
    onChoose: async (id) => {
      try {
        await invoke('menu_choose', { id });
      } finally {
        setMainMenuOpen(false);
      }
    },
    onEscape: () => {
      setMainMenuOpen(false);
      $('btn-menu').focus();
    },
    // 语言子菜单展开/收起会改变面板高度，同步 webview 高度。
    // menu.js 先发通知后改 DOM，这里等一帧让 DOM 就绪再实测
    onSubmenuChange: () => requestAnimationFrame(syncOverlayHeight),
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
    if (balanceExpanded && !$('balance-wrap').contains(event.target)) setBalanceExpanded(false);
  });
}

function bindWindowControls() {
  $('btn-min').addEventListener('click', () => invoke('titlebar_minimize').catch(() => {}));
  $('btn-max').addEventListener('click', async () => {
    try {
      applyMaxState(await invoke('titlebar_toggle_maximize'));
    } catch (error) {
      // 忽略偶发的 IPC 失败：后续 resize/状态轮询会重试
    }
  });
  $('btn-close').addEventListener('click', () => invoke('titlebar_close').catch(() => {}));
}

function applyMaxState(maximized) {
  $('btn-max').classList.toggle('maximized', maximized);
  $('btn-max').title = dshdT(maximized ? 'restore' : 'maximize');
  $('btn-max').setAttribute('aria-label', dshdT(maximized ? 'restoreWindow' : 'maximize'));
}

async function refreshMaxState() {
  try {
    applyMaxState(await invoke('titlebar_is_maximized'));
  } catch (error) {
    // 后端就绪后，下一次 resize 刷新会重试
  }
}

async function init() {
  dshdApplyI18n();
  initPlatform();
  bindMainMenu();
  bindBalance();
  bindWindowControls();
  document.addEventListener('contextmenu', (event) => event.preventDefault());

  let maxCheckTimer = null;
  window.addEventListener('resize', () => {
    clearTimeout(maxCheckTimer);
    maxCheckTimer = setTimeout(refreshMaxState, 150);
  });
  window.addEventListener('blur', () => {
    if (mainMenuOpen) setMainMenuOpen(false);
    if (balanceExpanded) setBalanceExpanded(false);
  });
  document.addEventListener('keydown', (event) => {
    if (event.key === 'Escape' && balanceExpanded) {
      event.preventDefault();
      setBalanceExpanded(false);
    }
  });
  window.addEventListener('dshd-language-changed', () => {
    if (lastBalance) renderBalance(lastBalance);
    else renderPop(null);
    applyMaxState($('btn-max').classList.contains('maximized'));
    refreshMainMenu();
  });

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
  refreshMaxState();
  // 窗口焦点状态由 Rust 侧广播（WebView2 子窗口的 window focus/blur
  // 与主窗口焦点不同步），挂载全局函数供 Rust eval 直呼
  window.__dshdSetWindowActive = (active) => {
    document.body.classList.toggle('window-inactive', !active);
  };
  const { listen } = window.__TAURI__.event;
  await listen('balance-updated', (event) => renderBalance(event.payload));
  refreshMainMenu();
  refreshBalance();
  // 初始化完成回报：Rust 启动自愈看门狗据此判断本页面是否加载成功
  invoke('titlebar_ready').catch(() => {});
}

init();
