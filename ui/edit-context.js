// 内置页面与 dsh 注入页共用的文本编辑菜单能力。
// 宿主负责渲染菜单；本文件只处理目标识别、选区保持和编辑动作。
(function installDshdEditContext(global) {
  'use strict';

  const NON_TEXT_INPUT_TYPES = new Set([
    'button', 'checkbox', 'color', 'file', 'hidden', 'image', 'radio', 'range', 'reset', 'submit',
  ]);
  const IS_MAC = /Mac/i.test(navigator.userAgent);

  function findEditable(target) {
    const element = target instanceof Element ? target : target && target.parentElement;
    if (!element || !element.closest) return null;
    const editable = element.closest(
      'textarea, input, [contenteditable="true"], [contenteditable=""], [role="textbox"]',
    );
    if (!editable || editable.disabled || editable.getAttribute('aria-disabled') === 'true') return null;
    if (editable.tagName === 'INPUT') {
      const type = String(editable.type || 'text').toLowerCase();
      if (NON_TEXT_INPUT_TYPES.has(type)) return null;
    }
    return editable;
  }

  function isTextControl(editable) {
    return editable.tagName === 'INPUT' || editable.tagName === 'TEXTAREA';
  }

  function rangeBelongsTo(editable, range) {
    const container = range && range.commonAncestorContainer;
    const element = container && container.nodeType === Node.ELEMENT_NODE
      ? container
      : container && container.parentElement;
    return Boolean(element && (element === editable || editable.contains(element)));
  }

  function capture(editable) {
    const textControl = isTextControl(editable);
    let start = null;
    let end = null;
    let direction = 'none';
    let range = null;
    if (textControl) {
      try {
        start = editable.selectionStart;
        end = editable.selectionEnd;
        direction = editable.selectionDirection || 'none';
      } catch (_) {}
    } else {
      const selection = window.getSelection();
      if (selection && selection.rangeCount) {
        const candidate = selection.getRangeAt(0);
        if (rangeBelongsTo(editable, candidate)) range = candidate.cloneRange();
      }
    }
    const writable = !editable.readOnly && editable.getAttribute('aria-readonly') !== 'true';
    const hasSelection = textControl
      ? Number.isInteger(start) && Number.isInteger(end) && start !== end
      : Boolean(range && !range.collapsed);
    const hasContent = textControl
      ? String(editable.value || '').length > 0
      : String(editable.textContent || '').length > 0;
    let canUndo = false;
    let canRedo = false;
    try {
      canUndo = document.queryCommandEnabled('undo');
      canRedo = document.queryCommandEnabled('redo');
    } catch (_) {}
    return {
      editable, textControl, start, end, direction, range, writable, hasSelection, hasContent,
      canUndo, canRedo,
    };
  }

  function restore(snapshot) {
    const editable = snapshot.editable;
    if (!editable || !document.contains(editable)) return false;
    try { editable.focus({ preventScroll: true }); } catch (_) {
      try { editable.focus(); } catch (_) {}
    }
    if (snapshot.textControl && Number.isInteger(snapshot.start) && Number.isInteger(snapshot.end)) {
      try { editable.setSelectionRange(snapshot.start, snapshot.end, snapshot.direction); } catch (_) {}
    } else if (snapshot.range) {
      try {
        const selection = window.getSelection();
        selection.removeAllRanges();
        selection.addRange(snapshot.range);
      } catch (_) {}
    }
    return true;
  }

  function exec(snapshot, command) {
    if (!restore(snapshot)) return false;
    try { return document.execCommand(command); } catch (_) { return false; }
  }

  function dispatchPasteInput(editable, text) {
    let event;
    try {
      event = new InputEvent('input', {
        bubbles: true,
        data: text,
        inputType: 'insertFromPaste',
      });
    } catch (_) {
      event = new Event('input', { bubbles: true });
    }
    editable.dispatchEvent(event);
  }

  function insertText(snapshot, text) {
    if (!restore(snapshot)) return false;
    try {
      if (document.execCommand('insertText', false, text)) return true;
    } catch (_) {}
    if (!snapshot.textControl || typeof snapshot.editable.setRangeText !== 'function') return false;
    const editable = snapshot.editable;
    const start = Number.isInteger(editable.selectionStart) ? editable.selectionStart : editable.value.length;
    const end = Number.isInteger(editable.selectionEnd) ? editable.selectionEnd : start;
    editable.setRangeText(text, start, end, 'end');
    dispatchPasteInput(editable, text);
    return true;
  }

  function paste(snapshot) {
    if (!snapshot.writable || !restore(snapshot)) return Promise.resolve(false);
    try {
      if (document.execCommand('paste')) return Promise.resolve(true);
    } catch (_) {}
    if (!navigator.clipboard || !navigator.clipboard.readText) return Promise.resolve(false);
    return navigator.clipboard.readText()
      .then((text) => insertText(snapshot, text))
      .catch(() => false);
  }

  function selectAll(snapshot) {
    if (!restore(snapshot)) return false;
    if (snapshot.textControl && typeof snapshot.editable.select === 'function') {
      snapshot.editable.select();
      return true;
    }
    try { return document.execCommand('selectAll'); } catch (_) { return false; }
  }

  function createMenuItems(editable, labels, iconPrefix) {
    const snapshot = capture(editable);
    const prefix = iconPrefix || '';
    const modifier = IS_MAC ? '⌘' : 'Ctrl';
    const ariaModifier = IS_MAC ? 'Meta' : 'Control';
    return [
      {
        id: 'edit-undo', label: labels.undo, icon: prefix + 'undo', key: modifier + '+Z',
        ariaKey: ariaModifier + '+Z', enabled: snapshot.canUndo,
        act: () => exec(snapshot, 'undo'),
      },
      {
        id: 'edit-redo', label: labels.redo, icon: prefix + 'redo',
        key: IS_MAC ? '⇧+⌘+Z' : 'Ctrl+Y', ariaKey: IS_MAC ? 'Shift+Meta+Z' : 'Control+Y',
        enabled: snapshot.canRedo, act: () => exec(snapshot, 'redo'),
      },
      { sep: true },
      {
        id: 'edit-cut', label: labels.cut, icon: prefix + 'cut', key: modifier + '+X',
        ariaKey: ariaModifier + '+X', enabled: snapshot.writable && snapshot.hasSelection,
        act: () => exec(snapshot, 'cut'),
      },
      {
        id: 'edit-copy', label: labels.copy, icon: prefix + 'copy', key: modifier + '+C',
        ariaKey: ariaModifier + '+C', enabled: snapshot.hasSelection,
        act: () => exec(snapshot, 'copy'),
      },
      {
        id: 'edit-paste', label: labels.paste, icon: prefix + 'paste', key: modifier + '+V',
        ariaKey: ariaModifier + '+V', enabled: snapshot.writable,
        act: () => paste(snapshot),
      },
      { sep: true },
      {
        id: 'edit-select-all', label: labels.selectAll, icon: prefix + 'select', key: modifier + '+A',
        ariaKey: ariaModifier + '+A', enabled: snapshot.hasContent,
        act: () => selectAll(snapshot),
      },
    ];
  }

  global.__DSHD_EDIT_CONTEXT = Object.freeze({ findEditable, createMenuItems });
})(window);
