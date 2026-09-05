// common.js 已把 window.__TAURI__ 兜底为「响亮抛错」的 polyfill，此处直接用
const invoke = window.__TAURI__.core.invoke;
const listen = window.__TAURI__.event.listen;

dshdApplyI18n();

let openKind = '';
let currentOpen = null;
let lastCheckResult = null;
// 更新源已切换标记：切到检查更新页时强制重查（不用关弹窗重开）
let dshChannelChanged = false;
const esc = dshdEsc;
const $ = (id) => document.getElementById(id);
const cur = (c) => esc(dshdCurrency(c)) + ' ';

// 仅为实际截断的元素设置 title（显示完整无额外信息则不出现悬停提示）；
// data-tip-extra 提供"额外信息"提示（无论是否截断都显示，截断时
// 截断的文本优先）。带 data-trunc-tip 的元素在每次渲染后调用
function applyTruncationTips(root) {
  const scope = root || document;
  scope.querySelectorAll('[data-trunc-tip]').forEach((el) => {
    const truncated = el.scrollWidth > el.clientWidth + 1;
    const extra = el.dataset.tipExtra || '';
    el.title = truncated ? el.textContent.trim() : extra;
  });
}

// —— 用量与余额（统一页：历史用量聚合 + 供应商账户/订阅） ——
let usageSeq = 0;

// 账户区只展示「有意义」的账户：已配凭据 + 有可用适配器的路由。
// 避免把未配置的所有供应商都铺出来（屏效低、信息噪音大）。
function meaningfulAccounts(accounts, subs) {
  const out = [];
  for (const a of (accounts || [])) {
    // 余额快照：仅当有余额可显示或状态非 not-configured 时才展示。
    const hasBalance = a.balance && (a.balance.remaining !== null && a.balance.remaining !== undefined);
    if (a.status === 'not-configured' && !hasBalance) continue;
    out.push({ ...a });
  }
  for (const s of (subs || [])) {
    // 订阅：未配置凭据的不展示。
    if (s.status === 'not-configured') continue;
    out.push({ ...s });
  }
  return out;
}

// 用量导出：保存对话框由后端弹出；进行中禁用触发按钮并反馈状态，防重复触发
async function exportUsage(kind, trigger) {
  trigger.disabled = true;
  const label = trigger.querySelector('.lb');
  const prev = label.textContent;
  const restore = () => {
    label.textContent = prev;
    trigger.disabled = false;
  };
  label.textContent = dshdT('usageExporting');
  try {
    await invoke('usage_export', { format: kind });
    dshdToast(dshdT('usageExportDone'), { kind: 'ok' });
  } catch (e) {
    dshdToast(dshdT('usageExportFailed'), { kind: 'err' });
  } finally {
    restore();
  }
}

// 外点收起的 document 监听只注册一次：renderUsagePage 每次渲染都会重建
// 菜单实例，若随 setup 挂在 document 上会不断累积；当前实例经 dismiss 换新，
// 被替换的旧实例 openState 恒为 false，成为无害的空操作。
let usageExportDismiss = null;
document.addEventListener('pointerdown', (event) => {
  if (!usageExportDismiss) return;
  const target = event.target;
  if (target && target.closest && target.closest('.usage-export-dd')) return;
  usageExportDismiss();
});

// 导出下拉：触发按钮 + 共享菜单组件（与标题栏/托盘菜单同一套视觉与键盘导航）。
// 悬停即出（短延迟防误触，移出后短暂保留以容纳移入菜单）；点击与方向键仍可
// 切换。后续新增导出格式只需在 items 里加一行。
function setupUsageExportMenu() {
  const dd = document.querySelector('.usage-export-dd');
  if (!dd) return;
  const trigger = dd.querySelector('.usage-export-btn');
  const surface = dd.querySelector('.usage-export-menu');
  let openState = false;
  let hoverTimer = 0;
  let leaveTimer = 0;
  const close = (focusTrigger) => {
    if (!openState) return;
    openState = false;
    trigger.setAttribute('aria-expanded', 'false');
    // 退场动效播完再隐藏，避免 90ms 淡出被 display:none 硬切
    menuMotion.close(() => { surface.hidden = true; });
    if (focusTrigger) trigger.focus();
  };
  const menu = dshdCreateMenu(surface, {
    onChoose: (id) => {
      close(true);
      exportUsage(id, trigger);
    },
    onEscape: () => close(true),
  });
  menu.setItems([
    { id: 'csv', label: 'CSV' },
    { id: 'json', label: 'JSON' },
  ]);
  const menuMotion = dshdCreateMenuMotion(surface);
  const open = (focusMenu) => {
    // 导出进行中（触发按钮禁用）不再唤出，避免经菜单项重复触发
    if (trigger.disabled) return;
    openState = true;
    surface.hidden = false;
    trigger.setAttribute('aria-expanded', 'true');
    menuMotion.open('-4px');
    if (focusMenu) menu.focusFirst();
  };
  // 悬停打开不抢焦点（鼠标路径）；点击/方向键打开时进入菜单，便于键盘导航
  dd.addEventListener('mouseenter', () => {
    window.clearTimeout(leaveTimer);
    if (!openState) hoverTimer = window.setTimeout(() => open(false), 120);
  });
  dd.addEventListener('mouseleave', () => {
    window.clearTimeout(hoverTimer);
    if (openState) leaveTimer = window.setTimeout(() => close(false), 220);
  });
  trigger.addEventListener('click', () => {
    if (openState) close(true);
    else open(true);
  });
  trigger.addEventListener('keydown', (event) => {
    if (event.key === 'Escape' && openState) {
      // 菜单开着时按钮上的 Esc 只收菜单：不阻断会冒泡到弹窗窗口级
      // Esc 把整个弹窗关掉（焦点在菜单内时由 surface 上的同名处理负责）
      event.stopPropagation();
      close(true);
      return;
    }
    if (!openState && (event.key === 'ArrowDown' || event.key === 'ArrowUp')) {
      event.preventDefault();
      open(true);
    }
  });
  // 菜单自身的 Esc 只关菜单：menu.js 处理后仍会冒泡，不阻断会触发弹窗
  // 窗口级的全局 Esc 把整个弹窗关掉
  surface.addEventListener('keydown', (event) => {
    if (event.key === 'Escape') event.stopPropagation();
  });
  usageExportDismiss = () => close(false);
}

// 加载占位延迟出现：用量报表读本地缓存，多数情况下近乎即时返回，
// 立即渲染 spinner 会造成加载态闪烁；超时仍未返回才显示（真正慢路径无损失）
let usageLoadTimer = 0;
function showUsageLoadDelayed() {
  clearTimeout(usageLoadTimer);
  usageLoadTimer = window.setTimeout(() => {
    usageLoadTimer = 0;
    const el = $('usage-load');
    if (el) el.hidden = false;
  }, 300);
}
function clearUsageLoadDelay() {
  clearTimeout(usageLoadTimer);
  usageLoadTimer = 0;
}

async function renderUsagePage(keep) {
  const seq = ++usageSeq;
  clearUsageLoadDelay();
  const body = $('body');
  // keep（手动刷新）：已有内容原地保留作加载占位，不重置骨架、不重绑导出
  // 菜单；仅当前一次渲染从未产出数据（骨架/报错态）时才保留慢路径 spinner
  const wrap = body.querySelector('.usage-wrap');
  const summaryFilled = !!(wrap && $('usage-summary') && $('usage-summary').childElementCount);
  if (!wrap) {
    body.innerHTML =
      '<div class="usage-wrap">' +
      '<section class="usage-card" aria-labelledby="usage-summary-heading">' +
      '<div class="usage-h-row">' +
      '<h3 id="usage-summary-heading" class="usage-h">' + dshdT('usageTokenSection') + '</h3>' +
      '<div class="usage-export-dd">' +
      '<button type="button" class="usage-export-btn" aria-haspopup="menu" aria-expanded="false">' +
      '<span class="lb">' + dshdT('usageExport') + '</span>' +
      dshdIcon('chevronDown', 'aria-hidden="true"') +
      '</button>' +
      '<div class="usage-export-menu dshd-menu-surface dshd-menu-motion" role="menu" hidden></div>' +
      '</div>' +
      '</div>' +
      '<div class="usage-summary" id="usage-summary"></div>' +
      '</section>' +
      '<div class="usage-load" id="usage-load" role="status" aria-live="polite" hidden><span class="spin" aria-hidden="true"></span>' + dshdT('usageLoading') + '</div>' +
      '</div>';
    setupUsageExportMenu();
  }
  if (!summaryFilled) showUsageLoadDelayed();
  try {
    // 预测读后台缓存（预警任务每 10 分钟刷新），失败静默为 null 不影响主报表
    const [report, prediction] = await Promise.all([
      invoke('usage_report_get'),
      invoke('usage_prediction_get').catch(() => null),
    ]);
    if (openKind !== 'usage' || seq !== usageSeq) return;
    clearUsageLoadDelay();
    renderUsageReport(report, seq, prediction);
  } catch (e) {
    if (openKind !== 'usage' || seq !== usageSeq) return;
    clearUsageLoadDelay();
    const load = $('usage-load');
    if (load) { load.hidden = false; load.className = 'usage-load err'; load.textContent = dshdT('usageFailed') + ': ' + e; }
    // keep 路径成功渲染过就没有 load 槽位：报错走 toast，不清掉正在展示的数据
    else dshdToast(dshdT('usageFailed') + ': ' + e, { kind: 'err' });
  }
}

function fmtTokens(n) {
  return String(Math.round(n)).replace(/\B(?=(\d{3})+(?!\d))/g, ',');
}

function formatPercent(p) {
  return (p === null || p === undefined) ? '—' : String(p) + '%';
}

// 估算成本（USD，两位小数 + 千分位）。cost_complete 为 false 或金额为 0 时
// 显示「—」（含未定价样本 → fail-closed，不低估）；title 提示原因。
function costText(entry) {
  if (!entry || !entry.cost_complete) {
    return '<span class="usage-cost-unknown" title="' + esc(dshdT('usageCostUnknown')) + '">—</span>';
  }
  const usd = Number(entry.cost_usd) || 0;
  if (usd <= 0) {
    return '<span class="usage-cost-unknown">—</span>';
  }
  const locale = (typeof dshdLocale === 'function' && dshdLocale()) || undefined;
  return '$' + usd.toLocaleString(locale, { minimumFractionDigits: 2, maximumFractionDigits: 2 });
}

// 跨天聚合成本（按模型）：任一天该模型不可信 → 整体「—」
function aggregateModelCostText(days, modelKey) {
  let usd = 0;
  let any = false;
  for (const d of days) {
    for (const m of (d.models || [])) {
      if (m.model !== modelKey) continue;
      any = true;
      if (!m.cost_complete) return costText(null);
      usd += Number(m.cost_usd) || 0;
    }
  }
  return costText(any ? { cost_complete: true, cost_usd: usd } : null);
}

