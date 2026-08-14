// dsh 桌面端 —— 启动画面逻辑
// 通过 Tauri IPC 与 Rust 后端通信：事件 dsh-status / update-progress / update-result，
// 命令 get_status / retry_boot / quit / open_logs / api_balance / check_updates / apply_updates

const $ = (id) => document.getElementById(id);

const PHASE_TEXT = {
  'starting': '正在启动…',
  'installing-node': '正在准备 Node.js 运行时…',
  'installing-dsh': '正在安装 dsh（首次运行，需要联网）…',
  'starting-server': '正在启动 dsh 服务…',
  'ready': '已就绪，正在进入界面…',
  'error': '启动失败',
};

function setStatus(phase, message, detail) {
  const text = $('status-text');
  const spinner = $('spinner');
  const fill = $('bar-fill');
  // 终态（就绪/失败）用固定文案；其余阶段优先显示后端动态消息（如安装计时、端口回退）
  let display;
  if (phase === 'ready' || phase === 'error') {
    display = PHASE_TEXT[phase] || phase;
  } else {
    display = (message && message.length) ? message : (PHASE_TEXT[phase] || phase);
  }
  text.textContent = display;
  if (detail) $('status-detail').textContent = detail;
  if (phase === 'ready') {
    spinner.classList.add('hidden');
    fill.classList.add('done');
    // 整体淡出后由后端 navigate 进入 dsh 界面（与 WebView 背景色衔接，消除白闪）
    document.body.classList.add('fade-out');
  }
  else if (phase === 'error') { spinner.classList.add('hidden'); fill.classList.add('err'); }
  else { spinner.classList.remove('hidden'); fill.classList.remove('done', 'err'); }
}

function showError(message) {
  setStatus('error');
  $('error-msg').textContent = message || '未知错误';
  $('error-box').classList.remove('hidden');
}

function hideError() {
  $('error-box').classList.add('hidden');
}

function renderVersions(payload) {
  const el = $('versions');
  if (!el) return;
  const parts = [];
  if (payload.dsh_version) parts.push('dsh v' + payload.dsh_version);
  if (payload.node_version) parts.push('Node ' + payload.node_version);
  if (payload.port) parts.push('端口 ' + payload.port);
  el.textContent = parts.join(' · ');
}

function renderStatus(payload) {
  setStatus(payload.phase, payload.message, payload.detail);
  renderVersions(payload);
  const fill = $('bar-fill');
  // 终态清除 inline width，避免覆盖 .done/.err 的 100%
  if (payload.phase === 'ready' || payload.phase === 'error') {
    fill.style.width = '';
  }
  if (typeof payload.progress === 'number') {
    // 确定进度（如 Node 下载百分比）
    fill.classList.add('determinate');
    fill.style.width = Math.max(2, Math.min(100, payload.progress)) + '%';
  } else {
    fill.classList.remove('determinate');
    fill.style.width = '';
  }
  if (payload.phase === 'error') showError(payload.message);
  else hideError();
}

function renderUpdate(result) {
  const box = $('update-box');
  box.classList.remove('hidden');
  const line = $('update-text');
  const applyBtn = $('btn-update-apply');
  if (!result || result.error) {
    line.textContent = result && result.error ? '检查更新失败：' + result.error : '检查更新失败';
    applyBtn.classList.add('hidden');
    return;
  }
  const d = result.dsh;
  if (d && d.update_available) {
    line.textContent = 'dsh 有新版本：' + d.latest + '（当前 ' + d.installed + '）';
    applyBtn.classList.remove('hidden');
  } else if (d) {
    line.textContent = 'dsh 已是最新（' + d.installed + '）' +
      (result.node && result.node.latest_lts ? ' · Node.js LTS ' + result.node.latest_lts : '');
  }
}

function bind() {
  $('btn-retry').addEventListener('click', async () => {
    hideError();
    setStatus('starting');
    try {
      await window.__TAURI__.core.invoke('retry_boot');
    } catch (e) {
      showError('重试失败：' + e);
    }
  });
  $('btn-logs').addEventListener('click', async () => {
    try {
      await window.__TAURI__.core.invoke('open_logs');
    } catch (e) {
      showError('打开日志失败：' + e);
    }
  });
  $('btn-quit').addEventListener('click', () => window.__TAURI__.core.invoke('quit'));
  $('btn-update-check').addEventListener('click', async () => {
    $('update-text').textContent = '检查更新中…';
    try {
      await window.__TAURI__.core.invoke('check_updates');
    } catch (e) {
      $('update-text').textContent = '检查更新失败：' + e;
    }
  });
  $('btn-update-apply').addEventListener('click', async () => {
    $('update-text').textContent = '正在更新 dsh，请稍候…';
    try {
      await window.__TAURI__.core.invoke('apply_updates', { which: 'dsh' });
    } catch (e) {
      $('update-text').textContent = '更新失败：' + e;
    }
  });
}

async function init() {
  bind();
  // 禁用 WebView2 默认右键菜单（启动页是我们自己的 UI）
  document.addEventListener('contextmenu', (e) => e.preventDefault());
  const { listen } = window.__TAURI__.event;
  await listen('dsh-status', (e) => renderStatus(e.payload));
  await listen('update-result', (e) => renderUpdate(e.payload));
  await listen('update-progress', (e) => {
    if (e.payload && e.payload.message) $('update-text').textContent = e.payload.message;
  });
  try {
    const payload = await window.__TAURI__.core.invoke('get_status');
    renderStatus(payload);
  } catch (e) { /* 后端未就绪时忽略 */ }
}

init();
