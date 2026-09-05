// DSHBox 启动页交互与状态机。
// 通过 Tauri IPC 与 Rust 后端通信：事件 dsh-status / update-progress / update-result，
// 命令 get_status / retry_boot / quit / open_logs / check_updates / apply_updates

const $ = (id) => document.getElementById(id);

/// onboarding 面板是否可见（尚未保存）：期间 boot 后台推进，
/// 只把必要进度投影到面板内，不让独立状态区/更新区/整体淡出打断配置。
const onboardingActive = () => {
  const box = $('ob-box');
  return !!(box && !box.classList.contains('hidden'));
};
let onboardingPausedByError = false;
let onboardingPausedByServiceChoice = false;
const onboardingPendingView = () => onboardingActive()
  || onboardingPausedByError
  || onboardingPausedByServiceChoice;

const PHASE_KEYS = {
  'starting': 'starting',
  'switching-service': 'switchingLocalService',
  'service-choice': 'serviceChoiceTitle',
  'installing-node': 'installingNode',
  'installing-dsh': 'installingDsh',
  'starting-server': 'startingServer',
  'ready': 'ready',
  'cancelled': 'installationCancelled',
  'error': 'startupFailed',
};

let lastStatusPayload = null;
let lastUpdateResult = null;
// 更新结果区只在两种情况下显示：用户手动点了“检查更新”，
// 或静默检查发现 dsh 有新版可更——静默检查的“已是最新/检查失败”
// 在安装进行中弹出来纯属噪音
let updateCheckRequested = false;
let installCancelRequested = false;
let installGeneration = 0;
let installCanCancel = false;
let readyTransitionSequence = 0;
let serviceChoiceVisible = false;

function notifyReadyTransition() {
  const sequence = ++readyTransitionSequence;
  let sent = false;
  let onEnd = null;
  let timer = 0;
  // 序列失效（新一轮过渡已开始）同样要清理：旧监听器/定时器不得残留
  const cleanup = () => {
    if (onEnd) {
      document.body.removeEventListener('transitionend', onEnd);
      onEnd = null;
    }
    if (timer) {
      clearTimeout(timer);
      timer = 0;
    }
  };
  const finish = () => {
    if (sent) return;
    cleanup();
    if (sequence !== readyTransitionSequence) return;
    sent = true;
    window.__TAURI__.core.invoke('startup_transition_done').catch(() => {});
  };
  const reduced = window.matchMedia('(prefers-reduced-motion: reduce)').matches;
  if (reduced) {
    finish();
    return;
  }
  onEnd = (event) => {
    if (event.target === document.body && event.propertyName === 'opacity') finish();
  };
  document.body.addEventListener('transitionend', onEnd);
  // transitionend 可能因页面不可见、动画被系统取消而不触发；兜底保证导航必达。
  // 220ms = 0.18s 淡出 + 余量，与 Rust 侧 STARTUP_TRANSITION_TIMEOUT 匹配
  timer = setTimeout(finish, 220);
}

function phaseText(phase) {
  return PHASE_KEYS[phase] ? dshdT(PHASE_KEYS[phase]) : phase;
}

function phaseDisplay(phase, message, onboarding) {
  if (phase === 'ready') {
    // 首次设置就绪后需用户手动点“开始使用”，文案不同于普通 loading 的自动进入。
    return onboarding ? dshdT('readyOnboarding') : phaseText(phase);
  }
  if (phase === 'error' || phase === 'cancelled') return phaseText(phase);
  return (message && message.length) ? message : phaseText(phase);
}

// 步骤计数只属于安装流程（第 1/2 步只在 installing-* 阶段出现）：
// 普通启动没有安装步骤，直接显示"第 3/3 步"语义错误；只有本次
// 启动确实经历过安装阶段后，starting-server 才续上第 3 步。
let installStepsSeen = false;
function statusDetail(payload) {
  const STEP_OF = { 'installing-node': 1, 'installing-dsh': 2, 'starting-server': 3 };
  if (payload.phase === 'installing-node' || payload.phase === 'installing-dsh') {
    installStepsSeen = true;
  }
  const showStep = STEP_OF[payload.phase]
    && (payload.phase !== 'starting-server' || installStepsSeen);
  const step = showStep ? STEP_OF[payload.phase] : 0;
  const stepLine = step ? dshdT('stepOf', { n: step, total: 3 }) : '';
  return stepLine
    ? (stepLine + (payload.detail ? ' · ' + payload.detail : ''))
    : (payload.detail || '');
}