function renderUsageReport(report, seq, prediction) {
  const wrap = document.querySelector('.usage-wrap');
  const summary = $('usage-summary');
  if (!wrap || !summary) return;
  const total = report.total || {};
  const todayInfo = todayEntry(report);
  const todayHit = todayInfo ? todayInfo.cache_hit_rate : null;
  const totalHit = report.total ? report.total.cache_hit_rate : null;
  // 本月成本 = 当月各天求和；任一天不可信即整月「—」（与 Rust 总账同款
  // fail-closed 口径，前端不重新发明阈值）
  const monthPrefix = localDayKey(Date.now()).slice(0, 7);
  const monthDays = (report.days || []).filter((d) => d.date.startsWith(monthPrefix));
  let monthCost = { cost_complete: true, cost_usd: 0 };
  for (const d of monthDays) {
    if (!d.cost_complete) { monthCost = null; break; }
    monthCost.cost_usd += Number(d.cost_usd) || 0;
  }
  // 预计今日：后台无预测（今日尚无用量）时不渲染该格，保持栅格语义诚实。
  // 该格带 id：usage-prediction-updated 事件（预警任务每 10 分钟一发）到达时
  // 就地更新，不整页重载，避免与并排「今日」数字出现可见的时差。
  const p = prediction && prediction.prediction;
  const projectedStat = p
    ? '<div class="usage-stat" id="usage-projected"><span class="usage-stat-l">' + dshdT('usageProjectedToday') + '</span><b data-trunc-tip data-tip-extra="' + esc(dshdT('usageProjectedTip')) + '">' + fmtTokens(p.projected_today_tokens || 0) + '</b></div>'
    : '';
  summary.innerHTML =
    '<div class="usage-stat"><span class="usage-stat-l">' + dshdT('usageToday') + '</span><b data-trunc-tip>' + fmtTokens(todayTokens(report)) + '</b></div>' +
    projectedStat +
    '<div class="usage-stat"><span class="usage-stat-l">' + dshdT('usageThisMonth') + '</span><b data-trunc-tip>' + fmtTokens(monthTokens(report)) + '</b></div>' +
    '<div class="usage-stat"><span class="usage-stat-l">' + dshdT('usageTotal') + '</span><b data-trunc-tip>' + fmtTokens(total.tokens || 0) + '</b></div>' +
    '<div class="usage-stat"><span class="usage-stat-l">' + dshdT('usageTodayHit') + '</span><b>' + formatPercent(todayHit) + '</b></div>' +
    '<div class="usage-stat"><span class="usage-stat-l">' + dshdT('usageTotalHit') + '</span><b>' + formatPercent(totalHit) + '</b></div>' +
    '<div class="usage-stat"><span class="usage-stat-l">' + dshdT('usageMonthCost') + '</span><b>' + costText(monthCost) + '</b></div>' +
    '<div class="usage-stat"><span class="usage-stat-l">' + dshdT('usageTotalCost') + '</span><b>' + costText(total) + '</b></div>';
  // 汇总区 token 数值可能因列宽不足被截断：仅截断时才给悬停 title 显示完整值。
  applyTruncationTips(summary);
  const load = $('usage-load');
  if (load) load.remove();
  // 区块换装：账户壳（余额卡与「更新于」槽）原位保留——它是唯一异步填充的
  // 区块，拆掉重建会在取数间隙闪空；其余区块同步重建且与拆除同任务完成，
  // 单次绘制无中间空白帧。首次渲染时 wrap 只有骨架，循环为空操作。
  const accShell = wrap.querySelector('[aria-labelledby="usage-accounts-heading"]');
  while (wrap.children.length > 1 && wrap.lastElementChild !== accShell) {
    wrap.lastElementChild.remove();
  }
  // 区块顺序：账户（余额/订阅）最常看 → 每日用量热图 → 最近 14 天 → 模型下钻。
  renderAccountsSection(wrap, seq, accShell);
  renderHeatmap(wrap, report);
  renderRecentDays(wrap, report);
  renderModelBreakdown(wrap, report);
  ensureUsagePredictionListener();
}

// 预测推送事件：用量页打开期间常驻，切走/关闭时随账户监听一并卸载。
// 只更新「预计今日」一格；无预测（今日尚无用量）时移除该格。
let usagePredictionUnlisten = null;
async function ensureUsagePredictionListener() {
  if (usagePredictionUnlisten) return;
  try {
    usagePredictionUnlisten = await dshdListen('usage-prediction-updated', (e) => {
      if (openKind !== 'usage' || !e.payload) return;
      const cell = $('usage-projected');
      const p = e.payload.prediction;
      if (!p) {
        if (cell) cell.remove();
        return;
      }
      if (!cell) {
        // 首次出现预测（打开页面时今日尚无用量）：插到「今日」之后
        const summary = $('usage-summary');
        if (!summary || !summary.firstElementChild) return;
        const div = document.createElement('div');
        div.className = 'usage-stat';
        div.id = 'usage-projected';
        div.innerHTML = '<span class="usage-stat-l">' + dshdT('usageProjectedToday') + '</span><b data-trunc-tip data-tip-extra="' + esc(dshdT('usageProjectedTip')) + '"></b>';
        summary.firstElementChild.after(div);
      }
      const slot = $('usage-projected');
      if (slot) {
        const b = slot.querySelector('b');
        if (b) b.textContent = fmtTokens(p.projected_today_tokens || 0);
        applyTruncationTips(slot);
      }
    });
  } catch (err) {
    usagePredictionUnlisten = null;
  }
}

function todayEntry(report) {
  const today = localDayKey(Date.now());
  return (report.days || []).find((d) => d.date === today) || null;
}
function todayTokens(report) {
  const day = todayEntry(report);
  return day ? day.tokens : 0;
}
function monthTokens(report) {
  const prefix = localDayKey(Date.now()).slice(0, 7);
  return (report.days || [])
    .filter((d) => d.date.startsWith(prefix))
    .reduce((sum, d) => sum + d.tokens, 0);
}
function localDayKey(ms) {
  const d = new Date(ms);
  const m = String(d.getMonth() + 1).padStart(2, '0');
  const day = String(d.getDate()).padStart(2, '0');
  return d.getFullYear() + '-' + m + '-' + day;
}

// 本地 HH:MM（updated_at 为秒级时间戳；「更新于」与 stale 标记共用）
function fmtClockTime(sec) {
  return new Date(sec * 1000).toLocaleTimeString(dshdLocale(), { hour: '2-digit', minute: '2-digit' });
}

// 最近 14 天行日期：今天/昨天语义化，其余本地短日期
function usageDayLabel(key) {
  if (key === localDayKey(Date.now())) return dshdT('usageTodayBtn');
  const yesterday = new Date();
  yesterday.setDate(yesterday.getDate() - 1);
  if (key === localDayKey(yesterday.getTime())) return dshdT('usageYesterday');
  const [y, m, d] = key.split('-').map(Number);
  try { return new Date(y, m - 1, d).toLocaleDateString(dshdLocale(), { month: 'short', day: 'numeric' }); }
  catch (e) { return key; }
}

// 百分比数值守卫：IPC 数据先钳制再进 style/文本，异常值不得进 DOM
function clampPct(v) {
  const n = Number(v);
  return Number.isFinite(n) ? Math.max(0, Math.min(100, Math.round(n))) : 0;
}

// 预警等级：以 Rust 快照字段为准；旧内核无该字段时按同一阈值在 UI 侧推导
// （余额按 剩余/总额，订阅按最紧窗口剩余%），与 Rust 并行开发期间也可用。
function warnLevelOf(item) {
  if (item.warn_level === 'none' || item.warn_level === 'warning' || item.warn_level === 'critical') {
    return item.warn_level;
  }
  const b = item.balance;
  if (b && !b.unlimited && b.remaining !== null && b.remaining !== undefined
      && b.total !== null && b.total !== undefined && Number(b.total) > 0) {
    const ratio = Number(b.remaining) / Number(b.total);
    return ratio <= 0.1 ? 'critical' : ratio <= 0.3 ? 'warning' : 'none';
  }
  const pcts = (item.windows || []).map((w) => Number(w.remaining_percent)).filter(Number.isFinite);
  if (pcts.length) {
    const min = Math.min(...pcts);
    return min <= 10 ? 'critical' : min <= 30 ? 'warning' : 'none';
  }
  return 'none';
}

// 供应商字母徽标：已知供应商固定缩写，未知取名称前两位字母数字
const PROVIDER_MARKS = {
  deepseek: 'DS', 'deepseek-official': 'DS',
  zai: 'Z', 'zai-coding-cn': 'Z',
  kimi: 'K', 'kimi-coding': 'K', moonshotai: 'K', 'moonshotai-cn': 'K',
  openrouter: 'OR', ollama: 'OL', 'opencode-go': 'GO', minimax: 'MM',
};
function providerMark(item) {
  const known = PROVIDER_MARKS[item.id] || PROVIDER_MARKS[item.adapter];
  if (known) return known;
  const letters = String(item.display_name || item.id || '').replace(/[^A-Za-z0-9]/g, '');
  return (letters.slice(0, 2) || '?').toUpperCase();
}

const WARN_ICON = '<svg viewBox="0 0 24 24" focusable="false" aria-hidden="true"><path d="m21.73 18-8-14a2 2 0 0 0-3.48 0l-8 14A2 2 0 0 0 4 21h16a2 2 0 0 0 1.73-3"></path><path d="M12 9v4"></path><path d="M12 17h.01"></path></svg>';
const CRIT_ICON = '<svg viewBox="0 0 24 24" focusable="false" aria-hidden="true"><path d="M15.312 2a2 2 0 0 1 1.414.586l4.688 4.688A2 2 0 0 1 22 8.688v6.624a2 2 0 0 1-.586 1.414l-4.688 4.688a2 2 0 0 1-1.414.586H8.688a2 2 0 0 1-1.414-.586l-4.688-4.688A2 2 0 0 1 2 15.312V8.688a2 2 0 0 1 .586-1.414l4.688-4.688A2 2 0 0 1 8.688 2z"></path><path d="M12 8v4"></path><path d="M12 16h.01"></path></svg>';
const CLOCK_ICON = dshdIcon('clock', 'focusable="false" aria-hidden="true"');

// —— 最近 14 天：本地日历窗口，只列有用量的日期（无用量省略、未来日不计）——
function renderRecentDays(wrap, report) {
  const cutoff = new Date();
  cutoff.setDate(cutoff.getDate() - 13);
  const cutoffKey = localDayKey(cutoff.getTime());
  const todayKey = localDayKey(Date.now());
  const rows = (report.days || [])
    .filter((d) => d.date >= cutoffKey && d.date <= todayKey && (d.tokens || 0) > 0)
    .sort((a, b) => (a.date < b.date ? 1 : -1));
  const section = document.createElement('section');
  section.className = 'usage-card';
  section.setAttribute('aria-labelledby', 'usage-recent-heading');
  section.innerHTML = '<h3 id="usage-recent-heading" class="usage-h">' + dshdT('usageRecent14') + '</h3>';
  if (!rows.length) {
    const empty = document.createElement('span');
    empty.className = 'usage-empty';
    empty.textContent = dshdT('usageEmpty');
    section.appendChild(empty);
    wrap.appendChild(section);
    return;
  }
  const max = Math.max(1, ...rows.map((d) => d.tokens || 0));
  const ul = document.createElement('ul');
  ul.className = 'usage-models usage-recent';
  for (const d of rows) {
    const hitRaw = d.cache_hit_rate;
    const hit = hitRaw === null || hitRaw === undefined ? null : Number(hitRaw);
    const li = document.createElement('li');
    li.className = 'usage-model';
    // 与按天下钻行同构：日期 + 命中率 + token + 分布条
    li.innerHTML =
      '<span class="usage-model-name usage-recent-date">' + esc(usageDayLabel(d.date)) + '</span>' +
      '<span class="usage-model-hit">' + (hit !== null && Number.isFinite(hit) ? hit + '%' : '—') + '</span>' +
      '<span class="usage-model-cost">' + costText(d) + '</span>' +
      '<b>' + fmtTokens(d.tokens || 0) + '</b>' +
      '<span class="usage-model-bar" aria-hidden="true"><span style="width:' + Math.max(4, Math.round(100 * (d.tokens || 0) / max)) + '%"></span></span>';
    ul.appendChild(li);
  }
  section.appendChild(ul);
  wrap.appendChild(section);
}

