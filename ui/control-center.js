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

// 用量导出：保存对话框由后端弹出；进行中禁用全部导出按钮防重复触发
async function exportUsage(kind, btn) {
  const all = document.querySelectorAll('.usage-export-btn');
  all.forEach((b) => { b.disabled = true; });
  const prev = btn.textContent;
  btn.textContent = dshdT('usageExporting');
  try {
    await invoke('usage_export', { format: kind });
    btn.textContent = dshdT('usageExportDone');
  } catch (e) {
    btn.textContent = dshdT('usageExportFailed');
    window.setTimeout(() => { btn.textContent = prev; }, 2500);
  }
  window.setTimeout(() => {
    btn.textContent = prev;
    all.forEach((b) => { b.disabled = false; });
  }, 1800);
}

async function renderUsagePage() {
  const seq = ++usageSeq;
  const body = $('body');
  body.innerHTML =
    '<div class="usage-wrap">' +
    '<section class="usage-card" aria-labelledby="usage-summary-heading">' +
    '<div class="usage-h-row">' +
    '<h3 id="usage-summary-heading" class="usage-h">' + dshdT('usageTokenSection') + '</h3>' +
    '<div class="usage-export-btns">' +
    '<button type="button" class="usage-export-btn" data-usage-export="csv">' + dshdT('usageExportCsv') + '</button>' +
    '<button type="button" class="usage-export-btn" data-usage-export="json">JSON</button>' +
    '</div>' +
    '</div>' +
    '<div class="usage-summary" id="usage-summary"></div>' +
    '</section>' +
    '<div class="usage-load" id="usage-load" role="status" aria-live="polite"><span class="spin" aria-hidden="true"></span>' + dshdT('usageLoading') + '</div>' +
    '</div>';
  body.querySelectorAll('.usage-export-btn').forEach((btn) => {
    btn.addEventListener('click', () => exportUsage(btn.dataset.usageExport, btn));
  });
  try {
    const report = await invoke('usage_report_get');
    if (openKind !== 'usage' || seq !== usageSeq) return;
    renderUsageReport(report, seq);
  } catch (e) {
    if (openKind !== 'usage' || seq !== usageSeq) return;
    const load = $('usage-load');
    if (load) { load.className = 'usage-load err'; load.textContent = dshdT('usageFailed') + ': ' + e; }
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

function renderUsageReport(report, seq) {
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
  summary.innerHTML =
    '<div class="usage-stat"><span class="usage-stat-l">' + dshdT('usageToday') + '</span><b data-trunc-tip>' + fmtTokens(todayTokens(report)) + '</b></div>' +
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
  // 区块顺序：账户（余额/订阅）最常看 → 每日用量热图 → 最近 14 天 → 模型下钻。
  renderAccountsSection(wrap, seq);
  renderHeatmap(wrap, report);
  renderRecentDays(wrap, report);
  renderModelBreakdown(wrap, report);
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

const WARN_ICON = '<svg viewBox="0 0 24 24" focusable="false" aria-hidden="true"><path d="M10.3 3.9 1.8 18a2 2 0 0 0 1.7 3h17a2 2 0 0 0 1.7-3L13.7 3.9a2 2 0 0 0-3.4 0z"></path><path d="M12 9v4"></path><path d="M12 17h.01"></path></svg>';
const CRIT_ICON = '<svg viewBox="0 0 24 24" focusable="false" aria-hidden="true"><path d="M7.9 2h8.2L22 7.9v8.2L16.1 22H7.9L2 16.1V7.9L7.9 2z"></path><path d="M12 8v4"></path><path d="M12 16h.01"></path></svg>';
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
    '<div class="usage-cal" role="img" aria-label="' + esc(dshdT('usageHeatmap')) + '"></div>' +
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
    detail.innerHTML = '<span class="usage-empty">' + dshdT('usageEmpty') + '</span>';
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

async function renderAccountsSection(wrap, seq) {
  const section = document.createElement('section');
  section.className = 'usage-card';
  section.setAttribute('aria-labelledby', 'usage-accounts-heading');
  section.innerHTML =
    '<div class="usage-acc-head-row">' +
    '<h3 id="usage-accounts-heading" class="usage-h">' + dshdT('usageProviders') + '</h3>' +
    '<span class="usage-upd" id="usage-upd"></span>' +
    '</div><div class="usage-accounts" role="status" aria-live="polite"></div>';
  wrap.appendChild(section);
  ensureUsageAccountsListener();
  const box = section.querySelector('.usage-accounts');
  box.innerHTML = '<span class="usage-empty"><span class="spin" aria-hidden="true"></span>' + dshdT('queryingBalance') + '</span>';
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
  if (updEl) updEl.textContent = upd ? dshdT('updatedAt', { time: fmtClockTime(upd) }) : '';
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
    usageAccountsUnlisten = await listen('usage-accounts-updated', (e) => {
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
  // 切走/关闭时复位标题栏刷新态与会话上下文（防残留到下次打开）
  clearTimeout(usageRefreshTimer);
  usageRefreshTimer = null;
  setUsageRefreshBusy(false);
  usageSessionContext = null;
}

function renderAccountCards(box, items) {
  box.textContent = '';
  if (!items.length) {
    box.innerHTML = '<span class="usage-empty" role="status">' + dshdT('accountNotConfiguredHint') + '</span>';
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
function updBtn(id, label, which, primary) {
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
    invoke('app_dialog_update', { which }).catch((e) => {
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
function renderCheckResult(r) {
  if (openKind !== 'check') return;
  lastCheckResult = r;
  const body = $('body');
  let html = '';
  if (r.dsh) {
    const hint = r.dsh.latest_error ? ' data-tip-extra="' + esc(r.dsh.latest_error) + '"' : '';
    // 空 latest：查询失败时如实标注"暂无法获取版本信息"，不显示"已是最新"；
    // 版本号只输出一次（verHtml 内含 esc(cur)，此处仅拼接状态标签）
    const verLabel = !r.dsh.latest
      ? '<span class="v-ok">' + dshdT('versionServiceUnavailable') + '</span>'
      : (r.dsh.update_available
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
  if (r.node && r.node.update_available) updBtn('u-node', dshdT(r.node.installed ? 'update' : 'install'), 'node', false);
  if (r.pwsh && r.pwsh.update_available) updBtn('u-pwsh', dshdT(r.pwsh.installed ? 'update' : 'install'), 'pwsh', false);
  if (r.npm && r.npm.update_available) updBtn('u-npm', dshdT('update'), 'npm', false);
  if (r.app && r.app.update_available) updBtn('u-app', dshdT('updateApp'), 'app', false);
  const any = (r.dsh && r.dsh.update_available) || (r.node && r.node.update_available) || (r.pwsh && r.pwsh.update_available) || (r.npm && r.npm.update_available) || (r.app && r.app.update_available);
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
  applyTruncationTips(body);
}
let lastProgress = '';
function renderProgress(message) {
  if (openKind !== 'check') return;
  if (!message || message === lastProgress) return;
  lastProgress = message;
  const body = $('body');
  const line = body.querySelector('.msg');
  if (line) { line.innerHTML = '<span class="spin" aria-hidden="true"></span>' + esc(message); return; }
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
  document.querySelectorAll('.uprow .dshd-btn').forEach((button) => {
    if (p.ok) {
      button.disabled = true;
      if (button.textContent === dshdT('processing')) button.textContent = dshdT('completed');
    } else {
      button.disabled = false;
      button.textContent = button.dataset.label || dshdT('retry');
    }
  });
  if (line) {
    line.className = 'msg ' + (p.ok ? 'success' : 'error');
    line.setAttribute('role', p.ok ? 'status' : 'alert');
    line.textContent = text;
  }
  else {
    const div = document.createElement('div');
    div.className = 'msg ' + (p.ok ? 'success' : 'error');
    div.setAttribute('role', p.ok ? 'status' : 'alert');
    div.textContent = text;
    body.append(div);
  }
}

// —— 关于 ——
const ABOUT_LOGO =
  '<svg xmlns="http://www.w3.org/2000/svg" width="48" height="48" viewBox="0 0 64 64">' +
  '<rect x="0" y="0" width="64" height="64" rx="14.08" fill="#4D6BFE"/>' +
  '<g transform="translate(12.16 12.16) scale(0.7936)">' +
  '<path d="M48.8354 10.0479C48.3232 9.79199 48.1025 10.2798 47.8032 10.5278C47.7007 10.6079 47.6143 10.7119 47.5273 10.8076C46.7793 11.624 45.9048 12.1597 44.7622 12.0957C43.0923 12 41.666 12.5356 40.4058 13.8398C40.1377 12.2319 39.2476 11.272 37.8926 10.6558C37.1836 10.3359 36.4668 10.0156 35.9702 9.31982C35.6235 8.82373 35.5293 8.27197 35.356 7.72754C35.2456 7.3999 35.1353 7.06396 34.7651 7.00781C34.3633 6.94385 34.2056 7.2876 34.0479 7.57568C33.418 8.75195 33.1733 10.0479 33.1973 11.3599C33.2524 14.312 34.4736 16.6641 36.8999 18.3359C37.1758 18.5278 37.2466 18.7197 37.1597 19C36.9946 19.5757 36.7974 20.1357 36.624 20.7119C36.5137 21.0801 36.3486 21.1597 35.9624 21C34.6309 20.4321 33.481 19.5918 32.4644 18.5757C30.7393 16.8721 29.1792 14.9917 27.2334 13.52C26.7764 13.1758 26.3193 12.856 25.8467 12.5518C23.8618 10.584 26.1069 8.96777 26.627 8.77588C27.1704 8.57568 26.8159 7.8877 25.0591 7.896C23.3022 7.90381 21.6953 8.50391 19.647 9.30371C19.3477 9.42383 19.0322 9.51172 18.7095 9.58398C16.8501 9.22363 14.9199 9.14355 12.9033 9.37598C9.10596 9.80762 6.07275 11.6396 3.84326 14.7681C1.16455 18.5278 0.53418 22.7998 1.30664 27.2559C2.11768 31.9521 4.46582 35.8398 8.07373 38.8799C11.8159 42.0322 16.1255 43.5762 21.041 43.2803C24.0269 43.104 27.3516 42.6963 31.1016 39.4561C32.0469 39.936 33.0396 40.1279 34.686 40.272C35.9546 40.3921 37.1758 40.208 38.1211 40.0078C39.6021 39.688 39.4995 38.2881 38.9639 38.0322C34.623 35.9678 35.5762 36.8081 34.71 36.1279C36.9155 33.4639 40.2402 30.6958 41.54 21.728C41.6426 21.0161 41.5557 20.5679 41.54 19.9917C41.5322 19.6396 41.6108 19.5039 42.0049 19.4639C43.0923 19.3359 44.1479 19.0317 45.1167 18.4878C47.9292 16.9199 49.064 14.3438 49.3315 11.2559C49.3711 10.7837 49.3237 10.2959 48.8354 10.0479ZM24.3262 37.8398C20.1196 34.4639 18.0791 33.3521 17.2358 33.3999C16.4482 33.4482 16.5898 34.3682 16.7632 34.9678C16.9443 35.5601 17.1812 35.9683 17.5117 36.4878C17.7402 36.832 17.8979 37.3442 17.2832 37.728C15.9282 38.584 13.5728 37.4399 13.4624 37.3838C10.7207 35.7358 8.42822 33.5601 6.81348 30.584C5.25342 27.7197 4.34766 24.6479 4.19775 21.3677C4.1582 20.5757 4.38672 20.2959 5.15869 20.1519C6.17529 19.96 7.22314 19.9199 8.23926 20.0718C12.5327 20.7119 16.1885 22.6719 19.2529 25.7759C21.002 27.5439 22.3252 29.6558 23.6885 31.7202C25.1377 33.9121 26.6978 36 28.6831 37.7119C29.3843 38.312 29.9434 38.7681 30.479 39.104C28.8643 39.2881 26.1699 39.3281 24.3262 37.8398ZM26.3433 24.6001C26.3433 24.248 26.6191 23.9678 26.9658 23.9678C27.0444 23.9678 27.1152 23.9839 27.1782 24.0078C27.2651 24.04 27.3438 24.0879 27.4067 24.1602C27.5171 24.272 27.5801 24.4321 27.5801 24.6001C27.5801 24.9521 27.3042 25.2319 26.9575 25.2319C26.6108 25.2319 26.3433 24.9521 26.3433 24.6001ZM32.6064 27.8799C32.2046 28.0479 31.8027 28.1919 31.4165 28.208C30.8179 28.2397 30.1641 27.9922 29.8096 27.688C29.2583 27.2158 28.8643 26.9521 28.6987 26.1279C28.6279 25.7759 28.6675 25.2319 28.7305 24.9199C28.8721 24.248 28.7144 23.8159 28.2495 23.4238C27.8716 23.104 27.3911 23.0161 26.8633 23.0161C26.666 23.0161 26.4849 22.9277 26.3511 22.856C26.1304 22.7441 25.9492 22.4639 26.1226 22.1201C26.1777 22.0078 26.4458 21.7358 26.5088 21.688C27.2256 21.272 28.0527 21.4077 28.8169 21.7197C29.5259 22.0161 30.0615 22.5601 30.834 23.3281C31.6216 24.2559 31.7632 24.5117 32.2124 25.208C32.5669 25.752 32.8901 26.312 33.1104 26.9521C33.2446 27.3521 33.0713 27.6802 32.6064 27.8799Z" fill="#FFFFFF"/>' +
  '</g>' +
  '</svg>';
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
    '<div class="update-prompt">' +
    '<div class="up-copy dshd-scroll">' +
    '<div class="up-desc">' + esc(headline) + '</div>' +
    viewLink +
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
    button.disabled = true;
    button.setAttribute('aria-busy', 'true');
    invoke('app_dialog_update', { which }).catch(() => {
      button.disabled = false;
      button.removeAttribute('aria-busy');
    });
  });
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
  chart: '<svg viewBox="0 0 24 24" focusable="false" aria-hidden="true"><path d="M4 19V9"></path><path d="M10 19V5"></path><path d="M16 19v-7"></path><path d="M22 19H2"></path></svg>',
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
  if (activeKind === 'update-prompt') {
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
}
function renderCurrent(opts) {
  const k = openKind;
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
  else if (k === 'plugins') renderPlugins();
  else if (k === 'settings') renderSettings();
}
function playViewTransition() {
  const content = $('body');
  content.classList.remove('view-enter');
  void content.offsetWidth;
  content.classList.add('view-enter');
}
function navigateTo(kind) {
  if (!currentOpen || openKind === kind) return;
  const item = NAV_ITEMS.find((candidate) => candidate.kind === kind);
  if (!navCapability(item).enabled) return;
  // 离开用量页：卸载账户推送监听并复位刷新态（避免后台事件打靶到已切走的页）
  if (openKind === 'usage') dropUsageAccountsListener();
  openKind = kind;
  currentOpen = { kind, title: '', initial: currentOpen.initial };
  document.body.classList.toggle('update-prompt-mode', kind === 'update-prompt');
  renderNav(kind);
  applyTruncationTips(document);
  renderCurrent({ triggerCheck: true });
  $('body').scrollTop = 0;
  playViewTransition();
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
  document.body.classList.toggle('update-prompt-mode', p.kind === 'update-prompt');
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
listen('app-dialog-open', (e) => applyOpen(e.payload)).catch(() => {});
listen('dsh-status', (e) => {
  if (!currentOpen || !e.payload) return;
  currentOpen.initial = currentOpen.initial || {};
  currentOpen.initial.service_mode = e.payload.service_mode || 'none';
  currentOpen.initial.service_ready = e.payload.phase === 'ready'
    && (e.payload.service_mode === 'managed' || e.payload.service_mode === 'external');
  renderNav(openKind);
}).catch(() => {});
document.addEventListener('visibilitychange', async () => {
  if (document.visibilityState === 'visible') {
    // 不做动效重放：该 WebView 对“隐藏→显示”的可见性事件不可靠，
    // 迟到重放会中途重启动画造成抖动（托盘菜单的教训）；入场动效
    // 由 applyOpen 在预渲染时播放，show 后可见部分自然呈现
    try { applyOpen(await invoke('app_dialog_get')); } catch (e) {}
  }
});
listen('update-result', (e) => {
  if (openKind === 'check') renderCheckResult(e.payload);
}).catch(() => {});
listen('update-progress', (e) => {
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
setInterval(async () => {
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
}, 1500);

$('btn-x').addEventListener('click', close);
// 用量页刷新（唯一刷新入口）：先触发 Rust 账户后台全量刷新（失败静默），
// 再整页重载；转圈至 usage-accounts-updated 事件到达（至少 900ms 防抖），
// 命令失败立即收尾，事件丢失或后台异常时 30s 超时兜底，期间禁用防连点。
let usageRefreshBusy = false;
let usageRefreshTimer = null;
let usageRefreshStart = 0;
function setUsageRefreshBusy(busy) {
  usageRefreshBusy = busy;
  const button = $('btn-refresh');
  if (!button) return;
  button.classList.toggle('refreshing', busy);
  button.disabled = busy;
  button.toggleAttribute('aria-busy', busy);
}
function finishUsageRefresh() {
  if (!usageRefreshBusy) return;
  clearTimeout(usageRefreshTimer);
  usageRefreshTimer = null;
  const wait = Math.max(0, 900 - (Date.now() - usageRefreshStart));
  setTimeout(() => setUsageRefreshBusy(false), wait);
}
$('btn-refresh').addEventListener('click', () => {
  if (usageRefreshBusy) return;
  setUsageRefreshBusy(true);
  usageRefreshStart = Date.now();
  clearTimeout(usageRefreshTimer);
  usageRefreshTimer = setTimeout(finishUsageRefresh, 30000);
  invoke('usage_accounts_refresh').catch(() => finishUsageRefresh());
  renderUsagePage();
});
window.addEventListener('keydown', (e) => {
  if (e.key === 'Escape') close();
});
