// 控制中心：插件搜索、安装、卸载与更新。

// —— 插件管理（统一弹窗内） ——
let pluginsBusy = false;
let pluginSearchSeq = 0;
let pluginApplyStamp = '';
// 当前选中的分类（'' 为「全部」）。分类与搜索框关键词叠加为查询条件，
// 切换分类不清空已输入的关键词。
let activeCat = '';
const CAT_QUERIES = {
  skin: 'dsh-plugin skin OR dsh-web-ui OR dsh-ui theme',
  tool: 'dsh-plugin tool',
  workflow: 'dsh-plugin workflow',
  memory: 'dsh-plugin memory',
  network: 'dsh-plugin mcp OR http OR lan OR notify',
};
// 关键词与分类叠加成 npm registry 查询：分类基准词 + 用户关键词。
// 未知分类（CAT_QUERIES 未配置）按无分类处理，避免拼出 "undefined <kw>"。
function buildQuery(kw, cat) {
  const base = (cat && CAT_QUERIES[cat]) || '';
  const q = (kw || '').trim();
  if (base && q) return base + ' ' + q;
  return base || q;
}
function renderPlugins() {
  // 重新进入页面时让上一次尚未返回的搜索失效，避免旧结果覆盖新页面。
  pluginSearchSeq += 1;
  // DOM 重建后按钮回到「全部」高亮，分类状态必须同步重置，否则
  // 搜索会用残留的旧分类叠加，与界面显示不符。
  activeCat = '';
  const body = $('body');
  body.innerHTML =
    '<div class="psearch">' +
    '<div class="psearch-field">' +
    '<label class="psearch-label" for="p-query">' + esc(dshdT('pluginSearchHint')) + '</label>' +
    '<div class="psearch-control">' +
    '<span class="psearch-ic" aria-hidden="true"><svg viewBox="0 0 24 24" focusable="false"><circle cx="11" cy="11" r="7"></circle><path d="M21 21l-4.35-4.35"></path></svg></span>' +
    '<input class="dshd-input pinput" id="p-query" type="search" placeholder="dshmarket" autocomplete="off" spellcheck="false" />' +
    '</div></div>' +
    '<button type="button" class="dshd-btn" id="p-search">' + esc(dshdT('pluginSearch')) + '</button>' +
    '</div>' +
    '<div class="pcats">' +
    '<button type="button" class="pcat-btn active" data-cat="" aria-pressed="true">' + esc(dshdT('pluginCatAll')) + '</button>' +
    '<button type="button" class="pcat-btn" data-cat="skin" aria-pressed="false">' + esc(dshdT('pluginCatSkin')) + '</button>' +
    '<button type="button" class="pcat-btn" data-cat="tool" aria-pressed="false">' + esc(dshdT('pluginCatTool')) + '</button>' +
    '<button type="button" class="pcat-btn" data-cat="workflow" aria-pressed="false">' + esc(dshdT('pluginCatWorkflow')) + '</button>' +
    '<button type="button" class="pcat-btn" data-cat="memory" aria-pressed="false">' + esc(dshdT('pluginCatMemory')) + '</button>' +
    '<button type="button" class="pcat-btn" data-cat="network" aria-pressed="false">' + esc(dshdT('pluginCatNetwork')) + '</button>' +
    '</div>' +
    '<div class="pstatus" id="p-status" role="status" aria-live="polite"></div>' +
    '<div class="papply hidden" id="p-apply" role="status" aria-live="polite">' +
    '<div class="papply-copy"><strong id="p-apply-title"></strong><span id="p-apply-detail"></span></div>' +
    '<button type="button" class="dshd-btn primary" id="p-apply-btn">' + esc(dshdT('pluginApply')) + '</button>' +
    '</div>' +
    '<div class="psection hidden" id="p-results-sec">' +
    '<h3>' + esc(dshdT('pluginResults')) + ' <span class="pcount" id="p-results-count"></span></h3>' +
    '<ul class="plist" id="p-results"></ul></div>' +
    '<div class="psection" id="p-installed-sec"><h3>' + esc(dshdT('pluginInstalledTitle')) + ' <span class="pcount" id="p-installed-count"></span>' +
    ' <button type="button" class="dshd-btn small" id="p-builtin-check">' + esc(dshdT('pluginCheckUpdates')) + '</button></h3>' +
    '<ul class="plist" id="p-installed"></ul></div>';
  // 右上角关闭已够，右下角不再放纯关闭按钮（dsh 原生设置同）
  // 无底部操作区（dsh 设置弹窗无 footer）
  $('p-search').addEventListener('click', () => doPluginSearch(buildQuery($('p-query').value, activeCat)));
  $('p-query').addEventListener('keydown', (e) => { if (e.key === 'Enter') doPluginSearch(buildQuery($('p-query').value, activeCat)); });
  $('p-builtin-check').addEventListener('click', () => refreshBuiltinStatus(false));
  $('p-apply-btn').addEventListener('click', doPluginApply);
  document.querySelectorAll('.pcat-btn').forEach((btn) => {
    btn.addEventListener('click', () => {
      document.querySelectorAll('.pcat-btn').forEach((b) => {
        const active = b === btn;
        b.classList.toggle('active', active);
        b.setAttribute('aria-pressed', String(active));
      });
      activeCat = btn.dataset.cat || '';
      const kw = $('p-query').value.trim();
      if (activeCat) {
        // 分类：关键词已输入则叠加过滤，否则按分类查询
        if (kw) doPluginSearch(buildQuery(kw, activeCat));
        else doPluginSearch(CAT_QUERIES[activeCat]);
      } else if (kw) {
        // 「全部」：保留输入，仅按关键词搜索
        doPluginSearch(kw);
      } else {
        // 「全部」且无关键词：回到默认态——清空搜索，结果区隐藏、已装区恢复
        pluginSearchSeq += 1;
        $('p-results-sec').classList.add('hidden');
        $('p-results').innerHTML = '';
        $('p-installed-sec').classList.remove('hidden');
        pluginStatus('');
      }
    });
  });
  refreshPlugins();
  refreshPluginApplyStatus();
  // 进入页面自动检查：静默（行内状态已表达结果，不刷“检查完成”提示）
  refreshBuiltinStatus(true);
}

