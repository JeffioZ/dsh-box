const tauri = window.__TAURI__;
const invoke = tauri ? tauri.core.invoke : async () => null;
const listen = tauri ? tauri.event.listen : async () => () => {};

dshdApplyI18n();

let openKind = '';
let currentOpen = null;
let lastBalanceData = null;
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

// —— 余额 ——
function renderBalance(data) {
  // 轮询响应晚到时（期间已切换/关闭弹窗）不写已重置的 DOM
  if (openKind !== 'balance') return;
  lastBalanceData = data;
  const body = $('body');
  if (!data) {
    body.innerHTML = '<div class="msg" role="status" aria-live="polite"><span class="spin" aria-hidden="true"></span>' + dshdT('queryingBalance') + '</div>';
    // 无底部操作区（dsh 设置弹窗无 footer）
    return;
  }
  if (!data.ok || !data.balances || !data.balances.length) {
    body.innerHTML = '<div class="msg error" role="alert">' + esc((data && data.error) || dshdT('balanceQueryFailed')) + '</div>';
    // 无底部操作区（dsh 设置弹窗无 footer）
    return;
  }
  const b = data.balances[0];
  const fmt = (v) => esc(dshdBalanceValue(v));
  const parts = '<div class="balance-parts">' +
    '<div class="balance-part"><span data-trunc-tip>' + dshdT('toppedUpBalance') + '</span><b data-trunc-tip>' + cur(b.currency) + fmt(b.topped_up_balance) + '</b></div>' +
    '<div class="balance-part"><span data-trunc-tip>' + dshdT('grantedBalance') + '</span><b data-trunc-tip>' + cur(b.currency) + fmt(b.granted_balance) + '</b></div>' +
    '</div>';
  const upd = data.updated_at
    ? '<span class="upd">' + dshdT('updatedAt', { time: new Date(data.updated_at * 1000).toLocaleTimeString(dshdLocale(), { hour: '2-digit', minute: '2-digit' }) }) + '</span>'
    : '';
  const dot = data.is_available ? '' : ' warn';
  const st = data.is_available ? dshdT('accountAvailable') : dshdT('accountUnavailable');
  body.innerHTML =
    '<div class="center-wrap">' +
    '<div class="bal-api">DeepSeek API</div>' +
    '<div class="big">' + cur(b.currency) + fmt(b.total_balance) +
    '<span class="cur">' + esc(b.currency) + '</span></div>' + parts +
    '<div class="status"><span class="dot' + dot + '"></span>' + st + upd + '</div>' +
    '</div>';
  // 右上角关闭已够，右下角不再放纯关闭按钮（dsh 原生设置同）
  // 无底部操作区（dsh 设置弹窗无 footer）
  applyTruncationTips($('body'));
}