function renderProgress(payload, progressBar, fill) {
  if (!progressBar || !fill) return;
  fill.classList.toggle('done', payload.phase === 'ready');
  fill.classList.toggle('err', payload.phase === 'error');
  if (payload.phase === 'ready' || payload.phase === 'error' || payload.phase === 'cancelled') {
    fill.classList.remove('determinate');
    fill.style.width = '';
    if (payload.phase === 'ready') progressBar.setAttribute('aria-valuenow', '100');
    else progressBar.removeAttribute('aria-valuenow');
  } else if (typeof payload.progress === 'number') {
    fill.classList.add('determinate');
    fill.style.width = Math.max(2, Math.min(100, payload.progress)) + '%';
    progressBar.setAttribute('aria-valuenow', String(Math.max(0, Math.min(100, payload.progress))));
  } else {
    fill.classList.remove('determinate');
    fill.style.width = '';
    progressBar.removeAttribute('aria-valuenow');
  }
}

/** 首次设置与普通启动页共用同一套状态文案、明细和进度映射。 */
function renderRuntimePresentation(payload, view) {
  // 普通启动 ready 即淡出进入 dsh：再切换文案只会闪一帧大绿字（无信息量），
  // 保留进入前的文案与样式；绿色就绪态只属于 onboarding 运行时卡片（供用户确认可点击）
  const holdText = payload.phase === 'ready' && !view.onboarding;
  const state = holdText ? '' : phaseDisplay(payload.phase, payload.message, view.onboarding);
  const detail = statusDetail(payload);
  if (view.state) {
    if (!holdText) {
      view.state.textContent = state;
      view.state.title = state;
    }
    view.state.classList.toggle('state-ready', payload.phase === 'ready' && view.onboarding);
  }
  if (view.detail) {
    view.detail.textContent = detail;
    view.detail.title = detail;
    // 与普通启动的 status-detail 对齐：详情行恒占位（CSS min-height），
    // 不因内容有无而显示/隐藏，避免进度区高度跳动。
  }
  renderProgress(payload, view.progressBar, view.fill);
  return { state, detail };
}

/** 取消/重新安装按钮显隐：cancellable（下载中）与 cancelled（已取消）互斥，
    onboarding 面板与普通状态区共用同一套切换。 */
function syncInstallActionButtons(phase) {
  const cancellable = installCanCancel
    && (phase === 'installing-node' || phase === 'installing-dsh');
  const cancelled = phase === 'cancelled';
  document.querySelectorAll('[data-install-cancel]').forEach((button) => {
    button.classList.toggle('hidden', !cancellable);
  });
  document.querySelectorAll('[data-install-reinstall]').forEach((button) => {
    button.classList.toggle('hidden', !cancelled);
  });
  return { cancellable, cancelled };
}

function renderOnboardingRuntime(payload) {
  const box = $('ob-runtime');
  if (!box || !onboardingActive()) return;
  box.classList.remove('hidden');
  renderRuntimePresentation(payload, {
    state: $('ob-runtime-state'),
    detail: $('ob-runtime-detail'),
    progressBar: $('ob-runtime-progress'),
    fill: $('ob-runtime-progress-fill'),
    onboarding: true,
  });
  // 从准备安装到启动服务始终保留这一行；Ready 后来源不再影响本轮启动。
  const showSource = !['ready', 'service-choice'].includes(payload.phase);
  $('ob-runtime-actions').classList.toggle('dshd-hold', !showSource);
  syncInstallActionButtons(payload.phase);
}

