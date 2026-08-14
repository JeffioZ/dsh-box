// 自绘标题栏逻辑：
// - 产品名 + 常驻 API 余额（hover 弹出详情浮层）
// - Windows/Linux：自绘窗口按钮（Segoe Fluent Icons 字体字形：最小化/最大化还原/关闭到托盘）
// - macOS：系统红绿灯（左侧留白），隐藏自绘按钮
const $ = (id) => document.getElementById(id);

let lastBalance = null;

const esc = dshdEsc;

function initPlatform() {
  const platform = (navigator.userAgentData && navigator.userAgentData.platform)
    ? navigator.userAgentData.platform
    : navigator.platform;
  const isMac =
    platform.toLowerCase().includes('mac') ||
    navigator.userAgent.includes('Macintosh');
  if (isMac) {
    document.body.classList.add('macos');
  } else {
    $('win-buttons').classList.remove('hidden');
  }
}

function renderPop(data) {
  const pop = $('balance-pop');
  if (!data) {
    pop.innerHTML = '<div class="pop-head">DeepSeek API 余额</div><div class="pop-error">查询中…</div>';
    return;
  }
  if (!data.ok) {
    const kind = data.error_kind;
    const head = kind === 'no_key' ? '未配置 API Key'
      : kind === 'invalid_key' ? 'API Key 无效'
      : '余额查询失败';
    pop.innerHTML = `<div class="pop-head">${head}</div><div class="pop-error">${esc(data.error || '')}</div>`;
    return;
  }
  if (!data.balances || !data.balances.length) {
    pop.innerHTML = '<div class="pop-head">DeepSeek API 余额</div><div class="pop-error">暂无余额信息</div>';
    return;
  }
  const b = data.balances[0];
  const cur = esc(dshdCurrency(b.currency)) + ' ';
  // 明细仅在赠送 > 0 时展示：此时"总余额 = 已充值 + 赠送"的拆分才有信息量；
  // 赠送为 0 时两行与总数重复，不展示
  const fmt = (v) => esc(dshdBalanceValue(v));
  const row = (label, v) =>
    `<div class="pop-row"><span>${label}</span><b title="${fmt(v)}">${cur}${fmt(v)}</b></div>`;
  const granted = parseFloat(b.granted_balance || '0') || 0;
  const rows = granted > 0
    ? `<div class="pop-rows">${row('已充值', b.topped_up_balance)}${row('赠送', b.granted_balance)}</div>`
    : '';
  const upd = data.updated_at
    ? '<span class="pop-upd">更新于 ' +
      new Date(data.updated_at * 1000).toLocaleTimeString('zh-CN', { hour: '2-digit', minute: '2-digit' }) +
      '</span>'
    : '';
  const status = data.is_available
    ? `<div class="pop-status"><span class="dot"></span>账户状态：可用${upd}</div>`
    : `<div class="pop-status"><span class="dot warn"></span>账户状态：不可用${upd}</div>`;
  pop.innerHTML =
    `<div class="pop-head">DeepSeek API 余额</div>` +
    `<div class="pop-total">${cur}${fmt(b.total_balance)}<span class="cur">${esc(b.currency)}</span></div>` +
    rows + status;
}

function renderChip(data) {
  const chip = $('balance-chip');
  const txt = $('balance-text');
  const dot = chip.querySelector('.dot');
  if (!data || !data.ok) {
    chip.classList.remove('hidden');
    const kind = data && data.error_kind;
    if (kind === 'no_key') {
      dot.className = 'dot warn';
      txt.textContent = '未配置 API Key';
    } else if (kind === 'invalid_key') {
      dot.className = 'dot err';
      txt.textContent = 'API Key 无效';
    } else {
      dot.className = 'dot warn';
      txt.textContent = '余额查询失败';
    }
    return;
  }
  if (data.balances && data.balances.length) {
    const b = data.balances[0];
    const cur = dshdCurrency(b.currency);
    chip.classList.remove('hidden');
    dot.className = 'dot' + (data.is_available ? '' : ' warn');
    txt.textContent = cur + (b.currency === 'CNY' ? '' : ' ') + b.total_balance;
  } else {
    chip.classList.remove('hidden');
    dot.className = 'dot err';
    txt.textContent = '暂无余额';
  }
}

function renderBalance(data) {
  lastBalance = data;
  renderChip(data);
  renderPop(data);
}

async function refreshBalance() {
  renderPop(lastBalance); // 保持浮层与当前数据一致
  try {
    renderBalance(await window.__TAURI__.core.invoke('api_balance'));
  } catch (e) {
    renderBalance({ ok: false, error: String(e) });
  }
}

function bind() {
  const wrap = $('balance-wrap');
  // 点击 chip：打开自绘余额弹窗（与托盘“查询 API 余额…”一致）
  wrap.addEventListener('click', (e) => {
    e.stopPropagation();
    window.__TAURI__.core.invoke('app_dialog_open_balance');
  });
  // hover 浮层：延迟 100ms 展开（快速扫过不触发），离开立即收起；
  // 展开/收起时调整标题栏 webview 高度以承载浮层
  let hoverTimer = null;
  wrap.addEventListener('mouseenter', () => {
    clearTimeout(hoverTimer);
    hoverTimer = setTimeout(
      () => window.__TAURI__.core.invoke('titlebar_expand', { expand: true }),
      100,
    );
  });
  wrap.addEventListener('mouseleave', () => {
    clearTimeout(hoverTimer);
    window.__TAURI__.core.invoke('titlebar_expand', { expand: false });
  });
  $('btn-min').addEventListener('click', () =>
    window.__TAURI__.core.invoke('titlebar_minimize'),
  );
  $('btn-max').addEventListener('click', async () => {
    const maxed = await window.__TAURI__.core.invoke('titlebar_toggle_maximize');
    applyMaxState(maxed);
  });
  $('btn-close').addEventListener('click', () =>
    window.__TAURI__.core.invoke('titlebar_close'),
  );
}

function applyMaxState(maxed) {
  $('btn-max').classList.toggle('maximized', maxed);
  $('btn-max').title = maxed ? '还原' : '最大化';
}

async function refreshMaxState() {
  try {
    applyMaxState(await window.__TAURI__.core.invoke('titlebar_is_maximized'));
  } catch (e) {
    /* 后端未就绪时忽略 */
  }
}

async function init() {
  initPlatform();
  bind();
  // 禁用 WebView2 默认右键菜单（“保存图片”等）——标题栏是我们自己的 UI
  document.addEventListener('contextmenu', (e) => e.preventDefault());
  // 拖动标题栏退出最大化/系统调整窗口时同步按钮图标
  let maxCheckTimer = null;
  window.addEventListener('resize', () => {
    clearTimeout(maxCheckTimer);
    maxCheckTimer = setTimeout(refreshMaxState, 150);
  });
  refreshMaxState();
  const { listen } = window.__TAURI__.event;
  await listen('balance-updated', (e) => renderBalance(e.payload));
  refreshBalance();
}

init();
