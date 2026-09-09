// 底部状态栏：会话统计（session-stats-updated）+ 余额 chip（balance-updated）。
// 悬停说明使用系统默认 title；WebView 始终固定高度，避免透明层扩缩重绘残影。

const invoke = (command, args) => window.__TAURI__.core.invoke(command, args);
const listen = (event, handler) => window.__TAURI__.event.listen(event, handler);
const $ = (id) => document.getElementById(id);

// 统计组图标（装饰性：旁边有可见文本，aria-hidden；单族 outline stroke）
// 12px 渲染下控制细节密度：speeds/cache 保留 lucide 形态的简化版
// （闪电用直线剪影、圆柱去中间弧），其余对齐 lucide 官方路径
const GROUP_ICONS = {
  counts: '<svg viewBox="0 0 24 24" aria-hidden="true"><path d="M22 17a2 2 0 0 1-2 2H6.828a2 2 0 0 0-1.414.586l-2.202 2.202A.71.71 0 0 1 2 21.286V5a2 2 0 0 1 2-2h16a2 2 0 0 1 2 2z"></path></svg>',
  durations: dshdIcon('clock', 'aria-hidden="true"'),
  speeds: '<svg viewBox="0 0 24 24" aria-hidden="true"><path d="M13 2 4 14h6l-1 8 9-12h-6z"></path></svg>',
  cache: '<svg viewBox="0 0 24 24" aria-hidden="true"><path d="M5 6c0-1.7 3.1-3 7-3s7 1.3 7 3-3.1 3-7 3-7-1.3-7-3z"></path><path d="M5 6v12c0 1.7 3.1 3 7 3s7-1.3 7-3V6"></path></svg>',
  tokens: '<svg viewBox="0 0 24 24" aria-hidden="true"><path d="m16 3 4 4-4 4"></path><path d="M20 7H4"></path><path d="m8 21-4-4 4-4"></path><path d="M4 17h16"></path></svg>',
};
const WALLET_ICON = '<svg class="c-ic" viewBox="0 0 24 24" aria-hidden="true"><path d="M19 7V4a1 1 0 0 0-1-1H5a2 2 0 0 0 0 4h15a1 1 0 0 1 1 1v4h-3a2 2 0 0 0 0 4h3a1 1 0 0 0 1-1v-2a1 1 0 0 0-1-1"></path><path d="M3 5v14a2 2 0 0 0 2 2h15a1 1 0 0 0 1-1v-4"></path></svg>';

let statsGroups = [];
let avgTps = null; // stats 载荷的平均解码速率（tok/s）
let liveTps = null; // 实时速率（流式期间有值，空闲为 null）
let detailsMap = {}; // key → 额外明细行（状态栏未显示的补充数据）
let lastBalance = null;
let currentSettings = null; // 设置状态（hide_balance 控制余额 chip 显隐）
let serviceMode = 'none';
let serviceReady = false;

const esc = dshdEsc;

// ---------- 会话统计 ----------

function renderStats() {
  const el = $('stats');
  const managedReady = serviceReady && serviceMode === 'managed';
  if (!managedReady || !statsGroups.length) {
    el.innerHTML = '';
    el.dataset.truncated = '0';
    el.title = managedReady ? '' : dshdT('navRequiresReady');
    el.setAttribute('aria-label', dshdT('statsRegion'));
    el.disabled = true;
    return;
  }
  el.disabled = false;
  // tok/s 实时优先、平均回退（speeds 组的 Rust 文本只含首 token）
  const tps = liveTps != null ? liveTps : avgTps;
  const tpsText = tps != null ? formatTps(tps) : '';
  el.innerHTML = statsGroups.map((g, i) => {
    const text = g.key === 'speeds' && tpsText
      ? esc(g.text) + ' · ' + tpsText
      : esc(g.text);
    return (
      '<span class="g' + (g.key === 'cache' ? ' cache' : '') + '" data-key="' + esc(g.key) + '">' +
      (GROUP_ICONS[g.key] ? '<span class="g-ic">' + GROUP_ICONS[g.key] + '</span>' : '') +
      '<span class="g-t">' + text + '</span>' +
      '</span>' + (i < statsGroups.length - 1 ? '<span class="vsep" aria-hidden="true"></span>' : '')
    );
  }).join('');
  fitGroups();
}

function formatTps(v) {
  return (Math.round(v * 10) / 10) + ' tok/s';
}