function setStatus(phaseOrPayload, message, detail) {
  const payload = typeof phaseOrPayload === 'object'
    ? phaseOrPayload
    : { phase: phaseOrPayload, message: message || '', detail: detail || '' };
  const phase = payload.phase;
  const spinner = $('spinner');
  renderRuntimePresentation(payload, {
    state: $('status-text'),
    detail: $('status-detail'),
    progressBar: $('progress-bar'),
    fill: $('bar-fill'),
  });
  const { cancellable, cancelled } = syncInstallActionButtons(phase);
  const installActions = $('install-actions');
  const cancelButtons = document.querySelectorAll('[data-install-cancel]');
  installActions.classList.toggle('dshd-hold', !cancellable && !cancelled);
  if (!cancellable) installCancelRequested = false;
  if (cancellable) {
    cancelButtons.forEach((button) => {
      button.disabled = installCancelRequested;
      button.toggleAttribute('aria-busy', installCancelRequested);
      button.textContent = installCancelRequested ? dshdT('cancellingInstall') : dshdT('cancelInstall');
    });
  } else cancelButtons.forEach((button) => button.removeAttribute('aria-busy'));
  if (phase === 'ready') {
    spinner.classList.add('hidden');
    // onboarding 未完成时跳过整体淡出：ready 可能在面板显示期间到达
    // （服务复用/并发路径下 get_status 直接返回 Ready），淡出会让配置
    // 面板视觉消失而 boot 仍在等待用户操作；保存后面板隐藏即恢复正常
    if (!onboardingPendingView()) {
      document.body.classList.add('fade-out');
      notifyReadyTransition();
    }
  }
  else if (phase === 'error') { spinner.classList.add('hidden'); }
  else if (phase === 'cancelled') {
    spinner.classList.add('hidden');
  }
  else {
    document.body.classList.remove('fade-out');
    spinner.classList.remove('hidden');
  }
}

function showError(message) {
  setStatus('error');
  $('error-msg').textContent = message || dshdT('unknownError');
  if (onboardingActive()) {
    onboardingPausedByError = true;
    $('ob-box').classList.add('hidden');
    document.body.classList.remove('onboarding-mode');
  }
  $('status').classList.add('hidden');
  const box = $('error-box');
  box.classList.remove('hidden');
  // 每次显示重新触发入场动画；聚焦错误框供屏幕阅读器/键盘用户定位错误摘要
  box.classList.remove('reveal');
  void box.offsetWidth;
  box.classList.add('reveal');
  $('startup-error-title').focus();
}

function hideError() {
  $('error-box').classList.add('hidden');
  if (onboardingPausedByError) {
    onboardingPausedByError = false;
    $('ob-box').classList.remove('hidden');
    document.body.classList.add('onboarding-mode');
    $('status').classList.add('hidden');
    if (lastStatusPayload) renderOnboardingRuntime(lastStatusPayload);
    return;
  }
  if (!onboardingActive()) $('status').classList.remove('hidden');
}

function setServiceChoiceBusy(busy) {
  ['btn-connect-external', 'btn-start-local'].forEach((id) => {
    const button = $(id);
    if (!button) return;
    button.disabled = busy;
    button.toggleAttribute('aria-busy', busy);
  });
}

function renderServiceChoice(payload) {
  const box = $('service-choice-box');
  if (!box) return false;
  const candidate = payload.external_service;
  const visible = payload.phase === 'service-choice' && candidate;
  box.classList.toggle('hidden', !visible);
  if (!visible) {
    if (onboardingPausedByServiceChoice) {
      onboardingPausedByServiceChoice = false;
      if (payload.service_mode !== 'external' && payload.service_mode !== 'external-disconnected') {
        $('ob-box').classList.remove('hidden');
        document.body.classList.add('onboarding-mode');
        $('status').classList.add('hidden');
        renderOnboardingRuntime(payload);
      }
    }
    serviceChoiceVisible = false;
    setServiceChoiceBusy(false);
    return false;
  }
  if (onboardingActive()) {
    onboardingPausedByServiceChoice = true;
    $('ob-box').classList.add('hidden');
    document.body.classList.remove('onboarding-mode');
  }
  $('service-choice-port').textContent = String(candidate.port || '—');
  $('service-choice-cwd').textContent = candidate.cwd || candidate.home || '—';
  $('service-choice-cwd').title = candidate.cwd || candidate.home || '';
  $('status').classList.add('hidden');
  $('error-box').classList.add('hidden');
  if (!serviceChoiceVisible) {
    $('service-choice-feedback').classList.add('hidden');
    $('service-choice-feedback').textContent = '';
    $('service-choice-title').focus();
  }
  serviceChoiceVisible = true;
  return true;
}

