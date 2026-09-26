// 内置页面与 dsh 注入页共用的文本编辑菜单能力。
// 宿主负责渲染菜单；本文件只处理目标识别、选区保持和编辑动作。
(function installDshdEditContext(global) {
  'use strict';

  const NON_TEXT_INPUT_TYPES = new Set([
    'button', 'checkbox', 'color', 'file', 'hidden', 'image', 'radio', 'range', 'reset', 'submit',
    // 日期时间类：有文本外观但 selectionStart 等文本选区 API 不可用/语义不同
    'date', 'time', 'datetime-local', 'month', 'week',
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

  // contenteditable（Lexical 类富编辑器）撤销/重做可用性的观察性跟踪。
  // 编辑器的历史栈深藏在模块闭包里读不到真实深度，设计上对**所有路径**
  // 封闭，三条原则：
  // ① 事件信号（beforeinput/paste/cut/drop/撤销与重做键）覆盖外部编辑
  //    （键盘、IME、Ctrl+V/X、拖放）——Lexical 拦截 paste/cut 后自行写入、
  //    beforeinput 不会发出，必须直听事件本身；菜单的合成键不是
  //    isTrusted，由动作侧自报；
  // ② 本模块自己执行的编辑（菜单粘贴/剪切）由动作侧调 noteEdit 自报，
  //    不赌浏览器对 execCommand 发不发事件（「菜单粘贴后撤销灰」的教训）；
  // ③ 到判定双向封口：撤销生效后文本回到**基线**（首见时的纯文本）⟹ 历史到底，
  //    熄灭撤销；重做生效后文本回到**栈顶**（本撤销序列开始前的纯文本）⟹
  //    重做栈已尽，熄灭重做——两端都是零死点击。程序化写入（草稿恢复/
  //    引用插入）无事件可观察：草稿恢复后 dsh 会切断历史，灰是真相；
  //    引用插入是可撤销的，灰属安全方向的罕见缺失（快捷键可用）。
  // 状态指纹 = 文本 + 光标（折叠态与偏移）：键下发前后纹丝不动 ⟺ 动作
  // 空操作，按用户感知判定（内部历史走没走一格与死点击无关）。
  // 「撤销到底」的判定用**基线**：首次见到编辑器（任何交互发生前——
  // beforeinput 在编辑应用前到达、菜单在 capture 时先记基线）的纯文本
  // 即可撤销历史的观测起点；撤销应用后的文本回到基线 ⟺ 真正到底，即刻
  // 熄灭。不用「textContent 为空」判定——编辑器的空态可能带占位结构/
  // 残留节点，文本非空但历史已尽（「撤销成功后菜单仍亮、再点一次才灰」
  // 即此）。基线天然适配草稿恢复：基线=恢复的草稿内容，正是 dsh 切断
  // 历史之处。基线只比文本不比光标——撤销到底后光标位置与首见时不同
  // 不应阻碍熄灭。
  const richEditorHistory = (() => {
    const edited = new WeakSet();
    const redoable = new WeakSet();
    const exhausted = new WeakSet(); // 撤销已到底，下次编辑前保持熄灭
    const baselines = new WeakMap(); // 首见时的纯文本——历史的观测起点
    const redoTops = new WeakMap(); // 重做栈顶：本撤销序列开始前的纯文本
    const asRichEditable = (target) => {
      if (!(target instanceof Element) || !target.closest) return null;
      const el = target.closest('[contenteditable="true"], [contenteditable=""]');
      return el && el.isContentEditable ? el : null;
    };
    const fingerprint = (el) => {
      const sel = el.ownerDocument.getSelection();
      const inEl = sel && sel.anchorNode && el.contains(sel.anchorNode);
      return el.textContent + '|'
        + (inEl ? (sel.isCollapsed ? '1' : '0') + ':' + sel.anchorOffset + ':' + sel.focusOffset : 'x');
    };
    const ensureBaseline = (el) => {
      if (!baselines.has(el)) baselines.set(el, String(el.textContent || ''));
    };
    const noteEdit = (el) => {
      edited.add(el);
      exhausted.delete(el);
      redoable.delete(el); // 新编辑清空重做栈
      redoTops.delete(el); // …并终结当前撤销序列（栈顶作废）
    };
    const verifyApplied = (el, onNoop, onApplied) => {
      const before = fingerprint(el);
      setTimeout(() => {
        if (fingerprint(el) === before) onNoop();
        else onApplied(el);
      }, 150);
    };
    const undoIssued = (el) => {
      // 撤销应用前的文本 = 本撤销序列中重做可爬回的最高点（栈顶）。
      // 只记首个生效撤销的：后续撤销只往重做栈压入更低状态，顶不变；
      // 序列被新编辑终结时 noteEdit 清除
      const topBeforeUndo = String(el.textContent || '');
      verifyApplied(
        el,
        // 空操作 ⟹ 撤销栈已空，熄灭撤销；但不能动 redoable——「撤销栈空」
        // 不等于「重做栈空」（基线漏采的边缘下用户可能已全部撤销、重做栈
        // 是满的），此前生效撤销留下的重做证据仍然有效
        () => { exhausted.add(el); edited.delete(el); },
        (applied) => {
          exhausted.delete(applied);
          redoable.add(applied);
          if (!redoTops.has(applied)) redoTops.set(applied, topBeforeUndo);
          // 撤销生效且文本回到基线：已到底，提前熄灭（零死点击）
          if (baselines.get(applied) === String(applied.textContent || '')) {
            exhausted.add(applied);
          }
        },
      );
    };
    const redoIssued = (el) => verifyApplied(
      el,
      () => redoable.delete(el),
      (applied) => {
        // 重做产生的新状态可撤销，撤销复活
        exhausted.delete(applied);
        edited.add(applied);
        // 重做生效且文本回到栈顶（本撤销序列开始前的状态）：重做栈已尽，
        // 即刻熄灭——与撤销的基线判定对称，零死点击
        if (redoTops.get(applied) === String(applied.textContent || '')) {
          redoable.delete(applied);
        }
      },
    );
    document.addEventListener(
      'keydown',
      (e) => {
        // 本菜单发出的合成键不是 isTrusted，由 undo()/redo() 直接自报
        if (!e.isTrusted || !(e.ctrlKey || e.metaKey)) return;
        const editable = asRichEditable(e.target);
        if (!editable) return;
        const key = (e.key || '').toLowerCase();
        if (key === 'z' && !e.shiftKey) { ensureBaseline(editable); undoIssued(editable); }
        else if (key === 'y' || (key === 'z' && e.shiftKey)) { ensureBaseline(editable); redoIssued(editable); }
      },
      true,
    );
    document.addEventListener(
      'beforeinput',
      (e) => {
        if (e.inputType === 'historyUndo' || e.inputType === 'historyRedo') return;
        const editable = asRichEditable(e.target);
        if (editable) { ensureBaseline(editable); noteEdit(editable); }
      },
      true,
    );
    // Lexical 拦截 paste/cut 后自行写入、beforeinput 不发；捕获阶段先于其
    // 处理器运行，preventDefault 与否都能观察到；拖放文本同理
    for (const type of ['paste', 'cut', 'drop']) {
      document.addEventListener(
        type,
        (e) => {
          const editable = asRichEditable(e.target);
          if (editable) { ensureBaseline(editable); noteEdit(editable); }
        },
        true,
      );
    }
    return {
      // 可撤销：见过编辑（事件信号或本模块自报）且未被指纹证实「到底」。
      // 不能加「有内容 ⟹ 可撤销」兜底：dsh 恢复上次草稿/程序化写入的内容
      // 没有任何编辑事件，且恢复后历史被 dsh 切断——有内容但点亮是谎言。
      // 参考：官方桌面端（Electron）的编辑菜单干脆全部恒亮（不猜栈深），
      // 动作一律合成 Ctrl+Z/Y/X/C/V（sendInputEvent，源码注释明言「编辑器
      // 自管历史、听键事件而非 Chromium 原生撤销栈」）；本壳按用户要求
      // 提供比官方更诚实的亮灭，代价是这些观察性近似。
      canUndo: (el) => !exhausted.has(el) && edited.has(el),
      redoable: (el) => redoable.has(el),
      ensureBaseline,
      noteEdit,
      undoIssued,
      redoIssued,
    };
  })();

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
    // 可用性：原生输入框问原生撤销栈；contenteditable（Lexical）的历史在
    // 模块闭包里读不到真实深度，用观察信号近似（见 richEditorHistory）——
    // 恒启用会出现「启用但点击无反应」的错位（典型：刚输入文字时重做
    // 本应为灰）
    let canUndo = false;
    let canRedo = false;
    if (textControl) {
      try {
        canUndo = document.queryCommandEnabled('undo');
        canRedo = document.queryCommandEnabled('redo');
      } catch (_) {}
    } else {
      // 菜单开在编辑发生前时（典型：空框右键粘贴），此刻即基线
      richEditorHistory.ensureBaseline(editable);
      canUndo = richEditorHistory.canUndo(editable);
      canRedo = richEditorHistory.redoable(editable);
    }
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

  // 富文本编辑器（dsh 输入框是 Lexical）自管撤销历史并挂在 keydown 键绑定
  // 上，与原生 execCommand 互不相通：Lexical 的编辑全部是程序化 DOM 写入，
  // 原生撤销栈恒空（queryCommandEnabled 恒 false），execCommand('undo')
  // 触不到它的历史。合成 keydown 可以让编辑器键绑定接手——非受信事件不会
  // 触发浏览器默认行为，对原生控件无副作用。
  function sendPrimaryKey(editable, key, withShift) {
    try {
      editable.dispatchEvent(new KeyboardEvent('keydown', {
        key,
        bubbles: true,
        cancelable: true,
        ctrlKey: !IS_MAC,
        metaKey: IS_MAC,
        shiftKey: Boolean(withShift),
      }));
    } catch (_) {}
  }

  // 撤销/重做的动作链：原生输入框走 execCommand（原生撤销栈）。contenteditable
  // 必须直达合成主键，不能先试 execCommand——菜单开合伴随的 focus/选区恢复
  // 会让 Chromium 在原生栈里记下**不可见的选区事务**，execCommand('undo')
  // 先把它消耗掉（返回 true、无文本变化），表现正是「点两次才有一次效果」；
  // Lexical 的历史只能经键绑定触达（Z / Windows Ctrl+Y / mac ⇧⌘Z，
  // HistoryPlugin 默认绑定）。本壳面内的 contenteditable 只有 Lexical 一类
  // 富编辑器，无依赖原生 execCommand 撤销的普通 contenteditable。
  function undo(snapshot) {
    if (!restore(snapshot)) return false;
    if (snapshot.textControl) return exec(snapshot, 'undo');
    sendPrimaryKey(snapshot.editable, 'z', false);
    // 撤销已下发：指纹校验记账（生效 ⟹ 可重做并按基线判定熄灭撤销；
    // 空操作 ⟹ 撤销栈空，熄灭撤销）
    richEditorHistory.undoIssued(snapshot.editable);
    return true;
  }

  function redo(snapshot) {
    if (!restore(snapshot)) return false;
    if (snapshot.textControl) return exec(snapshot, 'redo');
    if (IS_MAC) sendPrimaryKey(snapshot.editable, 'z', true);
    else sendPrimaryKey(snapshot.editable, 'y', false);
    // 重做已下发：指纹校验记账（空操作 ⟹ 重做栈耗尽，熄灭）
    richEditorHistory.redoIssued(snapshot.editable);
    return true;
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
    let inserted = false;
    try {
      inserted = document.execCommand('insertText', false, text);
    } catch (_) {}
    if (!inserted && snapshot.textControl && typeof snapshot.editable.setRangeText === 'function') {
      const editable = snapshot.editable;
      const start = Number.isInteger(editable.selectionStart) ? editable.selectionStart : editable.value.length;
      const end = Number.isInteger(editable.selectionEnd) ? editable.selectionEnd : start;
      editable.setRangeText(text, start, end, 'end');
      dispatchPasteInput(editable, text);
      inserted = true;
    }
    // 编辑由本模块亲手执行：直接自报（不赌浏览器对 execCommand 发不发
    // beforeinput——「菜单粘贴后撤销灰」的教训）
    if (inserted && !snapshot.textControl) richEditorHistory.noteEdit(snapshot.editable);
    return inserted;
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
        act: () => undo(snapshot),
      },
      {
        id: 'edit-redo', label: labels.redo, icon: prefix + 'redo',
        key: IS_MAC ? '⇧+⌘+Z' : 'Ctrl+Y', ariaKey: IS_MAC ? 'Shift+Meta+Z' : 'Control+Y',
        enabled: snapshot.canRedo, act: () => redo(snapshot),
      },
      { sep: true },
      {
        id: 'edit-cut', label: labels.cut, icon: prefix + 'cut', key: modifier + '+X',
        ariaKey: ariaModifier + '+X', enabled: snapshot.writable && snapshot.hasSelection,
        // 剪切同粘贴：成功即自报编辑（cut 事件是否发出不赌浏览器）
        act: () => {
          const ok = exec(snapshot, 'cut');
          if (ok && !snapshot.textControl) richEditorHistory.noteEdit(snapshot.editable);
          return ok;
        },
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