// 窄窗口降级：从尾到首隐藏次要组（首组保底），隐藏组的完整信息经
// 系统默认 tooltip 可获。
function fitGroups() {
  const el = $('stats');
  const groups = [...el.querySelectorAll('.g')];
  if (!groups.length) {
    el.dataset.truncated = '0';
    return;
  }
  // 先恢复全部再按需隐藏：窗口变宽后信息逐步回来
  groups.forEach((g) => { g.style.display = ''; });
  el.querySelectorAll('.vsep').forEach((s) => { s.style.display = ''; });
  let hidden = 0;
  for (let i = groups.length - 1; i >= 1; i--) {
    if (el.scrollWidth <= el.clientWidth) break;
    groups[i].style.display = 'none';
    const sepBefore = groups[i].previousElementSibling;
    if (sepBefore && sepBefore.classList.contains('vsep')) sepBefore.style.display = 'none';
    hidden += 1;
  }
  // 截断 = 有隐藏组 或 首组仍溢出（首组保底显示，溢出部分被裁切）；
  // tooltip 始终提供组含义；发生截断时改为提供全部组的完整信息。
  const truncated = hidden > 0 || el.scrollWidth > el.clientWidth + 1;
  el.dataset.truncated = truncated ? '1' : '0';
  applyNativeTips();
}

function onStats(payload) {
  // show_stats=false：设置里已关闭隐藏——dsh 页面自己显示统计，
  // 状态栏统计区互斥隐藏（余额 chip 保留）
  const show = !payload || payload.show_stats !== false;
  statsGroups = show && payload && Array.isArray(payload.groups) ? payload.groups : [];
  avgTps = payload && typeof payload.avg_tps === 'number' ? payload.avg_tps : null;
  detailsMap = {};
  if (payload && Array.isArray(payload.details)) {
    payload.details.forEach((d) => {
      if (d && d.key && Array.isArray(d.lines)) detailsMap[d.key] = d.lines;
    });
  }
  renderStats();
  updateEdgeSeparator();
}

function onLiveRate(payload) {
  // 实时速率：流式期间有值（替换平均显示），空闲 null（回落平均）
  liveTps = payload && typeof payload.tps === 'number' ? payload.tps : null;
  // 仅当 speeds 组存在且显示值变化时重渲染（2s 周期，全量重建开销可忽略）
  if (statsGroups.some((g) => g.key === 'speeds')) renderStats();
}

// ---------- 余额 chip（点击入口 + 系统默认悬停提示） ----------

// 余额预警：remaining/total ≤30% warning、≤10% critical；total 未知不加色
// （字段由后端扩展，缺失时比率不可算——不渲染假预警，IPC 契约不变）
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
  // 币种前缀：符号类（¥）紧贴金额；ISO 代码类（USD）补空格避免粘连
  const curText = cur.length === 1 ? cur : cur + ' ';
  const low = chipLowLevel(first);
  // chip 只显示金额（币种符号足够，currency 代码与拆分明细留给详情弹窗）；
  // stale：保留上次金额但状态点转 warn，悬停提示刷新失败
  return {
    text: curText + dshdBalanceValue(first.total_balance),
    dot: low === 'critical' ? 'err' : low === 'warning' ? 'warn' : b.stale ? 'warn' : b.is_available ? 'ok' : 'warn',
    kind: 'ok',
    low,
  };
}

