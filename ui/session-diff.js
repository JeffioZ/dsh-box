// 会话文件变更窗口：查看最新会话对文件的改动（只读解析会话日志），
// 纯 edit 改动的文件可一键还原（反向应用）。

const $ = (id) => document.getElementById(id);

function setStatus(text, kind) {
  const el = $('sd-status');
  el.textContent = text || '';
  el.className = 'pstatus' + (kind ? ' ' + kind : '');
}

function esc(s) {
  return String(s == null ? '' : s).replace(/[&<>"']/g, (c) => (
    { '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c]
  ));
}

function render(data) {
  const list = $('sd-list');
  list.textContent = '';
  const sub = $('sd-sub');
  if (data.session_id) {
    sub.textContent = dshdT('sessionDiffSession', { id: data.session_id });
  }
  if (!data.files || !data.files.length) {
    const empty = document.createElement('li');
    empty.className = 'pempty';
    empty.textContent = dshdT('sessionDiffNone');
    list.append(empty);
    return;
  }
  for (const f of data.files) {
    const li = document.createElement('li');
    li.className = 'pitem';

    const head = document.createElement('div');
    head.className = 'pitem-head';
    const path = document.createElement('span');
    path.className = 'pitem-path';
    path.textContent = f.path;
    path.title = f.path;
    head.append(path);

    const meta = document.createElement('span');
    meta.className = 'pitem-meta';
    meta.textContent = dshdT('sessionDiffEdits', { count: f.edits.length }) + (f.rewritten ? ' · ' + dshdT('sessionDiffRewritten') : '');
    head.append(meta);

    const tag = document.createElement('span');
    tag.className = 'pitem-tag ' + (f.revertible ? 'ok' : 'no');
    tag.textContent = f.revertible ? dshdT('sessionDiffRevertible') : dshdT('sessionDiffNotRevertible');
    head.append(tag);

    if (f.revertible) {
      const btn = document.createElement('button');
      btn.type = 'button';
      btn.className = 'pbtn primary';
      btn.textContent = dshdT('sessionDiffRevert');
      btn.addEventListener('click', async (e) => {
        e.stopPropagation();
        if (!window.confirm(dshdT('sessionDiffRevertConfirm', { path: f.path }))) return;
        btn.disabled = true;
        setStatus(dshdT('sessionDiffReverting', { path: f.path }));
        try {
          await window.__TAURI__.core.invoke('session_revert', { path: f.path });
          setStatus(dshdT('sessionDiffReverted', { path: f.path }), 'ok');
        } catch (err) {
          setStatus(dshdT('sessionDiffFailed', { message: err }), 'err');
        } finally {
          btn.disabled = false;
        }
      });
      head.append(btn);
    }

    head.addEventListener('click', () => li.classList.toggle('open'));
    li.append(head);

    // diff 区：每处 edit 的 old → new 片段
    const diff = document.createElement('div');
    diff.className = 'pdiff dshd-scroll';
    for (const op of f.edits) {
      const wrap = document.createElement('div');
      wrap.className = 'dop';
      if (op.old && op.new) {
        const label = document.createElement('div');
        label.className = 'dop-label';
        label.textContent = '#' + op.seq;
        wrap.append(label);
        const old = document.createElement('div');
        old.className = 'dop-old';
        old.textContent = op.old;
        const arrow = document.createElement('div');
        arrow.className = 'dop-arrow';
        arrow.textContent = '→';
        const neu = document.createElement('div');
        neu.className = 'dop-new';
        neu.textContent = op.new;
        wrap.append(old, arrow, neu);
      } else {
        const label = document.createElement('div');
        label.className = 'dop-label';
        label.textContent = '#' + op.seq + ' ' + dshdT('sessionDiffRewriteOp');
        wrap.append(label);
        const neu = document.createElement('div');
        neu.className = 'dop-new';
        neu.textContent = op.new;
        wrap.append(neu);
      }
      diff.append(wrap);
    }
    li.append(diff);
    list.append(li);
  }
}

async function refresh() {
  try {
    const data = await window.__TAURI__.core.invoke('session_changes');
    render(data);
  } catch (e) {
    setStatus(dshdT('sessionDiffFailed', { message: e }), 'err');
  }
}

async function init() {
  dshdApplyI18n();
  document.addEventListener('contextmenu', (e) => e.preventDefault());
  await refresh();
}

init();
