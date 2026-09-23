// 控制中心：用户设置。

// —— 设置（按桌面行为 / 界面 / 服务与运行时三组渲染，见 renderSettings）——
let settingsBusy = false;
// 最近一次 applySettingState 的状态快照：radio 在忙碌期的同步弹回用它取
// "已应用值"（否则只能等在途 invoke 的状态回填，期间显示错误选择）
let settingsStateCache = null;
// 开关项状态键（渲染顺序见 renderSettings；此数组只用于状态绑定遍历）
const SETTING_KEYS = ['autostart', 'task_notifications', 'hide_tool_calls', 'hide_balance', 'auto_update_plugins'];
// radio 组名 → 状态键（dsh-channel 特例，其余同名）
function revertRadiosToApplied(name) {
  const state = settingsStateCache;
  if (!state) return;
  const key = name === 'dsh-channel' ? 'dsh_update_channel' : name;
  const applied = state[key];
  if (applied == null) return;
  document.querySelectorAll('input[name="' + name + '"]').forEach((radio) => {
    radio.checked = radio.value === applied;
  });
}
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
// 每日用量提醒阈值：可见 label + type=number + 行内错误（ux 规则：
// label 不靠 placeholder、错误挂在字段旁、保存给反馈）
function usageLimitRow() {
  return (
    '<div class="srow">' +
    '<span class="srow-txt">' +
    '<label class="srow-name" for="settings-usage-limit">' + dshdT('settingsUsageLimit') + '</label>' +
    '<span class="srow-desc">' + dshdT('settingsUsageLimitDesc') + '</span>' +
    '</span>' +
    '<span class="usage-limit-field">' +
    '<input id="settings-usage-limit" class="dshd-input" type="number" inputmode="numeric" min="1" max="1000000" step="1" placeholder="' + esc(dshdT('settingsUsageLimitPlaceholder')) + '" aria-describedby="settings-usage-limit-unit settings-usage-limit-error" />' +
    '<span id="settings-usage-limit-unit" class="usage-limit-unit">' + dshdT('settingsUsageLimitUnit') + '</span>' +
    '</span>' +
    '</div>' +
    '<div id="settings-usage-limit-error" class="usage-limit-error" role="alert" hidden></div>'
  );
}
function dshChannelRow() {  return (
    '<div class="srow">' +
    '<span class="srow-txt">' +
    '<span class="srow-name">' + dshdT('settingsDshChannel') + '</span>' +
    '<span class="srow-desc">' + dshdT('settingsDshChannelDesc') + '</span>' +
    '</span>' +
    '<span class="dshd-seg" id="dsh-channel" role="radiogroup" aria-label="' + esc(dshdT('settingsDshChannel')) + '">' +
    '<label class="dshd-seg-opt"><input type="radio" name="dsh-channel" value="latest" /><span>' + esc(dshdT('settingsChannelLatest')) + '</span></label>' +
    '<label class="dshd-seg-opt"><input type="radio" name="dsh-channel" value="next" /><span>' + esc(dshdT('settingsChannelNext')) + '</span></label>' +
    '<label class="dshd-seg-opt"><input type="radio" name="dsh-channel" value="alpha" /><span>' + esc(dshdT('settingsChannelAlpha')) + '</span></label>' +
    '</span>' +
    '</div>'
  );
}
async function renderSettings() {
  const body = $('body');
  // 先拿到配置状态再一次性渲染，避免「先渲染再回填」造成闪烁。
  // 等候期间给出轻量占位。
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
  const desktopSec =
    '<section class="psection settings-section" aria-labelledby="settings-desktop-heading">' +
    '<h3 id="settings-desktop-heading">' + dshdT('settingsDesktopTitle') + '</h3>' +
    settingsRow('autostart', 'autostart', 'settingsAutostartDesc') +
    settingsRow('task_notifications', 'settingsTaskNotifications', 'settingsTaskNotificationsDesc') +
    usageLimitRow() +
    behaviorRow('launch_behavior', 'settingsLaunchBehavior', 'settingsLaunchBehaviorDesc', 'window', 'settingsLaunchWindow', 'tray', 'settingsLaunchTray') +
    behaviorRow('close_behavior', 'settingsCloseBehavior', 'settingsCloseBehaviorDesc', 'tray', 'settingsCloseTray', 'quit', 'settingsCloseQuit') +
    '</section>';
  const interfaceSec =
    '<section class="psection settings-section" aria-labelledby="settings-interface-heading">' +
    '<h3 id="settings-interface-heading">' + dshdT('settingsInterfaceTitle') + '</h3>' +
    '<p class="settings-follow-note">' + dshdT('settingsFollowDshNote') + '</p>' +
    settingsRow('hide_tool_calls', 'settingsHideTools', 'settingsHideToolsDesc') +
    settingsRow('hide_balance', 'settingsHideBalance', 'settingsHideBalanceDesc') +
    '</section>';
  const runtimeSec =
    '<section class="psection settings-section" aria-labelledby="settings-runtime-heading">' +
    '<h3 id="settings-runtime-heading">' + dshdT('settingsRuntimeTitle') + '</h3>' +
    '<div id="settings-external-note" class="settings-scope-note" role="status" hidden>' +
    '<span>' + dshdT('settingsExternalServiceNote') + '</span>' +
    '<button type="button" id="settings-use-local" class="dshd-btn small">' + dshdT('useLocalService') + '</button>' +
    '</div>' +
    dshChannelRow() +
    settingsRow('auto_update_plugins', 'settingsAutoUpdatePlugins', 'settingsAutoUpdatePluginsDesc') +
    '</section>';
  body.innerHTML = desktopSec + interfaceSec + runtimeSec;
  body.querySelectorAll('.sswitch').forEach((el) => {
    el.addEventListener('change', onSettingToggle);
  });
  body.querySelectorAll('input[name="dsh-channel"]').forEach((el) => {
    el.addEventListener('change', async () => {
      if (!el.checked) return;
      if (settingsBusy) {
        // 并发切换中：同步弹回已应用值（与开关行的弹回行为一致；等待中的
        // invoke 返回后统一应用最新状态），原生 radio 已乐观切到新值
        revertRadiosToApplied(el.name);
        return;
      }
      settingsBusy = true;
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
      if (!el.checked) return;
      if (settingsBusy) {
        revertRadiosToApplied(el.name);
        return;
      }
      settingsBusy = true;
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
  else showSettingError(dshdT('settingsLoadFailed'));
  initUsageLimit();
  $('settings-use-local').addEventListener('click', async () => {
    const button = $('settings-use-local');
    button.disabled = true;
    button.setAttribute('aria-busy', 'true');
    try {
      await invoke('use_local_service');
      await invoke('app_dialog_close');
    } catch (e) {
      showSettingError(dshdT('settingsFailed', { message: String(e) }));
      button.disabled = false;
      button.removeAttribute('aria-busy');
    }
  });
}
// —— 每日用量提醒阈值：change（失焦/回车）即保存；空值 = 关闭 ——
function usageLimitFeedback(message, isError) {
  const err = $('settings-usage-limit-error');
  const input = $('settings-usage-limit');
  if (!err || !input) return;
  if (!message) {
    err.hidden = true;
    err.textContent = '';
    input.removeAttribute('aria-invalid');
    return;
  }
  if (isError) {
    err.hidden = false;
    err.textContent = message;
    input.setAttribute('aria-invalid', 'true');
  } else {
    err.hidden = true;
    dshdToast(message, { kind: 'ok' });
  }
}
function initUsageLimit() {
  const input = $('settings-usage-limit');
  if (!input) return;
  input.addEventListener('input', () => usageLimitFeedback('', false));
  input.addEventListener('change', async () => {
    const raw = input.value.trim();
    if (!raw) {
      // 清空 = 关闭提醒
      usageLimitFeedback('', false);
      // 与开关/radio 同一串行口径：忙碌期丢弃本次 change，避免与在途
      // invoke 的状态回填乱序（输入值仍在框内，下次改动会再触发）
      if (settingsBusy) return;
      settingsBusy = true;
      try {
        const state = await invoke('set_usage_token_limit', { limitM: null });
        applySettingState(state);
      } catch (e) {
        showSettingError(dshdT('settingsFailed', { message: String(e) }));
        invoke('settings_get').then(applySettingState).catch(() => {});
      } finally {
        settingsBusy = false;
      }
      return;
    }
    const value = Number(raw);
    if (!Number.isInteger(value) || value < 1 || value > 1000000) {
      usageLimitFeedback(dshdT('settingsUsageLimitInvalid'), true);
      input.focus();
      return;
    }
    usageLimitFeedback('', false);
    if (settingsBusy) return;
    settingsBusy = true;
    try {
      const state = await invoke('set_usage_token_limit', { limitM: value });
      applySettingState(state);
      usageLimitFeedback(dshdT('settingsUsageLimitSaved'), false);
    } catch (e) {
      showSettingError(dshdT('settingsFailed', { message: String(e) }));
      invoke('settings_get').then(applySettingState).catch(() => {});
    } finally {
      settingsBusy = false;
    }
  });
}
function applySettingState(state) {
  const body = $('body');
  settingsStateCache = state;
  const external = Boolean(state.external_service);
  const externalNote = $('settings-external-note');
  if (externalNote) externalNote.hidden = !external;
  SETTING_KEYS.forEach((key) => {
    const el = body.querySelector('.sswitch[data-key="' + key + '"]');
    if (el) {
      el.checked = Boolean(state[key]);
      if (key === 'auto_update_plugins') el.disabled = external;
      if (key === 'task_notifications') el.disabled = external;
    }
  });
  body.querySelectorAll('input[name="dsh-channel"]').forEach((el) => {
    el.checked = el.value === state.dsh_update_channel;
    el.disabled = external;
  });
  body.querySelectorAll('input[name="close_behavior"], input[name="launch_behavior"]').forEach((el) => {
    el.checked = el.value === state[el.name];
  });
  const limitInput = $('settings-usage-limit');
  if (limitInput && document.activeElement !== limitInput) {
    limitInput.value = state.usage_token_limit_m != null ? String(state.usage_token_limit_m) : '';
  }
}
// 设置操作失败统一走 toast（原页底 #serr 位置太偏，不易察觉）
function showSettingError(message) {
  dshdToast(message, { kind: 'err' });
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
    else input.disabled = false;
  }
}
