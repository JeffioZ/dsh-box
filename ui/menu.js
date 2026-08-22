// 托盘菜单与标题栏主菜单共用的渲染与键盘导航逻辑。

function dshdNormalizeMenuItems(nextItems) {
  const normalized = [];
  for (const item of (Array.isArray(nextItems) ? nextItems : [])) {
    if (item && item.sep) {
      // 分隔线只表达两个相邻条目组的边界，不允许出现在菜单首尾或连续出现。
      if (!normalized.length || normalized[normalized.length - 1].sep) continue;
    }
    normalized.push(item);
  }
  if (normalized.length && normalized[normalized.length - 1].sep) normalized.pop();
  return normalized;
}

function dshdCreateMenu(container, options) {
  const settings = options || {};
  const keyboardNavKeys = new Set(['ArrowDown', 'ArrowUp', 'Home', 'End', 'Enter', ' ']);

  // 菜单条目图标（stroke 风格，与全局 --dshd 图标体系一致）
  const ICONS = {
    wallet: '<svg viewBox="0 0 24 24"><path d="M21 7H5a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h13"></path><path d="M3 5v14a2 2 0 0 0 2 2h16V7"></path><path d="M16 13h3"></path></svg>',
    window: '<svg viewBox="0 0 24 24"><rect x="3" y="4" width="18" height="16" rx="2"></rect><path d="M3 9h18"></path></svg>',
    globe: '<svg viewBox="0 0 24 24"><circle cx="12" cy="12" r="9"></circle><path d="M3 12h18"></path><path d="M12 3c3 3.4 3 14 0 18"></path><path d="M12 3c-3 3.4-3 14 0 18"></path></svg>',
    restart: '<svg viewBox="0 0 24 24"><path d="M3 12a9 9 0 1 1 3 6.7"></path><path d="M3 4v5h5"></path></svg>',
    download: '<svg viewBox="0 0 24 24"><path d="M12 3v12"></path><path d="M7 10l5 5 5-5"></path><path d="M4 21h16"></path></svg>',
    puzzle: '<svg viewBox="0 0 24 24"><path d="M8.5 3H5a2 2 0 0 0-2 2v3.5a2.5 2.5 0 1 1 0 5V19a2 2 0 0 0 2 2h3.5a2.5 2.5 0 1 1 5 0H19a2 2 0 0 0 2-2v-5.5a2.5 2.5 0 1 1 0-5V5a2 2 0 0 0-2-2h-5.5a2.5 2.5 0 1 1-5 0Z"></path></svg>',
    gear: '<svg viewBox="0 0 24 24"><circle cx="12" cy="12" r="3"></circle><path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 1 1-2.83 2.83l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 1 1-4 0v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 1 1-2.83-2.83l.06-.06a1.65 1.65 0 0 0 .33-1.82 1.65 1.65 0 0 0-1.51-1H3a2 2 0 1 1 0-4h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 1 1 2.83-2.83l.06.06a1.65 1.65 0 0 0 1.82.33H9a1.65 1.65 0 0 0 1-1.51V3a2 2 0 1 1 4 0v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 1 1 2.83 2.83l-.06.06a1.65 1.65 0 0 0-.33 1.82V9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 1 1 0 4h-.09a1.65 1.65 0 0 0-1.51 1z"></path></svg>',
    info: '<svg viewBox="0 0 24 24"><circle cx="12" cy="12" r="9"></circle><path d="M12 11v6"></path><path d="M12 7.5v.01"></path></svg>',
    exit: '<svg viewBox="0 0 24 24"><path d="M9 4h10a1 1 0 0 1 1 1v14a1 1 0 0 1-1 1H9"></path><path d="M4 12h10"></path><path d="M11 8l4 4-4 4"></path></svg>',
    cut: '<svg viewBox="0 0 24 24"><circle cx="6" cy="6" r="3"></circle><circle cx="6" cy="18" r="3"></circle><path d="M8.7 7.4 21 18"></path><path d="M8.7 16.6 21 6"></path></svg>',
    copy: '<svg viewBox="0 0 24 24"><rect x="8" y="8" width="12" height="12" rx="2"></rect><path d="M16 8V6a2 2 0 0 0-2-2H6a2 2 0 0 0-2 2v8a2 2 0 0 0 2 2h2"></path></svg>',
    paste: '<svg viewBox="0 0 24 24"><path d="M9 5h6"></path><path d="M9 3h6a2 2 0 0 1 2 2v2H7V5a2 2 0 0 1 2-2Z"></path><path d="M7 5H5a2 2 0 0 0-2 2v12a2 2 0 0 0 2 2h14a2 2 0 0 0 2-2V7a2 2 0 0 0-2-2h-2"></path></svg>',
    select: '<svg viewBox="0 0 24 24"><path d="M4 8V6a2 2 0 0 1 2-2h2"></path><path d="M16 4h2a2 2 0 0 1 2 2v2"></path><path d="M20 16v2a2 2 0 0 1-2 2h-2"></path><path d="M8 20H6a2 2 0 0 1-2-2v-2"></path><path d="M8 12h8"></path></svg>',
    undo: '<svg viewBox="0 0 24 24"><path d="M9 7 4 12l5 5"></path><path d="M4 12h10a6 6 0 0 1 6 6"></path></svg>',
    redo: '<svg viewBox="0 0 24 24"><path d="m15 7 5 5-5 5"></path><path d="M20 12H10a6 6 0 0 0-6 6"></path></svg>',
  };

  let items = [];
  let lastItemsKey = '';

  function rows() {
    return [...container.querySelectorAll('.dshd-row:not(:disabled)')];
  }

  function focusFirst() {
    container.classList.remove('dshd-menu-keyboard');
    const visibleRows = rows();
    visibleRows.forEach((row, index) => { row.tabIndex = index === 0 ? 0 : -1; });
    if (visibleRows.length && document.visibilityState === 'visible') visibleRows[0].focus();
  }

  function activate(item) {
    if (settings.onChoose) settings.onChoose(item.id);
  }

  function makeRow(item) {
    const row = document.createElement('button');
    row.className = 'dshd-row' + (item.id === 'quit' ? ' danger' : '');
    row.type = 'button';
    row.dataset.id = item.id;
    row.tabIndex = -1;
    row.setAttribute('role', 'menuitem');
    row.disabled = item.enabled === false;
    row.setAttribute('aria-disabled', String(item.enabled === false));
    if (item.ariaKey) row.setAttribute('aria-keyshortcuts', item.ariaKey);
    if (item.enabled === false && item.disabled_reason) row.title = item.disabled_reason;

    const label = document.createElement('span');
    label.className = 'lb';
    label.textContent = item.label;
    // 截断才提示（菜单 label 长时悬停看全）
    label.dataset.truncTip = '';
    // 图标（.ic 由 common.css 定义 16px；stroke SVG 随文字颜色）
    if (item.icon && ICONS[item.icon]) {
      const ic = document.createElement('span');
      ic.className = 'ic';
      ic.setAttribute('aria-hidden', 'true');
      ic.innerHTML = ICONS[item.icon];
      row.append(ic);
    }
    row.append(label);
    if (item.key) {
      const key = document.createElement('span');
      key.className = 'dim';
      key.setAttribute('aria-hidden', 'true');
      key.textContent = item.key;
      row.append(key);
    }

    row.addEventListener('mousedown', () => { if (!row.disabled) row.classList.add('pressed'); });
    row.addEventListener('mouseup', () => { if (!row.disabled) activate(item); });
    row.addEventListener('click', (event) => {
      if (event.detail === 0 && !row.disabled) activate(item);
    });
    row.addEventListener('mouseleave', () => row.classList.remove('pressed'));
    return row;
  }

  function render(preserveFocus, focusId) {
    container.textContent = '';
    for (const item of items) {
      if (item.sep) {
        const separator = document.createElement('div');
        separator.className = 'dshd-sep';
        separator.setAttribute('role', 'separator');
        container.append(separator);
        continue;
      }
      container.append(makeRow(item));
    }
    // 截断才提示（label 长时悬停看全；显示完整则不设 title）
    container.querySelectorAll('[data-trunc-tip]').forEach((el) => {
      const truncated = el.scrollWidth > el.clientWidth + 1;
      el.title = truncated ? el.textContent.trim() : '';
    });

    const visibleRows = rows();
    visibleRows.forEach((row) => { row.tabIndex = -1; });
    const focusTarget = preserveFocus && focusId
      ? visibleRows.find((row) => row.dataset.id === focusId)
      : visibleRows[0];
    if (focusTarget) {
      focusTarget.tabIndex = 0;
      if (preserveFocus && document.visibilityState === 'visible') focusTarget.focus();
    }
  }

  function setItems(nextItems, forceRender = false) {
    const normalized = dshdNormalizeMenuItems(nextItems);
    const key = JSON.stringify(normalized);
    if (key === lastItemsKey && !forceRender) {
      container.querySelectorAll('.dshd-row.pressed').forEach((row) => row.classList.remove('pressed'));
      return;
    }
    lastItemsKey = key;
    items = normalized;
    render(false);
  }

  container.addEventListener('pointerdown', () => container.classList.remove('dshd-menu-keyboard'));
  container.addEventListener('keydown', (event) => {
    if (keyboardNavKeys.has(event.key)) container.classList.add('dshd-menu-keyboard');
    if (event.key === 'Escape') {
      event.preventDefault();
      if (settings.onEscape) settings.onEscape();
      return;
    }
    const visibleRows = rows();
    if (!visibleRows.length) return;
    let index = visibleRows.indexOf(document.activeElement);
    if (!['ArrowDown', 'ArrowUp', 'Home', 'End'].includes(event.key)) return;
    event.preventDefault();
    if (event.key === 'Home') index = 0;
    else if (event.key === 'End') index = visibleRows.length - 1;
    else if (event.key === 'ArrowDown') {
      index = index < 0 ? 0 : (index + 1) % visibleRows.length;
    } else {
      index = index < 0 ? visibleRows.length - 1 : (index - 1 + visibleRows.length) % visibleRows.length;
    }
    visibleRows.forEach((row, rowIndex) => { row.tabIndex = rowIndex === index ? 0 : -1; });
    visibleRows[index].focus();
  });

  return { setItems, focusFirst };
}