// —— 月历热图：月份导航 + 周一起始网格 + 选中日下钻 ——
let usageViewMonth = null;   // 'YYYY-MM'；null = 当前月
let usageSelectedDay = null; // 'YYYY-MM-DD'

function usageCurrentMonthKey() {
  const d = new Date();
  return d.getFullYear() + '-' + String(d.getMonth() + 1).padStart(2, '0');
}
function usageShiftMonth(key, delta) {
  const [y, m] = key.split('-').map(Number);
  const date = new Date(y, m - 1 + delta, 1);
  return date.getFullYear() + '-' + String(date.getMonth() + 1).padStart(2, '0');
}
function usageMonthLabel(key) {
  const [y, m] = key.split('-').map(Number);
  const locale = dshdLocale && dshdLocale() === 'zh-CN' ? 'zh-CN' : 'en';
  try { return new Date(y, m - 1, 1).toLocaleDateString(locale, { year: 'numeric', month: 'long' }); }
  catch (e) { return key; }
}

function renderHeatmap(wrap, report) {
  const section = document.createElement('section');
  section.className = 'usage-card';
  section.setAttribute('aria-labelledby', 'usage-heatmap-heading');
  const days = report.days || [];
  const dayMap = new Map(days.map((d) => [d.date, d]));
  if (!usageViewMonth) usageViewMonth = usageCurrentMonthKey();
  const curMonth = usageCurrentMonthKey();
  const prevDisabled = usageViewMonth <= '1970-01';
  const nextDisabled = usageViewMonth >= curMonth;
  section.innerHTML =
    '<div class="usage-heat-header">' +
    '<h3 id="usage-heatmap-heading" class="usage-h">' + dshdT('usageHeatmap') + '</h3>' +
    '<div class="usage-month-nav">' +
    '<button type="button" class="usage-month-btn" data-usage-prev aria-label="' + dshdT('usagePrevMonth') + '"' + (prevDisabled ? ' disabled' : '') + '>' + CHEV_LEFT + '</button>' +
    '<span class="usage-month-title">' + esc(usageMonthLabel(usageViewMonth)) + '</span>' +
    '<button type="button" class="usage-month-btn" data-usage-next aria-label="' + dshdT('usageNextMonth') + '"' + (nextDisabled ? ' disabled' : '') + '>' + CHEV_RIGHT + '</button>' +
    '<button type="button" class="usage-month-btn usage-month-today" data-usage-today title="' + dshdT('usageBackToToday') + '">' + dshdT('usageTodayBtn') + '</button>' +
    '</div></div>' +
    '<div class="usage-cal" role="group" aria-label="' + esc(dshdT('usageHeatmap')) + '"></div>' +
    '<div class="usage-day-detail" hidden></div>';
  wrap.appendChild(section);

  const cal = section.querySelector('.usage-cal');
  const detail = section.querySelector('.usage-day-detail');
  const draw = () => {
    if (usageViewMonth === null) usageViewMonth = curMonth;
    usageRenderCalendar(cal, detail, usageViewMonth, dayMap, report);
    // 同步月份标题与「今天」按钮禁用态（此前只更新按钮禁用态，标题不动，
    // 导致翻页看似「月份不变」）；「今天」常显，当前月时禁用
    const title = section.querySelector('.usage-month-title');
    if (title) title.textContent = usageMonthLabel(usageViewMonth);
    const prev = section.querySelector('[data-usage-prev]');
    const next = section.querySelector('[data-usage-next]');
    if (prev) prev.disabled = usageViewMonth <= '1970-01';
    if (next) next.disabled = usageViewMonth >= usageCurrentMonthKey();
    const today = section.querySelector('[data-usage-today]');
    if (today) today.disabled = usageViewMonth === usageCurrentMonthKey();
  };
  section.querySelector('[data-usage-prev]').addEventListener('click', () => {
    usageViewMonth = usageShiftMonth(usageViewMonth, -1);
    draw();
  });
  section.querySelector('[data-usage-next]').addEventListener('click', () => {
    usageViewMonth = usageShiftMonth(usageViewMonth, 1);
    draw();
  });
  const todayBtn = section.querySelector('[data-usage-today]');
  if (todayBtn) todayBtn.addEventListener('click', () => {
    usageViewMonth = usageCurrentMonthKey();
    draw();
  });
  draw();
}

const CHEV_LEFT = '<svg viewBox="0 0 24 24" focusable="false" aria-hidden="true" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="m15 6-6 6 6 6"/></svg>';
const CHEV_RIGHT = '<svg viewBox="0 0 24 24" focusable="false" aria-hidden="true" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="m9 6 6 6-6 6"/></svg>';

function usageRenderCalendar(cal, detail, monthKey, dayMap, report) {
  const [year, month0] = monthKey.split('-').map(Number);
  const month = month0 - 1;
  const first = new Date(year, month, 1);
  const startDow = (first.getDay() + 6) % 7; // 周一起始
  const daysInMonth = new Date(year, month + 1, 0).getDate();
  const max = Math.max(1, ...(report.days || []).map((d) => d.tokens || 0));
  const weekLabels = (dshdLocale && dshdLocale() === 'zh-CN')
    ? ['一', '二', '三', '四', '五', '六', '日']
    : ['M', 'T', 'W', 'T', 'F', 'S', 'S'];
  cal.textContent = '';
  weekLabels.forEach((w) => {
    const span = document.createElement('span');
    span.className = 'usage-week-label';
    span.textContent = w;
    cal.appendChild(span);
  });
  for (let i = 0; i < startDow; i++) {
    const span = document.createElement('span');
    span.className = 'usage-day empty';
    cal.appendChild(span);
  }
  for (let d = 1; d <= daysInMonth; d++) {
    const key = year + '-' + String(month0).padStart(2, '0') + '-' + String(d).padStart(2, '0');
    const entry = dayMap.get(key);
    const tokens = entry ? entry.tokens : 0;
    const hit = entry ? entry.cache_hit_rate : null;
    const lv = tokens <= 0 ? 0 : Math.max(1, Math.min(4, Math.ceil(Math.sqrt(tokens / max) * 4)));
    const today = localDayKey(Date.now()) === key;
    const btn = document.createElement('button');
    btn.type = 'button';
    btn.className = 'usage-day' + (usageSelectedDay === key ? ' selected' : '') + (today ? ' today' : '');
    btn.dataset.lv = String(lv);
    // aria-label 与格内视觉信息对齐：日期（今天标注）+ token + 命中率
    btn.setAttribute('aria-label', (today ? dshdT('usageTodayBtn') + '，' : '') + key + '：' + fmtTokens(tokens) + (hit !== null && hit !== undefined ? '，' + dshdT('usageCacheHit') + ' ' + hit + '%' : ''));
    // 格子内只放日期号数（对齐插件：token/命中率进 tooltip/aria-label 与
    // 选中后的明细，避免格内塞三项导致换行溢出）。
    btn.innerHTML = '<span>' + d + '</span>';
    btn.title = key + '：' + fmtTokens(tokens) + (hit !== null && hit !== undefined ? '，' + dshdT('usageCacheHit') + ' ' + hit + '%' : '');
    btn.addEventListener('click', () => {
      const prevSel = cal.querySelector('.usage-day.selected');
      if (prevSel) prevSel.classList.remove('selected');
      usageSelectedDay = usageSelectedDay === key ? null : key;
      if (usageSelectedDay) btn.classList.add('selected');
      usageRenderDayDetail(detail, usageSelectedDay, dayMap);
    });
    cal.appendChild(btn);
  }
  usageRenderDayDetail(detail, usageSelectedDay, dayMap);
}

function usageRenderDayDetail(detail, dayKey, dayMap) {
  if (!dayKey) { detail.hidden = true; detail.textContent = ''; return; }
  const entry = dayMap.get(dayKey);
  if (!entry || !(entry.models && entry.models.length)) {
    detail.hidden = false;
    detail.innerHTML = '<span class="usage-empty empty-state">' +
      '<svg viewBox="0 0 24 24" aria-hidden="true"><circle cx="12" cy="12" r="10"></circle><path d="M12 16v-4"></path><path d="M12 8h.01"></path></svg>' +
      dshdT('usageEmpty') + '</span>';
    return;
  }
  const max = Math.max(1, ...entry.models.map((m) => m.tokens));
  detail.hidden = false;
  detail.innerHTML = '<ul class="usage-day-detail-list">' + entry.models.map((m) =>
    '<li class="usage-model">' +
    '<span class="usage-model-name" data-trunc-tip>' + esc(m.model) + '</span>' +
    '<span class="usage-model-hit">' + (m.cache_hit_rate !== null && m.cache_hit_rate !== undefined ? m.cache_hit_rate + '%' : '—') + '</span>' +
    '<span class="usage-model-cost">' + costText(m) + '</span>' +
    '<b>' + fmtTokens(m.tokens) + '</b>' +
    '<span class="usage-model-bar" aria-hidden="true"><span style="width:' + Math.max(4, Math.round(100 * m.tokens / max)) + '%"></span></span>' +
    '</li>'
  ).join('') + '</ul>';
  applyTruncationTips(detail);
}

function renderModelBreakdown(wrap, report) {
  const days = report.days || [];
  const byModel = new Map();
  for (const d of days) {
    for (const m of (d.models || [])) {
      if (m.tokens > 0) byModel.set(m.model, (byModel.get(m.model) || 0) + m.tokens);
    }
  }
  const rows = [...byModel.entries()].sort((a, b) => b[1] - a[1]).slice(0, 10);
  if (!rows.length) return;
  const maxTokens = Math.max(1, ...rows.map((r) => r[1]));
  const section = document.createElement('section');
  section.className = 'usage-card';
  section.setAttribute('aria-labelledby', 'usage-models-heading');
  section.innerHTML = '<h3 id="usage-models-heading" class="usage-h">' + dshdT('usageModels') + '</h3><ul class="usage-models"></ul>';
  const ul = section.querySelector('.usage-models');
  for (const [model, tokens] of rows) {
    const li = document.createElement('li');
    li.className = 'usage-model';
    // 不显示命中率列：Rust 聚合只提供按天/总计命中率，跨天按模型的
    // 聚合命中率没有来源，占位列只会误导（按天下钻行有真实命中率）
    li.innerHTML =
      '<span class="usage-model-name" data-trunc-tip>' + esc(model) + '</span>' +
      // 占位对齐：与最近 14 天/按天下钻行同为 4 列（此处无命中率列），
      // 使成本与 token 列的右缘在各区块一致
      '<span class="usage-model-hit" aria-hidden="true"></span>' +
      '<span class="usage-model-cost">' + aggregateModelCostText(days, model) + '</span>' +
      '<b>' + fmtTokens(tokens) + '</b>' +
      '<span class="usage-model-bar" aria-hidden="true"><span style="width:' + Math.max(4, Math.round(100 * tokens / maxTokens)) + '%"></span></span>';
    ul.appendChild(li);
  }
  wrap.appendChild(section);
  applyTruncationTips(wrap);
}