async function copyText(text) {
  if (navigator.clipboard && navigator.clipboard.writeText) {
    await navigator.clipboard.writeText(text);
    return;
  }
  const helper = document.createElement('textarea');
  helper.value = text;
  helper.style.position = 'fixed';
  helper.style.opacity = '0';
  document.body.append(helper);
  helper.select();
  const copied = document.execCommand('copy');
  helper.remove();
  if (!copied) throw new Error(dshdT('copyFailed'));
}

function renderVersions(payload) {
  const el = $('versions');
  if (!el) return;
  const parts = [];
  if (payload.dsh_version) parts.push('dsh v' + payload.dsh_version);
  if (payload.node_version) parts.push('Node ' + payload.node_version);
  if (payload.npm_version) parts.push('npm ' + payload.npm_version);
  if (payload.port) parts.push(dshdT('port', { port: payload.port }));
  if (payload.service_mode === 'external' || payload.service_mode === 'external-disconnected') {
    parts.push(dshdT('externalService'));
  }
  // 字段缺失时保留已显示内容（防御：事件载荷异常时不清空 footer）
  if (parts.length === 0) return;
  el.textContent = parts.join(' · ');
}

function renderStatus(payload) {
  lastStatusPayload = payload;
  installGeneration = Number(payload.install_generation || 0);
  installCanCancel = payload.can_cancel === true;
  if (payload.service_mode === 'external' || payload.service_mode === 'external-disconnected') {
    $('update-box').classList.add('hidden');
  } else if (lastUpdateResult) {
    // 切回托管服务时按已有结果立即恢复更新区显隐，不等下一次更新事件
    renderUpdate(lastUpdateResult);
  }
  // 语言切换后后端消息快照不会自动刷新（Rust 按旧语言生成）：
  // 纯固定文案的 phase 改用当前语言重译；动态消息（下载/安装进度、
  // 端口回退等）保持后端快照，避免错译。
  const fixedMsg = payload.phase === 'starting-server' ? phaseText('starting-server') : payload.message;
  const presentation = { ...payload, message: fixedMsg };
  setStatus(presentation);
  renderVersions(payload);
  renderOnboardingRuntime(presentation);
  syncOnboardingCompletionAction();
  $('btn-use-local').classList.toggle('hidden', payload.service_mode !== 'external-disconnected');
  if (renderServiceChoice(payload)) return;
  if (payload.phase === 'error') showError(payload.message);
  else hideError();
}

// —— 首次使用配置 ——

let onboardingSaving = false;
let obLangSel = null;
let obThemeSel = null;

/** 只有服务就绪后才能结束引导并进入可用的 dsh 页面。 */
function syncOnboardingCompletionAction() {
  const ready = lastStatusPayload && lastStatusPayload.phase === 'ready';
  const button = $('ob-start');
  if (button) button.disabled = onboardingSaving || !ready;
}

