// 控制中心：插件搜索、安装、卸载与更新。

// —— 插件管理（统一弹窗内） ——
let pluginsBusy = false;
let pluginSearchSeq = 0;
let pluginApplyStamp = '';
let pluginApplyWasPending = false;
let pluginStatusTimer = null;
let pluginRecommendedSeq = 0;
let pluginReinstallSeq = 0;
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
  clearTimeout(pluginStatusTimer);
  pluginStatusTimer = null;
  // 重新进入页面时让上一次尚未返回的搜索失效，避免旧结果覆盖新页面。
  pluginSearchSeq += 1;
  // DOM 重建后按钮回到「全部」高亮，分类状态必须同步重置，否则
  // 搜索会用残留的旧分类叠加，与界面显示不符。
  activeCat = '';
  pluginApplyWasPending = false;
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
    '<div class="pdirectory" id="p-directory">' +
    '<div class="psection" id="p-installed-sec"><div class="psection-head"><h3>' + esc(dshdT('pluginInstalledTitle')) + ' <span class="pcount" id="p-installed-count"></span></h3>' +
    '<button type="button" class="dshd-btn small" id="p-builtin-check">' + esc(dshdT('pluginCheckUpdates')) + '</button></div>' +
    '<ul class="plist" id="p-installed"></ul></div>' +
    '<div class="psection hidden" id="p-reinstall-sec"><h3>' + esc(dshdT('pluginReinstallTitle')) + '</h3>' +
    '<p class="p-section-hint">' + esc(dshdT('pluginReinstallHint')) + '</p>' +
    '<ul class="plist" id="p-reinstall"></ul></div>' +
    '<div class="psection" id="p-recommended-sec" aria-busy="true"><h3>' + esc(dshdT('pluginRecommendedTitle')) + '</h3>' +
    '<p class="p-section-hint">' + esc(dshdT('pluginRecommendedHint')) + '</p>' +
    '<ul class="plist recommended-loading" id="p-recommended"><li class="pempty" role="status">' +
    '<span class="spin" aria-hidden="true"></span>' + esc(dshdT('pluginRecommendedLoading')) + '</li></ul></div></div>';
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
        $('p-directory').classList.remove('hidden');
        pluginStatus('');
      }
    });
  });
  refreshPlugins();
  refreshReinstallableBuiltins();
  refreshRecommended();
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
  if (!visible) {
    if (pluginApplyWasPending) pluginStatus('');
    pluginApplyWasPending = false;
    return;
  }
  pluginApplyWasPending = true;
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
// pkg → UpdateStatus：plugin_updates 的结果缓存，渲染已安装列表时使用。
// 键的契约：一律为 npm 依赖包名（与 UpdateStatus.pkg 相同）。三个来源的
// 取法：已装列表与搜索结果取 p.name（PluginInfo.name 即依赖名）；内置重装/
// 社区推荐目录项取 p.id（目录项 name 是非唯一展示名，id 才是依赖名）。
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
      pluginStatus(dshdT('pluginUpdatesFound', { count: updatable }), '', 4000);
    } else {
      pluginStatus(dshdT('pluginUpdatesChecked'), 'ok', 3000);
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
function pluginStatus(text, kind, clearAfter) {
  const el = $('p-status');
  if (!el) return;
  clearTimeout(pluginStatusTimer);
  pluginStatusTimer = null;
  el.textContent = text || '';
  el.className = 'pstatus' + (kind ? ' ' + kind : '');
  if (text && clearAfter) {
    pluginStatusTimer = setTimeout(() => {
      if (el.textContent === text) pluginStatus('');
    }, clearAfter);
  }
}
async function refreshPlugins() {
  try {
    const list = await invoke('plugin_list');
    if (openKind !== 'plugins') return;
    renderInstalled(list || []);
  } catch (e) { pluginStatus(dshdT('pluginFailed', { message: e }), 'err'); }
}

function pluginHomepageButton(p, reportError) {
  if (!p.homepage) return null;
  const homepageBtn = document.createElement('button');
  homepageBtn.type = 'button';
  homepageBtn.className = 'dshd-btn small plugin-home';
  homepageBtn.title = dshdT('pluginHomepage');
  homepageBtn.setAttribute('aria-label', dshdT('pluginHomepage') + ': ' + p.name);
  homepageBtn.innerHTML = '<svg viewBox="0 0 24 24" focusable="false" aria-hidden="true"><path d="M15 3h6v6"></path><path d="m10 14 11-11"></path><path d="M18 13v6a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V8a2 2 0 0 1 2-2h6"></path></svg><span>' + esc(dshdT('pluginHomepage')) + '</span>';
  homepageBtn.addEventListener('click', () => {
    invoke('open_external_url', { url: p.homepage }).catch(reportError);
  });
  return homepageBtn;
}
function catalogPluginRow(p, actionKey) {
  const actions = document.createElement('div');
  actions.className = 'pactions';
  const installBtn = document.createElement('button');
  installBtn.type = 'button';
  installBtn.className = 'dshd-btn small plugin-action';
  installBtn.textContent = dshdT(actionKey);
  installBtn.disabled = pluginsBusy;
  const description = dshdLocale() === 'zh-CN'
    ? p.description_zh || p.description_en
    : p.description_en || p.description_zh;
  const li = pluginItemRow(p, actions, null, description || '');
  const rowStatus = document.createElement('div');
  rowStatus.className = 'pitem-status';
  rowStatus.setAttribute('role', 'status');
  rowStatus.setAttribute('aria-live', 'polite');
  li.querySelector('.info').append(rowStatus);
  const homepageBtn = pluginHomepageButton(p, () => {
    rowStatus.textContent = '';
    rowStatus.textContent = dshdT('pluginHomepageFailed');
    rowStatus.className = 'pitem-status err';
  });
  if (homepageBtn) actions.append(homepageBtn);
  actions.append(installBtn);
  installBtn.addEventListener('click', async () => {
    if (pluginsBusy) return;
    setPluginsBusy(true);
    const installing = dshdT('pluginInstalling', { name: p.name });
    li.setAttribute('aria-busy', 'true');
    rowStatus.textContent = installing;
    rowStatus.className = 'pitem-status';
    installBtn.innerHTML = '<span class="spin" aria-hidden="true"></span>' + esc(dshdT('processing'));
    try {
      await invoke('plugin_install', { name: p.spec });
      // 目录项 id 即依赖包名（键契约见 updateStatus 声明处）
      updateStatus.delete(p.id);
      pluginStatus(dshdT('pluginInstalled', { name: p.name }), 'ok');
      await refreshPlugins();
      await refreshReinstallableBuiltins();
      await refreshRecommended();
      await refreshPluginApplyStatus();
    } catch (e) {
      const message = dshdT('pluginFailed', { message: e });
      rowStatus.textContent = message;
      rowStatus.className = 'pitem-status err';
    } finally {
      li.removeAttribute('aria-busy');
      installBtn.textContent = dshdT(actionKey);
      setPluginsBusy(false);
    }
  });
  return li;
}

// —— 社区插件（仅手动安装；安装后移入已装、卸载后回归）——
async function refreshRecommended() {
  const ul = $('p-recommended');
  if (!ul) return;
  const section = $('p-recommended-sec');
  const seq = ++pluginRecommendedSeq;
  section.setAttribute('aria-busy', 'true');
  try {
    const list = await invoke('plugin_recommended');
    if (openKind !== 'plugins' || seq !== pluginRecommendedSeq || !$('p-recommended')) return;
    section.setAttribute('aria-busy', 'false');
    ul.classList.remove('recommended-loading');
    ul.textContent = '';
    if (!list || !list.length) {
      // 全部推荐都装上了：整块收起，不占空间
      section.classList.add('hidden');
      return;
    }
    section.classList.remove('hidden');
    for (const p of list) {
      ul.append(catalogPluginRow(p, 'pluginInstall'));
    }
    applyTruncationTips(ul);
  } catch (e) {
    section.setAttribute('aria-busy', 'false');
    section.classList.remove('hidden');
    ul.classList.remove('recommended-loading');
    const el = document.createElement('li');
    el.className = 'pempty';
    el.setAttribute('role', 'alert');
    el.textContent = dshdT('pluginFailed', { message: e });
    ul.textContent = '';
    ul.append(el);
  }
}

// —— 用户卸载过的内置插件：保留手动重装入口，但不恢复自动维护身份 ——
async function refreshReinstallableBuiltins() {
  const section = $('p-reinstall-sec');
  const ul = $('p-reinstall');
  if (!section || !ul) return;
  const seq = ++pluginReinstallSeq;
  section.setAttribute('aria-busy', 'true');
  try {
    const list = await invoke('plugin_reinstallable_builtins');
    if (openKind !== 'plugins' || seq !== pluginReinstallSeq || !$('p-reinstall')) return;
    section.setAttribute('aria-busy', 'false');
    ul.textContent = '';
    if (!list || !list.length) {
      section.classList.add('hidden');
      return;
    }
    section.classList.remove('hidden');
    for (const plugin of list) ul.append(catalogPluginRow(plugin, 'pluginReinstall'));
    applyTruncationTips(ul);
  } catch (e) {
    if (openKind !== 'plugins' || seq !== pluginReinstallSeq) return;
    section.setAttribute('aria-busy', 'false');
    section.classList.remove('hidden');
    const error = document.createElement('li');
    error.className = 'pempty';
    error.setAttribute('role', 'alert');
    error.textContent = dshdT('pluginFailed', { message: e });
    ul.textContent = '';
    ul.append(error);
  }
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
  if (!busy) {
    // 忙状态下渲染过的同一份状态会把应用按钮留在 disabled；解除忙碌后
    // 必须绕过去重再渲染一次。
    pluginApplyStamp = '';
    refreshPluginApplyStatus();
  }
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
  for (const p of list) {
    const actions = document.createElement('div');
    actions.className = 'pactions';
    const homepageBtn = pluginHomepageButton(p, () => {
      pluginStatus(dshdT('pluginHomepageFailed'), 'err');
    });
    if (homepageBtn) actions.append(homepageBtn);
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
        up.className = 'dshd-btn small plugin-action';
        up.textContent = dshdT('pluginUpdate');
        up.disabled = updateBusy || pluginsBusy;
        up.addEventListener('click', () => doPluginUpdate(p.name));
        actions.append(up);
      }
    }
    const btn = document.createElement('button');
    btn.type = 'button';
    btn.className = 'dshd-btn small danger plugin-action';
    btn.textContent = dshdT('pluginUninstall');
    btn.disabled = pluginsBusy;
    btn.addEventListener('click', async () => {
      if (pluginsBusy) return;
      setPluginsBusy(true);
      pluginStatus(dshdT('pluginRemoving', { name: p.name }));
      try {
        await invoke('plugin_remove', { name: p.name });
        updateStatus.delete(p.name);
        pluginStatus(dshdT('pluginRemoved', { name: p.name }), 'ok');
      } catch (e) { pluginStatus(dshdT('pluginFailed', { message: e }), 'err'); }
      finally {
        await refreshPlugins();
        await refreshReinstallableBuiltins();
        await refreshRecommended();
        await refreshPluginApplyStatus();
        setPluginsBusy(false);
      }
    });
    actions.append(btn);
    ul.append(pluginItemRow(p, actions, verText, descText));
  }
  // 已授权但安装缺失的内置包：保持可见，供用户立即修复。主动卸载的
  // 项目由下方独立目录展示，避免把手动重装误解为恢复自动维护。
  let repairCount = 0;
  for (const [name, st] of updateStatus) {
    if (!st || !st.builtin) continue;
    if (list.some((p) => p.name === name)) continue;
    const actions = document.createElement('div');
    actions.className = 'pactions';
    const btn = document.createElement('button');
    btn.type = 'button';
    btn.className = 'dshd-btn small plugin-action';
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
    repairCount += 1;
    ul.append(pluginItemRow(
      { name, version: '', description: '', builtin: true },
      actions,
      null,
      (st && st.error) || dshdT('pluginNotInstalled')
    ));
  }
  if (!list.length && repairCount === 0) {
    const empty = document.createElement('li');
    empty.className = 'pempty';
    empty.textContent = dshdT('pluginNone');
    ul.append(empty);
  }
  applyTruncationTips($('body'));
}
function renderResults(list) {
  const sec = $('p-results-sec');
  const ul = $('p-results');
  if (!sec || !ul) return;
  // 搜索态只保留结果，避免目录与搜索结果同时出现造成信息拥挤。
  sec.classList.remove('hidden');
  $('p-directory').classList.add('hidden');
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
    const actions = document.createElement('div');
    actions.className = 'pactions';
    const homepageBtn = pluginHomepageButton(p, () => {
      pluginStatus(dshdT('pluginHomepageFailed'), 'err');
    });
    if (homepageBtn) actions.append(homepageBtn);
    const btn = document.createElement('button');
    btn.type = 'button';
    btn.className = 'dshd-btn small primary';
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
            updateStatus.delete(p.name);
            btn.classList.remove('plugin-action');
            btn.textContent = dshdT('pluginInstalledTag');
            btn.disabled = true;
          }
          await refreshPlugins();
          await refreshReinstallableBuiltins();
          await refreshRecommended();
          await refreshPluginApplyStatus();
        }
      });
    }
    actions.append(btn);
    ul.append(pluginItemRow(p, actions));
  }
  applyTruncationTips($('body'));
}