async function renderAccountsSection(wrap, seq, reuse) {
  // reuse（刷新路径）：账户壳已在 wrap 原位，直接复用；首次渲染新建。
  // 壳内卡片区始终由快照整建（renderAccountCards 开头清空），壳本身
  // 结构不随数据变化，复用不会残留旧结构
  const section = reuse || document.createElement('section');
  if (!reuse) {
    section.className = 'usage-card';
    section.setAttribute('aria-labelledby', 'usage-accounts-heading');
    section.innerHTML =
      '<div class="usage-acc-head-row">' +
      '<h3 id="usage-accounts-heading" class="usage-h">' + dshdT('usageProviders') + '</h3>' +
      '<span class="usage-upd" id="usage-upd"></span>' +
      '</div><div class="usage-accounts" role="status" aria-live="polite"></div>';
    wrap.appendChild(section);
  }
  ensureUsageAccountsListener();
  const box = section.querySelector('.usage-accounts');
  // 「查询余额中」占位只给空盒：刷新路径沿用旧卡作占位，避免卡片闪断
  if (!box.childElementCount) {
    box.innerHTML = '<span class="usage-empty"><span class="spin" aria-hidden="true"></span>' + dshdT('queryingBalance') + '</span>';
  }
  try {
    // 当前会话上下文与账户/订阅并行拉取：单次渲染无竞态（若先渲染快照再
    // 异步补标注，晚到的事件推送会被旧快照回写覆盖）；命令失败静默为 null
    const [accounts, subs, ctx] = await Promise.all([
      invoke('usage_accounts_get'),
      invoke('usage_subscriptions_get'),
      invoke('usage_session_context_get').catch(() => null),
    ]);
    if (openKind !== 'usage' || seq !== usageSeq) return;
    usageSessionContext = ctx && (ctx.route_id || ctx.display_name || ctx.model) ? ctx : null;
    applyAccountsSnapshot(section, accounts || [], subs || []);
  } catch (e) {
    if (openKind !== 'usage' || seq !== usageSeq) return;
    box.innerHTML = '<span class="usage-empty err" role="alert">' + esc(dshdT('usageFailed')) + ': ' + esc(String(e)) + '</span>';
  }
}

// 账户区唯一渲染出口：缓存快照（get）与后台推送（事件）走同一路径
function applyAccountsSnapshot(section, accounts, subs) {
  const box = section.querySelector('.usage-accounts');
  if (!box) return;
  renderAccountCards(box, meaningfulAccounts(accounts, subs));
  const upd = latestUpdatedAt(accounts, subs);
  const updEl = section.querySelector('#usage-upd');
  if (updEl) {
    // 新快照到达即覆盖「刷新超时」提示：文案由下方赋值更新，err 着色须一并清除
    updEl.classList.remove('err');
    updEl.textContent = upd ? dshdT('updatedAt', { time: fmtClockTime(upd) }) : '';
  }
}

function latestUpdatedAt(accounts, subs) {
  let latest = 0;
  for (const a of accounts || []) if (a.updated_at && a.updated_at > latest) latest = a.updated_at;
  for (const s of subs || []) if (s.updated_at && s.updated_at > latest) latest = s.updated_at;
  return latest || null;
}

// —— 当前会话：加载账户区时并行拉取活动会话上下文（route_id/display_name/model，
// 全 null = 无活动会话）；匹配到账户卡则置顶并加「当前会话」徽标。
// 命令不存在或失败一律静默为 null，不影响账户区主流程。——
let usageSessionContext = null;
// 会话与账户卡的匹配：route_id 优先、display_name 兜底；都匹配不到则不留痕迹
function matchSessionAccount(items) {
  const ctx = usageSessionContext;
  if (!ctx) return null;
  if (ctx.route_id) {
    const byId = items.find((a) => a.id === ctx.route_id);
    if (byId) return byId;
  }
  if (ctx.display_name) {
    return items.find((a) => a.display_name === ctx.display_name) || null;
  }
  return null;
}

// 账户推送事件：用量页打开期间常驻，切走/关闭弹窗时卸载
let usageAccountsUnlisten = null;
async function ensureUsageAccountsListener() {
  if (usageAccountsUnlisten) return;
  try {
    usageAccountsUnlisten = await dshdListen('usage-accounts-updated', (e) => {
      if (openKind !== 'usage' || !e.payload) return;
      const box = document.querySelector('.usage-accounts');
      const section = box ? box.closest('.usage-card') : null;
      if (!section) return;
      applyAccountsSnapshot(section, e.payload.accounts || [], e.payload.subscriptions || []);
      // 后台刷新完成：若标题栏仍在转圈则收尾（900ms 最小转圈在 finish 内保证）
      finishUsageRefresh();
    });
  } catch (err) {
    usageAccountsUnlisten = null;
  }
}
function dropUsageAccountsListener() {
  if (usageAccountsUnlisten) { usageAccountsUnlisten(); usageAccountsUnlisten = null; }
  if (usagePredictionUnlisten) { usagePredictionUnlisten(); usagePredictionUnlisten = null; }
  // 切走/关闭时复位标题栏刷新态与会话上下文（防残留到下次打开）
  clearTimeout(usageRefreshTimer);
  usageRefreshTimer = null;
  setUsageRefreshBusy(false);
  usageSessionContext = null;
}

function renderAccountCards(box, items) {
  box.textContent = '';
  if (!items.length) {
    box.innerHTML = '<span class="usage-empty empty-state" role="status">' +
      '<svg viewBox="0 0 24 24" aria-hidden="true"><rect width="20" height="8" x="2" y="2" rx="2" ry="2"></rect><rect width="20" height="8" x="2" y="14" rx="2" ry="2"></rect><path d="M6 6h.01M6 18h.01"></path></svg>' +
      dshdT('accountNotConfiguredHint') + '</span>';
    return;
  }
  // 当前会话账户卡置顶；匹配不到会话则保持原顺序、不留任何痕迹
  const sessionItem = matchSessionAccount(items);
  if (sessionItem && items[0] !== sessionItem) {
    items = [sessionItem].concat(items.filter((a) => a !== sessionItem));
  }
  for (const a of items) {
    const card = document.createElement('div');
    card.className = 'usage-account';
    // accent 由 CSS 按 data-provider/data-adapter 映射（未知供应商回退 --dshd-accent）
    card.dataset.provider = a.id || '';
    card.dataset.adapter = a.adapter || '';
    const warn = warnLevelOf(a);
    if (warn !== 'none') card.dataset.warn = warn;
    const statusKey = ACCOUNT_STATUS_KEY[a.status] || 'accountUnavailable';
    const statusText = dshdT(statusKey);
    const statusTone = a.status === 'ok' ? 'ok' : (a.status === 'not-configured' ? 'dim' : 'err');
    let detail = '';
    if (a.status === 'not-configured') {
      detail = '<span class="usage-acc-hint">' + dshdT('accountNotConfiguredHint') + '</span>';
    } else if (a.balance && a.balance.remaining !== null && a.balance.remaining !== undefined) {
      // 金额与状态栏 chip 同一格式化（dshdCurrency + dshdBalanceValue），
      // token 简写不适用于钱；unlimited 显示 ∞
      detail = '<b class="usage-acc-amount">' + cur(a.balance.currency || '') + (a.balance.unlimited ? '∞' : dshdBalanceValue(a.balance.remaining)) + '</b>'
        + balanceRowsHtml(a.balance);
    } else if (a.windows && a.windows.length) {
      detail = a.windows.map((w) => usageWindowHtml(w)).join('');
    }
    // 标记行：当前会话徽标（pill 同体系，徽标旁附模型名）、
    // stale（上次成功时间）与预警（图标+文字，不得只靠颜色传信息）
    let flags = '';
    if (a === sessionItem) {
      flags += '<span class="usage-acc-session">' + esc(dshdT('usageCurrentSession')) + '</span>';
      if (usageSessionContext && usageSessionContext.model) {
        flags += '<span class="usage-acc-session-model" data-trunc-tip>' + esc(usageSessionContext.model) + '</span>';
      }
    }
    if (a.stale) {
      flags += '<span class="usage-acc-stale">' + CLOCK_ICON + esc(a.updated_at
        ? dshdT('usageLastSuccessAt', { time: fmtClockTime(a.updated_at) })
        : dshdT('usageStaleGeneric')) + '</span>';
    }
    if (warn !== 'none') {
      const crit = warn === 'critical';
      flags += '<span class="usage-acc-warn" data-level="' + warn + '">' + (crit ? CRIT_ICON : WARN_ICON) + esc(dshdT(crit ? 'usageWarnCritical' : 'usageWarnLow')) + '</span>';
    }
    card.innerHTML =
      '<div class="usage-acc-head"><span class="usage-acc-mark" aria-hidden="true">' + esc(providerMark(a)) + '</span>' +
      '<span class="usage-acc-name" data-trunc-tip>' + esc(a.display_name || a.id) + '</span>' +
      '<span class="usage-acc-status ' + statusTone + '">' + esc(statusText) + '</span></div>' +
      (detail ? '<div class="usage-acc-detail">' + detail + '</div>' : '') +
      (flags ? '<div class="usage-acc-flags">' + flags + '</div>' : '');
    box.appendChild(card);
  }
  applyTruncationTips(box);
}

// 余额拆分行：已用/总额/赠送/充值，有值才显示（与参考面板同构）
function balanceRowsHtml(balance) {
  const rows = [
    ['usageUsed', balance.used],
    ['usageBalanceTotal', balance.total],
    ['grantedBalance', balance.granted],
    ['toppedUpBalance', balance.topped_up],
  ].filter((pair) => pair[1] !== null && pair[1] !== undefined);
  if (!rows.length) return '';
  return '<div class="usage-acc-rows">' + rows.map((pair) =>
    '<div class="usage-acc-row"><span>' + dshdT(pair[0]) + '</span><span>' + cur(balance.currency || '') + dshdBalanceValue(pair[1]) + '</span></div>'
  ).join('') + '</div>';
}

// 订阅窗口行：剩余% 进度条 + 重置时间（resets_at 有则显示本地短日期）
function usageWindowHtml(w) {
  const pct = clampPct(w.remaining_percent);
  const resets = usageResetLabel(w.resets_at);
  return '<span class="usage-acc-window" role="status" aria-label="' + esc(w.kind) + ' ' + pct + '%">' +
    '<span class="usage-acc-wl" data-trunc-tip>' + esc(w.kind) + '</span>' +
    '<span class="usage-acc-bar" aria-hidden="true"><span style="width:' + pct + '%"></span></span>' +
    '<span class="usage-acc-wv">' + pct + '%</span></span>' +
    (resets ? '<span class="usage-acc-wreset">' + esc(resets) + '</span>' : '');
}
function usageResetLabel(resetsAt) {
  if (typeof resetsAt !== 'string' || !resetsAt) return '';
  const date = new Date(resetsAt);
  if (Number.isNaN(date.getTime())) return '';
  try {
    return dshdT('usageResetsAt', {
      time: date.toLocaleString(dshdLocale(), { month: 'short', day: 'numeric', hour: '2-digit', minute: '2-digit' }),
    });
  } catch (e) { return ''; }
}

const ACCOUNT_STATUS_KEY = {
  ok: 'accountOk',
  'not-configured': 'accountNotConfigured',
  unauthorized: 'accountUnauthorized',
  'rate-limited': 'accountRateLimited',
  unavailable: 'accountUnavailableStatus',
  'invalid-response': 'accountInvalidResponse',
  blocked: 'accountBlocked',
  unsupported: 'accountUnsupported',
};

