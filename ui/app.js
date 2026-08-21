// dsh 桌面端 —— 启动画面逻辑
// 通过 Tauri IPC 与 Rust 后端通信：事件 dsh-status / update-progress / update-result，
// 命令 get_status / retry_boot / quit / open_logs / check_updates / apply_updates

const $ = (id) => document.getElementById(id);

/// onboarding 面板是否可见（未保存/未跳过）：期间 boot 后台推进但不应
/// 让状态区/更新区/整体淡出干扰配置面板。
const onboardingActive = () => {
  const box = $('ob-box');
  return !!(box && !box.classList.contains('hidden'));
};

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
    // onboarding 未完成时跳过整体淡出：ready 可能在面板显示期间到达
    // （服务复用/并发路径下 get_status 直接返回 Ready），淡出会让配置
    // 面板视觉消失而 boot 仍在等待用户操作；保存后面板隐藏即恢复正常
    if (!onboardingActive()) document.body.classList.add('fade-out');
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
  const box = $('error-box');
  box.classList.remove('hidden');
  // 每次显示重新触发入场动画；聚焦错误框供屏幕阅读器/键盘用户定位错误摘要
  box.classList.remove('reveal');
  void box.offsetWidth;
  box.classList.add('reveal');
  box.focus();
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
  // 语言切换后后端消息快照不会自动刷新（Rust 按旧语言生成）：
  // 纯固定文案的 phase 改用当前语言重译；动态消息（下载/安装进度、
  // 端口回退等）保持后端快照，避免错译。
  const fixedMsg = payload.phase === 'starting-server' ? phaseText('starting-server') : payload.message;
  setStatus(payload.phase, fixedMsg, payload.detail);
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

// —— 模型配置导入（onboarding 面板内的可选区块）——

// 预览后缓存的结果：apiKeyEnv 列表（用于校验导入时 key 的归属）。
let importPreviewRefs = [];

function showImportError(message) {
  const errBox = $('ob-import-error');
  // 重置为纯错误样式（成功态可能把 className 改成了绿色 ob-import-ok）
  errBox.className = 'ob-error';
  errBox.textContent = message || dshdT('unknownError');
  errBox.classList.remove('hidden');
  errBox.focus();
}

function hideImportError() {
  $('ob-import-error').classList.add('hidden');
}

function importRefsFromPreview(preview) {
  const refs = [];
  for (const p of (preview && preview.providers) || []) {
    if (p.api_key_env && !refs.includes(p.api_key_env)) refs.push(p.api_key_env);
  }
  return refs;
}

function renderImportResult(preview) {
  importPreviewRefs = preview.api_key_envs || importRefsFromPreview(preview);
  const summary = $('ob-import-summary');
  const parts = [dshdT('modelImportSummary', { count: preview.providers.length })];
  if (preview.replaces_existing) parts.push(dshdT('modelImportReplaces'));
  summary.textContent = parts.join(' ');

  const keysBox = $('ob-import-keys');
  keysBox.textContent = '';
  const refs = importPreviewRefs;
  if (refs.length === 0) {
    const ok = document.createElement('p');
    ok.className = 'ob-import-ok';
    ok.textContent = dshdT('modelImportNoKeys');
    keysBox.appendChild(ok);
  } else {
    for (const ref of refs) {
      const row = document.createElement('label');
      row.className = 'ob-import-key-row';
      const label = document.createElement('span');
      label.className = 'ob-import-key-label';
      label.textContent = dshdT('modelImportKeyLabel', { ref });
      const input = document.createElement('input');
      input.className = 'ob-input';
      input.type = 'password';
      input.autocomplete = 'off';
      input.spellcheck = false;
      input.dataset.ref = ref;
      input.placeholder = dshdT('modelImportKeyHint');
      row.appendChild(label);
      row.appendChild(input);
      keysBox.appendChild(row);
    }
  }
  $('ob-import-result').classList.remove('hidden');
}

async function previewModelImport() {
  const yaml = $('ob-import').value;
  if (!yaml.trim()) {
    showImportError(dshdT('modelImportEmpty'));
    return;
  }
  hideImportError();
  $('ob-import-result').classList.add('hidden');
  const previewBtn = $('ob-import-preview');
  previewBtn.disabled = true;
  previewBtn.textContent = dshdT('modelImportPreviewing');
  try {
    const preview = await window.__TAURI__.core.invoke('preview_model_import', { yaml });
    renderImportResult(preview);
  } catch (e) {
    showImportError(String(e));
  } finally {
    previewBtn.disabled = false;
    previewBtn.textContent = dshdT('modelImportPreview');
  }
}

