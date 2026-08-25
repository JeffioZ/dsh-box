// 控制中心：用户设置与模型配置导入/导出。

// —— 设置（按桌面行为 / 界面 / 服务能力 / 凭据与模型分组） ——
let settingsBusy = false;
const SETTING_KEYS = ['autostart', 'hide_tool_calls', 'hide_stats_line', 'hide_statusbar', 'hide_balance', 'task_notifications', 'auto_update_plugins'];
function settingsRow(key, nameKey, descKey, className) {
  return (
    '<label class="srow' + (className ? ' ' + className : '') + '">' +
    '<span class="srow-txt">' +
    '<span class="srow-name">' + esc(dshdT(nameKey)) + '</span>' +
    '<span class="srow-desc">' + esc(dshdT(descKey)) + '</span>' +
    '</span>' +
    '<input type="checkbox" class="sswitch" role="switch" data-key="' + key + '">' +
    '</label>'
  );
}
function behaviorRow(key, nameKey, descKey, firstValue, firstLabel, secondValue, secondLabel) {
  return (
    '<div class="srow">' +
    '<span class="srow-txt"><span class="srow-name">' + dshdT(nameKey) + '</span>' +
    '<span class="srow-desc">' + dshdT(descKey) + '</span></span>' +
    '<span class="dshd-seg" role="radiogroup" aria-label="' + esc(dshdT(nameKey)) + '">' +
    '<label class="dshd-seg-opt"><input type="radio" name="' + key + '" value="' + firstValue + '" /><span>' + esc(dshdT(firstLabel)) + '</span></label>' +
    '<label class="dshd-seg-opt"><input type="radio" name="' + key + '" value="' + secondValue + '" /><span>' + esc(dshdT(secondLabel)) + '</span></label>' +
    '</span></div>'
  );
}
function dshChannelRow() {
  return (
    '<div class="srow">' +
    '<span class="srow-txt">' +
    '<span class="srow-name">' + dshdT('settingsDshChannel') + '</span>' +
    '<span class="srow-desc">' + dshdT('settingsDshChannelDesc') + '</span>' +
    '</span>' +
    '<span class="dshd-seg" id="dsh-channel" role="radiogroup" aria-label="' + esc(dshdT('settingsDshChannel')) + '">' +
    '<label class="dshd-seg-opt"><input type="radio" name="dsh-channel" value="latest" /><span>' + esc(dshdT('settingsChannelLatest')) + '</span></label>' +
    '<label class="dshd-seg-opt"><input type="radio" name="dsh-channel" value="next" /><span>' + esc(dshdT('settingsChannelNext')) + '</span></label>' +
    '</span>' +
    '</div>'
  );
}
async function renderSettings() {
  const body = $('body');
  // 先拿到配置状态再一次性渲染，才能决定模型配置板块放在最前还是原位，
  // 避免「先渲染再挪动」造成闪烁。等候期间给出轻量占位。
  body.innerHTML = '<div class="usage-load" role="status" aria-live="polite"><span class="spin" aria-hidden="true"></span>' + dshdT('usageLoading') + '</div>';
  let settings = null;
  try {
    settings = await invoke('settings_get');
  } catch (e) {
    settings = null;
  }
  // await 期间用户可能已切走 tab：settings_get 返回后若已不在设置页，
  // 直接放弃本轮渲染，避免覆盖当前页（对齐其他页面的 openKind 守卫）。
  if (openKind !== 'settings') return;
  // 无任何模型配置（无 DeepSeek Key 也无自定义路由）时模型配置板块置顶引导。
  const modelFirst = !!(settings && !settings.api_key_set && !settings.model_config_set);
  const desktopSec =
    '<section class="psection settings-section" aria-labelledby="settings-desktop-heading">' +
    '<h3 id="settings-desktop-heading">' + dshdT('settingsDesktopTitle') + '</h3>' +
    settingsRow('autostart', 'autostart', 'settingsAutostartDesc') +
    settingsRow('task_notifications', 'settingsTaskNotifications', 'settingsTaskNotificationsDesc') +
    behaviorRow('launch_behavior', 'settingsLaunchBehavior', 'settingsLaunchBehaviorDesc', 'window', 'settingsLaunchWindow', 'tray', 'settingsLaunchTray') +
    behaviorRow('close_behavior', 'settingsCloseBehavior', 'settingsCloseBehaviorDesc', 'tray', 'settingsCloseTray', 'quit', 'settingsCloseQuit') +
    '</section>';
  const interfaceSec =
    '<section class="psection settings-section" aria-labelledby="settings-interface-heading">' +
    '<h3 id="settings-interface-heading">' + dshdT('settingsInterfaceTitle') + '</h3>' +
    settingsRow('hide_tool_calls', 'settingsHideTools', 'settingsHideToolsDesc') +
    settingsRow('hide_stats_line', 'settingsHideStats', 'settingsHideStatsDesc') +
    settingsRow('hide_statusbar', 'settingsHideStatusbar', 'settingsHideStatusbarDesc') +
    settingsRow('hide_balance', 'settingsHideBalance', 'settingsHideBalanceDesc', 'srow-dependent') +
    '</section>';
  const runtimeSec =
    '<section class="psection settings-section" aria-labelledby="settings-runtime-heading">' +
    '<h3 id="settings-runtime-heading">' + dshdT('settingsRuntimeTitle') + '</h3>' +
    '<div id="settings-external-note" class="settings-scope-note" role="status" hidden>' +
    '<span>' + dshdT('settingsExternalServiceNote') + '</span>' +
    '<button type="button" id="settings-use-local" class="mi-btn">' + dshdT('useLocalService') + '</button>' +
    '</div>' +
    '<div class="mi-card settings-service-card">' +
    '<div class="mi-card-head">' +
    '<label class="mi-card-name" for="settings-api-key">' + dshdT('deepSeekApiKey') + '</label>' +
    '<span id="settings-api-key-help" class="mi-card-desc">' + dshdT('settingsApiKeyDesc') + '</span>' +
    '</div>' +
    '<div class="dshd-password-field api-key-input-row">' +
    '<input id="settings-api-key" class="dshd-input mi-key-input" type="password" autocomplete="off" spellcheck="false" placeholder="sk-..." aria-describedby="settings-api-key-help settings-api-key-feedback" />' +
    '<button type="button" id="settings-api-key-toggle" class="dshd-x dshd-password-action" aria-controls="settings-api-key" aria-pressed="false" hidden></button>' +
    '</div>' +
    '<div class="mi-actions">' +
    '<button type="button" id="settings-api-key-save" class="mi-btn primary">' + dshdT('settingsApiKeySave') + '</button>' +
    '<button type="button" id="settings-api-key-clear" class="mi-btn">' + dshdT('settingsApiKeyClear') + '</button>' +
    '<span id="settings-api-key-feedback" class="mi-feedback api-key-feedback" role="status" aria-live="polite"></span>' +
    '</div>' +
    '</div>' +
    dshChannelRow() +
    settingsRow('auto_update_plugins', 'settingsAutoUpdatePlugins', 'settingsAutoUpdatePluginsDesc') +
    '</section>';
  // 模型配置导入/导出：无任何模型配置时置顶，否则留在末尾。
  const modelsSec =
    '<section class="psection settings-section mi-box" aria-labelledby="settings-model-heading">' +
    '<h3 id="settings-model-heading">' + dshdT('modelImportTitle') + '</h3>' +
    '<div class="mi-card">' +
    '<div class="mi-card-head">' +
    '<label class="mi-card-name" for="mi-textarea">' + dshdT('modelImportPaste') + '</label>' +
    '<span class="mi-card-desc">' + dshdT('modelImportHint') + '</span>' +
    '</div>' +
    '<div class="mi-textarea-wrap">' +
    '<textarea id="mi-textarea" class="dshd-input dshd-textarea mi-textarea" rows="5" spellcheck="false" placeholder="' + esc(dshdT('modelImportPlaceholder')) + '"></textarea>' +
    '<div id="mi-resize-bar" class="mi-resize-bar" role="slider" aria-orientation="vertical" aria-valuemin="72" aria-valuemax="480" aria-valuenow="72" tabindex="0" title="' + esc(dshdT('modelResizeTip')) + '" aria-label="' + esc(dshdT('modelResizeAria')) + '"></div>' +
    '</div>' +
    '<div class="mi-actions">' +
    '<button type="button" id="mi-preview" class="mi-btn primary">' + dshdT('modelImportPreview') + '</button>' +
    '<button type="button" id="mi-export" class="mi-btn">' + dshdT('modelExport') + '</button>' +
    '<span id="mi-feedback" class="mi-feedback" role="status" aria-live="polite"></span>' +
    '</div>' +
    '<div id="mi-result" class="mi-result" hidden role="status" aria-live="polite"></div>' +
    '</div>' +
    '</section>';
  body.innerHTML =
    (modelFirst ? modelsSec : desktopSec) +
    (modelFirst ? desktopSec : interfaceSec) +
    (modelFirst ? interfaceSec : runtimeSec) +
    (modelFirst ? runtimeSec : modelsSec) +
    '<div class="serr" id="serr" role="alert" aria-live="polite" hidden></div>';
  body.querySelectorAll('.sswitch').forEach((el) => {
    el.addEventListener('change', onSettingToggle);
  });
  body.querySelectorAll('input[name="dsh-channel"]').forEach((el) => {
    el.addEventListener('change', async () => {
      if (!el.checked || settingsBusy) return;
      settingsBusy = true;
      hideSettingError();
      try {
        const state = await invoke('set_dsh_channel', { channel: el.value });
        applySettingState(state);
        dshChannelChanged = true;
      } catch (e) {
        showSettingError(dshdT('settingsFailed', { message: String(e) }));
        // 失败回滚：重新拉取真实状态回填 checked（radio 已乐观选中新值）
        invoke('settings_get').then(applySettingState).catch(() => {});
      } finally {
        settingsBusy = false;
      }
    });
  });
  body.querySelectorAll('input[name="close_behavior"], input[name="launch_behavior"]').forEach((el) => {
    el.addEventListener('change', async () => {
      if (!el.checked || settingsBusy) return;
      settingsBusy = true;
      hideSettingError();
      try {
        const state = await invoke('set_window_behavior', { key: el.name, value: el.value });
        applySettingState(state);
      } catch (e) {
        showSettingError(dshdT('settingsFailed', { message: String(e) }));
        invoke('settings_get').then(applySettingState).catch(() => {});
      } finally {
        settingsBusy = false;
      }
    });
  });
  // 用已从 settings_get 拿到的状态回填（不再二次请求，避免重复拉取）。
  if (settings) applySettingState(settings);
  else showSettingError(dshdT('settingsFailed', { message: 'settings_get' }));
  $('settings-use-local').addEventListener('click', async () => {
    const button = $('settings-use-local');
    button.disabled = true;
    button.setAttribute('aria-busy', 'true');
    hideSettingError();
    try {
      await invoke('use_local_service');
      await invoke('app_dialog_close');
    } catch (e) {
      showSettingError(dshdT('settingsFailed', { message: String(e) }));
      button.disabled = false;
      button.removeAttribute('aria-busy');
    }
  });
  initApiKeySettings();
  initModelImport();
}
function apiKeyFeedback(message, ok) {
  const el = $('settings-api-key-feedback');
  if (!el) return;
  el.textContent = message || '';
  el.className = 'mi-feedback api-key-feedback' + (ok ? ' ok' : message ? ' err' : '');
}
function setApiKeyBusy(busy) {
  ['settings-api-key', 'settings-api-key-toggle', 'settings-api-key-save', 'settings-api-key-clear'].forEach((id) => {
    const el = $(id);
    if (el) el.disabled = busy || el.dataset.locked === '1';
  });
}
function initApiKeySettings() {
  const input = $('settings-api-key');
  const toggle = $('settings-api-key-toggle');
  const save = $('settings-api-key-save');
  const clear = $('settings-api-key-clear');
  if (!input || !toggle || !save || !clear) return;
  dshdBindPasswordToggle(input, toggle);
  input.addEventListener('input', () => {
    input.removeAttribute('aria-invalid');
    apiKeyFeedback('', false);
  });
  save.addEventListener('click', async () => {
    const value = input.value.trim();
    if (!value) {
      input.setAttribute('aria-invalid', 'true');
      apiKeyFeedback(dshdT('settingsApiKeyEmpty'), false);
      input.focus();
      return;
    }
    setApiKeyBusy(true);
    apiKeyFeedback(dshdT('settingsApiKeySaving'), true);
    try {
      const state = await invoke('set_deepseek_api_key', { apiKey: value });
      input.value = '';
      input.type = 'password';
      if (toggle.__dshdPasswordSync) toggle.__dshdPasswordSync();
      applySettingState(state);
      apiKeyFeedback(dshdT('settingsApiKeySaved'), true);
    } catch (e) {
      apiKeyFeedback(dshdT('settingsFailed', { message: String(e) }), false);
    } finally {
      setApiKeyBusy(false);
    }
  });
  clear.addEventListener('click', async () => {
    setApiKeyBusy(true);
    apiKeyFeedback(dshdT('settingsApiKeyClearing'), true);
    try {
      const state = await invoke('set_deepseek_api_key', { apiKey: null });
      input.value = '';
      input.type = 'password';
      if (toggle.__dshdPasswordSync) toggle.__dshdPasswordSync();
      applySettingState(state);
      apiKeyFeedback(dshdT('settingsApiKeyCleared'), true);
    } catch (e) {
      apiKeyFeedback(dshdT('settingsFailed', { message: String(e) }), false);
    } finally {
      setApiKeyBusy(false);
    }
  });
}
// —— 模型配置导入（设置页）——
let miPreviewRefs = [];
function miFeedback(message, ok) {
  const el = $('mi-feedback');
  if (!el) return;
  el.textContent = message || '';
  el.className = 'mi-feedback' + (ok ? ' ok' : ' err');
}
function miClearFeedback() { const el = $('mi-feedback'); if (el) el.textContent = ''; }
// 高度模式：'auto' 默认自适应（内容多自动长到 60vh 上限出滚动条），
// 'manual' 用户拖拽/键盘调过的固定高度（不再被 autosize 覆盖）。
let miResizeMode = 'auto';
// 高度随内容自适应：内容增长时把高度撑到 scrollHeight，超过 CSS 的
// max-height(60vh) 后由 overflow-y 出滚动条。供 input 事件与程序赋值
// （粘贴/清空）后调用；manual 态下不干预。
function miTextareaAutosize() {
  const el = $('mi-textarea');
  if (!el || miResizeMode !== 'auto') return;
  el.style.height = 'auto';
  el.style.height = el.scrollHeight + 'px';
}
// 重置回自适应模式。
function miTextareaReset() {
  miResizeMode = 'auto';
  miTextareaAutosize();
}
function initModelImport() {
  const textarea = $('mi-textarea');
  const previewBtn = $('mi-preview');
  const resizeBar = $('mi-resize-bar');
  if (!textarea || !previewBtn) return;
  miResizeMode = 'auto';
  textarea.addEventListener('input', miTextareaAutosize);
  miTextareaAutosize();
  // —— 拖拽调整高度（不依赖原生 resizer，避免手柄随滚动条抖动/深浅色问题）——
  if (resizeBar) {
    const MIN_H = 72; // 与 .dshd-textarea min-height 对齐
    const maxH = () => {
      // 读取 computed max-height(60vh) 转 px；取不到时退回一个合理上限。
      const px = parseFloat(getComputedStyle(textarea).maxHeight);
      return Number.isFinite(px) && px > 0 ? px : 480;
    };
    const clampH = (h) => Math.max(MIN_H, Math.min(maxH(), h));
    // 同步 slider 的 aria-valuenow/max（无障碍），随拖拽/键盘/重置更新。
    const syncAria = (h) => {
      resizeBar.setAttribute('aria-valuenow', String(Math.round(h)));
      resizeBar.setAttribute('aria-valuemax', String(Math.round(maxH())));
    };
    // 初始同步一次，让 aria-valuemax 反映真实上限（60vh），而非占位的 480。
    syncAria(textarea.getBoundingClientRect().height);
    let dragState = null;
    resizeBar.addEventListener('mousedown', (ev) => {
      if (ev.button !== 0) return;
      ev.preventDefault(); // 避免选中文本/拖拽触发原生行为
      dragState = { startY: ev.clientY, startH: textarea.getBoundingClientRect().height };
      const onMove = (e) => {
        if (!dragState) return;
        miResizeMode = 'manual';
        const h = clampH(dragState.startH + (e.clientY - dragState.startY));
        textarea.style.height = h + 'px';
        syncAria(h);
      };
      const onUp = () => {
        dragState = null;
        document.removeEventListener('mousemove', onMove);
        document.removeEventListener('mouseup', onUp);
      };
      document.addEventListener('mousemove', onMove);
      document.addEventListener('mouseup', onUp);
    });
    // 双击重置回自适应。
    resizeBar.addEventListener('dblclick', () => {
      miTextareaReset();
      syncAria(textarea.getBoundingClientRect().height);
    });
    // 键盘可达（WCAG 2.2 AA）：上下方向键微调，Home 重置，End 拉到上限。
    resizeBar.addEventListener('keydown', (ev) => {
      const h = parseFloat(textarea.style.height);
      const cur = Number.isFinite(h) && h > 0 ? h : textarea.getBoundingClientRect().height;
      let next = null;
      switch (ev.key) {
        case 'ArrowUp':
          ev.preventDefault();
          miResizeMode = 'manual';
          // 垂直 slider 惯例：向上增大 value（高度）。
          next = clampH(cur + 16);
          break;
        case 'ArrowDown':
          ev.preventDefault();
          miResizeMode = 'manual';
          // 向下减小 value（高度）。
          next = clampH(cur - 16);
          break;
        case 'Home':
          ev.preventDefault();
          miTextareaReset();
          syncAria(textarea.getBoundingClientRect().height);
          return;
        case 'End':
          ev.preventDefault();
          miResizeMode = 'manual';
          next = maxH();
          break;
      }
      if (next !== null) {
        textarea.style.height = next + 'px';
        syncAria(next);
      }
    });
  }
  // 解析当前输入框文本，成功后渲染结果。
  async function runPreview() {
    const yaml = textarea.value;
    if (!yaml.trim()) {
      miFeedback(dshdT('modelImportEmpty'), false);
      return;
    }
    miClearFeedback();
    $('mi-result').hidden = true;
    previewBtn.disabled = true;
    previewBtn.textContent = dshdT('modelImportPreviewing');
    try {
      const preview = await invoke('preview_model_import', { yaml });
      miRenderResult(preview);
    } catch (e) {
      miFeedback(String(e), false);
    } finally {
      previewBtn.disabled = false;
      previewBtn.textContent = dshdT('modelImportPreview');
    }
  }
  previewBtn.addEventListener('click', async () => {
    // “粘贴并解析”：仅当输入框为空时才读取剪贴板填入并解析；输入框已有
    // 内容（手动粘贴）时不覆盖，直接用现有内容解析，避免误删手输配置。
    if (!textarea.value.trim()) {
      let clipboard = '';
      // 走 Tauri 原生命令读剪贴板，避免 WebView 的浏览器剪贴板权限弹窗；
      // 权限由 capability 声明，读取失败时当作空剪贴板回退。
      try {
        clipboard = (await invoke('plugin:clipboard-manager|read_text') || '').trim();
      } catch {
        clipboard = ''; // 无权限或读取失败时当作空剪贴板
      }
      if (clipboard) {
        textarea.value = clipboard;
        miTextareaAutosize();
      }
    }
    await runPreview();
  });
  textarea.addEventListener('input', () => {
    miClearFeedback();
    $('mi-result').hidden = true;
    miPreviewRefs = [];
  });
  const exportBtn = $('mi-export');
  if (exportBtn) {
    exportBtn.addEventListener('click', async () => {
      miClearFeedback();
      try {
        const yaml = await invoke('export_model_config');
        if (!yaml) {
          miFeedback(dshdT('modelExportNone'), false);
          return;
        }
        await navigator.clipboard.writeText(yaml);
        miFeedback(dshdT('modelExportCopied'), true);
      } catch (e) {
        miFeedback(String(e), false);
      }
    });
  }
}
function miRenderResult(preview) {
  miPreviewRefs = preview.api_key_envs || [];
  const box = $('mi-result');
  box.textContent = '';
  const summary = document.createElement('div');
  summary.className = 'srow-desc';
  let text = dshdT('modelImportSummary', { count: preview.providers.length });
  if (preview.replaces_existing) text += ' ' + dshdT('modelImportReplaces');
  summary.textContent = text;
  box.appendChild(summary);
  if (miPreviewRefs.length === 0) {
    const ok = document.createElement('div');
    ok.className = 'mi-ok';
    ok.textContent = dshdT('modelImportNoKeys');
    box.appendChild(ok);
  } else {
    miPreviewRefs.forEach((ref, index) => {
      const row = document.createElement('div');
      row.className = 'mi-key-row';
      const label = document.createElement('label');
      label.className = 'mi-key-label';
      label.textContent = dshdT('modelImportKeyLabel', { ref });
      const input = document.createElement('input');
      input.id = 'model-import-key-' + index;
      input.className = 'dshd-input mi-key-input';
      input.type = 'password';
      input.autocomplete = 'off';
      input.spellcheck = false;
      input.dataset.ref = ref;
      input.placeholder = 'sk-...';
      label.htmlFor = input.id;
      const inputRow = document.createElement('div');
      inputRow.className = 'dshd-password-field';
      const toggle = document.createElement('button');
      toggle.type = 'button';
      toggle.className = 'dshd-x dshd-password-action';
      toggle.setAttribute('aria-controls', input.id);
      toggle.setAttribute('aria-pressed', 'false');
      toggle.hidden = true;
      inputRow.append(input, toggle);
      dshdBindPasswordToggle(input, toggle);
      row.append(label, inputRow);
      box.appendChild(row);
    });
  }
  const actions = document.createElement('div');
  actions.className = 'mi-actions';
  const applyBtn = document.createElement('button');
  applyBtn.type = 'button';
  applyBtn.className = 'mi-btn primary';
  applyBtn.textContent = dshdT('modelImportApply');
  applyBtn.addEventListener('click', async () => {
    const yaml = $('mi-textarea').value;
    const keys = [];
    const filled = new Set();
    box.querySelectorAll('input[data-ref]').forEach((input) => {
      const value = input.value.trim();
      if (value) { keys.push([input.dataset.ref, value]); filled.add(input.dataset.ref); }
    });
    const missing = miPreviewRefs.filter((ref) => !filled.has(ref));
    if (missing.length > 0) {
      miFeedback(dshdT('modelImportKeyMissing', { ref: missing.join(', ') }), false);
      return;
    }
    miClearFeedback();
    applyBtn.disabled = true;
    applyBtn.textContent = dshdT('modelImportApplying');
    let applied = false;
    try {
      await invoke('apply_model_import', { payload: { yaml, keys } });
      applied = true;
      // 成功后收尾：清空粘贴区与凭据行（连同密码可见性按钮），已填 key
      // 不留残态；结果区只保留摘要与成功消息
      $('mi-textarea').value = '';
      miTextareaReset();
      miPreviewRefs = [];
      box.querySelectorAll('.mi-key-row').forEach((row) => row.remove());
      const ok = document.createElement('div');
      ok.className = 'mi-ok';
      ok.textContent = dshdT('modelImportSuccess');
      box.appendChild(ok);
      actions.remove();
      // 导入成功后内容高度骤减、滚动条可能停在原 key 位置：滚回结果区/成功
      // 消息可见（无论模型配置板块在顶部还是下部，scrollIntoView 都自适应）。
      requestAnimationFrame(() => {
        box.scrollIntoView({ block: 'center', behavior: 'auto' });
      });
    } catch (e) {
      miFeedback(String(e), false);
    } finally {
      // 成功时按钮已随操作区移除，不再复位其状态
      if (!applied) {
        applyBtn.disabled = false;
        applyBtn.textContent = dshdT('modelImportApply');
      }
    }
  });
  actions.appendChild(applyBtn);
  box.appendChild(actions);
  box.hidden = false;
  // 结果区/API Key 输入框在下方，展开后可能在视口外：滚动到可见，避免被遮挡。
  // 用 rAF 确保布局完成后滚动；此处在设置页滚动容器（#body）内，scrollIntoView 自动滚动祖先。
  requestAnimationFrame(() => {
    box.scrollIntoView({ block: 'center', behavior: 'auto' });
  });
}
function applySettingState(state) {
  const body = $('body');
  const external = Boolean(state.external_service);
  const externalNote = $('settings-external-note');
  if (externalNote) externalNote.hidden = !external;
  SETTING_KEYS.forEach((key) => {
    const el = body.querySelector('.sswitch[data-key="' + key + '"]');
    if (el) {
      el.checked = Boolean(state[key]);
      if (key === 'auto_update_plugins') el.disabled = external;
      if (key === 'task_notifications') el.disabled = external;
      if (key === 'hide_balance') el.disabled = Boolean(state.hide_statusbar);
    }
  });
  body.querySelectorAll('input[name="dsh-channel"]').forEach((el) => {
    el.checked = el.value === state.dsh_update_channel;
    el.disabled = external;
  });
  body.querySelectorAll('input[name="close_behavior"], input[name="launch_behavior"]').forEach((el) => {
    el.checked = el.value === state[el.name];
  });
  const apiInput = $('settings-api-key');
  const apiSave = $('settings-api-key-save');
  const apiClear = $('settings-api-key-clear');
  const apiToggle = $('settings-api-key-toggle');
  [apiInput, apiSave, apiToggle].forEach((el) => {
    if (!el) return;
    el.dataset.locked = state.api_key_external || external ? '1' : '';
    el.disabled = Boolean(state.api_key_external || external);
  });
  if (apiInput) {
    apiInput.placeholder = state.api_key_set ? dshdT('settingsApiKeyConfigured') : 'sk-...';
  }
  if (apiClear) {
    apiClear.dataset.locked = state.api_key_external || external || !state.api_key_set ? '1' : '';
    apiClear.disabled = Boolean(state.api_key_external || external || !state.api_key_set);
  }
  ['mi-textarea', 'mi-preview', 'mi-export'].forEach((id) => {
    const el = $(id);
    if (el) el.disabled = external;
  });
  if (external) apiKeyFeedback(dshdT('settingsExternalServiceShort'), true);
  else if (state.api_key_external) apiKeyFeedback(dshdT('settingsApiKeyExternal'), true);
}
function showSettingError(message) {
  const err = $('serr');
  if (!err) return;
  err.textContent = message;
  err.hidden = false;
}
function hideSettingError() {
  const err = $('serr');
  if (err) err.hidden = true;
}
async function onSettingToggle(ev) {
  const input = ev.target;
  if (settingsBusy) {
    // 并发切换中：弹回（等待中的 invoke 返回后统一应用最新状态）
    input.checked = !input.checked;
    return;
  }
  settingsBusy = true;
  input.disabled = true;
  hideSettingError();
  try {
    const state = await invoke('settings_set', {
      key: input.dataset.key,
      value: input.checked,
    });
    applySettingState(state);
  } catch (e) {
    input.checked = !input.checked;
    showSettingError(dshdT('settingsFailed', { message: String(e) }));
  } finally {
    settingsBusy = false;
    if (input.dataset.key === 'auto_update_plugins' || input.dataset.key === 'task_notifications') input.disabled = Boolean(document.querySelector('#settings-external-note:not([hidden])'));
    else if (input.dataset.key === 'hide_balance') input.disabled = Boolean($('body').querySelector('.sswitch[data-key="hide_statusbar"]')?.checked);
    else input.disabled = false;
  }
}
