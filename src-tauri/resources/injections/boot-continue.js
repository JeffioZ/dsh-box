// dsh 文档引导期续接遮罩（注入通道）：dsh 服务就绪后导航切换到的新文档
// 在 JS 引导期是裸 #root（上游 index.html 无加载壳），整页只有窗口底色——
// 「启动页淡出 → 空白 → 页面长出来」的断层即网页感来源。本遮罩在
// navigate-eval 时（on_page_load Started，先于 dsh 客户端挂载）复刻启动页
// boot 态视觉，dsh 挂载内容后自动退场，观感为同一段加载的延续。
//
// 视觉逐值复刻 ui/startup.css + ui/common.css 两主题令牌（注入目标文档没有
// 我们的令牌体系，只能内联字面量）；logo 内联 ui/assets/app-icon.svg 同源
// 内容（跨源取不到本地资源）。经 inject_dsh_page 组合注入，is_dsh_url 门控
// 不会画到自家启动页上；reload（心跳自愈/会话恢复）重跑同样成立。
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

  function install() {
    if (document.getElementById(ID)) return;
    var c = dark ? {
      bg: '#151517', text: '#f9fafb', dim: '#cfd3d6', track: '#3a3a46',
      a1: '#5686fe', a2: '#679efe',
    } : {
      bg: '#f7f8fa', text: '#0f1115', dim: '#61666b', track: 'rgba(0,0,0,.1)',
      a1: '#4176e6', a2: '#679efe',
    };
    var font = '-apple-system,BlinkMacSystemFont,"Segoe UI","PingFang SC",'
      + '"Hiragino Sans GB","Microsoft YaHei","Helvetica Neue",Helvetica,Arial,sans-serif';
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
      + ';display:flex;align-items:center;justify-content:center;font-family:' + font + ';';
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
      if (reduced) { finish(); return; }
      el.style.transition = 'opacity .15s ease';
      el.style.opacity = '0';
      setTimeout(finish, 180);
    };
    var mounted = function () {
      // dsh 挂载判定：#root 出现首个元素子节点（上游入口为裸 div#root）
      var root = document.getElementById('root');
      return !!root && root.childElementCount > 0;
    };
    var observer = new MutationObserver(function () {
      if (mounted()) dismiss();
    });
    observer.observe(document.documentElement, { childList: true, subtree: true });
    if (mounted()) { dismiss(); return; }
    // 双兜底：load 后短暂宽限 + 硬上限——挂载形态若变（观察器永不命中），
    // 遮罩也绝不永久残留
    window.addEventListener('load', function () { setTimeout(dismiss, 2500); });
    setTimeout(dismiss, 10000);
  }

  if (document.body) {
    install();
  } else {
    var bodyObs = new MutationObserver(function () {
      if (document.body) { bodyObs.disconnect(); install(); }
    });
    bodyObs.observe(document.documentElement, { childList: true });
  }
  } catch (e) { /* 遮罩失败静默：宁可露出短暂空白也不破坏其余注入 */ }
})();