function renderPluginApplyStatus(status) {
  if (openKind !== 'plugins') return;
  const box = $('p-apply');
  if (!box) return;
  const visible = status && (status.pending || status.applying || status.error);
  box.classList.toggle('hidden', !visible);
  if (!visible) return;
  const button = $('p-apply-btn');
  let title = dshdT('pluginApplyPending');
  let detail = dshdT('pluginApplyPendingDesc');
  if (status.applying) {
    title = dshdT('pluginApplying');
    detail = dshdT('pluginApplyingDesc');
  } else if (status.waiting_for_idle) {
    title = dshdT('pluginApplyQueued');
    detail = dshdT('pluginApplyQueuedDesc');
  } else if (status.error) {
    title = dshdT('pluginApplyFailed');
    detail = String(status.error);
  }
  $('p-apply-title').textContent = title;
  $('p-apply-detail').textContent = detail;
  button.disabled = !!(status.applying || status.waiting_for_idle || pluginsBusy);
  button.textContent = status.error ? dshdT('retry') : dshdT('pluginApply');
}

async function refreshPluginApplyStatus() {
  try {
    const status = await invoke('plugin_apply_status');
    if (openKind !== 'plugins') return;
    const key = JSON.stringify(status || null);
    if (key === pluginApplyStamp) return;
    pluginApplyStamp = key;
    renderPluginApplyStatus(status);
  } catch (e) {}
}