async function applyModelImport() {
  const yaml = $('ob-import').value;
  if (!yaml.trim()) {
    showImportError(dshdT('modelImportEmpty'));
    return;
  }
  hideImportError();
  // 收集用户填写的 key（仅声明过的引用）
  const keys = [];
  const filled = new Set();
  const inputs = $('ob-import-keys').querySelectorAll('input[data-ref]');
  for (const input of inputs) {
    const value = input.value.trim();
    if (value) {
      keys.push([input.dataset.ref, value]);
      filled.add(input.dataset.ref);
    }
  }
  // 声明过的凭据引用必须都填 key，否则导入后凭据缺失、路由不可用
  const missing = (importPreviewRefs || []).filter((ref) => !filled.has(ref));
  if (missing.length > 0) {
    showImportError(dshdT('modelImportKeyMissing', { ref: missing.join(', ') }));
    return;
  }
  const applyBtn = $('ob-import-apply');
  applyBtn.disabled = true;
  applyBtn.textContent = dshdT('modelImportApplying');
  try {
    await window.__TAURI__.core.invoke('apply_model_import', { payload: { yaml, keys } });
    // 成功后把结果区切换为成功态
    $('ob-import-result').classList.add('hidden');
    const okBox = $('ob-import-error');
    okBox.textContent = dshdT('modelImportSuccess');
    okBox.className = 'ob-error ob-import-ok';
    okBox.classList.remove('hidden');
  } catch (e) {
    showImportError(String(e));
  } finally {
    applyBtn.disabled = false;
    applyBtn.textContent = dshdT('modelImportApply');
  }
}

function initModelImport() {
  const previewBtn = $('ob-import-preview');
  const applyBtn = $('ob-import-apply');
  const textarea = $('ob-import');
  if (!previewBtn || !applyBtn || !textarea) return;
  previewBtn.addEventListener('click', previewModelImport);
  applyBtn.addEventListener('click', applyModelImport);
  // 文本域变化：收起错误，并隐藏预览结果区，强制用户重新解析——
  // 否则会用旧预览生成的 key 输入框配合新 yaml 提交，触发后端引用不匹配
  textarea.addEventListener('input', () => {
    hideImportError();
    $('ob-import-result').classList.add('hidden');
  });
}

// —— 首次使用配置 ——

let onboardingSaving = false;
let obLangSel = null;
let obThemeSel = null;

// dsh 风格下拉（语言/主题）：trigger + 列表，选中态 ghost-active-fill
function setupSelect(selId, onChange) {
  const root = $(selId);
  if (!root) return null;
  const trigger = root.querySelector('.dshd-select-trigger');
  const list = root.querySelector('.dshd-select-list');
  const valueEl = root.querySelector('.dshd-select-value');
  let value = '';
  const labelOf = (v) => {
    const opt = list.querySelector('[data-value="' + v + '"]');
    return opt ? opt.textContent : v;
  };
  const setValue = (v, fire) => {
    value = v;
    valueEl.textContent = labelOf(v);
    list.querySelectorAll('li').forEach((li) => {
      li.setAttribute('aria-selected', li.dataset.value === v ? 'true' : 'false');
    });
    if (fire && onChange) onChange(v);
  };
  trigger.addEventListener('click', (e) => {
    e.stopPropagation();
    const open = list.hidden;
    list.hidden = !open;
    trigger.setAttribute('aria-expanded', String(open));
  });
  list.querySelectorAll('li').forEach((li) => {
    li.addEventListener('click', () => {
      setValue(li.dataset.value, true);
      list.hidden = true;
      trigger.setAttribute('aria-expanded', 'false');
    });
  });
  document.addEventListener('click', (e) => {
    if (!root.contains(e.target)) {
      list.hidden = true;
      trigger.setAttribute('aria-expanded', 'false');
    }
  });
  return { get: () => value, set: (v) => setValue(v, false) };
}

async function initOnboarding() {
  try {
    const st = await window.__TAURI__.core.invoke('get_onboarding_state');
    if (!st || !st.needs_onboarding) return;
    // 语言/主题下拉：选中即预览（保存时才持久化）
    obLangSel = setupSelect('ob-lang', (lang) => {
      window.dshdSetLanguage && window.dshdSetLanguage(lang);
      window.__TAURI__.core.invoke('preview_language', { language: lang }).catch(() => {});
    });
    obThemeSel = setupSelect('ob-theme', (theme) => {
      window.__TAURI__.core.invoke('preview_theme', { theme }).catch(() => {});
    });
    if (obLangSel) obLangSel.set(st.language === 'en' ? 'en' : 'zh-CN');
    if (obThemeSel) obThemeSel.set(st.theme || 'system');
    $('ob-autostart').checked = !!st.autostart;
    $('ob-box').classList.remove('hidden');
    // 首次显示用 opacity 渐入（重排不可避免，但避免"整块跳出"的突兀感）；
    // status 隐藏前先让面板进场，避免内容闪跳
    $('ob-box').style.opacity = '0';
    requestAnimationFrame(() => {
      $('ob-box').style.transition = 'opacity .18s ease-out';
      $('ob-box').style.opacity = '1';
    });
    // onboarding 模式下聚焦配置面板：隐藏启动状态区
    $('status').classList.add('hidden');
    // 回报面板已显示：boot 等待切换为无限等待（无 60 秒兜底）
    window.__TAURI__.core.invoke('onboarding_shown').catch(() => {});
  } catch (e) { /* 后端未就绪时忽略 */ }
}

