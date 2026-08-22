// 控制中心：用户设置与模型配置导入/导出。

// —— 设置（按桌面行为 / 界面 / 服务能力 / 凭据与模型分组） ——
let settingsBusy = false;
const SETTING_KEYS = ['autostart', 'hide_tool_calls', 'hide_stats_line', 'hide_statusbar', 'hide_balance', 'auto_update_plugins'];
function settingsRow(key, nameKey, descKey, className) {
  return (
    '<label class="srow' + (className ? ' ' + className : '') + '">' +
    '<span class="srow-txt">' +
    '<span class="srow-name">' + dshdT(nameKey) + '</span>' +
    '<span class="srow-desc">' + dshdT(descKey) + '</span>' +
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
function renderSettings() {
  const body = $('body');
  body.innerHTML =
    '<section class="psection settings-section" aria-labelledby="settings-desktop-heading">' +
    '<h3 id="settings-desktop-heading">' + dshdT('settingsDesktopTitle') + '</h3>' +
    settingsRow('autostart', 'autostart', 'settingsAutostartDesc') +
    behaviorRow('launch_behavior', 'settingsLaunchBehavior', 'settingsLaunchBehaviorDesc', 'window', 'settingsLaunchWindow', 'tray', 'settingsLaunchTray') +
    behaviorRow('close_behavior', 'settingsCloseBehavior', 'settingsCloseBehaviorDesc', 'tray', 'settingsCloseTray', 'quit', 'settingsCloseQuit') +
    '</section>' +
    '<section class="psection settings-section" aria-labelledby="settings-interface-heading">' +
    '<h3 id="settings-interface-heading">' + dshdT('settingsInterfaceTitle') + '</h3>' +
    settingsRow('hide_tool_calls', 'settingsHideTools', 'settingsHideToolsDesc') +
    settingsRow('hide_stats_line', 'settingsHideStats', 'settingsHideStatsDesc') +
    settingsRow('hide_statusbar', 'settingsHideStatusbar', 'settingsHideStatusbarDesc') +
    settingsRow('hide_balance', 'settingsHideBalance', 'settingsHideBalanceDesc', 'srow-dependent') +
    '</section>' +
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
    '</section>' +
    // 模型配置导入/导出：独立卡片，随时可用
    '<section class="psection settings-section mi-box" aria-labelledby="settings-model-heading">' +
    '<h3 id="settings-model-heading">' + dshdT('modelImportTitle') + '</h3>' +
    '<div class="mi-card">' +
    '<div class="mi-card-head">' +
    '<label class="mi-card-name" for="mi-textarea">' + dshdT('modelImportPaste') + '</label>' +
    '<span class="mi-card-desc">' + dshdT('modelImportHint') + '</span>' +
    '</div>' +
    '<textarea id="mi-textarea" class="dshd-input dshd-textarea mi-textarea" rows="5" spellcheck="false" placeholder="' + esc(dshdT('modelImportPlaceholder')) + '"></textarea>' +
    '<div class="mi-actions">' +
    '<button type="button" id="mi-preview" class="mi-btn primary">' + dshdT('modelImportPreview') + '</button>' +
    '<button type="button" id="mi-export" class="mi-btn">' + dshdT('modelExport') + '</button>' +
    '<span id="mi-feedback" class="mi-feedback" role="status" aria-live="polite"></span>' +
    '</div>' +
    '<div id="mi-result" class="mi-result" hidden role="status" aria-live="polite"></div>' +
    '</div>' +
    '</section>' +
    '<div class="serr" id="serr" role="alert" aria-live="polite" hidden></div>';
  body.querySelectorAll('.sswitch').forEach((el) => {
    el.addEventListener('change', onSettingToggle);
  });
  body.querySelectorAll('input[name="dsh-channel"]').forEach((el) => {
    el.addEventListener('change', () => {
      if (!el.checked) return;
      invoke('set_dsh_channel', { channel: el.value })
        .then((state) => {
          applySettingState(state);
          dshChannelChanged = true;
        })
        .catch((e) => {
          showSettingError(e);
          // 失败回滚：重新拉取真实状态回填 checked（radio 已乐观选中新值）
          invoke('settings_get').then(applySettingState).catch(() => {});
        });
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
  invoke('settings_get').then(applySettingState).catch((e) => showSettingError(e));
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
function initModelImport() {
  const textarea = $('mi-textarea');
  const previewBtn = $('mi-preview');
  if (!textarea || !previewBtn) return;
  previewBtn.addEventListener('click', async () => {
    const yaml = textarea.value;
    if (!yaml.trim()) { miFeedback(dshdT('modelImportEmpty'), false); return; }
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
    try {
      await invoke('apply_model_import', { payload: { yaml, keys } });
      box.querySelectorAll('input[data-ref]').forEach((input) => {
        input.value = '';
        input.type = 'password';
        const toggle = input.parentElement && input.parentElement.querySelector('[data-dshd-password-toggle]');
        if (toggle && toggle.__dshdPasswordSync) toggle.__dshdPasswordSync();
      });
      const ok = document.createElement('div');
      ok.className = 'mi-ok';
      ok.textContent = dshdT('modelImportSuccess');
      box.appendChild(ok);
      applyBtn.remove();
    } catch (e) {
      miFeedback(String(e), false);
    } finally {
      applyBtn.disabled = false;
      applyBtn.textContent = dshdT('modelImportApply');
    }
  });
  actions.appendChild(applyBtn);
  box.appendChild(actions);
  box.hidden = false;
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
    if (input.dataset.key === 'auto_update_plugins') input.disabled = Boolean(document.querySelector('#settings-external-note:not([hidden])'));
    else if (input.dataset.key === 'hide_balance') input.disabled = Boolean($('body').querySelector('.sswitch[data-key="hide_statusbar"]')?.checked);
    else input.disabled = false;
  }
}