// —— 检查更新 ——
function renderCheckLoading(message) {
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
    const v = r.npm.installed;
    const hint = r.npm.latest_error ? ' data-tip-extra="' + esc(r.npm.latest_error) + '"' : '';
    const label = !r.npm.latest ? dshdT('versionServiceUnavailable')
      : (r.npm.update_available
        ? ''
        : dshdT('upToDate'));
    html += '<div class="uprow"><div class="info"><div class="name">npm</div>' +
      '<div class="ver" data-trunc-tip' + hint + '>' + (r.npm.update_available
        ? verHtml(v, r.npm.latest || dshdT('latestVersion'), true)
        : esc(v) + '<span class="v-ok">' + label + '</span>') + '</div></div>' +
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
  if (!any && !r.error && r.app && (r.dsh || r.node || r.pwsh || r.npm)) {
    const message = document.createElement('div');
    message.className = 'msg';
    message.setAttribute('role', 'status');
    message.textContent = dshdT('noUpdates');
    body.append(message);
  }
  // 右上角关闭已够，右下角不再放纯关闭按钮（dsh 原生设置同）
  // 无底部操作区（dsh 设置弹窗无 footer）
  applyTruncationTips(body);
}
let lastProgress = '';
function renderProgress(message) {
  if (openKind !== 'check') return;
  if (!message || message === dshdT('checkingUpdates') || message === lastProgress) return;
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

// —— 会话统计详情 ——
const STATS_HINTS = {
  counts: 'statsCountsHint', durations: 'statsDurationsHint', speeds: 'statsSpeedsHint',
  cache: 'statsCacheHint', tokens: 'statsTokensHint',
};
let statsDetailSeq = 0;
async function renderStatsDetail(initial) {
  const seq = ++statsDetailSeq;
  const body = $('body');
  body.innerHTML = '<div class="msg" role="status"><span class="spin" aria-hidden="true"></span>' + dshdT('statsLoading') + '</div>';
  try {
    const payload = await invoke('session_stats_get');
    if (openKind !== 'stats' || seq !== statsDetailSeq) return;
    const groups = payload && Array.isArray(payload.groups) ? payload.groups : [];
    if (!groups.length) {
      body.innerHTML = '<div class="msg">' + esc(dshdT('statsEmpty')) + '</div>';
      return;
    }
    const selected = initial && initial.group;
    const details = new Map(((payload && payload.details) || []).map((item) => [item.key, item.lines || []]));
    body.innerHTML = '<div class="stats-detail-grid">' + groups.map((group) => {
      const lines = details.get(group.key) || [];
      return '<section class="stats-detail-card' + (selected === group.key ? ' selected' : '') + '">' +
        '<h3>' + esc(dshdT(STATS_HINTS[group.key] || 'statsRegion')) + '</h3>' +
        '<div class="stats-detail-value">' + esc(group.text) + '</div>' +
        (lines.length ? '<div class="stats-detail-lines">' + lines.map((line) => '<span>' + esc(line) + '</span>').join('') + '</div>' : '') +
        '</section>';
    }).join('') + '</div>';
    const selectedCard = body.querySelector('.stats-detail-card.selected');
    if (selectedCard) selectedCard.scrollIntoView({ block: 'nearest' });
  } catch (e) {
    if (openKind === 'stats' && seq === statsDetailSeq) {
      body.innerHTML = '<div class="msg error">' + esc(String(e)) + '</div>';
    }
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
  const viewLink = p && p.release_url
    ? '<a class="up-link" id="up-view-release" href="' + esc(p.release_url) +
      '" rel="noopener noreferrer">' + dshdT('viewReleaseNotes') + ' ↗</a>'
    : '';
  body.innerHTML =
    '<div class="update-prompt">' +
    '<div class="up-desc">' + esc(headline) + '</div>' +
    viewLink +
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
  { kind: 'stats', label: 'navStats', icon: 'chart', capability: 'managed-ready' },
  { kind: 'balance', label: 'navBalance', icon: 'wallet', capability: 'local' },
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
  stats: 'statsTitle', balance: 'balanceTitle', check: 'checkUpdates', plugins: 'pluginsTitle',
  settings: 'settingsTitle', about: 'about',
};
const NAV_ICONS = {
  chart: '<svg viewBox="0 0 24 24" focusable="false" aria-hidden="true"><path d="M4 19V9"></path><path d="M10 19V5"></path><path d="M16 19v-7"></path><path d="M22 19H2"></path></svg>',
  wallet: '<svg viewBox="0 0 24 24" focusable="false" aria-hidden="true"><path d="M21 7H5a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h13"></path><path d="M3 5v14a2 2 0 0 0 2 2h16V7"></path><path d="M16 13h3"></path></svg>',
  download: '<svg viewBox="0 0 24 24" focusable="false" aria-hidden="true"><path d="M12 3v12"></path><path d="m7 10 5 5 5-5"></path><path d="M4 21h16"></path></svg>',
  puzzle: '<svg viewBox="0 0 24 24" focusable="false" aria-hidden="true"><path d="M8.5 3H5a2 2 0 0 0-2 2v3.5a2.5 2.5 0 1 1 0 5V19a2 2 0 0 0 2 2h3.5a2.5 2.5 0 1 1 5 0H19a2 2 0 0 0 2-2v-5.5a2.5 2.5 0 1 1 0-5V5a2 2 0 0 0-2-2h-5.5a2.5 2.5 0 1 1-5 0Z"></path></svg>',
  gear: '<svg viewBox="0 0 24 24" focusable="false" aria-hidden="true"><circle cx="12" cy="12" r="3"></circle><path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 1 1-2.83 2.83l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 1 1-4 0v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 1 1-2.83-2.83l.06-.06a1.65 1.65 0 0 0 .33-1.82 1.65 1.65 0 0 0-1.51-1H3a2 2 0 1 1 0-4h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 1 1 2.83-2.83l.06.06a1.65 1.65 0 0 0 1.82.33H9a1.65 1.65 0 0 0 1-1.51V3a2 2 0 1 1 4 0v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 1 1 2.83 2.83l-.06.06a1.65 1.65 0 0 0-.33 1.82V9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 1 1 0 4h-.09a1.65 1.65 0 0 0-1.51 1z"></path></svg>',
  info: '<svg viewBox="0 0 24 24" focusable="false" aria-hidden="true"><circle cx="12" cy="12" r="9"></circle><path d="M12 11v6"></path><path d="M12 7.5v.01"></path></svg>',
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
  if (k === 'stats') renderStatsDetail((currentOpen && currentOpen.initial) || {});
  else if (k === 'balance') {
    renderBalance(lastBalanceData);
    // 导航进入余额页：后台触发一次刷新——打开即显示缓存（有则），
    // 新数据就绪后由轮询替换；避免"切换到余额一直查询中"（此前仅
    // 直接打开才触发查询，导航切换不触发）
    if (opts && opts.triggerBalance) invoke('app_dialog_refresh_balance').catch(() => {});
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
  if (kind !== 'balance') setBalanceRefreshBusy(false);
  openKind = kind;
  currentOpen = { kind, title: '', initial: currentOpen.initial };
  document.body.classList.toggle('update-prompt-mode', kind === 'update-prompt');
  renderNav(kind);
  // 标题已由 renderNav 写入导航顶部 nav-title（#title 元素已随结构重构移除，
  // 此前此处 $('title') 抛 TypeError 导致 renderCurrent 不执行——导航切换失效的根因）
  applyTruncationTips(document);
  $('btn-refresh').classList.toggle('hidden', kind !== 'balance');
  renderCurrent({ triggerCheck: true, triggerBalance: true });
  $('body').scrollTop = 0;
  // 只提示右侧内容已切换；整窗保持稳定，快速连续点击会直接重启动画。
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
// 余额刷新按钮：转圈至少一圈；新数据经轮询到达后停止
let balanceRefreshStart = 0;
let pwshPromptShown = false;
// 更新执行中：期间“更新/安装”按钮保持禁用（结果行重建会重置禁用态）
let updateRunning = false;
function setBalanceRefreshBusy(busy) {
  const button = $('btn-refresh');
  button.classList.toggle('refreshing', busy);
  button.disabled = busy;
  button.toggleAttribute('aria-busy', busy);
}
$('btn-refresh').addEventListener('click', () => {
  const button = $('btn-refresh');
  if (button.classList.contains('refreshing')) return;
  setBalanceRefreshBusy(true);
  balanceRefreshStart = Date.now();
  invoke('app_dialog_refresh_balance').catch(() => {
    setBalanceRefreshBusy(false);
  });
});
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
  balanceStamp = '';
  checkStamp = '';
  lastResultKey = '';
  lastProgress = '';
  pendingRefreshData = null;
  pwshPromptShown = false;
  setBalanceRefreshBusy(false);
  // 刷新按钮仅余额弹窗显示
  $('btn-refresh').classList.toggle('hidden', p.kind !== 'balance');
  if (document.activeElement && document.activeElement !== document.body) document.activeElement.blur();
  currentOpen = p;
  openKind = p.kind;
  document.body.classList.toggle('update-prompt-mode', p.kind === 'update-prompt');
  applyTruncationTips(document);
  renderNav(p.kind);
  // Rust 打开时已按 kind 预置状态（open_check 触发检查等），此处只渲染
  renderCurrent();
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
  lastBalanceData = null;
  lastCheckResult = null;
  lastResultKey = '';
  lastProgress = '';
  openKind = '';
  document.body.classList.remove('update-prompt-mode');
  balanceStamp = '';
  checkStamp = '';
  pendingRefreshData = null;
  pwshPromptShown = false;
  updateRunning = false;
  renderNav(openKind);
  setBalanceRefreshBusy(false);
  pluginApplyStamp = '';
  statsDetailSeq += 1;
  // 标题由 renderNav 写导航顶部；此处仅清空残留
  $('body').innerHTML = '';
  // 无底部操作区（dsh 设置弹窗无 footer）
  const nav = $('nav');
  if (nav) nav.innerHTML = '';
};
listen('app-dialog-open', (e) => applyOpen(e.payload));
listen('dsh-status', (e) => {
  if (!currentOpen || !e.payload) return;
  currentOpen.initial = currentOpen.initial || {};
  currentOpen.initial.service_mode = e.payload.service_mode || 'none';
  currentOpen.initial.service_ready = e.payload.phase === 'ready'
    && (e.payload.service_mode === 'managed' || e.payload.service_mode === 'external');
  renderNav(openKind);
});
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
});
listen('update-progress', (e) => {
  if (e.payload && e.payload.message) renderProgress(e.payload.message);
});
window.addEventListener('dshd-language-changed', () => {
  if (!currentOpen) return;
  applyTruncationTips(document);
  renderNav(openKind);
  renderCurrent();
});
// —— 轮询拉取（事件通道对该窗口不可靠，此为主通道）——
let balanceStamp = '';
let checkStamp = '';
let lastResultKey = '';
let pendingRefreshData = null;
setInterval(async () => {
  try {
    if (openKind === 'balance') {
      const data = await invoke('app_dialog_balance_get');
      // await 期间可能已重置/切换：晚到响应不写已重置的 DOM 与状态
      if (openKind !== 'balance') return;
      const key = JSON.stringify(data || null);
      const button = $('btn-refresh');
      const refreshing = button.classList.contains('refreshing');
      // 本轮是否已按新鲜数据渲染：满圈同拍时防止随后用更早缓冲的旧数据
      // 二次渲染/回写旧值
      let renderedFresh = false;
      if (key !== balanceStamp) {
        if (!refreshing || Date.now() - balanceRefreshStart >= 900) {
          balanceStamp = key;
          renderBalance(data);
          renderedFresh = true;
        } else {
          // 转圈未满一圈：暂存新数据，待满圈后与停止动画一起应用——
          // 内容先更新、动画后停会显得“突然停下”（与浮层刷新节奏统一）
          pendingRefreshData = data;
        }
      }
      if (refreshing && Date.now() - balanceRefreshStart >= 900) {
        setBalanceRefreshBusy(false);
        if (pendingRefreshData) {
          if (!renderedFresh) {
            balanceStamp = JSON.stringify(pendingRefreshData);
            renderBalance(pendingRefreshData);
          }
          pendingRefreshData = null;
        }
      }
    } else if (openKind === 'check') {
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
window.addEventListener('keydown', (e) => {
  if (e.key === 'Escape') close();
});
