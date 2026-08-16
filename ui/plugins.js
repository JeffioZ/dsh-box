// 插件管理窗口：搜索/安装/卸载 dsh 插件（走 dsh CLI，装/卸后自动重启服务）。

const $ = (id) => document.getElementById(id);

let busy = false;

function setStatus(text, kind) {
  const el = $('p-status');
  el.textContent = text || '';
  el.className = 'pstatus' + (kind ? ' ' + kind : '');
}

function esc(s) {
  return String(s == null ? '' : s).replace(/[&<>"']/g, (c) => (
    { '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c]
  ));
}

function itemRow(p, actions) {
  const li = document.createElement('li');
  li.className = 'pitem';
  const info = document.createElement('div');
  info.className = 'info';
  const name = document.createElement('div');
  name.className = 'name';
  name.textContent = p.name;
  const desc = document.createElement('div');
  desc.className = 'desc';
  desc.textContent = p.description || '';
  info.append(name, desc);
  li.append(info);
  if (p.version) {
    const ver = document.createElement('span');
    ver.className = 'ver';
    ver.textContent = 'v' + p.version;
    li.append(ver);
  }
  li.append(actions);
  return li;
}

function renderInstalled(list) {
  const ul = $('p-installed');
  ul.textContent = '';
  if (!list.length) {
    const empty = document.createElement('li');
    empty.className = 'pempty';
    empty.textContent = dshdT('pluginNone');
    ul.append(empty);
    return;
  }
  for (const p of list) {
    const btn = document.createElement('button');
    btn.type = 'button';
    btn.className = 'pbtn small';
    btn.textContent = dshdT('pluginUninstall');
    btn.disabled = busy;
    btn.addEventListener('click', async () => {
      busy = true;
      btn.disabled = true;
      setStatus(dshdT('pluginRemoving', { name: p.name }));
      try {
        await window.__TAURI__.core.invoke('plugin_remove', { name: p.name });
        setStatus(dshdT('pluginRemoved', { name: p.name }), 'ok');
      } catch (e) {
        setStatus(dshdT('pluginFailed', { message: e }), 'err');
      } finally {
        busy = false;
        await refresh();
      }
    });
    ul.append(itemRow(p, btn));
  }
}

function renderResults(list) {
  const ul = $('p-results');
  ul.textContent = '';
  if (!list.length) {
    const empty = document.createElement('li');
    empty.className = 'pempty';
    empty.textContent = dshdT('pluginNoResult');
    ul.append(empty);
    return;
  }
  for (const p of list) {
    const btn = document.createElement('button');
    btn.type = 'button';
    btn.className = 'pbtn small primary';
    if (p.installed) {
      btn.textContent = dshdT('pluginInstalledTag');
      btn.disabled = true;
    } else {
      btn.textContent = dshdT('pluginInstall');
      btn.disabled = busy;
      btn.addEventListener('click', async () => {
        busy = true;
        btn.disabled = true;
        setStatus(dshdT('pluginInstalling', { name: p.name }));
        try {
          await window.__TAURI__.core.invoke('plugin_install', { name: p.name });
          setStatus(dshdT('pluginInstalled', { name: p.name }), 'ok');
        } catch (e) {
          setStatus(dshdT('pluginFailed', { message: e }), 'err');
        } finally {
          busy = false;
          await refresh();
        }
      });
    }
    ul.append(itemRow(p, btn));
  }
}

async function refresh() {
  try {
    const installed = await window.__TAURI__.core.invoke('plugin_list');
    renderInstalled(installed || []);
  } catch (e) {
    setStatus(dshdT('pluginFailed', { message: e }), 'err');
  }
}

async function doSearch(query) {
  const q = (query != null ? query : $('p-query').value).trim();
  if (!q) return;
  const btn = $('p-search');
  btn.disabled = true;
  setStatus(dshdT('pluginSearching'));
  try {
    const list = await window.__TAURI__.core.invoke('plugin_search', { query: q });
    renderResults(list || []);
    setStatus(dshdT('pluginSearchDone', { count: (list || []).length }), 'ok');
  } catch (e) {
    setStatus(dshdT('pluginFailed', { message: e }), 'err');
  } finally {
    btn.disabled = false;
  }
}

// 分类快捷搜索：预设关键词（皮肤/工具/工作流），命中 npm 上的 dsh 插件
const CAT_QUERIES = {
  skin: 'dsh-plugin skin OR dsh-web-ui OR dsh-ui theme',
  tool: 'dsh-plugin tool',
  workflow: 'dsh-plugin workflow',
};

function bindCats() {
  document.querySelectorAll('.pcats .pbtn').forEach((btn) => {
    btn.addEventListener('click', () => {
      const cat = btn.dataset.cat || '';
      document.querySelectorAll('.pcats .pbtn').forEach((b) => {
        b.classList.toggle('active', b === btn);
      });
      const q = cat ? CAT_QUERIES[cat] : '';
      if (q) {
        doSearch(q);
      } else {
        $('p-query').value = '';
        renderResults([]);
      }
    });
  });
}

async function init() {
  dshdApplyI18n();
  document.addEventListener('contextmenu', (e) => e.preventDefault());
  $('p-search').addEventListener('click', () => doSearch());
  $('p-query').addEventListener('keydown', (e) => {
    if (e.key === 'Enter') doSearch();
  });
  bindCats();
  await refresh();
}

init();
