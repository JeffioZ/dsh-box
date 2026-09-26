// dsh 文档引导期续接遮罩：dsh 服务就绪后导航切换到的新文档在 JS 引导期是
// 裸 #root（上游 index.html 无加载壳），整页只有窗口底色。本遮罩复刻启动页
// boot 态视觉（几何/渐变/文案逐值衔接），dsh 真实应用挂载后
// 自动退场，观感为同一段加载的帧级延续（启动页就绪不淡出，WebView 跨源
// 导航保留旧帧直到本遮罩首帧）。
//
// 双通道注入（__dshdBootContinue guard 幂等合流）：
// ① initialization_script 快路径（bootstrap.rs 的 page_init_script）——
//    文档创建即执行，紧贴启动页末帧，消除「旧页卸载 → 注入到达」的空档；
//    以 dsh 独有的 #root 静态标记做门控，本壳页面（启动页/过渡页，无 #root）
//    不受影响；
// ② navigate-eval 兜底（inject_dsh_page 组合的最前段）——initialization_
//    script 偶发不触发的保险；已挂载文档上不装（防迟到注入闪一下 loading）。
// reload（心跳自愈/会话恢复）后 #root 重新为空，双通道重跑同样成立。
// 退场为两段式：卡片先溶入同色渐变（.12s，两块内容不交叉淡化），再整体
// 背景溶解（.2s）露出应用——应用在不透明遮罩下多获得 ~300ms 稳定首屏。
// 退场只有两条路：观察器判定真实挂载（[data-dsh-boot] 排除）与 30s 绝对
// 上限；load 后 2.5s 只做死文档判定（#root 仍空 ⟹ 入口 module 从未执行，
// 一次性 cache:'reload' 自愈，见 healDeadDocument），不做无条件退场。
//
// 视觉逐值复刻 ui/startup.css + ui/common.css 两主题令牌（注入目标文档没有
// 我们的令牌体系，只能内联字面量；防漂移由 Rust 测试 mask_splash_parity_holds
// 对账）；logo 内联 ui/assets/app-icon.svg 同源内容（跨源取不到本地资源）；
// 版本信息常驻主界面标题栏（titlebar.js），遮罩不再渲染 footer。
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
  // 过渡屏）画进 #root，React 接管期的 hydration 壳同样带该标记；真实应用
  // 挂载后标记消失（实测 rc.1 应用根是 <div data-slot="root"
  // style="display: contents;">——盒模型恒为 0×0，**高度类判定永远为假**，
  // 曾致观察器退场永久失效、只能靠绝对上限兜底）。因此判定只看标记：
  // #root 存在未带 data-dsh-boot 的子元素 = 引导期已过、真实应用已接管。
  function bootMounted() {
    var root = document.getElementById('root');
    if (!root || root.childElementCount === 0) return false;
    for (var i = 0; i < root.childElementCount; i++) {
      if (!root.children[i].hasAttribute('data-dsh-boot')) return true;
    }
    return false;
  }

  // 进度条不做相位交接：跨站点导航清空 window.name（WHATWG 2021 起，
  // Chromium 已实现），启动页（tauri.localhost/localhost:4321）与 dsh
  // （127.0.0.1:port）恒跨站点——经 window.name 传递动画相位的机制从未
  // 生效，已于 2026-09-26 整体移除。遮罩进度条从头播放 1.4s 周期，回卷
  // 在加载过渡中不可辨。勿经 URL 查询传相位（dsh token 交换只接受精确
  // GET /?token=），如确需交接只能用 URL fragment 通道。

  function install() {
    if (document.getElementById(ID)) return;
    // dsh 已挂载：引导期已过，不装遮罩——迟到的注入（initialization_script
    // 未触发、由 navigate-eval 或其 1.5s 起的自愈重试补上）到达时页面可能
    // 已就绪，无脑装上再立即拆除，正是「页面出来后又闪一下 loading」的来源
    // （菜单/心跳等其余注入段幂等无害，照常进行）
    if (bootMounted()) return;
    var c = dark ? {
      bg: '#151517', text: '#f9fafb', dim: '#cfd3d6', track: '#3a3a46',
      a1: '#5686fe', a2: '#679efe', g1: '#191a1f',
    } : {
      bg: '#f7f8fa', text: '#0f1115', dim: '#61666b', track: 'rgba(0,0,0,.1)',
      // 浅色 a2 是 --dshd-accent-hover 浅色档（#3d63c8），不是深色档的
      // #679efe——遮罩渐变尾色漂移曾靠 mask_splash_parity_holds 扩面拦截
      a1: '#4176e6', a2: '#3d63c8', g1: '#edeef4',
    };
    var font = '-apple-system,BlinkMacSystemFont,"Segoe UI","PingFang SC",'
      + '"Hiragino Sans GB","Microsoft YaHei","Helvetica Neue",Helvetica,Arial,sans-serif';
    // 关键帧与 ui/startup.css 的 slide 逐值一致，注入目标文档没有这些定义，
    // 需随遮罩自带
    var style = document.createElement('style');
    style.textContent = '@keyframes __dshd_bc_slide{0%{transform:translateX(-167%);opacity:0;}'
      + '14%{opacity:1;}86%{opacity:1;}100%{transform:translateX(917%);opacity:0;}}';
    var el = document.createElement('div');
    el.id = ID;
    el.setAttribute('role', 'status');
    el.append(style);
    el.setAttribute('aria-label', zh ? '正在加载' : 'Loading');
    // 居中补偿：宿主 webview 起点在 36px 标题栏之下，视口内居中相对整窗
    // 偏下 18px；底部 padding 36px 把内容中心抬到整窗正中（与启动页
    // .stage 的 calc 补偿一致，两段 loading 切换不跳位）
    el.style.cssText = 'position:fixed;inset:0;z-index:2147483647;'
      // 背景与启动页 body 同款径向渐变（startup.css 逐值复刻，含浅色起点）
      + 'background:radial-gradient(1200px 600px at 50% -10%,' + c.g1 + ' 0%,' + c.bg + ' 60%);'
      + 'display:flex;align-items:center;justify-content:center;padding-bottom:36px;'
      // 行高 1.5 与启动页 body 的 font 设定一致：标题(30)/副标题(18)/状态行
      // (21)逐值对齐，卡片总高与启动页全等（差 1px 都会在切换时可见）
      + 'line-height:1.5;font-family:' + font + ';';
    var card = document.createElement('div');
    card.style.cssText = 'display:flex;flex-direction:column;align-items:center;'
      + 'gap:8px;width:min(480px,86vw);';
    card.innerHTML =
      '<div style="width:64px;height:64px;">__DSHD_ICON__</div>'
      + '<div style="font-size:20px;font-weight:600;letter-spacing:.2px;margin-top:2px;color:' + c.text + ';">DSHBox</div>'
      + '<div style="font-size:12px;letter-spacing:.4px;margin-top:2px;color:' + c.dim + ';">'
      + (zh ? 'DeepSeek Harness 桌面端' : 'DeepSeek Harness for desktop') + '</div>'
      + '<div style="width:100%;margin-top:20px;display:flex;flex-direction:column;gap:10px;">'
      // 进度指示由进度条独立承担（UEUI 单指示元素）：状态行只留文字，
      // 文案与启动页就绪末帧逐字一致（startup.js / i18n.js loading 键）
      + '<div style="display:flex;align-items:center;justify-content:center;'
      + 'font-size:14px;font-weight:500;color:' + c.text + ';min-height:20px;">'
      + '<span>' + (zh ? '正在加载…' : 'Loading…') + '</span></div>'
      + '<div style="height:5px;width:100%;background:' + c.track
      + ';border-radius:999px;overflow:hidden;">'
      + '<div style="height:100%;width:12%;border-radius:999px;'
      + 'background:linear-gradient(90deg,' + c.a1 + ',' + c.a2 + ');'
      + (reduced ? '' : 'animation:__dshd_bc_slide 1.4s linear infinite;')
      + '"></div></div>'
      // 明细行（18px）与按钮行（28px）占位：等高于启动页 .status-detail 的
      // min-height 与 .install-actions 的 dshd-hold——卡片总高与启动页全等，
      // 两段 loading 以同一几何居中，帧级切换不跳位
      + '<div style="height:18px;width:100%;"></div>'
      + '<div style="height:28px;width:100%;"></div></div>';
    el.append(card);
    document.body.append(el);
    scheduleRemoval(el, card);
  }

  var dismissed = false;
  // 中毒文档自愈（会话级一次）。中毒有两种形态（2026-09-23/24/25 三次 dsh
  // 更新现场）：①死文档——缓存持旧 index、其入口 chunk 被新服务端 404，
  // #root 永远空；②活体错配——旧 index 与旧 chunk **都**在缓存里，旧版
  // 应用完整启动画出 boot 页（#root 非空！）后对不上新服务端而卡死，遮罩
  // 等到 30s 上限退场露出原生 loading。统一检测信号是**入口 chunk 指纹**：
  // load+2.5s 若 #root 为空或仍只有 boot 页，fetch 回源服务器当前的 index，
  // 比对文档入口 chunk 名与服务器 index 的入口 chunk 名——不一致即中毒，
  // location.replace 到带时间戳查询的 '/'（09-25 rc.2 现场教训：渲染进程
  // 内存缓存按完整 URL 键控，reload 主资源仍命中同 URL 旧条目；换从未请求
  // 过的 URL 击穿所有缓存层；静态服务按路径匹配、带查询照常返回 index）。
  // 一致则是正常慢启动，遮罩继续等。fetch 兼作可达性确认（失败不导航）。
  // guard 落在 sessionStorage（跨导航同标签存续）：不会循环；应用重启清零。
  var HEAL_KEY = '__dshd_boot_heal';
  function documentEntryChunk() {
    var script = document.querySelector('script[type="module"][src]');
    return script ? script.getAttribute('src') : '';
  }
  function healStaleDocument() {
    try {
      if (sessionStorage.getItem(HEAL_KEY)) return;
      fetch('/', { cache: 'reload' }).then(function (resp) {
        if (!resp.ok) return;
        return resp.text().then(function (text) {
          var match = text.match(/<script[^>]*type="module"[^>]*src="([^"]+)"/);
          var serverChunk = match ? match[1] : '';
          if (!serverChunk || serverChunk === documentEntryChunk()) return;
          sessionStorage.setItem(HEAL_KEY, '1');
          location.replace('/?_dshd_heal=' + Date.now());
        });
      }).catch(function () {});
    } catch (e) { /* 自愈失败静默：维持绝对上限等既有兜底路径 */ }
  }
  function scheduleRemoval(el, card) {
    var finish = function () {
      var node = document.getElementById(ID);
      if (node) node.remove();
    };
    var dismiss = function () {
      if (dismissed) return;
      dismissed = true;
      if (observer) observer.disconnect();
      if (quietObs) quietObs.disconnect();
      clearTimeout(quietTimer);
      clearTimeout(capTimer);
      // 绝不依赖 transitionend 保证移除（页面不可见/动画被取消都不悬置）
      var fade = function () {
        if (reduced) { finish(); return; }
        // 两段退场：先让 loading 卡片溶入同色渐变（两块内容不交叉淡化，
        // 画面安静），卡片走完后再整体溶解露出应用
        card.style.transition = 'opacity .12s ease-out';
        card.style.opacity = '0';
        setTimeout(function () {
          el.style.transition = 'opacity .2s ease-out';
          el.style.opacity = '0';
          setTimeout(finish, 210);
        }, 130);
      };
      if (reduced) { fade(); return; }
      // 判定命中的那帧内容先走完 layout/paint（两个 rAF）再开始退场，
      // 避免退场过程中露出尚未合成的空白首屏
      requestAnimationFrame(function () { requestAnimationFrame(fade); });
    };
    // 收幕时机由应用的安静程度驱动：挂载判定命中的时刻只是 hydration
    // commit，首屏「骨架→填充」的重排还要持续一阵——固定时延收幕会在
    // 冷启动时揭开仍在变动的表面。这里等 #root 子树 150ms 无变更（首屏
    // 落定）才开始两段收幕；硬上限 950ms 兜底持续小变更的应用，绝不滞留
    var exitArmed = false;
    var quietObs = null;
    var quietTimer = 0;
    var capTimer = 0;
    var armExit = function () {
      if (exitArmed) return;
      exitArmed = true;
      var root = document.getElementById('root');
      if (!root) { dismiss(); return; }
      try {
        quietObs = new MutationObserver(function () {
          clearTimeout(quietTimer);
          quietTimer = setTimeout(dismiss, 150);
        });
        quietObs.observe(root, {
          childList: true, subtree: true, attributes: true, characterData: true,
        });
      } catch (e) { /* 观察失败按固定时延收幕 */ }
      quietTimer = setTimeout(dismiss, 150);
      capTimer = setTimeout(dismiss, 950);
    };
    var observer = new MutationObserver(function () {
      if (bootMounted()) armExit();
    });
    observer.observe(document.documentElement, { childList: true, subtree: true });
    if (bootMounted()) { armExit(); return; }
    // load 后 2.5s 只做中毒判定，不再无条件退场：dsh ≥0.1.7-rc 的插件
    // 装载常超 2.5s，boot 页仍是 #root 唯一内容时由遮罩继续覆盖（定时退场
    // 会露出 dsh 的 boot 页，「两个 loading」回归即此），真实挂载交给上面
    // 的观察器。判定扩展到两种中毒形态（见 healStaleDocument 注释）：
    // #root 为空（死文档）或仍只有 boot 页（活体错配——boot 页在但应用
    // 永远不来）→ 入口 chunk 指纹比对，与服务端不一致才导航。module 入口
    // 阻塞 load、boot 页在 main.ts 同步画入 #root：load 已触发而 #root 空
    // 只能是入口从未执行；boot 页在而应用迟到则可能是慢启动（正常）或错
    // 配（中毒），指纹比对可区分。迟到的 eval 注入通道注册时 load 可能已
    // 触发，按 readyState 直接补判。
    var judgeAfterLoad = function () {
      setTimeout(function () {
        // bootMounted 对「无 #root / 空 #root / 只有 boot 页」都返回 false，
        // 无需再并列前两项
        if (!bootMounted()) healStaleDocument();
      }, 2500);
    };
    if (document.readyState === 'complete') judgeAfterLoad();
    else window.addEventListener('load', judgeAfterLoad);
    // 绝对上限：挂载形态若变（观察器永不命中）遮罩也绝不永久残留。30s ≈
    // 实测最慢首启（~7s）的 4 倍余量，且早于心跳 35s 判死线；boot 页的插件
    // 失败报告最迟在此刻露出（reload 治不了插件不兼容，只露出不强救）
    setTimeout(dismiss, 30000);
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
  // 观察目标必须是 document 而非 documentElement：本段在文档创建时刻执行
  // （initialization_script，先于解析开始），彼时 documentElement 还是 null
  // （2026-09-25 现场探针实证：observe 抛 TypeError 被整体 catch 吞掉，guard
  // 已设导致 eval 兜底被挡，遮罩永不安装——且深色主题下 PAGE_INIT 会先抛
  // 同样的错炸掉初始化脚本、guard 没设，遮罩靠 eval 兜底「碰巧」正常，缺陷
  // 被主题时段门控掩盖）。Document 本身恒为 Node，subtree 观察对 html/body/
  // #root 的出现效果完全一致。
  bootDocObs.observe(document, { childList: true, subtree: true });
  if (!tryInstall()) {
    // 长期无 #root 的文档（本壳页面）观察器随页面卸载自然释放；上限兜底防泄漏
    setTimeout(function () { bootDocObs.disconnect(); }, 15000);
  }
  } catch (e) { /* 遮罩失败静默：宁可露出短暂空白也不破坏其余注入 */ }
})();
