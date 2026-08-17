// 托盘菜单与标题栏主菜单共用的渲染与键盘导航逻辑。

function dshdCreateMenu(container, options) {
  const settings = options || {};
  const keyboardNavKeys = new Set(['ArrowDown', 'ArrowUp', 'ArrowLeft', 'ArrowRight', 'Home', 'End', 'Enter', ' ']);

  // 菜单条目图标（stroke 风格，与全局 --dshd 图标体系一致）
  const ICONS = {
    wallet: '<svg viewBox="0 0 24 24"><path d="M21 7H5a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h13"></path><path d="M3 5v14a2 2 0 0 0 2 2h16V7"></path><path d="M16 13h3"></path></svg>',
    window: '<svg viewBox="0 0 24 24"><rect x="3" y="4" width="18" height="16" rx="2"></rect><path d="M3 9h18"></path></svg>',
    globe: '<svg viewBox="0 0 24 24"><circle cx="12" cy="12" r="9"></circle><path d="M3 12h18"></path><path d="M12 3c3 3.4 3 14 0 18"></path><path d="M12 3c-3 3.4-3 14 0 18"></path></svg>',
    restart: '<svg viewBox="0 0 24 24"><path d="M3 12a9 9 0 1 1 3 6.7"></path><path d="M3 4v5h5"></path></svg>',
    download: '<svg viewBox="0 0 24 24"><path d="M12 3v12"></path><path d="M7 10l5 5 5-5"></path><path d="M4 21h16"></path></svg>',
    puzzle: '<svg viewBox="0 0 24 24"><path d="M16 3h5v5"></path><path d="M8 3H3v5"></path><path d="M21 16v5h-5"></path><path d="M3 16v5h5"></path><rect x="7" y="7" width="10" height="10" rx="2"></rect></svg>',
    file: '<svg viewBox="0 0 24 24"><path d="M14 3H6a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V9z"></path><path d="M14 3v6h6"></path></svg>',
    eye: '<svg viewBox="0 0 24 24"><path d="M2 12s3.5-6 10-6 10 6 10 6-3.5 6-10 6-10-6-10-6z"></path><circle cx="12" cy="12" r="2.5"></circle></svg>',
    gear: '<svg viewBox="0 0 24 24"><circle cx="12" cy="12" r="3"></circle><path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 1 1-2.83 2.83l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 1 1-4 0v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 1 1-2.83-2.83l.06-.06a1.65 1.65 0 0 0 .33-1.82 1.65 1.65 0 0 0-1.51-1H3a2 2 0 1 1 0-4h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 1 1 2.83-2.83l.06.06a1.65 1.65 0 0 0 1.82.33H9a1.65 1.65 0 0 0 1-1.51V3a2 2 0 1 1 4 0v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 1 1 2.83 2.83l-.06.06a1.65 1.65 0 0 0-.33 1.82V9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 1 1 0 4h-.09a1.65 1.65 0 0 0-1.51 1z"></path></svg>',
    info: '<svg viewBox="0 0 24 24"><circle cx="12" cy="12" r="9"></circle><path d="M12 11v6"></path><path d="M12 7.5v.01"></path></svg>',
    exit: '<svg viewBox="0 0 24 24"><path d="M9 4h10a1 1 0 0 1 1 1v14a1 1 0 0 1-1 1H9"></path><path d="M4 12h10"></path><path d="M11 8l4 4-4 4"></path></svg>',
  };

  let items = [];
  let expandedId = '';
  let lastItemsKey = '';

  function rows() {
    return [...container.querySelectorAll('.dshd-row')];
  }

  function focusFirst() {
    container.classList.remove('dshd-menu-keyboard');
    const visibleRows = rows();
    visibleRows.forEach((row, index) => { row.tabIndex = index === 0 ? 0 : -1; });
    if (visibleRows.length && document.visibilityState === 'visible') visibleRows[0].focus();
  }

  let collapseToken = 0;

  function setExpanded(id, expanded) {
    const next = expanded ? id : '';
    if (expandedId === next) return;
    const previous = expandedId;
    const collapsing = Boolean(expandedId) && !next;
    if (collapsing) {
      // 收起：先让子行淡出上滑（100ms），再通知容器改高并移除子行——
      // 窗口变矮与行消失分开发生，避免瞬间消失的突兀
      const token = ++collapseToken;
      container.querySelectorAll('.dshd-row.child').forEach((row) => {
        row.classList.add('child-collapsing');
      });
      expandedId = '';
      setTimeout(() => {
        if (token !== collapseToken) return;
        if (settings.onSubmenuChange) settings.onSubmenuChange('', false);
        applyExpansion(previous);
      }, 100);
      return;
    }
    collapseToken += 1; // 使挂起的收起动画失效（收起期间快速再展开）
    expandedId = next;
    // 先通知容器（托盘窗口据此改高度），再改 DOM：
    // 窗口调整与插入新行并行，首帧即画在改好高度的窗口内，无裁切闪烁
    if (settings.onSubmenuChange) settings.onSubmenuChange(next, true);
    applyExpansion(previous);
  }

  // 增量展开/收起：只移除旧子行、插入新子行，不整表重建——
  // 整表重建会让所有行重绘一帧，与窗口改高叠加产生闪烁
  function applyExpansion(previousId) {
    container.querySelectorAll('.dshd-row.child').forEach((row) => row.remove());
    if (previousId) {
      const prevParent = container.querySelector('.dshd-row[data-id="' + previousId + '"]');
      if (prevParent) {
        prevParent.setAttribute('aria-expanded', 'false');
        const arrow = prevParent.querySelector('.arrow');
        if (arrow) arrow.classList.remove('expanded');
      }
    }
    if (expandedId) {
      const parent = container.querySelector('.dshd-row[data-id="' + expandedId + '"]');
      const item = items.find((entry) => entry.id === expandedId);
      if (parent && item && item.children && item.children.length) {
        parent.setAttribute('aria-expanded', 'true');
        const arrow = parent.querySelector('.arrow');
        if (arrow) arrow.classList.add('expanded');
        parent.after(...item.children.map((child) => makeRow(child, true)));
      }
    }
    const visibleRows = rows();
    const keepFocus = document.activeElement && container.contains(document.activeElement)
      ? document.activeElement
      : null;
    const target = keepFocus
      || (previousId && container.querySelector('.dshd-row[data-id="' + previousId + '"]'))
      || visibleRows[0];
    visibleRows.forEach((row) => { row.tabIndex = row === target ? 0 : -1; });
    if (!keepFocus && target && document.visibilityState === 'visible') target.focus();
  }

  function activate(item) {
    if (item.children && item.children.length) {
      setExpanded(item.id, expandedId !== item.id);
      return;
    }
    if (settings.onChoose) settings.onChoose(item.id);
  }

  function makeRow(item, child) {
    const row = document.createElement('button');
    row.className = 'dshd-row' + (child ? ' child' : '') +
      (item.children && item.children.length ? ' has-children' : '') +
      (item.id === 'quit' ? ' danger' : '');
    row.type = 'button';
    row.dataset.id = item.id;
    row.tabIndex = -1;

    if (typeof item.checked === 'boolean') {
      row.setAttribute('role', 'menuitemradio');
      row.setAttribute('aria-checked', String(item.checked));
      const check = document.createElement('span');
      check.className = 'check';
      check.textContent = item.checked ? '✓' : '';
      check.setAttribute('aria-hidden', 'true');
      row.append(check);
    } else {
      row.setAttribute('role', 'menuitem');
    }

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

    if (item.children && item.children.length) {
      row.setAttribute('aria-haspopup', 'menu');
      row.setAttribute('aria-expanded', String(expandedId === item.id));
      const arrow = document.createElement('span');
      arrow.className = 'arrow' + (expandedId === item.id ? ' expanded' : '');
      arrow.setAttribute('aria-hidden', 'true');
      row.append(arrow);
    }

    row.addEventListener('mousedown', () => row.classList.add('pressed'));
    row.addEventListener('mouseup', () => activate(item));
    row.addEventListener('click', (event) => {
      if (event.detail === 0) activate(item);
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
      container.append(makeRow(item, false));
      if (expandedId === item.id && item.children) {
        for (const child of item.children) container.append(makeRow(child, true));
      }
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

  function setItems(nextItems) {
    const normalized = nextItems || [];
    const key = JSON.stringify(normalized);
    if (key === lastItemsKey) {
      container.querySelectorAll('.dshd-row.pressed').forEach((row) => row.classList.remove('pressed'));
      return;
    }
    lastItemsKey = key;
    items = normalized;
    if (!items.some((item) => item.id === expandedId)) expandedId = '';
    render(false);
  }

  function collapseSubmenus(notify = true) {
    if (!expandedId) return;
    const previous = expandedId;
    expandedId = '';
    if (notify && settings.onSubmenuChange) settings.onSubmenuChange('', false);
    applyExpansion(previous);
  }

  container.addEventListener('pointerdown', () => container.classList.remove('dshd-menu-keyboard'));
  container.addEventListener('keydown', (event) => {
    if (keyboardNavKeys.has(event.key)) container.classList.add('dshd-menu-keyboard');
    if (event.key === 'Escape') {
      event.preventDefault();
      if (expandedId) collapseSubmenus();
      else if (settings.onEscape) settings.onEscape();
      return;
    }
    const visibleRows = rows();
    if (!visibleRows.length) return;
    let index = visibleRows.indexOf(document.activeElement);
    if (event.key === 'ArrowRight') {
      const item = items.find((entry) => entry.id === document.activeElement?.dataset.id);
      if (item && item.children && item.children.length && expandedId !== item.id) {
        event.preventDefault();
        setExpanded(item.id, true);
        const firstChild = rows().find((row) => row.classList.contains('child'));
        if (firstChild) {
          rows().forEach((row) => { row.tabIndex = row === firstChild ? 0 : -1; });
          firstChild.focus();
        }
      }
      return;
    }
    if (event.key === 'ArrowLeft' && expandedId) {
      event.preventDefault();
      collapseSubmenus();
      return;
    }
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

  return { setItems, focusFirst, collapseSubmenus };
}
