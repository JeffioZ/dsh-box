// dsh 桌面端 —— 启动画面逻辑
// 通过 Tauri IPC 与 Rust 后端通信：事件 dsh-status / update-progress / update-result，
// 命令 get_status / retry_boot / quit / open_logs / check_updates / apply_updates

const $ = (id) => document.getElementById(id);

const PHASE_KEYS = {
  'starting': 'starting',
  'installing-node': 'installingNode',
  'installing-dsh': 'installingDsh',
  'starting-server': 'startingServer',
  'ready': 'ready',
  'error': 'startupFailed',
};

let lastStatusPayload = null;
let lastUpdateResult = null;
// 更新结果区只在两种情况下显示：用户手动点了“检查更新”，
// 或静默检查发现 dsh 有新版可更——静默检查的“已是最新/检查失败”
// 在安装进行中弹出来纯属噪音
let updateCheckRequested = false;

function phaseText(phase) {
  return PHASE_KEYS[phase] ? dshdT(PHASE_KEYS[phase]) : phase;
}

function setStatus(phase, message, detail) {
  const text = $('status-text');
  const spinner = $('spinner');
  const fill = $('bar-fill');
  // 终态（就绪/失败）用固定文案；其余阶段优先显示后端动态消息（如安装计时、端口回退）
  let display;
  if (phase === 'ready' || phase === 'error') {
    display = phaseText(phase);
  } else {
    display = (message && message.length) ? message : phaseText(phase);
  }
  text.textContent = display;
  $('status-detail').textContent = detail || '';
  if (phase === 'ready') {
    spinner.classList.add('hidden');
    fill.classList.add('done');
    // 整体淡出后由后端 navigate 进入 dsh 界面（与 WebView 背景色衔接，消除白闪）
    document.body.classList.add('fade-out');
  }
  else if (phase === 'error') { spinner.classList.add('hidden'); fill.classList.add('err'); }
  else {
    document.body.classList.remove('fade-out');
    spinner.classList.remove('hidden');
    fill.classList.remove('done', 'err');
  }
}

function showError(message) {
  setStatus('error');
  $('error-msg').textContent = message || dshdT('unknownError');
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
  if (payload.port) parts.push(dshdT('port', { port: payload.port }));
  // 字段缺失时保留已显示内容（防御：事件载荷异常时不清空 footer）
  if (parts.length === 0) return;
  el.textContent = parts.join(' · ');
}

function renderStatus(payload) {
  lastStatusPayload = payload;
  setStatus(payload.phase, payload.message, payload.detail);
  renderVersions(payload);
  // 多步骤引导：安装 Node → 安装 dsh → 启动服务，显示"第 x 步 / 共 3 步"
  const STEP_OF = { 'installing-node': 1, 'installing-dsh': 2, 'starting-server': 3 };
  const step = STEP_OF[payload.phase];
  const stepLine = step ? dshdT('stepOf', { n: step, total: 3 }) : '';
  $('status-detail').textContent = stepLine
    ? (stepLine + (payload.detail ? ' · ' + payload.detail : ''))
    : (payload.detail || '');
  const progressBar = $('progress-bar');
  const fill = $('bar-fill');
  // 终态清除 inline width，避免覆盖 .done/.err 的 100%
  if (payload.phase === 'ready' || payload.phase === 'error') {
    fill.classList.remove('determinate');
    fill.style.width = '';
    if (payload.phase === 'ready') progressBar.setAttribute('aria-valuenow', '100');
    else progressBar.removeAttribute('aria-valuenow');
  } else if (typeof payload.progress === 'number') {
    // 确定进度（如 Node 下载百分比）
    fill.classList.add('determinate');
    fill.style.width = Math.max(2, Math.min(100, payload.progress)) + '%';
    progressBar.setAttribute('aria-valuenow', String(Math.max(0, Math.min(100, payload.progress))));
  } else {
    fill.classList.remove('determinate');
    fill.style.width = '';
    progressBar.removeAttribute('aria-valuenow');
  }
  if (payload.phase === 'error') showError(payload.message);
  else hideError();
}

// —— 首次使用配置 ——

let onboardingSaving = false;

async function initOnboarding() {
  try {
    const st = await window.__TAURI__.core.invoke('get_onboarding_state');
    if (!st || !st.needs_onboarding) return;
    document.querySelectorAll('input[name="ob-lang"]').forEach((r) => {
      r.checked = r.value === (st.language === 'en' ? 'en' : 'zh-CN');
    });
    document.querySelectorAll('input[name="ob-theme"]').forEach((r) => {
      r.checked = r.value === (st.theme || 'system');
    });
    $('ob-autostart').checked = !!st.autostart;
    $('ob-box').classList.remove('hidden');
    // onboarding 模式下聚焦配置面板：隐藏启动状态区
    $('status').classList.add('hidden');
    // 回报面板已显示：boot 等待切换为无限等待（无 60 秒兜底）
    window.__TAURI__.core.invoke('onboarding_shown').catch(() => {});
    // 语言/主题实时预览：选中即生效（自然过渡由分段控件的选中动画承接；
    // 保存时才持久化——主题只切窗口不写 settings）
    document.querySelectorAll('input[name="ob-lang"]').forEach((r) => {
      r.addEventListener('change', () => {
        if (!r.checked) return;
        const lang = r.value === 'en' ? 'en' : 'zh-CN';
        window.dshdSetLanguage && window.dshdSetLanguage(lang);
      });
    });
    document.querySelectorAll('input[name="ob-theme"]').forEach((r) => {
      r.addEventListener('change', () => {
        if (!r.checked) return;
        // 跟随系统：清除强制主题回系统深浅色（preview_theme 处理）
        window.__TAURI__.core.invoke('preview_theme', { theme: r.value }).catch(() => {});
      });
    });
  } catch (e) { /* 后端未就绪时忽略 */ }
}

