// dsh 文档引导期续接遮罩：dsh 服务就绪后导航切换到的新文档在 JS 引导期是
// 裸 #root（上游 index.html 无加载壳），整页只有窗口底色——「启动页淡出 →
// 空白 → 页面长出来」的断层即网页感来源。本遮罩复刻启动页 boot 态视觉，
// dsh 真实应用挂载后自动退场，观感为同一段加载的延续。
//
// 双通道注入（__dshdBootContinue guard 幂等合流）：
// ① initialization_script 快路径（bootstrap.rs 的 page_init_script）——
//    文档创建即执行，紧贴启动页淡出，消除「旧页卸载 → 注入到达」的空档；
//    以 dsh 独有的 #root 静态标记做门控，本壳页面（启动页/过渡页，无 #root）
//    不受影响；
// ② navigate-eval 兜底（inject_dsh_page 组合的最前段）——initialization_
//    script 偶发不触发的保险；已挂载文档上不装（防迟到注入闪一下 loading）。
// reload（心跳自愈/会话恢复）后 #root 重新为空，双通道重跑同样成立。
//
// 视觉逐值复刻 ui/startup.css + ui/common.css 两主题令牌（注入目标文档没有
// 我们的令牌体系，只能内联字面量）；logo 内联 ui/assets/app-icon.svg 同源
// 内容（跨源取不到本地资源）。
// 整体 try/catch：本段排在注入序列最前，任何异常不得中断后续菜单/心跳注入。
(function () {
  try {
  if (window.__dshdBootContinue) return;
  window.__dshdBootContinue = true;
  var ID = '__dshd_boot_continue';
  var reduced = window.matchMedia
    && window.matchMedia('(prefers-reduced-motion: reduce)').matches;
  var dark = window.matchMedia
    && window.matchMedia('(prefers-color-scheme: dark)').matches;
  var zh = String(navigator.language || 'zh').toLowerCase().indexOf('zh') === 0;

  // dsh 挂载判定：#root 出现真实应用内容。dsh ≥0.1.7-alpha.2 的 main.ts
  // 一执行就把 boot 页（[data-dsh-boot]：HARNESS 字标 + Loading plugins
  // 过渡屏）画进 #root，boot 页自身就是全屏高度——仅凭「#root 有子节点
  // 且有高度」会在插件装载期提前退场，露出 dsh boot 页再等真实内容
  // （「我们的 loading 消失一下、又冒出第二个偏下的 loading」即此）。
  // 真实应用挂载 = #root 出现 boot 页之外的内容（应用根元素替换或并存）。
  function bootMounted() {
    var root = document.getElementById('root');
    if (!root || root.childElementCount === 0) return false;
    for (var i = 0; i < root.childElementCount; i++) {
      var child = root.children[i];
      if (child.hasAttribute('data-dsh-boot')) continue;
      return child.getBoundingClientRect().height > 120;
    }
    return false;
  }

  function install() {
    if (document.getElementById(ID)) return;
    // dsh 已挂载：引导期已过，不装遮罩——迟到的注入（initialization_script
    // 未触发、由 navigate-eval 或其 1.5s 起的自愈重试补上）到达时页面可能
    // 已就绪，无脑装上再立即拆除，正是「页面出来后又闪一下 loading」的来源
    // （菜单/心跳等其余注入段幂等无害，照常进行）
    if (bootMounted()) return;
    var c = dark ? {
      bg: '#151517', text: '#f9fafb', dim: '#cfd3d6', track: '#3a3a46',
      a1: '#5686fe', a2: '#679efe',
    } : {
      bg: '#f7f8fa', text: '#0f1115', dim: '#61666b', track: 'rgba(0,0,0,.1)',
      a1: '#4176e6', a2: '#679efe',
    };
    var font = '-apple-system,BlinkMacSystemFont,"Segoe UI","PingFang SC",'
      + '"Hiragino Sans GB","Microsoft YaHei","Helvetica Neue",Helvetica,Arial,sans-serif';
    // 居中补偿：宿主 webview 起点在 36px 标题栏之下，视口内居中相对整窗
    // 偏下 18px；底部 padding 36px 把内容中心抬到整窗正中（与启动页
    // .stage 的 calc 补偿一致，两段 loading 切换不跳位）
    // 关键帧与 ui/common.css 的 spin / ui/startup.css 的 slide 逐值一致，
    // 注入目标文档没有这些定义，需随遮罩自带
    var style = document.createElement('style');
    style.textContent = '@keyframes __dshd_bc_spin{to{transform:rotate(360deg)}}'
      + '@keyframes __dshd_bc_slide{0%{transform:translateX(-167%);opacity:0;}'
      + '14%{opacity:1;}86%{opacity:1;}100%{transform:translateX(917%);opacity:0;}}';
    var el = document.createElement('div');
    el.id = ID;
    el.setAttribute('role', 'status');
    el.append(style);
    el.setAttribute('aria-label', zh ? '正在加载' : 'Loading');
    el.style.cssText = 'position:fixed;inset:0;z-index:2147483647;background:' + c.bg
      + ';display:flex;align-items:center;justify-content:center;padding-bottom:36px;font-family:' + font + ';';
    var card = document.createElement('div');
    card.style.cssText = 'display:flex;flex-direction:column;align-items:center;'
      + 'gap:8px;width:min(480px,86vw);';
    card.innerHTML =
      '<div style="width:64px;height:64px;">__DSHD_ICON__</div>'
      + '<div style="font-size:20px;font-weight:600;letter-spacing:.2px;color:' + c.text + ';">DSHBox</div>'
      + '<div style="font-size:12px;letter-spacing:.4px;color:' + c.dim + ';">'
      + (zh ? 'DeepSeek Harness 桌面端' : 'DeepSeek Harness for desktop') + '</div>'
      + '<div style="width:100%;margin-top:20px;display:flex;flex-direction:column;gap:10px;">'
      + '<div style="display:flex;align-items:center;gap:10px;justify-content:center;'
      + 'font-size:14px;font-weight:500;color:' + c.text + ';min-height:20px;">'
      + '<span style="width:16px;height:16px;flex:none;border:2px solid ' + c.track
      + ';border-top-color:' + c.a1 + ';border-radius:50%;' + (reduced ? '' : 'animation:__dshd_bc_spin .9s linear infinite;') + '"></span>'
      + '<span>' + (zh ? '正在加载…' : 'Loading…') + '</span></div>'
      + '<div style="height:5px;width:100%;background:' + c.track
      + ';border-radius:999px;overflow:hidden;">'
      + '<div style="height:100%;width:12%;border-radius:999px;'
      + 'background:linear-gradient(90deg,' + c.a1 + ',' + c.a2 + ');'
      + (reduced ? '' : 'animation:__dshd_bc_slide 1.4s linear infinite;') + '"></div></div></div>';
    el.append(card);
    document.body.append(el);
    scheduleRemoval(el);
  }

  var dismissed = false;
  function scheduleRemoval(el) {
    var finish = function () {
      var node = document.getElementById(ID);
      if (node) node.remove();
    };
    var dismiss = function () {
      if (dismissed) return;
      dismissed = true;
      if (observer) observer.disconnect();
      // 绝不依赖 transitionend 保证移除（页面不可见/动画被取消都不悬置）
      var fade = function () {
        if (reduced) { finish(); return; }
        el.style.transition = 'opacity .15s ease';
        el.style.opacity = '0';
        setTimeout(finish, 180);
      };
      if (reduced) { fade(); return; }
      // 判定命中的那帧内容先走完 layout/paint（两个 rAF）再开始淡出，
      // 避免淡出过程中露出尚未合成的空白首屏
      requestAnimationFrame(function () { requestAnimationFrame(fade); });
    };
    var observer = new MutationObserver(function () {
      if (bootMounted()) dismiss();
    });
    observer.observe(document.documentElement, { childList: true, subtree: true });
    if (bootMounted()) { dismiss(); return; }
    // 双兜底：load 后短暂宽限 + 硬上限——挂载形态若变（观察器永不命中），
    // 遮罩也绝不永久残留
    window.addEventListener('load', function () { setTimeout(dismiss, 2500); });
    setTimeout(dismiss, 10000);
  }

  // 本段经 initialization_script 在**所有**文档执行（文档创建即跑，先于
  // 任何网络内容，消除「启动页卸载 → 遮罩注入」之间的空档）；dsh 的
  // #root 是 HTML 里的静态标记、解析即有，本壳页面（启动页/过渡页）没有
  // ——以它做门控，遮罩只装进 dsh 文档。navigate-eval 的组合注入仍兜底
  // （initialization_script 偶发不触发时，__dshdBootContinue guard 幂等）
  var bootDocObs = new MutationObserver(function () { tryInstall(); });
  function tryInstall() {
    if (document.body && document.getElementById('root')) {
      bootDocObs.disconnect();
      install();
      return true;
    }
    return false;
  }
  bootDocObs.observe(document.documentElement, { childList: true, subtree: true });
  if (!tryInstall()) {
    // 长期无 #root 的文档（本壳页面）观察器随页面卸载自然释放；上限兜底防泄漏
    setTimeout(function () { bootDocObs.disconnect(); }, 15000);
  }
  } catch (e) { /* 遮罩失败静默：宁可露出短暂空白也不破坏其余注入 */ }
})();
