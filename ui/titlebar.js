// 自绘标题栏：品牌区/拖拽区、共用主菜单与窗口控制按钮。
// （余额 chip 已迁移到窗口底部状态栏，见 statusbar.js）
const $ = (id) => document.getElementById(id);
const invoke = (command, args) => window.__TAURI__.core.invoke(command, args);

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
// 底缘硬切（阴影边缘突然截断）。Rust 端再按 36..620 收敛
function syncOverlayHeight() {
  const content = mainMenuOpen && $('main-menu-panel') ? $('main-menu-panel').offsetHeight : 0;
  const height = content > 0 ? 36 + content + 24 : 36;
  invoke('titlebar_expand', { expand: height > 36, height }).catch(() => {});
}

// 收起时推迟收缩 webview：淡出动画（140/160ms）期间立即收缩会把
// 正在淡出的面板硬裁切，裁切边贴着标题栏分隔线，产生“衔接处闪烁”
let overlaySyncTimer = null;

async function refreshMainMenu() {
  try {
    mainMenu.setItems(await invoke('menu_get', { traySurface: false }));
  } catch (error) {
    // 刷新失败时保留上一次的菜单模型，功能不受影响
  }
}

function setMainMenuOpen(open, focusMenu = false) {
  mainMenuOpen = Boolean(open);
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
    // 子菜单展开/收起会改变面板高度，同步 webview 高度。
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
  bindWindowControls();
  document.addEventListener('contextmenu', (event) => event.preventDefault());

  let maxCheckTimer = null;
  window.addEventListener('resize', () => {
    clearTimeout(maxCheckTimer);
    maxCheckTimer = setTimeout(refreshMaxState, 150);
  });
  window.addEventListener('blur', () => {
    if (mainMenuOpen) setMainMenuOpen(false);
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
  refreshMainMenu();
  // 初始化完成回报：Rust 启动自愈看门狗据此判断本页面是否加载成功
  invoke('titlebar_ready').catch(() => {});
}

init();