function renderBalance() {
  const chip = $('balance-chip');
  const hide = currentSettings && currentSettings.hide_balance;
  if (hide) {
    chip.style.display = 'none';
    updateEdgeSeparator();
    return;
  }
  chip.style.display = '';
  updateEdgeSeparator();
  if (serviceMode === 'external' || serviceMode === 'external-disconnected') {
    chip.disabled = true;
    chip.classList.remove('low-warning', 'low-critical');
    chip.innerHTML = WALLET_ICON + '<span id="balance-text">--</span>';
    chip.dataset.credentialIssue = '';
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
  // 耗尽行仅在预警行缺失时补位：remaining≤0 且 total 可算时已由「余量告急」覆盖
  const depleted = state.kind === 'ok' && state.low === 'none' && !stale
    && !!lastBalance && !lastBalance.is_available;
  let hints;
  if (state.kind === 'no_key' || state.kind === 'invalid_key') {
    // 未配置/无效 Key：引导点击去设置页（不点 Details 语义）
    hints = [dshdT(state.kind === 'no_key' ? 'balanceNoKeyHint' : 'balanceInvalidKeyHint')];
  } else {
    // 状态行在前、操作提示在后：悬停先看到"怎么了"再看到"点哪去"；
    // 状态点语义不只靠颜色：悬停文字解释当前状态（不可用/阈值/过期/耗尽）
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
  chip.dataset.credentialIssue = credentialIssue ? '1' : '';
  const actionHint = state.kind === 'invalid_key' ? dshdT('balanceInvalidKeyHint') : dshdT('balanceNoKeyHint');
  // 读屏信息与视觉对齐：状态行插中间；'--' 态用状态词替代无意义的杠杠，
  // 并滤掉与首段重复的状态行避免同一词读两遍
  const ariaLead = state.kind === 'unavailable' && state.text === '--'
    ? dshdT('balanceUnavailable') : state.text;
  const ariaStatus = credentialIssue ? [] : hints.slice(0, -1).filter((line) => line !== ariaLead);
  chip.setAttribute('aria-label',
    [ariaLead].concat(ariaStatus, [credentialIssue ? actionHint : dshdT('balanceDetailsAria')]).join(' — '));
}

function updateEdgeSeparator() {
  const sep = $('edge-sep');
  if (!sep) return;
  const balanceVisible = !(currentSettings && currentSettings.hide_balance);
  const statsVisible = serviceReady && serviceMode === 'managed' && statsGroups.length > 0;
  sep.style.display = statsVisible && balanceVisible ? '' : 'none';
}

function onBalance(payload) {
  // stale 保留：刷新失败但已有成功数据时保留上次金额（标记过期），
  // 而非丢弃旧值显示 --（借鉴 dsh-api-balance 的 stale-while-revalidate）
  if (payload && !payload.ok && !payload.error_kind && lastBalance && lastBalance.ok) {
    lastBalance = Object.assign({}, lastBalance, { stale: true });
    renderBalance();
    return;
  }
  lastBalance = payload;
  renderBalance();
}

// ---------- 系统默认悬停提示 ----------

// 组含义文案 key（悬停各分组时作为 tooltip 标题）
const GROUP_HINTS = {
  counts: 'statsCountsHint',
  durations: 'statsDurationsHint',
  speeds: 'statsSpeedsHint',
  cache: 'statsCacheHint',
  tokens: 'statsTokensHint',
};
function groupTipText(key) {
  const lines = [dshdT(GROUP_HINTS[key] || 'statsCountsHint')];
  const extra = (detailsMap[key] || []).map((t) => String(t));
  if (key === 'speeds') {
    const tps = liveTps != null ? liveTps : avgTps;
    if (tps != null) extra.push(formatTps(tps));
  }
  lines.push(...extra);
  return lines.join('\n');
}

function applyNativeTips() {
  const el = $('stats');
  const fullTip = statsGroups.map((group) => (
    group.text + '\n' + groupTipText(group.key)
  )).join('\n\n');
  const truncated = el.dataset.truncated === '1';
  el.querySelectorAll('.g').forEach((group) => {
    group.title = truncated ? fullTip : groupTipText(group.dataset.key);
  });
  el.title = truncated ? fullTip : '';
  el.setAttribute('aria-label', statsGroups.map((group) => group.text).join(' · ') || dshdT('statsRegion'));
}

// ---------- 其他 ----------

// 数值文本与 12px 图标/状态点的垂直光学补偿：数字无下降部，其墨迹中心
// 相对行盒中心的偏移 = (布局 ascent − 布局 descent − 数字墨迹高) / 2，
// 只取决于字体度量且逐平台不同（Windows Segoe UI ≈0.7px、macOS SF Pro
// 近似 0）——用 canvas 实测当前字体栈渲染 '0' 的度量来计算补偿，避免
// 硬编码单一平台常数在其余平台反向过矫；度量不可用则不设变量，
// CSS 回退 0px 维持原状（本次改动前就是 0 偏移）。
function applyTextOpticalShift() {
  try {
    const ctx = document.createElement('canvas').getContext('2d');
    if (!ctx) return;
    ctx.font = '500 12px ' + getComputedStyle(document.body).fontFamily;
    // 赋值被拒时 canvas 静默回落 10px 默认字体，度量会基于错误字体
    if (ctx.font.indexOf('12px') === -1) return;
    const m = ctx.measureText('0');
    const metrics = [m.fontBoundingBoxAscent, m.fontBoundingBoxDescent, m.actualBoundingBoxAscent];
    if (!metrics.every(Number.isFinite)) return;
    // 行盒中心到墨迹中心的偏移，限幅 ±1px 防异常字体度量
    const offset = Math.max(-1, Math.min(1, (metrics[0] - metrics[1] - metrics[2]) / 2));
    // 量化到 0.1px；不足 0.2px 视为已对齐，不引入无谓的亚像素偏移
    const shift = Math.round(offset * 10) / 10;
    if (Math.abs(shift) >= 0.2) {
      document.documentElement.style.setProperty('--dshd-text-opt-shift', (-shift).toFixed(1) + 'px');
    }
  } catch (e) { /* 度量失败维持 0 回退 */ }
}

// 语言热切换：静态文案经 common.js 的 dshdSetLanguage 重渲染；
// stats 文案由 Rust 侧下一次轮询刷新（≤5s），余额 chip 即时重渲染
function onLanguageChanged() {
  renderStats();
  renderBalance();
}

// WebView2 合成层重绘脉冲（Rust repaint_pulse 调用）
window.__dshdRepaint = () => {};

// 失焦样式跟随主窗口焦点（Rust Focused 事件广播）
window.__dshdSetWindowActive = (focused) => {
  // 样式切换走 common.js 的去抖实现（启动期焦点往返不渲染）
  window.__dshdApplyWindowFocus(focused);
};

function init() {
  const statsEl = $('stats');
  const chip = $('balance-chip');
  chip.addEventListener('click', () => {
    // 未配置 Key 时点击直达设置页（引导配置）；其余状态打开用量与余额
    const cmd = chip.dataset.credentialIssue ? 'app_dialog_open_settings' : 'app_dialog_open_usage';
    invoke(cmd).catch(() => {});
  });
  statsEl.addEventListener('click', () => {
    invoke('app_dialog_open_usage').catch(() => {});
  });
  // 宽度变化重排分组并重判截断（fitGroups 内未截断时自动收起 tooltip）
  if (typeof ResizeObserver !== 'undefined') {
    new ResizeObserver(() => fitGroups()).observe(statsEl);
  }
  window.addEventListener('dshd-language-changed', onLanguageChanged);
  dshdListen('session-stats-updated', (e) => onStats(e.payload));
  dshdListen('live-rate-updated', (e) => onLiveRate(e.payload));
  dshdListen('balance-updated', (e) => onBalance(e.payload));
  dshdListen('dsh-status', (e) => {
    const payload = e.payload || {};
    const previousMode = serviceMode;
    const previousReady = serviceReady;
    serviceMode = payload.service_mode || 'none';
    serviceReady = payload.phase === 'ready'
      && (serviceMode === 'managed' || serviceMode === 'external');
    if (serviceMode !== 'managed') statsGroups = [];
    renderStats();
    renderBalance();
    if (serviceMode === 'managed' && serviceReady && (previousMode !== serviceMode || !previousReady)) {
      invoke('api_balance').then(onBalance).catch(() => {});
    }
  }).catch(() => {});
  dshdListen('settings-changed', (e) => {
    currentSettings = e.payload || null;
    renderBalance();
  }).catch(() => {});
  // 初始拉取设置状态（hide_balance 决定余额 chip 是否显示）
  invoke('settings_get').then((s) => {
    currentSettings = s;
    renderBalance();
  }).catch(() => {});
  dshdApplyI18n();
  applyTextOpticalShift();
  renderStats();
  renderBalance();
  // 初始化完成回报：Rust 侧加载自愈看门狗据此判断页面是否就绪
  // （与标题栏的 titlebar_ready 同款握手）
  invoke('statusbar_ready').catch(() => {});
  // 初始状态决定外部服务隔离；本地模式再主动拉一次余额。
  invoke('get_status').then((payload) => {
    serviceMode = payload.service_mode || 'none';
    serviceReady = payload.phase === 'ready'
      && (serviceMode === 'managed' || serviceMode === 'external');
    renderStats();
    renderBalance();
    if (serviceMode !== 'external' && serviceMode !== 'external-disconnected') {
      invoke('api_balance').then(onBalance).catch(() => {});
    }
  }).catch(() => {});
}

document.addEventListener('DOMContentLoaded', init);