// —— 检查更新 ——
function renderCheckLoading(message) {
  // 占位文案记入 lastProgress：后续进度事件与占位相同则不再重复渲染
  // （不做跨语言文案相等判断——语言切换后旧比较会失效）
  lastProgress = message || dshdT('checkingUpdates');
  $('body').innerHTML = '<div class="msg" role="status" aria-live="polite"><span class="spin" aria-hidden="true"></span>' + (message || dshdT('checkingUpdates')) + '</div>';
  // 无底部操作区（dsh 设置弹窗无 footer）
}
function updBtn(id, label, which, primary, pre) {
  const btn = document.createElement('button');
  btn.type = 'button';
  btn.className = 'dshd-btn' + (primary ? ' primary' : '');
  btn.textContent = label;
  btn.dataset.label = label;
  // 更新执行中重建结果行时，按钮保持禁用（UAC 确认期间同理）
  btn.disabled = updateRunning;
  btn.addEventListener('click', () => {
    updateRunning = true;
    renderNav(openKind);
    document.querySelectorAll('.uprow .dshd-btn').forEach((button) => {
      button.disabled = true;
    });
    btn.textContent = dshdT('processing');
    // pre：可选前置动作（跨通道入口先切通道再更新），失败与更新请求同路反馈
    const run = pre ? pre() : Promise.resolve();
    run.then(() => invoke('app_dialog_update', { which })).catch((e) => {
      updateRunning = false;
      renderNav(openKind);
      btn.disabled = false;
      btn.textContent = label;
      renderUpdateDone({ ok: false, message: dshdT('operationNotStarted', { message: e }) });
    });
  });
  document.getElementById(id).append(btn);
}
// 版本对比行：当前版本弱化 + 箭头 + 最新版本高亮（可更新时）
function verHtml(cur, latest, available) {
  if (available) {
    return esc(cur) + '<span class="v-arrow">→</span><b class="v-latest">' + esc(latest) + '</b>';
  }
  return esc(cur) + '<span class="v-ok">' + dshdT('upToDate') + '</span>';
}
// 通道标签复用设置页文案（稳定版/预览版/尝鲜版），key 缺失时兜底显示 dist-tag 名
const DSH_CHANNEL_LABEL_KEYS = {
  latest: 'settingsChannelLatest',
  next: 'settingsChannelNext',
  alpha: 'settingsChannelAlpha',
};
function dshdChannelLabel(channel) {
  return dshdT(DSH_CHANNEL_LABEL_KEYS[channel] || channel);
}
function renderCheckResult(r) {
  if (openKind !== 'check') return;
  lastCheckResult = r;
  const body = $('body');
  let html = '';
  if (r.dsh) {
    // 降级行同时带"查询错误"与"降级警示"时合并展示（前者优先，便于排障）
    let dshTip = r.dsh.latest_error || '';
    if (r.dsh.downgrade_available && !dshTip) dshTip = dshdT('downgradeTip');
    if (!dshTip && r.dsh.other_channel) {
      dshTip = dshdT('otherChannelTip', {
        channel: dshdChannelLabel(r.dsh.other_channel.channel),
        version: r.dsh.other_channel.version,
      });
    }
    const hint = dshTip ? ' data-tip-extra="' + esc(dshTip) + '"' : '';
    // 空 latest：查询失败时如实标注"暂无法获取版本信息"，不显示"已是最新"；
    // 版本号只输出一次（verHtml 内含 esc(cur)，此处仅拼接状态标签）
    const verLabel = !r.dsh.latest
      ? '<span class="v-ok">' + dshdT('versionServiceUnavailable') + '</span>'
      : (r.dsh.update_available || r.dsh.downgrade_available
        ? '<span class="v-arrow">→</span><b class="v-latest">' + esc(r.dsh.latest) + '</b>'
        : '<span class="v-ok">' + dshdT('upToDate') + '</span>');
    html += '<div class="uprow"><div class="info"><div class="name">dsh</div>' +
      '<div class="ver" data-trunc-tip' + hint + '>' + esc(r.dsh.installed) + verLabel + '</div></div>' +
      '<span id="u-dsh"></span></div>';
  }
  if (r.node) {
    const v = r.node.installed || dshdT('notInstalled');
    const label = !r.node.managed
      ? (r.node.latest_lts
        ? dshdT('systemManagedLatest', { version: esc(r.node.latest_lts) })
        : dshdT('systemManagedUnavailable'))
      : !r.node.latest_lts ? dshdT('versionServiceUnavailable')
      : dshdT('upToDateLts');
    const hint = r.node.latest_error ? ' data-tip-extra="' + esc(r.node.latest_error) + '"' : '';
    html += '<div class="uprow"><div class="info"><div class="name">Node.js</div>' +
      '<div class="ver" data-trunc-tip' + hint + '>' + (r.node.update_available
        ? verHtml(v, r.node.latest_lts || dshdT('latestVersion'), true)
        : (r.node.installed ? esc(v) + '<span class="v-ok">' + label + '</span>' : esc(v))) + '</div></div>' +
      '<span id="u-node"></span></div>';
  }
  if (r.pwsh) {
    const v = r.pwsh.installed || dshdT('notInstalled');
    const label = !r.pwsh.installed ? dshdT('notInstalled')
      : !r.pwsh.latest ? dshdT('versionServiceUnavailable')
      : dshdT('upToDate');
    const hint = r.pwsh.latest_error ? ' data-tip-extra="' + esc(r.pwsh.latest_error) + '"' : '';
    html += '<div class="uprow"><div class="info"><div class="name">PowerShell 7</div>' +
      '<div class="ver" data-trunc-tip' + hint + '>' + (r.pwsh.update_available
        ? verHtml(v, r.pwsh.latest || dshdT('latestVersion'), true)
        : (r.pwsh.installed ? esc(v) + '<span class="v-ok">' + label + '</span>' : esc(v))) + '</div></div>' +
      '<span id="u-pwsh"></span></div>';
  }
  // npm 是 Node 自带但可独立维护的工具，版本与操作入口单列，避免把它
  // 误解成 Node 版本的一部分。
  if (r.npm) {
    const v = r.npm.installed || dshdT('notInstalled');
    const hint = r.npm.latest_error ? ' data-tip-extra="' + esc(r.npm.latest_error) + '"' : '';
    const label = !r.npm.installed ? dshdT('notInstalled')
      : !r.npm.latest ? dshdT('versionServiceUnavailable')
      : dshdT('upToDate');
    html += '<div class="uprow"><div class="info"><div class="name">npm</div>' +
      '<div class="ver" data-trunc-tip' + hint + '>' + (r.npm.update_available
        ? verHtml(v, r.npm.latest || dshdT('latestVersion'), true)
        : (r.npm.installed ? esc(v) + '<span class="v-ok">' + label + '</span>' : esc(v))) + '</div></div>' +
      '<span id="u-npm"></span></div>';
  }
  // GitHub 查询失败时 r.app 为空，但应用本机版本仍应始终可见；
  // 远端状态明确标注不可用，避免用户误以为没有检查应用本体。
  const localAppVersion = currentOpen && currentOpen.initial
    ? currentOpen.initial.app_version || ''
    : '';
  if (r.app || localAppVersion) {
    const installed = r.app ? r.app.installed : localAppVersion;
    // 空 latest 显示"暂无法获取版本信息"；错误原因经 data-tip-extra 悬停展示
    // 注意：verLabel 不再重复拼接 installed（曾显示成 0.1200.1.20）
    const verLabel = !r.app || !r.app.latest
      ? '<span class="v-ok">' + dshdT('versionServiceUnavailable') + '</span>'
      : (r.app.update_available
        ? '<span class="v-arrow">→</span><b class="v-latest">' + esc(r.app.latest) + '</b>'
        : '<span class="v-ok">' + dshdT('upToDate') + '</span>');
    const hint = r.app && r.app.latest_error
      ? ' data-tip-extra="' + esc(r.app.latest_error) + '"'
      : '';
    html += '<div class="uprow"><div class="info"><div class="name">DSHBox</div>' +
      '<div class="ver" data-trunc-tip' + hint + '>' + esc(installed) + verLabel + '</div></div>' +
      '<span id="u-app"></span></div>';
  }
  if (r.error) html += '<div class="msg error" role="alert">' + esc(r.error) + '</div>';
  if (!r.dsh && !r.node && !r.pwsh && !r.app && !r.error) html += '<div class="msg error" role="alert">' + dshdT('checkFailedRetry') + '</div>';
  body.innerHTML = html;
  if (r.dsh && r.dsh.update_available) updBtn('u-dsh', dshdT('update'), 'dsh', true);
  else if (r.dsh && r.dsh.downgrade_available) updBtn('u-dsh', dshdT('switchVersion', { version: r.dsh.latest }), 'dsh', true);
  // 跨通道发现：当前通道已最新，但其他通道指向更高版本——先切通道再走更新
  else if (r.dsh && r.dsh.other_channel) {
    const hint = r.dsh.other_channel;
    updBtn(
      'u-dsh',
      dshdT('switchOtherChannel', { channel: dshdChannelLabel(hint.channel), version: hint.version }),
      'dsh',
      true,
      () => invoke('set_dsh_channel', { channel: hint.channel }),
    );
  }
  if (r.node && r.node.update_available) updBtn('u-node', dshdT(r.node.installed ? 'update' : 'install'), 'node', false);
  if (r.pwsh && r.pwsh.update_available) updBtn('u-pwsh', dshdT(r.pwsh.installed ? 'update' : 'install'), 'pwsh', false);
  if (r.npm && r.npm.update_available) updBtn('u-npm', dshdT('update'), 'npm', false);
  if (r.app && r.app.update_available) updBtn('u-app', dshdT('updateApp'), 'app', false);
  const any = (r.dsh && (r.dsh.update_available || r.dsh.downgrade_available || r.dsh.other_channel)) || (r.node && r.node.update_available) || (r.pwsh && r.pwsh.update_available) || (r.npm && r.npm.update_available) || (r.app && r.app.update_available);
  if (!any && !r.error && (r.dsh || r.node || r.pwsh || r.npm || r.app)) {
    const message = document.createElement('div');
    message.className = 'msg';
    message.setAttribute('role', 'status');
    // r.app 为空 = GitHub 查询失败：其余全最新也不能宣称「没有可用更新」，
    // 明确区分部分检查未完成（DSHBox 行内已标注「暂无法获取版本信息」）
    message.textContent = r.app ? dshdT('noUpdates') : dshdT('noUpdatesPartial');
    body.append(message);
  }
  // 右上角关闭已够，右下角不再放纯关闭按钮（dsh 原生设置同）
  // 无底部操作区（dsh 设置弹窗无 footer）
  renderPluginConflictBanner();
  applyTruncationTips(body);
}
// —— 插件更新冲突（dsh 更新因插件加载崩溃回滚）——
// 轮询通道 app_dialog_check_get 的 plugin_conflict 字段驱动；卸载完成
// 后字段清空，下次轮询重建结果区时提示条随之消失。
let lastPluginConflict = null;
function renderPluginConflictBanner() {
  const existing = document.getElementById('plugin-conflict');
  const name = lastPluginConflict;
  // 冲突已解决（记录清空）：移除提示条
  if (!name) {
    if (existing) existing.remove();
    return;
  }
  // 同名冲突的提示条已存在则不重建：结果区重渲染不应触发读屏重复播报
  if (existing && existing.dataset.name === name) return;
  if (existing) existing.remove();
  if (updateRunning) return;
  const body = $('body');
  const box = document.createElement('div');
  box.className = 'conflict-banner';
  box.id = 'plugin-conflict';
  box.dataset.name = name;
  box.setAttribute('role', 'alert');
  const text = document.createElement('span');
  text.className = 'conflict-text';
  text.textContent = dshdT('pluginConflictBanner', { name });
  // 卸载是破坏性操作：两步确认（再点一次），5 秒无操作自动复位
  const btn = document.createElement('button');
  btn.type = 'button';
  btn.className = 'dshd-btn small danger';
  btn.dataset.label = dshdT('pluginConflictRemove');
  btn.textContent = btn.dataset.label;
  let armed = false;
  let resetTimer = 0;
  btn.addEventListener('click', () => {
    if (updateRunning || btn.disabled) return;
    if (!armed) {
      armed = true;
      btn.textContent = dshdT('pluginConflictRemoveConfirm', { name });
      resetTimer = setTimeout(() => {
        armed = false;
        btn.textContent = btn.dataset.label;
      }, 5000);
      return;
    }
    clearTimeout(resetTimer);
    updateRunning = true;
    renderNav(openKind);
    document.querySelectorAll('.uprow .dshd-btn').forEach((button) => {
      button.disabled = true;
    });
    btn.disabled = true;
    btn.textContent = dshdT('processing');
    invoke('plugin_resolve_update_conflict', { package: name }).catch((e) => {
      updateRunning = false;
      renderNav(openKind);
      renderUpdateDone({ ok: false, message: dshdT('operationNotStarted', { message: e }) });
    });
  });
  box.append(text, btn);
  body.append(box);
}
let lastProgress = '';
function renderProgress(message) {
  if (openKind !== 'check') return;
  if (!message || message === lastProgress) return;
  lastProgress = message;
  const body = $('body');
  const line = body.querySelector('.msg');
  if (line) {
    // 进度文案更新只改文字节点：innerHTML 重建会销毁 .spin 元素、
    // CSS 动画从 0deg 重启，下载期约 200ms 一次的更新让转圈永远
    // 转不完就被拉回起点（视觉抖动）
    const spinner = line.querySelector('.spin');
    if (spinner) {
      while (spinner.nextSibling) spinner.nextSibling.remove();
      spinner.insertAdjacentText('afterend', message);
      return;
    }
    line.innerHTML = '<span class="spin" aria-hidden="true"></span>' + esc(message);
    return;
  }
  const div = document.createElement('div');
  div.className = 'msg';
  div.setAttribute('role', 'status');
  div.textContent = message;
  body.append(div);
}
function renderUpdateDone(p) {
  if (openKind !== 'check') return;
  // 更新流程结束：此后按钮按结果复位
  updateRunning = false;
  renderNav(openKind);
  // UAC“继续”期间页脚被替换为确认按钮：完成后恢复“关闭”
  // 右上角关闭已够，右下角不再放纯关闭按钮（dsh 原生设置同）
  // 无底部操作区（dsh 设置弹窗无 footer）
  const body = $('body');
  const line = body.querySelector('.msg');
  const text = p.ok ? p.message : dshdT('notCompleted', { message: p.message });
  // 冲突提示条的按钮与结果行按钮一起复位（失败可重试，成功置完成）
  document.querySelectorAll('.uprow .dshd-btn, #plugin-conflict .dshd-btn').forEach((button) => {
    if (p.ok) {
      button.disabled = true;
      if (button.textContent === dshdT('processing')) button.textContent = dshdT('completed');
    } else {
      button.disabled = false;
      button.textContent = button.dataset.label || dshdT('retry');
    }
  });
  const box = line || (() => {
    const div = document.createElement('div');
    body.append(div);
    return div;
  })();
  box.className = 'msg ' + (p.ok ? 'success' : 'error');
  box.setAttribute('role', p.ok ? 'status' : 'alert');
  // 长文案（含启动日志尾部等）经空行分为“标题 + 详情”：详情默认折叠，
  // 读屏只播报可操作的标题行；无空行分隔的普通消息保持原样整体展示
  const sep = text.indexOf('\n\n');
  // 赋值 textContent 会清掉上一次渲染的折叠控件，无需手动移除
  box.textContent = sep >= 0 ? text.slice(0, sep).trimEnd() : text;
  if (sep >= 0) {
    const detailText = text.slice(sep + 2).trim();
    if (detailText) {
      const toggle = document.createElement('button');
      toggle.type = 'button';
      toggle.className = 'done-toggle';
      toggle.textContent = dshdT('viewDetail');
      toggle.setAttribute('aria-expanded', 'false');
      const detail = document.createElement('div');
      detail.className = 'done-detail hidden';
      detail.textContent = detailText;
      toggle.addEventListener('click', () => {
        const collapsed = detail.classList.toggle('hidden');
        toggle.textContent = collapsed ? dshdT('viewDetail') : dshdT('hideDetail');
        toggle.setAttribute('aria-expanded', String(!collapsed));
      });
      box.append(toggle, detail);
    }
  }
}