// dsh 风格下拉（语言/主题）：trigger + 浮层列表，选中态 ghost-active-fill
function setupSelect(selId, onChange) {
  const root = $(selId);
  if (!root) return null;
  const trigger = root.querySelector('.dshd-select-trigger');
  const list = root.querySelector('.dshd-select-list');
  const valueEl = root.querySelector('.dshd-select-value');
  let value = '';
  const labelOf = (v) => {
    const opt = list.querySelector('[data-value="' + v + '"]');
    if (!opt) return v;
    return opt.dataset.triggerI18n ? dshdT(opt.dataset.triggerI18n) : opt.textContent;
  };
  const options = () => [...list.querySelectorAll('li')];
  const closeList = (restoreFocus) => {
    list.hidden = true;
    trigger.setAttribute('aria-expanded', 'false');
    if (restoreFocus) trigger.focus();
  };
  const focusOption = (index) => {
    const all = options();
    if (!all.length) return;
    const target = all[(index + all.length) % all.length];
    all.forEach((option) => { option.tabIndex = option === target ? 0 : -1; });
    target.focus();
  };
  const openList = (focusLast) => {
    if (trigger.disabled) return;
    list.hidden = false;
    trigger.setAttribute('aria-expanded', 'true');
    const all = options();
    const selected = all.findIndex((option) => option.dataset.value === value);
    focusOption(focusLast ? all.length - 1 : Math.max(0, selected));
  };
  const setValue = (v, fire) => {
    value = v;
    valueEl.textContent = labelOf(v);
    options().forEach((li) => {
      li.setAttribute('aria-selected', li.dataset.value === v ? 'true' : 'false');
      li.tabIndex = li.dataset.value === v ? 0 : -1;
    });
    if (fire && onChange) onChange(v);
  };
  trigger.addEventListener('click', (e) => {
    e.stopPropagation();
    if (list.hidden) {
      list.hidden = false;
      trigger.setAttribute('aria-expanded', 'true');
    } else closeList(false);
  });
  trigger.addEventListener('keydown', (event) => {
    if (event.key === 'ArrowDown' || event.key === 'ArrowUp') {
      event.preventDefault();
      openList(event.key === 'ArrowUp');
    } else if (event.key === 'Enter' || event.key === ' ') {
      event.preventDefault();
      if (list.hidden) openList(false);
      else closeList(false);
    } else if (event.key === 'Escape' && !list.hidden) {
      event.preventDefault();
      closeList(false);
    }
  });
  options().forEach((li) => {
    li.addEventListener('click', () => {
      if (li.getAttribute('aria-disabled') === 'true') return;
      setValue(li.dataset.value, true);
      closeList(true);
    });
  });
  list.addEventListener('keydown', (event) => {
    if (trigger.disabled) return;
    const all = options();
    const current = all.indexOf(document.activeElement);
    if (event.key === 'ArrowDown' || event.key === 'ArrowUp') {
      event.preventDefault();
      focusOption(current + (event.key === 'ArrowDown' ? 1 : -1));
    } else if (event.key === 'Home' || event.key === 'End') {
      event.preventDefault();
      focusOption(event.key === 'Home' ? 0 : all.length - 1);
    } else if ((event.key === 'Enter' || event.key === ' ') && current >= 0) {
      event.preventDefault();
      setValue(all[current].dataset.value, true);
      closeList(true);
    } else if (event.key === 'Escape') {
      event.preventDefault();
      closeList(true);
    } else if (event.key === 'Tab') closeList(false);
  });
  document.addEventListener('click', (e) => {
    if (!root.contains(e.target)) closeList(false);
  });
  const setDisabled = (disabled) => {
    trigger.disabled = disabled;
    options().forEach((option) => option.setAttribute('aria-disabled', String(disabled)));
    if (disabled) {
      closeList(false);
      options().forEach((option) => { option.tabIndex = -1; });
    } else setValue(value, false);
  };
  return {
    get: () => value,
    set: (v) => setValue(v, false),
    setDisabled,
    refresh: () => setValue(value, false),
  };
}

function setOnboardingBusy(busy) {
  const box = $('ob-box');
  box.querySelectorAll('input, button').forEach((control) => { control.disabled = busy; });
  if (obLangSel) obLangSel.setDisabled(busy);
  if (obThemeSel) obThemeSel.setDisabled(busy);
  box.toggleAttribute('aria-busy', busy);
  const startButton = $('ob-start');
  const startKey = busy ? 'enteringApp' : 'startUsing';
  startButton.dataset.i18n = startKey;
  startButton.textContent = dshdT(startKey);
  startButton.toggleAttribute('aria-busy', busy);
  if (!busy && onboardingActive() && lastStatusPayload) {
    renderOnboardingRuntime(lastStatusPayload);
  }
  syncOnboardingCompletionAction();
}