async function submitOnboarding(skip) {
  if (onboardingSaving) return;
  onboardingSaving = true;
  const box = $('ob-box');
  const start = $('ob-start');
  const skipButton = $('ob-skip');
  start.disabled = true;
  skipButton.disabled = true;
  box.setAttribute('aria-busy', 'true');
  const payload = {
    skip,
    language: (document.querySelector('input[name="ob-lang"]:checked') || {}).value,
    theme: (document.querySelector('input[name="ob-theme"]:checked') || {}).value,
    autostart: $('ob-autostart').checked,
  };
  if (!skip) {
    const key = $('ob-apikey').value.trim();
    if (key) payload.api_key = key;
  }
  try {
    await window.__TAURI__.core.invoke('save_onboarding', { payload });
    $('ob-box').classList.add('hidden');
    $('ob-error').classList.add('hidden');
    // boot 将立即继续：恢复启动状态区显示（保存前为聚焦面板而隐藏）
    $('status').classList.remove('hidden');
  } catch (e) {
    $('ob-error').textContent = dshdT('saveFailed') + ': ' + e;
    $('ob-error').classList.remove('hidden');
  } finally {
    onboardingSaving = false;
    start.disabled = false;
    skipButton.disabled = false;
    box.removeAttribute('aria-busy');
  }
}

function renderUpdate(result) {  lastUpdateResult = result;
  const box = $('update-box');
  const line = $('update-text');
  const applyBtn = $('btn-update-apply');
  const hasUpdate = !!(result && result.dsh && result.dsh.update_available);
  if (!updateCheckRequested && !hasUpdate) {
    box.classList.add('hidden');
    return;
  }
  box.classList.remove('hidden');
  $('btn-update-check').disabled = false;
  applyBtn.disabled = false;
  if (!result || result.error) {
    line.textContent = result && result.error
      ? dshdT('checkFailed') + ': ' + result.error
      : dshdT('checkFailed');
    applyBtn.classList.add('hidden');
    return;
  }
  const d = result.dsh;
  if (d && d.update_available) {
    line.textContent = dshdT('dshUpdateAvailable', { latest: d.latest, current: d.installed });
    applyBtn.classList.remove('hidden');
  } else if (d) {
    line.textContent = dshdT('dshUpToDate', { version: d.installed }) +
      (result.node && result.node.latest_lts ? ' · Node.js ' + dshdT('latestLts', { version: result.node.latest_lts }) : '');
  }
}

function bind() {
  $('btn-retry').addEventListener('click', async () => {
    const button = $('btn-retry');
    button.disabled = true;
    hideError();
    setStatus('starting');
    try {
      await window.__TAURI__.core.invoke('retry_boot');
    } catch (e) {
      showError(dshdT('retry') + ': ' + e);
    } finally {
      button.disabled = false;
    }
  });
  $('btn-logs').addEventListener('click', async () => {
    try {
      await window.__TAURI__.core.invoke('open_logs');
    } catch (e) {
      showError(dshdT('openLogs') + ': ' + e);
    }
  });
  $('btn-quit').addEventListener('click', () => window.__TAURI__.core.invoke('quit'));
  $('btn-update-check').addEventListener('click', async () => {
    const button = $('btn-update-check');
    button.disabled = true;
    updateCheckRequested = true;
    // 立即显示“检查更新中…”，结果到达后 renderUpdate 填充
    $('update-box').classList.remove('hidden');
    $('update-text').textContent = dshdT('checkingUpdates');
    try {
      await window.__TAURI__.core.invoke('check_updates');
    } catch (e) {
      $('update-text').textContent = dshdT('checkFailed') + ': ' + e;
      button.disabled = false;
    }
  });
  $('btn-update-apply').addEventListener('click', async () => {
    const button = $('btn-update-apply');
    button.disabled = true;
    $('update-text').textContent = dshdT('updateDshWait');
    try {
      await window.__TAURI__.core.invoke('apply_updates', { which: 'dsh' });
    } catch (e) {
      $('update-text').textContent = dshdT('updateFailed', { message: e });
      button.disabled = false;
    }
  });
  $('ob-start').addEventListener('click', () => submitOnboarding(false));
  $('ob-skip').addEventListener('click', () => submitOnboarding(true));
}

async function init() {
  dshdApplyI18n();
  bind();
  initOnboarding();
  window.addEventListener('dshd-language-changed', () => {
    if (lastStatusPayload) renderStatus(lastStatusPayload);
    if (lastUpdateResult) renderUpdate(lastUpdateResult);
  });
  // 启动页为内置界面，禁用 WebView2 默认右键菜单
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