// —— 关于 ——
// 品牌图标直接引用 assets/app-icon.svg（与启动页/窗口图标同源，
// 不维护内联 SVG 副本，避免品牌资源漂移）
const ABOUT_LOGO =
  '<img class="about-logo" src="assets/app-icon.svg" alt="" width="48" height="48">';
function renderAbout(initial) {
  $('body').innerHTML =
    '<div class="about">' +
    ABOUT_LOGO +
    '<div class="nm">DSHBox</div>' +
    '<div class="tag">' + dshdT('aboutTagline') + '</div>' +
    '<div class="ver-row">' +
    '<span>' + dshdT('appVersion', { version: esc(initial.app_version) }) + '</span>' +
    '<span>dsh ' + esc(initial.dsh_version || '—') + '</span>' +
    '</div>' +
    '<button type="button" class="dshd-btn" id="about-check">' + dshdT('checkUpdates') + '</button>' +
    '<div class="cp">© ' + new Date().getFullYear() + ' JeffioZ</div>' +
    '</div>';
  // 右上角关闭已够，右下角不再放纯关闭按钮（dsh 原生设置同）
  // 无底部操作区（dsh 设置弹窗无 footer）
  const checkBtn = $('about-check');
  if (checkBtn) checkBtn.addEventListener('click', () => navigateTo('check'));
}

// —— 更新提示（dsh 发现新版 / DSHBox 应用更新就绪，共用）——
function renderUpdatePrompt(p) {
  const isApp = p && p.kind === 'app';
  const version = (p && p.version) || '';
  const current = (p && p.current) || '';
  const headline = isApp
    ? dshdT('appUpdateReadyDesc', { version })
    : dshdT('dshUpdatePromptDesc', { version, current });
  const body = $('body');
  // href 只接受 https：中键/新标签等绕过 click 处理器的边缘路径下，
  // javascript: 等伪协议不应有生效机会（click 常规路径已由 Rust 校验）
  const releaseHref = p && p.release_url && String(p.release_url).startsWith('https:')
    ? esc(p.release_url) : '';
  const viewLink = releaseHref
    ? '<a class="up-link" id="up-view-release" href="' + releaseHref +
      '" rel="noopener noreferrer">' + dshdT('viewReleaseNotes') + ' ↗</a>'
    : '';
  body.innerHTML =
    '<div class="update-prompt" data-tauri-drag-region="deep">' +
    '<div class="up-copy dshd-scroll">' +
    '<div class="up-row">' +
    '<span class="up-icon" aria-hidden="true">' + dshdIcon('download', 'focusable="false"') + '</span>' +
    '<div class="up-text">' +
    '<div class="up-desc">' + esc(headline) + '</div>' +
    viewLink +
    '</div>' +
    '</div>' +
    '</div>' +
    '<div class="up-actions">' +
    '<button type="button" class="dshd-btn" id="up-later">' + dshdT('later') + '</button>' +
    '<button type="button" class="dshd-btn primary" id="up-confirm">' +
      (isApp ? dshdT('restartAndUpdate') : dshdT('updateNow')) + '</button>' +
    '</div>' +
    '</div>';
  const viewBtn = $('up-view-release');
  if (viewBtn) {
    viewBtn.addEventListener('click', (event) => {
      event.preventDefault();
      invoke('open_external_url', { url: p.release_url }).catch(() => {});
    });
  }
  $('up-later').addEventListener('click', () => close());
  $('up-confirm').addEventListener('click', () => {
    const button = $('up-confirm');
    const which = isApp ? 'app' : 'dsh';
    // 防双击：更新动作不可并发重复触发（dsh 会起第二条 apply 线程）。
    if (button.disabled) return;
    // dev 效果预览的模拟数据：确认按钮不触发真实更新
    if (p && p.simulated) {
      dshdToast(dshdT('appRestartSimulatedToast'), { kind: 'info' });
      return;
    }
    button.disabled = true;
    button.setAttribute('aria-busy', 'true');
    invoke('app_dialog_update', { which }).catch(() => {
      button.disabled = false;
      button.removeAttribute('aria-busy');
    });
  });
  refreshCompactDrag();
}

// —— “更新应用”确认（自绘，替代原生 msgbox）——
function renderAppRestartConfirm(p) {
  const simulated = !!(p && p.simulated);
  const rawVersion = p && typeof p.version === 'string' ? p.version.replace(/^v/i, '') : '';
  const headline = rawVersion
    ? dshdT('appRestartConfirmDescWithVersion', { version: rawVersion })
    : dshdT('appRestartConfirmDesc');
  const body = $('body');
  body.innerHTML =
    '<div class="update-prompt" data-tauri-drag-region="deep">' +
    '<div class="up-copy dshd-scroll">' +
    '<div class="up-row">' +
    '<span class="up-icon" aria-hidden="true">' + dshdIcon('restart', 'focusable="false"') + '</span>' +
    '<div class="up-text"><div class="up-desc">' + esc(headline) + '</div></div>' +
    '</div>' +
    '</div>' +
    '<div class="up-actions">' +
    '<button type="button" class="dshd-btn" id="up-cancel">' + dshdT('cancel') + '</button>' +
    '<button type="button" class="dshd-btn primary" id="up-confirm">' + dshdT('updateAndRestart') + '</button>' +
    '</div>' +
    '</div>';
  $('up-cancel').addEventListener('click', () => {
    // 模拟弹窗取消直接关闭，不切到检查更新视图（那会触发真实检查）
    if (simulated) { close(); return; }
    invoke('app_dialog_cancel_app_restart').catch(() => close());
  });
  $('up-confirm').addEventListener('click', () => {
    // 防双击：确认即进入退出替换流程，不可并发重复触发
    const button = $('up-confirm');
    if (button.disabled) return;
    // dev 效果测试的模拟弹窗：不触发真实更新
    if (simulated) {
      dshdToast(dshdT('appRestartSimulatedToast'), { kind: 'info' });
      return;
    }
    button.disabled = true;
    button.setAttribute('aria-busy', 'true');
    invoke('app_dialog_update', { which: 'app' }).catch(() => {
      button.disabled = false;
      button.removeAttribute('aria-busy');
    });
  });
  refreshCompactDrag();
}

// —— 轻量提示（自绘，单“关闭”按钮：托盘动作失败/拒绝类）——
// 紧凑弹窗公共收尾：文案区溢出时恢复其指针命中（滚动/选择优先于
// 拖动）；未溢出时保持整卡可拖（pointer-events 已由 CSS 收起）
function refreshCompactDrag() {
  const copy = document.querySelector('.update-prompt .up-copy');
  if (!copy) return;
  // 窗口 set_size 与内容渲染同帧发生，立即量取可能拿到旧视口；
  // 等一帧布局稳定后再判定是否溢出。溢出时以 data-tauri-drag-region="false"
  // 阻断该子树拖动（滚动/选择优先于拖动）
  requestAnimationFrame(() => {
    const overflow = copy.scrollHeight > copy.clientHeight;
    copy.classList.toggle('drag-exempt', overflow);
    if (overflow) copy.setAttribute('data-tauri-drag-region', 'false');
    else copy.removeAttribute('data-tauri-drag-region');
  });
}