async function initOnboarding() {
  try {
    const st = await window.__TAURI__.core.invoke('get_onboarding_state');
    if (!st || !st.needs_onboarding) return;
    // 语言/主题下拉：选中即预览（保存时才持久化）
    obLangSel = setupSelect('ob-lang', (lang) => {
      window.dshdSetLanguage && window.dshdSetLanguage(lang);
      // 选项文案由 i18n 原地更新；刷新两个 trigger，避免仍显示切换前语言。
      if (obLangSel) obLangSel.refresh();
      if (obThemeSel) obThemeSel.refresh();
      window.__TAURI__.core.invoke('preview_language', { language: lang }).catch(() => {});
    });
    obThemeSel = setupSelect('ob-theme', (theme) => {
      window.__TAURI__.core.invoke('preview_theme', { theme }).catch(() => {});
    });
    if (obLangSel) obLangSel.set(st.language === 'en' ? 'en' : 'zh-CN');
    if (obThemeSel) obThemeSel.set(st.theme || 'system');
    $('ob-autostart').checked = !!st.autostart;
    $('ob-builtin-plugins').checked = st.install_builtin_plugins !== false;
    document.body.classList.add('onboarding-mode');
    $('ob-box').classList.remove('hidden');
    // CSS 动画受 prefers-reduced-motion 统一控制，避免内联 transition 绕过系统偏好。
    $('ob-box').classList.add('reveal');
    // onboarding 模式下聚焦配置面板：隐藏启动状态区
    $('status').classList.add('hidden');
    syncOnboardingCompletionAction();
    // 回报面板已显示：boot 等待切换为无限等待（无 60 秒兜底）
    window.__TAURI__.core.invoke('onboarding_shown').catch(() => {});
  } catch (e) { /* 后端未就绪时忽略 */ }
}

// Rust 侧 60 秒兜底窗口内主动探活；通过带代次的结果回报确认本次面板
// 是否仍可见，避免固定等待后误判页面已经显示。
window.__dshdOnboardingVisible = (generation) => {
  const visible = onboardingActive();
  window.__TAURI__.core.invoke('onboarding_probe_result', { generation, visible }).catch(() => {});
  return visible;
};

async function submitOnboarding() {
  if (onboardingSaving || !lastStatusPayload || lastStatusPayload.phase !== 'ready') return;
  const errBox = $('ob-error');
  // 格式校验（不占用 saving 状态）：非空 key 必须以 sk- 开头，否则提示并
  // 聚焦输入框；留空仍允许（之后可在桌面端设置中配置）
  const key = $('ob-apikey').value.trim();
  if (key && !/^sk-/.test(key)) {
    errBox.textContent = dshdT('apiKeyFormatHint');
    errBox.classList.remove('hidden');
    $('ob-apikey').setAttribute('aria-invalid', 'true');
    $('ob-apikey').focus();
    return;
  }
  errBox.classList.add('hidden');
  onboardingSaving = true;
  setOnboardingBusy(true);
  const payload = {
    language: obLangSel ? obLangSel.get() : 'zh-CN',
    theme: obThemeSel ? obThemeSel.get() : 'system',
    autostart: $('ob-autostart').checked,
    install_builtin_plugins: $('ob-builtin-plugins').checked,
  };
  if (key) payload.api_key = key;
  try {
    await window.__TAURI__.core.invoke('save_onboarding', { payload });
    onboardingPausedByError = false;
    $('ob-box').classList.add('hidden');
    $('ob-runtime').classList.add('hidden');
    document.body.classList.remove('onboarding-mode');
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
    // boot 将立即继续；错误页已接管时不再把重复状态区强行显示出来。
    if ($('error-box').classList.contains('hidden')) $('status').classList.remove('hidden');
  } catch (e) {
    errBox.textContent = dshdT('saveFailed') + ': ' + e;
    errBox.classList.remove('hidden');
  } finally {
    onboardingSaving = false;
    setOnboardingBusy(false);
  }
}