async function submitOnboarding(skip) {
  if (onboardingSaving) return;
  const box = $('ob-box');
  const start = $('ob-start');
  const skipButton = $('ob-skip');
  const errBox = $('ob-error');
  // 格式校验（不占用 saving 状态）：非空 key 必须以 sk- 开头，否则提示并
  // 聚焦输入框；留空仍允许（之后在 dsh 设置页配置）
  if (!skip) {
    const key = $('ob-apikey').value.trim();
    if (key && !/^sk-/.test(key)) {
      errBox.textContent = dshdT('apiKeyFormatHint');
      errBox.classList.remove('hidden');
      $('ob-apikey').focus();
      return;
    }
  }
  onboardingSaving = true;
  start.disabled = true;
  skipButton.disabled = true;
  box.setAttribute('aria-busy', 'true');
  const payload = {
    skip,
    language: obLangSel ? obLangSel.get() : 'zh-CN',
    theme: obThemeSel ? obThemeSel.get() : 'system',
    autostart: $('ob-autostart').checked,
  };
  if (!skip) {
    const key = $('ob-apikey').value.trim();
    if (key) payload.api_key = key;
  }
  try {
    await window.__TAURI__.core.invoke('save_onboarding', { payload });
    $('ob-box').classList.add('hidden');
    errBox.classList.add('hidden');
    // 用最新状态 + 当前语言刷新状态区。注意：快照的 message/detail 是
    // 语言切换前生成的 Rust 旧语言文本（首次安装场景保存时 boot 仍在
    // 安装/启动阶段），直接显示会"语言不跟随"——剥离后由 phaseText/
    // stepLine 按当前语言重译固定文案，进度数字与语言无关保留。
    // 失败时回退给 Ready 事件自然刷新
    try {
      const st = await window.__TAURI__.core.invoke('get_status');
      renderStatus({ ...st, message: '', detail: '' });
    } catch (e) { /* 忽略：后续事件到达会刷新 */ }
    // boot 将立即继续：恢复启动状态区显示（保存前为聚焦面板而隐藏）
    $('status').classList.remove('hidden');
  } catch (e) {
    errBox.textContent = dshdT('saveFailed') + ': ' + e;
    errBox.classList.remove('hidden');
  } finally {
    onboardingSaving = false;
    start.disabled = false;
    skipButton.disabled = false;
    box.removeAttribute('aria-busy');
  }
}

function renderUpdate(result) {
  lastUpdateResult = result;
  // onboarding 未完成时不显示更新区（静默检查事件不应干扰配置面板）：
  // 快照已存，保存后由后续事件或语言切换重渲染正常展示；
  // 若面板显示前已有结果（服务复用场景），先隐藏避免与面板同卡堆叠
  if (onboardingActive()) {
    $('update-box').classList.add('hidden');
    return;
  }
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
  initModelImport();
}

async function init() {
  dshdApplyI18n();
  bind();
  // 先等 onboarding 面板显示（含磁盘读状态），再拉取/监听状态：
  // 否则服务已就绪时 get_status 先返回 ready，而面板未显示导致
  // onboardingActive()=false → setStatus 触发整体淡出 → 页面透明但
  // boot 仍在等用户操作（白屏卡死）。initOnboarding 内部已 catch。
  await initOnboarding();
  window.addEventListener('dshd-language-changed', () => {
    // 快照的 message/detail 是 Rust 按旧语言生成的文本，重渲染时必须剥离，
    // 由 phaseText/stepLine 按当前语言重译固定文案（进度数字语言无关保留）
    if (lastStatusPayload) renderStatus({ ...lastStatusPayload, message: '', detail: '' });
    if (lastUpdateResult) renderUpdate(lastUpdateResult);
  });
  // 启动页为内置界面，禁用 WebView2 默认右键菜单
  document.addEventListener('contextmenu', (e) => e.preventDefault());
  const { listen } = window.__TAURI__.event;
  await listen('dsh-status', (e) => renderStatus(e.payload));
  await listen('update-result', (e) => renderUpdate(e.payload));
  await listen('update-progress', (e) => {
    if (e.payload && e.payload.message) {
      // onboarding 期间不覆盖更新文案（面板显示时更新区不可见，
      // 且 Rust 文本是旧语言快照，语言切换后不重译）
      if (onboardingActive()) return;
      $('update-text').textContent = e.payload.message;
    }
  });
  try {
    const payload = await window.__TAURI__.core.invoke('get_status');
    renderStatus(payload);
  } catch (e) { /* 后端未就绪时忽略 */ }
}

init();