function renderNotice(p) {
  const message = p && typeof p.message === 'string' ? p.message : '';
  // 语义级别：warn=琥珀三角（拒绝/失败），info=蓝色圆 i（中性说明）；
  // 图标形状与颜色双通道表意
  const warn = !(p && p.severity === 'info');
  const body = $('body');
  body.innerHTML =
    '<div class="update-prompt' + (warn ? ' warn' : '') + '" data-tauri-drag-region="deep">' +
    '<div class="up-copy dshd-scroll">' +
    '<div class="up-row">' +
    '<span class="up-icon" aria-hidden="true">' +
      dshdIcon(warn ? 'warning' : 'info', 'focusable="false"') +
    '</span>' +
    '<div class="up-text"><div class="up-desc">' + esc(message) + '</div></div>' +
    '</div>' +
    '</div>' +
    '<div class="up-actions">' +
    '<button type="button" class="dshd-btn primary" id="notice-close">' + dshdT('close') + '</button>' +
    '</div>' +
    '</div>';
  $('notice-close').addEventListener('click', () => close());
  refreshCompactDrag();
}

// —— 左侧导航（单窗口多功能切换） ——
const NAV_ITEMS = [
  { kind: 'usage', label: 'navUsage', icon: 'chart', capability: 'managed-ready' },
  { separator: true },
  { kind: 'plugins', label: 'navPlugins', icon: 'puzzle', capability: 'managed-ready' },
  { kind: 'settings', label: 'navSettings', icon: 'gear' },
  { separator: true },
  { kind: 'check', label: 'navCheck', icon: 'download' },
  { kind: 'about', label: 'navAbout', icon: 'info' },
];
function navCapability(item) {
  if (!item || !item.capability) return { enabled: true, reason: '' };
  const initial = (currentOpen && currentOpen.initial) || {};
  const external = initial.service_mode === 'external' || initial.service_mode === 'external-disconnected';
  if (item.capability === 'local') {
    return external
      ? { enabled: false, reason: dshdT('navManagedOnly') }
      : { enabled: true, reason: '' };
  }
  if (external) return { enabled: false, reason: dshdT('navManagedOnly') };
  if (item.kind === 'plugins' && updateRunning) {
    return { enabled: false, reason: dshdT('navUpdateRunning') };
  }
  return initial.service_ready && initial.service_mode === 'managed'
    ? { enabled: true, reason: '' }
    : { enabled: false, reason: dshdT('navRequiresReady') };
}
const NAV_TITLE_KEY = {
  usage: 'usageTitle', check: 'checkUpdates', plugins: 'pluginsTitle',
  settings: 'settingsTitle', about: 'about',
};
const NAV_ICONS = {
  chart: '<svg viewBox="0 0 24 24" focusable="false" aria-hidden="true"><path d="M3 3v16a2 2 0 0 0 2 2h16"></path><path d="M18 17V9"></path><path d="M13 17V5"></path><path d="M8 17v-3"></path></svg>',
  download: dshdIcon('download', 'focusable="false" aria-hidden="true"'),
  puzzle: dshdIcon('puzzle', 'focusable="false" aria-hidden="true"'),
  gear: dshdIcon('gear', 'focusable="false" aria-hidden="true"'),
  info: dshdIcon('info', 'focusable="false" aria-hidden="true"'),
};
function renderNav(activeKind) {
  const nav = $('nav');
  if (!nav) return;
  // 更新提示是即时弹窗，无左侧功能导航；但头部标题仍要写入 dialog-title
  // （与 ✕ 同一排，由 Rust 传入的 currentOpen.title 提供）。
  if (activeKind === 'update-prompt' || activeKind === 'app-restart' || activeKind === 'notice') {
    nav.innerHTML = '';
    const pageTitle = $('dialog-title');
    if (pageTitle) pageTitle.textContent = (currentOpen && currentOpen.title) || '';
    return;
  }
  nav.setAttribute('aria-label', dshdT('navLabel'));
  nav.innerHTML = '';
  // 导航顶部标题（对齐 dsh 设置弹窗 navTitle：16px/24px 500）；
  // 可拖拽区（head 只占内容区，导航顶部需承担部分窗口拖动）
  const title = document.createElement('div');
  title.className = 'nav-title';
  title.setAttribute('data-tauri-drag-region', 'deep');
  title.textContent = 'DSHBox';
  nav.append(title);
  for (const item of NAV_ITEMS) {
    if (item.separator) {
      const separator = document.createElement('div');
      separator.className = 'nav-sep';
      separator.setAttribute('role', 'separator');
      nav.append(separator);
      continue;
    }
    const btn = document.createElement('button');
    btn.type = 'button';
    btn.className = 'nav-item' + (item.kind === activeKind ? ' active' : '');
    btn.dataset.kind = item.kind;
    if (item.kind === activeKind) btn.setAttribute('aria-current', 'page');
    const capability = navCapability(item);
    btn.disabled = !capability.enabled;
    if (!capability.enabled) btn.title = capability.reason;
    btn.innerHTML = '<span class="nic">' + NAV_ICONS[item.icon] + '</span><span class="nav-label">' + esc(dshdT(item.label)) + '</span>';
    btn.addEventListener('click', () => navigateTo(item.kind));
    nav.append(btn);
  }
  const titleKey = NAV_TITLE_KEY[activeKind];
  const pageTitle = $('dialog-title');
  if (pageTitle) pageTitle.textContent = titleKey ? dshdT(titleKey) : '';
  // 导航底部版本号（填充空间，弱化显示）
  const ver = document.createElement('div');
  ver.className = 'nav-ver';
  const v = (currentOpen && currentOpen.initial && currentOpen.initial.app_version) || '';
  ver.textContent = v ? 'v' + v : '';
  nav.append(ver);
  // 导航列空白区域可拖动整窗（deep=子树可拖，列表项为 button 由
  // Tauri 自动豁免）；列内出现滚动（小屏）时以 "false" 阻断，
  // 避免 mousedown 劫持滚动条。等一帧布局稳定后再量取
  requestAnimationFrame(() => {
    if (nav.scrollHeight <= nav.clientHeight) nav.setAttribute('data-tauri-drag-region', 'deep');
    else nav.setAttribute('data-tauri-drag-region', 'false');
  });
}
function renderCurrent(opts) {
  const k = openKind;
  // 切换视图即清空旧页面的瞬态提示（toast 挂在 document.body，不随内容区销毁）
  dshdToastClearAll();
  // 刷新按钮仅用量页显示（旧余额页已合并进用量页）
  const refresh = $('btn-refresh');
  if (refresh) refresh.classList.toggle('hidden', k !== 'usage');
  if (k === 'usage') {
    renderUsagePage();
  }
  else if (k === 'check') {
    const updating = !!(currentOpen && currentOpen.initial && currentOpen.initial.updating);
    if (updating) {
      // 更新后台执行中：显示"正在更新 dsh…"（win32 提示框确认路径，
      // 或更新中重开弹窗）；进度事件随后覆盖本行
      renderCheckLoading(dshdT('updatingDsh'));
    } else if (lastCheckResult && !dshChannelChanged) renderCheckResult(lastCheckResult);
    else {
      // 仅导航进入时触发检查（applyOpen 时 Rust 已触发，避免重复网络请求）
      renderCheckLoading();
      dshChannelChanged = false;
      if (opts && opts.triggerCheck) invoke('app_dialog_run_check').catch(() => {});
    }
  } else if (k === 'about') renderAbout((currentOpen && currentOpen.initial) || {});
  else if (k === 'update-prompt') renderUpdatePrompt((currentOpen && currentOpen.initial) || {});
  else if (k === 'app-restart') renderAppRestartConfirm((currentOpen && currentOpen.initial) || {});
  else if (k === 'notice') renderNotice((currentOpen && currentOpen.initial) || {});
  else if (k === 'plugins') renderPlugins();
  else if (k === 'settings') renderSettings();
}
// 导航切换的内容过渡：旧内容先退场（上浮淡出），再换内容并入场
// （下浮淡入）。快速连点时重置退场定时器，旧内容重新起退场，不叠加。
let viewSwapTimer = null;
function playViewEnter() {
  const content = $('body');
  content.classList.remove('view-enter');
  void content.offsetWidth;
  content.classList.add('view-enter');
  // 入场结束移除动画类：will-change 随之类移除，不常驻合成层
  // （子元素的 animationend 会冒泡，须校验目标与动画名）
  content.addEventListener('animationend', function onEnd(e) {
    if (e.target === content && e.animationName === 'view-enter') {
      content.classList.remove('view-enter');
      content.removeEventListener('animationend', onEnd);
    }
  });
}
function navigateTo(kind) {
  if (!currentOpen || openKind === kind) return;
  const item = NAV_ITEMS.find((candidate) => candidate.kind === kind);
  if (!navCapability(item).enabled) return;
  // 离开用量页：卸载账户推送监听并复位刷新态（避免后台事件打靶到已切走的页）
  if (openKind === 'usage') dropUsageAccountsListener();
  openKind = kind;
  currentOpen = { kind, title: '', initial: currentOpen.initial };
  document.body.classList.toggle('update-prompt-mode', kind === 'update-prompt' || kind === 'app-restart' || kind === 'notice');
  renderNav(kind);
  applyTruncationTips(document);
  const reduced = window.matchMedia('(prefers-reduced-motion: reduce)').matches;
  clearTimeout(viewSwapTimer);
  const content = $('body');
  if (reduced) {
    $('body').scrollTop = 0;
    renderCurrent({ triggerCheck: true });
    return;
  }
  // 入场动画进行中再次切换：移除 view-enter 会把内容瞬间拉回原位，
  // 再走退场会闪跳——直接换内容并重播入场（连续位移，视觉不抖）
  if (content.classList.contains('view-enter')) {
    content.classList.remove('view-enter', 'view-exit');
    $('body').scrollTop = 0;
    renderCurrent({ triggerCheck: true });
    playViewEnter();
    return;
  }
  // 退场期间旧内容仍在（renderCurrent 尚未执行）；退完先归位滚动、
  // 再换内容入场（渲染前归位避免渲染后的二次滚动调整）
  content.classList.remove('view-enter', 'view-exit');
  void content.offsetWidth;
  content.classList.add('view-exit');
  viewSwapTimer = setTimeout(() => {
    viewSwapTimer = null;
    content.classList.remove('view-exit');
    $('body').scrollTop = 0;
    renderCurrent({ triggerCheck: true });
    playViewEnter();
  }, 90);
}

let closeTimer = null;
function close() {      // 内容快速淡出（不露出“…”标题），90ms 后请 Rust 清空并隐藏；
  // 淡出后的表面近似空卡片，天然充当下次打开前的中性帧
  if (document.body.classList.contains('fade-out')) return;
  document.body.classList.add('fade-out');
  const delay = window.matchMedia('(prefers-reduced-motion: reduce)').matches ? 0 : 90;
  closeTimer = setTimeout(() => {
    closeTimer = null;
    invoke('app_dialog_close').catch(() => {});
  }, delay);
}