function renderUpdate(result) {
  lastUpdateResult = result;
  // onboarding 未完成时不显示更新区（静默检查事件不应干扰配置面板）：
  // 快照已存，保存后由后续事件或语言切换重渲染正常展示；
  // 若面板显示前已有结果（服务复用场景），先隐藏避免与面板同卡堆叠
  if (onboardingPendingView()) {
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
  $('btn-update-check').removeAttribute('aria-busy');
  applyBtn.disabled = false;
  applyBtn.removeAttribute('aria-busy');
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

function setInstallCancelPending(pending) {
  document.querySelectorAll('[data-install-cancel]').forEach((button) => {
    button.disabled = pending;
    button.toggleAttribute('aria-busy', pending);
    button.textContent = pending ? dshdT('cancellingInstall') : dshdT('cancelInstall');
  });
}

function resetInstallPendingControls() {
  installCancelRequested = false;
  setInstallCancelPending(false);
  if (lastStatusPayload) renderOnboardingRuntime(lastStatusPayload);
}

function bind() {
  dshdBindPasswordToggle($('ob-apikey'), $('ob-apikey-toggle'));
  $('ob-apikey').addEventListener('input', () => {
    $('ob-apikey').removeAttribute('aria-invalid');
    const error = $('ob-error');
    if (error.textContent === dshdT('apiKeyFormatHint')) error.classList.add('hidden');
  });
  document.querySelectorAll('[data-install-cancel]').forEach((button) => {
    button.addEventListener('click', async () => {
      installCancelRequested = true;
      setInstallCancelPending(true);
      try {
        const accepted = await window.__TAURI__.core.invoke('cancel_install', {
          generation: installGeneration,
        });
        if (!accepted) {
          resetInstallPendingControls();
          const payload = await window.__TAURI__.core.invoke('get_status');
          renderStatus(payload);
        }
      } catch (e) {
        resetInstallPendingControls();
        showError(String(e));
      }
    });
  });
  document.querySelectorAll('[data-install-reinstall]').forEach((button) => {
    button.addEventListener('click', async () => {
      document.querySelectorAll('[data-install-reinstall]').forEach((item) => {
        item.disabled = true;
        item.setAttribute('aria-busy', 'true');
      });
      setStatus('starting', dshdT('starting'));
      try {
        await window.__TAURI__.core.invoke('retry_boot');
      } catch (e) {
        showError(dshdT('retry') + ': ' + e);
      } finally {
        document.querySelectorAll('[data-install-reinstall]').forEach((item) => {
          item.disabled = false;
          item.removeAttribute('aria-busy');
        });
      }
    });
  });
  $('btn-retry').addEventListener('click', async () => {
    const button = $('btn-retry');
    button.disabled = true;
    button.setAttribute('aria-busy', 'true');
    hideError();
    setStatus('starting');
    try {
      await window.__TAURI__.core.invoke('retry_boot');
    } catch (e) {
      showError(dshdT('retry') + ': ' + e);
    } finally {
      button.disabled = false;
      button.removeAttribute('aria-busy');
    }
  });
  const chooseService = async (reuse) => {
    setServiceChoiceBusy(true);
    $('service-choice-feedback').classList.add('hidden');
    try {
      const accepted = await window.__TAURI__.core.invoke('choose_service', {
        generation: installGeneration,
        reuse,
      });
      if (!accepted) {
        setServiceChoiceBusy(false);
        const payload = await window.__TAURI__.core.invoke('get_status');
        renderStatus(payload);
      }
    } catch (e) {
      setServiceChoiceBusy(false);
      const feedback = $('service-choice-feedback');
      feedback.textContent = dshdT('serviceChoiceFailed', { message: String(e) });
      feedback.classList.remove('hidden');
    }
  };
  $('btn-connect-external').addEventListener('click', () => chooseService(true));
  $('btn-start-local').addEventListener('click', () => chooseService(false));
  $('btn-use-local').addEventListener('click', async () => {
    const button = $('btn-use-local');
    button.disabled = true;
    button.setAttribute('aria-busy', 'true');
    try {
      hideError();
      setStatus('starting', dshdT('starting'));
      await window.__TAURI__.core.invoke('use_local_service');
    } catch (e) {
      showError(dshdT('serviceChoiceFailed', { message: String(e) }));
    } finally {
      button.disabled = false;
      button.removeAttribute('aria-busy');
    }
  });
  $('btn-logs').addEventListener('click', async () => {
    try {
      await window.__TAURI__.core.invoke('open_logs');
    } catch (e) {
      showError(dshdT('openLogs') + ': ' + e);
    }
  });
  $('btn-copy-error').addEventListener('click', async () => {
    const button = $('btn-copy-error');
    try {
      await copyText($('error-msg').textContent || '');
      button.textContent = dshdT('copied');
      setTimeout(() => { button.textContent = dshdT('copyError'); }, 1500);
    } catch (e) {
      button.textContent = dshdT('copyFailed');
    }
  });
  $('btn-quit').addEventListener('click', () => window.__TAURI__.core.invoke('quit').catch(() => {}));
  $('btn-update-check').addEventListener('click', async () => {
    const button = $('btn-update-check');
    button.disabled = true;
    button.setAttribute('aria-busy', 'true');
    updateCheckRequested = true;
    // 立即显示“检查更新中…”，结果到达后 renderUpdate 填充
    $('update-box').classList.remove('hidden');
    $('update-text').textContent = dshdT('checkingUpdates');
    try {
      await window.__TAURI__.core.invoke('check_updates');
    } catch (e) {
      $('update-text').textContent = dshdT('checkFailed') + ': ' + e;
      button.disabled = false;
      button.removeAttribute('aria-busy');
    }
  });
  $('btn-update-apply').addEventListener('click', async () => {
    const button = $('btn-update-apply');
    button.disabled = true;
    button.setAttribute('aria-busy', 'true');
    $('update-text').textContent = dshdT('updateDshWait');
    try {
      await window.__TAURI__.core.invoke('apply_updates', { which: 'dsh' });
    } catch (e) {
      $('update-text').textContent = dshdT('updateFailed', { message: e });
      button.disabled = false;
      button.removeAttribute('aria-busy');
    }
  });
  $('ob-start').addEventListener('click', submitOnboarding);
}

async function init() {
  dshdApplyI18n();
  bind();
  // 监听先挂并缓冲，再去查 onboarding 状态（含磁盘读 + IPC 往返）：
  // 面板确定前到达的状态事件不丢失，消除窗口打开到面板出现之间的事件空窗。
  // 回放发生在 initOnboarding 之后，onboardingActive() 已是真实值，
  // 「面板未显示不淡出」的白屏防护语义不变（原先靠丢弃事件保证，现在靠
  // 正确的时序判断——事件顺序保持旧→新，get_status 快照最后兜底覆盖）。
  const buffered = [];
  let buffering = true;
  const onStatus = (payload) => (buffering ? buffered.push(payload) : renderStatus(payload));
  await dshdListen('dsh-status', (e) => onStatus(e.payload));
  await dshdListen('update-result', (e) => {
    if (buffering) return; // 缓冲窗口极短（单次 IPC 往返）且静默检查在服务就绪后才跑；
    // 真错过结果时，后续事件与发现新版的弹窗兜底
    renderUpdate(e.payload);
  });
  await dshdListen('update-progress', (e) => {
    if (!buffering && e.payload && e.payload.message) {
      // onboarding 期间不覆盖更新文案（面板显示时更新区不可见，
      // 且 Rust 文本是旧语言快照，语言切换后不重译）
      if (onboardingPendingView()) return;
      $('update-text').textContent = e.payload.message;
    }
  });
  await initOnboarding();
  buffering = false;
  for (const payload of buffered) renderStatus(payload);
  buffered.length = 0;
  window.addEventListener('dshd-language-changed', () => {
    // 下拉 trigger 的值不是 data-i18n 节点，语言切换后按当前 option 文案刷新。
    if (obLangSel) obLangSel.set(obLangSel.get());
    if (obThemeSel) obThemeSel.set(obThemeSel.get());
    // 快照的 message/detail 是 Rust 按旧语言生成的文本，重渲染时必须剥离，
    // 由 phaseText/stepLine 按当前语言重译固定文案（进度数字语言无关保留）
    if (lastStatusPayload) renderStatus({ ...lastStatusPayload, message: '', detail: '' });
    if (lastUpdateResult) renderUpdate(lastUpdateResult);
  });
  try {
    const payload = await window.__TAURI__.core.invoke('get_status');
    renderStatus(payload);
  } catch (e) { /* 后端未就绪时忽略 */ }
}

init();

// 窗口失焦变淡：Rust 侧 Focused 广播驱动（与标题栏/状态栏同一机制），
// 仅启动阶段有效——导航到 dsh 页面后广播即停止
window.__dshdSetWindowActive = (active) => {
  document.body.classList.toggle('window-inactive', !active);
};