async function doPluginApply() {
  const button = $('p-apply-btn');
  if (!button || button.disabled) return;
  button.disabled = true;
  try {
    const status = await invoke('plugin_apply_changes');
    pluginApplyStamp = '';
    renderPluginApplyStatus(status);
  } catch (e) {
    pluginStatus(dshdT('pluginFailed', { message: e }), 'err');
    button.disabled = false;
  }
}
// —— 插件更新状态（检查更新覆盖全部已安装插件；内置标签 + 版本对比 + 按包更新） ——
let updateBusy = false;
// 检查更新序号：连点/并发时旧结果不覆盖新结果（仿 pluginSearchSeq）
let updateCheckSeq = 0;
// pkg → UpdateStatus：plugin_updates 的结果缓存，渲染已安装列表时使用
let updateStatus = new Map();
// silent=true：自动检查（进入页面/安装后刷新），不显示结果提示——
// 行内版本状态已表达检查结果；手动点击“检查更新”才显示汇总提示
async function refreshBuiltinStatus(silent) {
  const seq = ++updateCheckSeq;
  try {
    const list = await invoke('plugin_updates');
    if (openKind !== 'plugins' || seq !== updateCheckSeq) return;
    updateStatus = new Map((list || []).map((s) => [s.pkg, s]));
    await refreshPlugins();
    if (silent) return;
    // 冷却期内的新版已不计入 update_available：统计即“现在可更新”的数量
    const updatable = (list || []).filter((s) => s.update_available).length;
    if (updatable > 0) {
      pluginStatus(dshdT('pluginUpdatesFound', { count: updatable }), '');
    } else {
      pluginStatus(dshdT('pluginUpdatesChecked'), 'ok');
    }
  } catch (e) {
    if (openKind === 'plugins' && seq === updateCheckSeq && !silent) {
      pluginStatus(dshdT('pluginUpdateFailed', { message: e }), 'err');
    }
  }
}
// 插件的更新状态描述（显示在行内 desc 位置；仅在有值得注意的状态时替换插件描述）
function updateStatusText(st) {
  if (!st) return null;
  if (st.error) return st.error;
  if (!st.installed) return dshdT('pluginNotInstalled');
  if (st.cooldown_until) return dshdT('pluginCooldown', { version: cleanVersion(st.latest) });
  if (st.update_available) return dshdT('pluginUpdateAvailable');
  return null; // 已是最新：保持插件描述，无箭头无按钮即暗示
}
async function doPluginUpdate(pkg) {
  if (updateBusy) return;
  updateBusy = true;
  setPluginsBusy(true);
  const checkBtn = $('p-builtin-check');
  if (checkBtn) checkBtn.disabled = true;
  pluginStatus(dshdT('pluginUpdating'));
  try {
    const st = await invoke('plugin_update', { name: pkg });
    if (openKind !== 'plugins') return;
    if (st && st.error) {
      pluginStatus(dshdT('pluginUpdateFailed', { message: st.error }), 'err');
    } else if (st && st.cooldown_until) {
      pluginStatus(dshdT('pluginCooldown', { version: cleanVersion(st.latest) }), 'err');
    } else {
      pluginStatus(dshdT('pluginUpdated'), 'ok');
    }
    updateStatus = new Map(updateStatus).set(pkg, st);
    await refreshPlugins();
    await refreshPluginApplyStatus();
  } catch (e) {
    pluginStatus(dshdT('pluginUpdateFailed', { message: e }), 'err');
  } finally {
    updateBusy = false;
    setPluginsBusy(false);
    if (checkBtn) checkBtn.disabled = false;
  }
}
function pluginStatus(text, kind) {
  const el = $('p-status');
  if (!el) return;
  el.textContent = text || '';
  el.className = 'pstatus' + (kind ? ' ' + kind : '');
}
async function refreshPlugins() {
  try {
    const list = await invoke('plugin_list');
    if (openKind !== 'plugins') return;
    renderInstalled(list || []);
  } catch (e) { pluginStatus(dshdT('pluginFailed', { message: e }), 'err'); }
}
async function doPluginSearch(query) {
  const q = (query != null ? query : $('p-query').value).trim();
  if (!q) return;
  const seq = ++pluginSearchSeq;
  pluginStatus(dshdT('pluginSearching'));
  try {
    const list = await invoke('plugin_search', { query: q });
    if (openKind !== 'plugins' || seq !== pluginSearchSeq) return;
    renderResults(list || []);
    pluginStatus(dshdT('pluginSearchDone', { count: (list || []).length }), 'ok');
  } catch (e) {
    if (openKind === 'plugins' && seq === pluginSearchSeq) {
      pluginStatus(dshdT('pluginFailed', { message: e }), 'err');
    }
  }
}
function setPluginsBusy(busy) {
  pluginsBusy = busy;
  document.querySelectorAll('.plugin-action').forEach((button) => {
    button.disabled = busy;
  });
  const applyButton = $('p-apply-btn');
  if (applyButton && busy) applyButton.disabled = true;
  if (!busy) refreshPluginApplyStatus();
}
function pluginItemRow(p, actions, verText, descText) {
  const li = document.createElement('li');
  li.className = 'pitem';
  // 包名首字母徽标（无图标源时的视觉锚点）
  const mark = document.createElement('span');
  mark.className = 'pmark';
  mark.setAttribute('aria-hidden', 'true');
  const seg = p.name.replace(/^@[^/]+\//, ''); // 去 scope 前缀
  mark.textContent = (seg[0] || '?').toUpperCase();
  li.append(mark);
  const info = document.createElement('div');
  info.className = 'info';
  const name = document.createElement('div');
  name.className = 'name';
  if (p.builtin) {
    // 内置插件：包名 + 低调标签（单行不换行）
    const txt = document.createElement('span');
    txt.textContent = p.name;
    const badge = document.createElement('span');
    badge.className = 'pbadge';
    badge.textContent = dshdT('pluginBuiltin');
    name.append(txt, badge);
  } else {
    name.textContent = p.name;
  }
  name.dataset.truncTip = '';
  const desc = document.createElement('div');
  desc.className = 'desc';
  // 内置插件行传入更新状态文本（新版本/冷却中/失败等），否则显示插件描述
  desc.textContent = descText != null ? descText : (p.description || '');
  // 截断才提示（显示完整无额外信息则不设 title）
  desc.dataset.truncTip = '';
  info.append(name, desc);
  li.append(info);
  if (p.version || verText) {
    const ver = document.createElement('span');
    ver.className = 'ver';
    ver.textContent = verText || cleanVersion(p.version);
    li.append(ver);
  }
  li.append(actions);
  return li;
}
// 版本 spec 清理：package.json 里的 ^1.8.0 / ~1.2 / v2.0.1 → 1.8.0
function cleanVersion(v) {
  return String(v || '').replace(/^[vV^~<>= ]+/, '').trim() || '?';
}
function renderInstalled(list) {
  const ul = $('p-installed');
  if (!ul) return;
  const countEl = $('p-installed-count');
  if (countEl) countEl.textContent = list.length ? '(' + list.length + ')' : '';
  ul.textContent = '';
  if (!list.length) {
    const e = document.createElement('li');
    e.className = 'pempty';
    e.textContent = dshdT('pluginNone');
    ul.append(e);
    applyTruncationTips($('body'));
    return;
  }
  for (const p of list) {
    const actions = document.createElement('div');
    actions.className = 'pactions';
    // 检查过的插件行：状态文本 + 版本对比 + 更新按钮（冷却期/失败时无按钮）
    let verText = null;
    let descText = null;
    const st = updateStatus.get(p.name);
    if (st) {
      descText = updateStatusText(st);
      if (st.latest && st.installed && st.update_available) {
        verText = cleanVersion(st.installed) + ' → ' + cleanVersion(st.latest);
      }
      if (st.update_available && !st.cooldown_until && !st.error) {
        const up = document.createElement('button');
        up.type = 'button';
        up.className = 'dshd-btn plugin-action';
        up.textContent = dshdT('pluginUpdate');
        up.disabled = updateBusy || pluginsBusy;
        up.addEventListener('click', () => doPluginUpdate(p.name));
        actions.append(up);
      }
    }
    const btn = document.createElement('button');
    btn.type = 'button';
    btn.className = 'dshd-btn danger plugin-action';
    btn.textContent = dshdT('pluginUninstall');
    btn.disabled = pluginsBusy;
    btn.addEventListener('click', async () => {
      if (pluginsBusy) return;
      setPluginsBusy(true);
      pluginStatus(dshdT('pluginRemoving', { name: p.name }));
      try {
        await invoke('plugin_remove', { name: p.name });
        pluginStatus(dshdT('pluginRemoved', { name: p.name }), 'ok');
      } catch (e) { pluginStatus(dshdT('pluginFailed', { message: e }), 'err'); }
      finally { setPluginsBusy(false); await refreshPlugins(); await refreshPluginApplyStatus(); }
    });
    actions.append(btn);
    ul.append(pluginItemRow(p, actions, verText, descText));
  }
  // 已卸载的内置包：保持可见（未安装 + 安装按钮），避免失去重装入口
  for (const [name, st] of updateStatus) {
    if (!st || !st.builtin) continue;
    if (list.some((p) => p.name === name)) continue;
    const actions = document.createElement('div');
    actions.className = 'pactions';
    const btn = document.createElement('button');
    btn.type = 'button';
    btn.className = 'dshd-btn plugin-action';
    btn.textContent = dshdT('pluginInstall');
    btn.disabled = pluginsBusy;
    btn.addEventListener('click', async () => {
      if (pluginsBusy) return;
      setPluginsBusy(true);
      pluginStatus(dshdT('pluginInstalling', { name }));
      try {
        await invoke('plugin_install', { name });
        pluginStatus(dshdT('pluginInstalled', { name }), 'ok');
      } catch (e) { pluginStatus(dshdT('pluginFailed', { message: e }), 'err'); }
      finally {
        setPluginsBusy(false);
        await refreshPlugins();
        await refreshBuiltinStatus(true);
        await refreshPluginApplyStatus();
      }
    });
    actions.append(btn);
    ul.append(pluginItemRow(
      { name, version: (st && st.latest) || '', description: '', builtin: true },
      actions,
      null,
      (st && st.error) || dshdT('pluginNotInstalled')
    ));
  }
  applyTruncationTips($('body'));
}
function renderResults(list) {
  const sec = $('p-results-sec');
  const ul = $('p-results');
  if (!sec || !ul) return;
  // 未搜索（null）：整个结果区隐藏，恢复已装区（搜索态覆盖浏览态）
  if (list === null || list === undefined) {
    sec.classList.add('hidden');
    $('p-installed-sec').classList.remove('hidden');
    return;
  }
  // 搜索态：结果区覆盖已装区——避免两者并存挤压空间
  sec.classList.remove('hidden');
  $('p-installed-sec').classList.add('hidden');
  const countEl = $('p-results-count');
  if (countEl) countEl.textContent = list.length ? '(' + list.length + ')' : '';
  ul.textContent = '';
  if (!list.length) {
    const e = document.createElement('li');
    e.className = 'pempty';
    e.textContent = dshdT('pluginNoResult');
    ul.append(e);
    return;
  }
  for (const p of list) {
    const btn = document.createElement('button');
    btn.type = 'button';
    btn.className = 'dshd-btn primary';
    if (p.installed) {
      btn.textContent = dshdT('pluginInstalledTag');
      btn.disabled = true;
    } else {
      btn.classList.add('plugin-action');
      btn.textContent = dshdT('pluginInstall');
      btn.disabled = pluginsBusy;
      btn.addEventListener('click', async () => {
        if (pluginsBusy) return;
        setPluginsBusy(true);
        pluginStatus(dshdT('pluginInstalling', { name: p.name }));
        let installed = false;
        try {
          await invoke('plugin_install', { name: p.name });
          installed = true;
          pluginStatus(dshdT('pluginInstalled', { name: p.name }), 'ok');
        } catch (e) { pluginStatus(dshdT('pluginFailed', { message: e }), 'err'); }
        finally {
          setPluginsBusy(false);
          if (installed) {
            btn.classList.remove('plugin-action');
            btn.textContent = dshdT('pluginInstalledTag');
            btn.disabled = true;
          }
          await refreshPlugins();
          await refreshPluginApplyStatus();
        }
      });
    }
    ul.append(pluginItemRow(p, btn));
  }
  applyTruncationTips($('body'));
}