let lastOpenKey = '';
let pwshPromptShown = false;
// 更新执行中：期间“更新/安装”按钮保持禁用（结果行重建会重置禁用态）
let updateRunning = false;
function applyOpen(p) {
  if (!p) return;
  // 快速重开：先取消挂起的关闭 invoke 并撤掉淡出——即使载荷与上次
  // 完全相同（余额/关于等恒定载荷）也必须先执行，否则旧关闭定时器
  // 会把刚重开的弹窗误关、页面保持淡出空白
  clearTimeout(closeTimer);
  closeTimer = null;
  document.body.classList.remove('fade-out');
  // 载荷印章去重：一次打开会被多条通道触发（eval / emit / 可见性变化），
  // 只渲染一次，避免重复重建 DOM 与重复动效
  const key = JSON.stringify(p);
  if (key === lastOpenKey) return;
  lastOpenKey = key;
  // 复位轮询/流程状态：关闭前的 __dshdReset 可能尚未执行（130ms 关闭
  // 窗口内快速重开），残留的印章会让轮询误判“无变化”而不渲染——
  // UAC“继续”不再出现、结果行永久转圈
  checkStamp = '';
  lastResultKey = '';
  lastProgress = '';
  pwshPromptShown = false;
  if (document.activeElement && document.activeElement !== document.body) document.activeElement.blur();
  currentOpen = p;
  openKind = p.kind;
  // 检查更新视图以 Rust 载荷的 updating 为准复位按钮禁用态：
  // “更新应用”确认弹窗取消返回时，前端残留的 updateRunning=true
  // 会让更新按钮永久卡在禁用（后台并无更新在进行）
  if (p.kind === 'check' && !(p.initial && p.initial.updating)) updateRunning = false;
  document.body.classList.toggle('update-prompt-mode', p.kind === 'update-prompt' || p.kind === 'app-restart' || p.kind === 'notice');
  applyTruncationTips(document);
  renderNav(p.kind);
  // Rust 打开时已按 kind 预置状态（open_check 触发检查等），此处只渲染
  renderCurrent();
  // 独立 WebView 模态窗没有可继承的页面焦点。把焦点放到标题，让键盘和
  // 读屏用户立即进入当前对话语境，随后 Tab 按自然 DOM 顺序到达控件。
  requestAnimationFrame(() => {
    if (currentOpen !== p) return;
    const title = $('dialog-title');
    if (title) title.focus({ preventScroll: true });
  });
  // 整窗入场仅打开时播一次；导航切换只播放右侧内容区的轻量提示。
  // 透明窗口创建即定位，纯 opacity 淡入不会再闪烁。
  document.body.classList.remove('dshd-pop-in');
  void document.body.offsetWidth;
  document.body.classList.add('dshd-pop-in');
}
// Rust 在隐藏状态下同步直呼（载荷内联）：show 前就渲染好本次内容，
// 第一帧即正确内容，无上一弹窗残影
window.__dshdOpen = applyOpen;
// Rust 在关闭时直呼：清空内容并复位印章。空卡片会保持可见一帧后被
// 隐藏（隐藏窗口不绘制），该帧成为下次打开前的第一帧——不再闪上一弹窗残影。
// 印章复位也保证：再次打开时即使载荷与上次完全相同也会重新渲染
window.__dshdReset = () => {
  if (document.activeElement && document.activeElement !== document.body) document.activeElement.blur();
  // 与 applyOpen 对称：撤掉挂起的关闭定时器与进度去重（关闭 invoke 已
  // 发出时定时器早已触发，此处只是防御性清理，无副作用）
  clearTimeout(closeTimer);
  closeTimer = null;
  lastOpenKey = '';
  currentOpen = null;
  lastCheckResult = null;
  lastResultKey = '';
  lastProgress = '';
  openKind = '';
  dropUsageAccountsListener();
  document.body.classList.remove('update-prompt-mode');
  checkStamp = '';
  pwshPromptShown = false;
  updateRunning = false;
  renderNav(openKind);
  pluginApplyStamp = '';
  // 标题由 renderNav 写导航顶部；此处仅清空残留
  $('body').innerHTML = '';
  // 无底部操作区（dsh 设置弹窗无 footer）
  const nav = $('nav');
  if (nav) nav.innerHTML = '';
};
dshdListen('app-dialog-open', (e) => applyOpen(e.payload));
dshdListen('dsh-status', (e) => {
  if (!currentOpen || !e.payload) return;
  currentOpen.initial = currentOpen.initial || {};
  currentOpen.initial.service_mode = e.payload.service_mode || 'none';
  currentOpen.initial.service_ready = e.payload.phase === 'ready'
    && (e.payload.service_mode === 'managed' || e.payload.service_mode === 'external');
  renderNav(openKind);
}).catch(() => {});
document.addEventListener('visibilitychange', async () => {
  if (document.visibilityState === 'visible') {
    // 轮询挂起期间错过的状态（后台更新完成等）立即补拉
    pollDialogState();
    // 不做动效重放：该 WebView 对“隐藏→显示”的可见性事件不可靠，
    // 迟到重放会中途重启动画造成抖动（托盘菜单的教训）；入场动效
    // 由 applyOpen 在预渲染时播放，show 后可见部分自然呈现
    try { applyOpen(await invoke('app_dialog_get')); } catch (e) {}
  }
});
dshdListen('update-result', (e) => {
  if (openKind === 'check') renderCheckResult(e.payload);
}).catch(() => {});
dshdListen('update-progress', (e) => {
  if (e.payload && e.payload.message) renderProgress(e.payload.message);
}).catch(() => {});
window.addEventListener('dshd-language-changed', () => {
  if (!currentOpen) return;
  applyTruncationTips(document);
  renderNav(openKind);
  renderCurrent();
});
// —— 轮询拉取（事件通道对该窗口不可靠，此为主通道）——
let checkStamp = '';
let lastResultKey = '';
async function pollDialogState() {
  // 弹窗隐藏（关闭/被抢占）时挂起轮询：零开销等待，可见性恢复由
  // visibilitychange 立即调用本函数补上（隐藏 WebView 的事件通道本就
  // 不可靠，轮询只服务可见页面）
  if (document.hidden) return;
  try {
    if (openKind === 'check') {
      const s = await invoke('app_dialog_check_get');
      if (openKind !== 'check') return;
      const key = JSON.stringify(s);
      if (key !== checkStamp) {
        checkStamp = key;
        // 后端更新执行中：按钮全程保持禁用（结果行重建不再复活）
        if (s && s.updating && !updateRunning) {
          updateRunning = true;
          renderNav(openKind);
        }
        // 冲突状态独立于检查结果：即使结果未变（如在插件页卸载了冲突插件），
        // 提示条也要即时出现/消失
        const conflict = (s && s.plugin_conflict) || null;
        if (conflict !== lastPluginConflict) {
          lastPluginConflict = conflict;
          renderPluginConflictBanner();
        }
        // UAC 确认等待期间不重建结果行（保持按钮禁用态）；
        // 结果未变化时不重复重建——进度每次更新只刷新文案行，
        // 被点按钮的“处理中…”文案得以保留
        if (s && s.result && !s.pwsh_pending) {
          const rk = JSON.stringify(s.result);
          if (rk !== lastResultKey) {
            lastResultKey = rk;
            renderCheckResult(s.result);
          }
        }
        // 进度独立于结果渲染：关闭重开后，进行中的更新进度经状态拉取
        if (s && s.progress) renderProgress(s.progress);
        if (s && s.done) renderUpdateDone(s.done);
      }
      // PowerShell UAC 预告：弹窗内展示并等待“继续”确认（kind 守卫：
      // 晚到响应不得把预告写到已重置的空白弹窗上）
      if (openKind === 'check' && s && s.pwsh_pending && !pwshPromptShown) {
        pwshPromptShown = true;
        const body = $('body');
        const div = document.createElement('div');
        div.className = 'msg';
        div.textContent = dshdT('pwshUacNotice');
        body.append(div);
        // 底部操作区已随 dsh 布局重构移除（footer 区域删除），
        // "继续"按钮直接挂在正文后（原 #foot 引用已失效，会静默抛错
        // 导致确认按钮不渲染、UAC 更新流程卡死）
        const btn = document.createElement('button');
        btn.type = 'button';
        btn.className = 'dshd-btn primary';
        btn.textContent = dshdT('continue');
        btn.addEventListener('click', () => {
          btn.disabled = true;
          invoke('app_dialog_pwsh_confirm').catch(() => { btn.disabled = false; });
        });
        body.append(btn);
      }
    } else if (openKind === 'plugins') {
      await refreshPluginApplyStatus();
    }
  } catch (e) {}
}
setInterval(pollDialogState, 1500);

$('btn-x').addEventListener('click', close);
// 用量页刷新（唯一刷新入口）：先触发 Rust 账户后台全量刷新（失败静默），
// 再原位重载（旧内容保留作加载占位，数据到达后同任务换装，不整页重建）；
// 转圈至 usage-accounts-updated 事件到达（至少 900ms 防抖），成功收尾出
// 轻量确认 toast；命令失败立即静默收尾，事件丢失或后台异常时 30s 超时
// 兜底（行内提示），期间禁用防连点。
let usageRefreshBusy = false;
let usageRefreshTimer = null;
let usageRefreshStart = 0;
let usageRefreshFinishing = false;
function setUsageRefreshBusy(busy) {
  usageRefreshBusy = busy;
  // 复位时一并清「收尾中」：切页/关窗经 set(false) 直接中断在途收尾，
  // 不清会把下一次刷新卡在早退上
  if (!busy) usageRefreshFinishing = false;
  const button = $('btn-refresh');
  if (!button) return;
  button.classList.toggle('refreshing', busy);
  button.disabled = busy;
  button.toggleAttribute('aria-busy', busy);
}
function finishUsageRefresh(timedOut, silent) {
  // 收尾中再触发（事件连发、命令失败与事件竞速）只认首个：避免重复
  // 排程收尾与重复 toast
  if (!usageRefreshBusy || usageRefreshFinishing) return;
  usageRefreshFinishing = true;
  clearTimeout(usageRefreshTimer);
  usageRefreshTimer = null;
  const wait = Math.max(0, 900 - (Date.now() - usageRefreshStart));
  setTimeout(() => {
    setUsageRefreshBusy(false);
    // 成功收尾给轻量确认（静默完成不可感知）；仅在用量页仍打开时提示，
    // 免得切页/关窗后补一声。silent=命令失败静默收尾、timedOut=超时
    // （已有 usage-upd 行内提示），都不出成功 toast
    if (!timedOut && !silent && openKind === 'usage') {
      dshdToast(dshdT('usageRefreshed'), { kind: 'ok' });
    }
    // 30s 超时兜底触发：事件丢失/后台异常时不得静默收场，在「更新于」
    // 槽位短暂提示失败并保留原快照（4s 后还原上次更新时间）
    if (!timedOut) return;
    const upd = document.querySelector('.usage-upd');
    if (!upd) return;
    const prev = upd.textContent;
    const timeoutText = dshdT('usageRefreshTimeout');
    upd.classList.add('err');
    upd.textContent = timeoutText;
    setTimeout(() => {
      if (upd.classList.contains('err') && upd.textContent === timeoutText) {
        upd.classList.remove('err');
        upd.textContent = prev;
      }
    }, 4000);
  }, wait);
}
$('btn-refresh').addEventListener('click', () => {
  if (usageRefreshBusy) return;
  setUsageRefreshBusy(true);
  usageRefreshStart = Date.now();
  clearTimeout(usageRefreshTimer);
  usageRefreshTimer = setTimeout(() => finishUsageRefresh(true), 30000);
  invoke('usage_accounts_refresh').catch(() => finishUsageRefresh(false, true));
  renderUsagePage(true);
});
window.addEventListener('keydown', (e) => {
  if (e.key === 'Escape') close();
});
