// 控制中心：用户设置与模型配置导入/导出。

// —— 设置（统一弹窗内：开机自启 / 隐藏工具调用 / 隐藏统计行） ——
let settingsBusy = false;
const SETTING_KEYS = ['autostart', 'hide_tool_calls', 'hide_stats_line', 'hide_statusbar', 'hide_balance', 'auto_update_plugins'];
function settingsRow(key, nameKey, descKey) {
  return (
    '<label class="srow">' +
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
function renderSettings() {
  const body = $('body');
  body.innerHTML =
    '<div class="psection api-key-box">' +
    '<h3>' + dshdT('settingsApiKeyTitle') + '</h3>' +
    '<div class="mi-card">' +
    '<div class="mi-card-head">' +
    '<label class="mi-card-name" for="settings-api-key">DeepSeek API Key</label>' +
    '<span id="settings-api-key-help" class="mi-card-desc">' + dshdT('settingsApiKeyDesc') + '</span>' +
    '</div>' +
    '<div class="api-key-input-row">' +
    '<input id="settings-api-key" class="mi-key-input" type="password" autocomplete="off" spellcheck="false" placeholder="sk-..." aria-describedby="settings-api-key-help settings-api-key-feedback" />' +
    '<button type="button" id="settings-api-key-toggle" class="mi-btn" aria-controls="settings-api-key" aria-pressed="false">' + dshdT('settingsApiKeyShow') + '</button>' +
    '</div>' +
    '<div class="mi-actions">' +
    '<button type="button" id="settings-api-key-save" class="mi-btn primary">' + dshdT('settingsApiKeySave') + '</button>' +
    '<button type="button" id="settings-api-key-clear" class="mi-btn">' + dshdT('settingsApiKeyClear') + '</button>' +
    '<span id="settings-api-key-feedback" class="mi-feedback api-key-feedback" role="status" aria-live="polite"></span>' +
    '</div>' +
    '</div>' +
    '</div>' +
    '<div class="psection">' +
    '<h3>' + dshdT('preferences') + '</h3>' +
    settingsRow('autostart', 'autostart', 'settingsAutostartDesc') +
    behaviorRow('close_behavior', 'settingsCloseBehavior', 'settingsCloseBehaviorDesc', 'tray', 'settingsCloseTray', 'quit', 'settingsCloseQuit') +
    behaviorRow('launch_behavior', 'settingsLaunchBehavior', 'settingsLaunchBehaviorDesc', 'window', 'settingsLaunchWindow', 'tray', 'settingsLaunchTray') +
    settingsRow('hide_tool_calls', 'settingsHideTools', 'settingsHideToolsDesc') +
    settingsRow('hide_stats_line', 'settingsHideStats', 'settingsHideStatsDesc') +
    settingsRow('hide_statusbar', 'settingsHideStatusbar', 'settingsHideStatusbarDesc') +
    settingsRow('hide_balance', 'settingsHideBalance', 'settingsHideBalanceDesc') +
    settingsRow('auto_update_plugins', 'settingsAutoUpdatePlugins', 'settingsAutoUpdatePluginsDesc') +
    '<label class="srow">' +
    '<span class="srow-txt">' +
    '<span class="srow-name">' + dshdT('settingsDshChannel') + '</span>' +
    '<span class="srow-desc">' + dshdT('settingsDshChannelDesc') + '</span>' +
    '</span>' +
    '<span class="dshd-seg" id="dsh-channel" role="radiogroup" aria-label="' + esc(dshdT('settingsDshChannel')) + '">' +
    '<label class="dshd-seg-opt"><input type="radio" name="dsh-channel" value="latest" /><span>' + esc(dshdT('settingsChannelLatest')) + '</span></label>' +
    '<label class="dshd-seg-opt"><input type="radio" name="dsh-channel" value="next" /><span>' + esc(dshdT('settingsChannelNext')) + '</span></label>' +
    '</span>' +
    '</label>' +
    '</div>' +
    // 模型配置导入/导出：独立卡片，随时可用
    '<div class="psection mi-box">' +
    '<h3>' + dshdT('modelImportTitle') + '</h3>' +
    '<div class="mi-card">' +
    '<div class="mi-card-head">' +
    '<label class="mi-card-name" for="mi-textarea">' + dshdT('modelImportPaste') + '</label>' +
    '<span class="mi-card-desc">' + dshdT('modelImportHint') + '</span>' +
    '</div>' +
    '<textarea id="mi-textarea" class="mi-textarea" rows="5" spellcheck="false" placeholder="' + esc(dshdT('modelImportPlaceholder')) + '"></textarea>' +
    '<div class="mi-actions">' +
    '<button type="button" id="mi-preview" class="mi-btn primary">' + dshdT('modelImportPreview') + '</button>' +
    '<button type="button" id="mi-export" class="mi-btn">' + dshdT('modelExport') + '</button>' +
    '<span id="mi-feedback" class="mi-feedback" role="status" aria-live="polite"></span>' +
    '</div>' +
    '<div id="mi-result" class="mi-result" hidden role="status" aria-live="polite"></div>' +
    '</div>' +
    '</div>' +
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
  toggle.addEventListener('click', () => {
    const show = input.type === 'password';
    input.type = show ? 'text' : 'password';
    toggle.textContent = dshdT(show ? 'settingsApiKeyHide' : 'settingsApiKeyShow');
    toggle.setAttribute('aria-pressed', show ? 'true' : 'false');
    input.focus();
  });
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
    for (const ref of miPreviewRefs) {
      const row = document.createElement('label');
      row.className = 'mi-key-row';
      const label = document.createElement('span');
      label.className = 'mi-key-label';
      label.textContent = dshdT('modelImportKeyLabel', { ref });
      const input = document.createElement('input');
      input.className = 'mi-key-input';
      input.type = 'password';
      input.autocomplete = 'off';
      input.spellcheck = false;
      input.dataset.ref = ref;
      input.placeholder = dshdT('modelImportKeyHint');
      row.appendChild(label);
      row.appendChild(input);
      box.appendChild(row);
    }
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
  SETTING_KEYS.forEach((key) => {
    const el = body.querySelector('.sswitch[data-key="' + key + '"]');
    if (el) el.checked = Boolean(state[key]);
  });
  body.querySelectorAll('input[name="dsh-channel"]').forEach((el) => {
    el.checked = el.value === state.dsh_update_channel;
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
    el.dataset.locked = state.api_key_external ? '1' : '';
    el.disabled = Boolean(state.api_key_external);
  });
  if (apiInput) {
    apiInput.placeholder = state.api_key_set ? dshdT('settingsApiKeyConfigured') : 'sk-...';
  }
  if (apiClear) {
    apiClear.dataset.locked = state.api_key_external || !state.api_key_set ? '1' : '';
    apiClear.disabled = Boolean(state.api_key_external || !state.api_key_set);
  }
  if (state.api_key_external) apiKeyFeedback(dshdT('settingsApiKeyExternal'), true);
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
    input.disabled = false;
  }
}
