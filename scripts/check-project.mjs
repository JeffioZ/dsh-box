import fs from 'node:fs';
import path from 'node:path';
import vm from 'node:vm';
import { execFileSync, spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const failures = [];
const fail = (message) => failures.push(message);
const read = (file) => fs.readFileSync(path.join(root, file), 'utf8');
const tracked = execFileSync(
  'git',
  ['ls-files', '--cached', '--others', '--exclude-standard'],
  { cwd: root, encoding: 'utf8' },
)
  .split(/\r?\n/)
  .filter((file) => file && fs.existsSync(path.join(root, file)));

const knownText = new Set([
  '', '.css', '.html', '.js', '.json', '.lock', '.md', '.mjs', '.ps1', '.rs',
  '.svg', '.toml', '.xml', '.yaml', '.yml',
]);
const knownBinary = new Set(['.icns', '.ico', '.png']);
const strictUtf8 = new TextDecoder('utf-8', { fatal: true });

for (const file of tracked) {
  const ext = path.extname(file).toLowerCase();
  if (!knownText.has(ext) && !knownBinary.has(ext)) fail(`未纳入项目检查的文件类型: ${file}`);
  const stat = fs.statSync(path.join(root, file));
  if (stat.size === 0) fail(`空文件: ${file}`);
  if (knownText.has(ext)) {
    const bytes = fs.readFileSync(path.join(root, file));
    let text = '';
    try {
      text = strictUtf8.decode(bytes);
    } catch {
      fail(`${file}: 不是有效的 UTF-8 文本`);
    }
    if (text.includes('\0')) fail(`${file}: 文本中包含 NUL 字节`);
  }
}

const normalizedNames = new Set();
for (const file of tracked) {
  const normalized = file.replaceAll('\\', '/').normalize('NFC').toLowerCase();
  if (normalizedNames.has(normalized)) fail(`文件名在大小写不敏感系统上冲突: ${file}`);
  normalizedNames.add(normalized);
}

for (const file of tracked.filter((file) => path.extname(file) === '.json')) {
  try {
    JSON.parse(read(file));
  } catch (error) {
    fail(`${file}: JSON 解析失败: ${error.message}`);
  }
}

for (const file of tracked.filter((file) => /\.(?:js|mjs)$/.test(file))) {
  const result = spawnSync(process.execPath, ['--check', file], { cwd: root, encoding: 'utf8' });
  if (result.status !== 0) fail(`${file}: JavaScript 语法失败\n${result.stderr.trim()}`);
}

for (const file of tracked.filter((file) => file.endsWith('.html'))) {
  const html = read(file);
  const withoutScripts = html
    .replace(/<script\b[\s\S]*?<\/script>/gi, '')
    .replace(/<style\b[\s\S]*?<\/style>/gi, '');
  const ids = [...withoutScripts.matchAll(/\bid=["']([^"']+)["']/g)].map((match) => match[1]);
  for (const id of new Set(ids)) {
    if (ids.filter((value) => value === id).length > 1) fail(`${file}: 重复静态 id=${id}`);
  }
  for (const match of html.matchAll(/<script(?![^>]*\bsrc=)[^>]*>([\s\S]*?)<\/script>/gi)) {
    try {
      new vm.Script(match[1], { filename: `${file}:inline-script` });
    } catch (error) {
      fail(`${file}: 内联 JavaScript 语法失败: ${error.message}`);
    }
  }
  if (!/<meta\s+name=["']viewport["'][^>]*>/i.test(html)) {
    fail(`${file}: 缺少逻辑视口 meta，高 DPI/窄视口布局会失去基准`);
  }
  for (const match of html.matchAll(/<button\b([^>]*)>/gi)) {
    if (!/\btype=["']button["']/i.test(match[1])) fail(`${file}: button 必须显式声明 type="button"`);
  }
  for (const match of html.matchAll(/\b(?:src|href)=["']([^"']+)["']/gi)) {
    const target = match[1];
    if (!target || /^(?:#|[a-z]+:|\/\/)/i.test(target)) continue;
    const local = path.resolve(root, path.dirname(file), decodeURIComponent(target.split(/[?#]/)[0]));
    if (!fs.existsSync(local)) fail(`${file}: 本地引用不存在: ${target}`);
  }
}

const i18n = read('ui/i18n.js');
const messageMatch = i18n.match(/const DSHD_MESSAGES = (\{[\s\S]*?\n\});/);
if (!messageMatch) {
  fail('ui/i18n.js: 找不到 DSHD_MESSAGES');
} else {
  const keyLines = [...messageMatch[1].matchAll(/^\s{2}([A-Za-z_$][\w$]*):/gm)].map((match) => match[1]);
  for (const key of new Set(keyLines)) {
    if (keyLines.filter((value) => value === key).length > 1) fail(`ui/i18n.js: 重复 i18n key: ${key}`);
  }
  try {
    const messages = vm.runInNewContext(`(${messageMatch[1]})`);
    for (const [key, pair] of Object.entries(messages)) {
      if (!Array.isArray(pair) || pair.length !== 2 || pair.some((value) => typeof value !== 'string' || !value)) {
        fail(`ui/i18n.js: ${key} 必须包含非空中英双语`);
        continue;
      }
      const placeholders = (value) => [...new Set(
        [...value.matchAll(/\{([A-Za-z_$][\w$]*)\}/g)].map((match) => match[1]),
      )].sort();
      const zhPlaceholders = placeholders(pair[0]);
      const enPlaceholders = placeholders(pair[1]);
      if (JSON.stringify(zhPlaceholders) !== JSON.stringify(enPlaceholders)) {
        fail(`ui/i18n.js: ${key} 的中英文占位符不一致`);
      }
    }
    const usages = new Set();
    for (const file of tracked.filter((file) => file.startsWith('ui/') && /\.(?:html|js)$/.test(file))) {
      const text = read(file);
      for (const match of text.matchAll(/\bdata-i18n(?:-placeholder|-title|-aria-label)?=["']([^"']+)["']/g)) usages.add(match[1]);
      for (const match of text.matchAll(/\bdshdT\(\s*["']([^"']+)["']/g)) usages.add(match[1]);
    }
    for (const key of usages) {
      if (!(key in messages)) fail(`UI 使用了未定义的 i18n key: ${key}`);
    }
  } catch (error) {
    fail(`ui/i18n.js: DSHD_MESSAGES 无法求值: ${error.message}`);
  }
}

for (const file of tracked.filter((file) => file.startsWith('ui/') && file.endsWith('.html'))) {
  const html = read(file);
  if (html.includes('common.js')) {
    const i18nAt = html.indexOf('i18n.js');
    const commonAt = html.indexOf('common.js');
    if (i18nAt < 0 || i18nAt > commonAt) fail(`${file}: i18n.js 必须在 common.js 之前加载`);
  }
}
for (const file of ['ui/index.html', 'ui/control-center.html']) {
  const html = read(file);
  if (/<(?:style|script)(?![^>]*\bsrc=)[^>]*>[\s\S]*?<\/(?:style|script)>/i.test(html)) {
    fail(`${file}: 不应保留内联 CSS/JavaScript`);
  }
  const editContextAt = html.indexOf('edit-context.js');
  const commonAt = html.indexOf('common.js');
  const menuAt = html.indexOf('menu.js');
  if (editContextAt < 0 || commonAt < editContextAt || menuAt < commonAt) {
    fail(`${file}: 文本编辑能力必须先于 common.js 加载，menu.js 必须随后提供自绘菜单渲染`);
  }
}
const modelUi = read('ui/control-center-settings.js');
const runtimeHeading = modelUi.indexOf('settings-runtime-heading');
const apiKeyField = modelUi.indexOf('settings-api-key', runtimeHeading);
const runtimeSectionEnd = modelUi.indexOf("'</section>' +", apiKeyField);
if (runtimeHeading < 0 || apiKeyField < runtimeHeading || runtimeSectionEnd < apiKeyField
    || modelUi.includes('settings-api-heading') || modelUi.includes('api-key-box')) {
  fail('设置页的 DeepSeek API 必须归入“服务管理”，不能保留单独的浅层板块');
}
const renderStart = modelUi.indexOf('function miRenderResult(preview)');
const applyAction = modelUi.indexOf("applyBtn.textContent = dshdT('modelImportApply')", renderStart);
const renderEnd = modelUi.indexOf('box.hidden = false', renderStart);
if (renderStart < 0 || applyAction < renderStart || renderEnd < applyAction) {
  fail('模型导入预览必须在有无 API Key 两种情况下都渲染应用按钮');
}
try {
  new vm.Script([
    modelUi,
    read('ui/control-center-plugins.js'),
    read('ui/control-center.js'),
  ].join('\n'), { filename: 'control-center:combined-classic-scripts' });
} catch (error) {
  fail(`控制中心分拆脚本存在跨文件声明/语法冲突: ${error.message}`);
}
const pluginUi = read('ui/control-center-plugins.js');
const pluginCommands = read('src-tauri/src/commands/mod.rs');
for (const contract of [
  "invoke('plugin_reinstallable_builtins')",
  "catalogPluginRow(plugin, 'pluginReinstall')",
  'refreshReinstallableBuiltins()',
]) {
  if (!pluginUi.includes(contract)) fail(`插件页缺少卸载后手动重装契约: ${contract}`);
}
if (!pluginCommands.includes('plugins::plugin_reinstallable_builtins')) {
  fail('Rust IPC 未注册卸载内置插件的手动重装目录');
}

const sharedMenu = read('ui/menu.js');
try {
  const context = {};
  vm.runInNewContext(sharedMenu, context, { filename: 'ui/menu.js' });
  const normalized = context.dshdNormalizeMenuItems([
    { sep: true },
    { id: 'first' },
    { sep: true },
    { sep: true },
    { id: 'last' },
    { sep: true },
  ]);
  if (JSON.stringify(normalized) !== JSON.stringify([
    { id: 'first' },
    { sep: true },
    { id: 'last' },
  ])) {
    fail('ui/menu.js: 菜单分隔线只能保留在两个相邻条目组之间');
  }
} catch (error) {
  fail(`ui/menu.js: 共享菜单契约无法执行: ${error.message}`);
}

const startupCss = read('ui/startup.css');
const startupButton = startupCss.match(/\.btn\s*\{([^}]*)\}/)?.[1] || '';
for (const contract of ['display: inline-flex', 'align-items: center', 'justify-content: center', 'text-align: center']) {
  if (!startupButton.includes(contract)) fail(`ui/startup.css: 启动页按钮缺少居中契约 ${contract}`);
}
const sharedCss = read('ui/common.css');
if (!sharedCss.includes('.dshd-password-field .dshd-input::-ms-reveal')) {
  fail('ui/common.css: 自绘密码可见性按钮必须隐藏 WebView2 原生 reveal 控件');
}
const controlCss = read('ui/control-center.css');
const titlebarHtml = read('ui/titlebar.html');
for (const [name, rule] of [
  ['共享按钮', sharedCss.match(/^\.dshd-btn\s*\{([^}]*)\}/m)?.[1] || ''],
  ['模型配置按钮', controlCss.match(/^\.mi-btn\s*\{([^}]*)\}/m)?.[1] || ''],
  ['标题栏按钮', titlebarHtml.match(/^\.tb-btn\s*\{([^}]*)\}/m)?.[1] || ''],
]) {
  for (const contract of ['display: inline-flex', 'align-items: center', 'justify-content: center']) {
    if (!rule.includes(contract)) fail(`${name}缺少居中契约 ${contract}`);
  }
}
if (/\.dshd-menu-surface\s*\{[^}]*border-bottom\s*:\s*0/s.test(sharedCss)
    || /\.dshd-menu-surface\s*\{[^}]*padding\s*:\s*4px\s+4px\s+8px/s.test(sharedCss)) {
  fail('ui/common.css: 菜单表面不得在退出项下保留额外空白或移除底边');
}
for (const contract of [
  '--dshd-shadow-lv3:',
  'box-shadow: var(--dshd-shadow-lv3)',
  '.dshd-menu-motion.dshd-menu-animating',
  '.dshd-row.danger:not(:disabled):hover .ic',
]) {
  if (!sharedCss.includes(contract)) fail(`ui/common.css: 浮层视觉契约缺少 ${contract}`);
}
const baseMenuMotion = sharedCss.match(/\.dshd-menu-motion\s*\{([^}]*)\}/)?.[1] || '';
if (baseMenuMotion.includes('will-change')) {
  fail('ui/common.css: 菜单静止时不得长期保留 will-change 合成层');
}
for (const [file, css] of [
  ['ui/control-center.css', controlCss],
  ['ui/startup.css', startupCss],
]) {
  if (!css.includes('box-shadow: var(--dshd-shadow-lv3)')) {
    fail(`${file}: 独立浮层必须复用统一 shadow-lv3 token`);
  }
}
const trayMenuHtml = read('ui/tray-menu.html');
if (!/padding:\s*24px\s+36px\s+48px/.test(trayMenuHtml)
    || !trayMenuHtml.includes('align-items: flex-start')
    || !trayMenuHtml.includes('flex: none; width: 100%')
    || /#card\s*\{[^}]*flex:\s*1\b/s.test(trayMenuHtml)
    || !trayMenuHtml.includes('menu.setItems(items, true)')) {
  fail('ui/tray-menu.html: 托盘菜单必须按内容撑高、预留完整阴影安全区并在每次展示前重建视觉状态');
}
if ((sharedCss.match(/--dshd-shadow-lv3:/g) || []).length !== 1) {
  fail('ui/common.css: shadow-lv3 必须由单一共享 token 定义，主题不得重复覆盖同值');
}
const titlebarJs = read('ui/titlebar.js');
if (!titlebarJs.includes('36 + content + 48') || !titlebarJs.includes("mainMenuMotion.open('-4px')")) {
  fail('ui/titlebar.js: 主菜单阴影安全区或入场位移未与共享浮层对齐');
}
const titlebarRust = read('src-tauri/src/titlebar.rs');
if (!titlebarRust.includes('main.set_auto_resize(false)')
    || !titlebarRust.includes('sync_bounds_for_size')
    || !titlebarRust.includes('Size::Physical')) {
  fail('src-tauri/src/titlebar.rs: 主窗口缩放必须由单一物理像素布局路径负责');
}
const trayMenuRust = read('src-tauri/src/tray_menu.rs');
if (!trayMenuRust.includes('eval_with_callback')
    || !trayMenuRust.includes('wait_for_geometry_then_prepare')) {
  fail('src-tauri/src/tray_menu.rs: 托盘菜单必须等待几何与首帧脚本完成后再显示');
}
if (read('ui/control-center.html').includes('&#xE8BB;')) {
  fail('ui/control-center.html: 控制中心关闭按钮必须使用跨平台 SVG');
}
const startupJs = read('ui/startup.js');
const runtimeRenderer = startupJs.indexOf('function renderRuntimePresentation(');
const onboardingRenderer = startupJs.indexOf(
  'renderRuntimePresentation(payload, {', startupJs.indexOf('function renderOnboardingRuntime('),
);
const startupRenderer = startupJs.indexOf(
  'renderRuntimePresentation(payload, {', startupJs.indexOf('function setStatus('),
);
if (runtimeRenderer < 0 || onboardingRenderer < runtimeRenderer || startupRenderer < runtimeRenderer) {
  fail('ui/startup.js: 普通启动与首次配置必须复用同一套运行状态渲染器');
}
for (const key of ['ArrowDown', 'ArrowUp', 'Home', 'End', 'Escape']) {
  if (!startupJs.includes(`event.key === '${key}'`)) fail(`ui/startup.js: 自绘下拉缺少 ${key} 键盘交互`);
}
const startupHtml = read('ui/index.html');
for (const contract of [
  'id="ob-runtime" class="ob-runtime hidden"',
  'id="ob-runtime-progress"',
  'data-install-cancel',
  'data-install-reinstall',
]) {
  if (!startupHtml.includes(contract)) fail(`ui/index.html: 首次配置运行环境区缺少契约 ${contract}`);
}
for (const contract of [
  'renderOnboardingRuntime',
  'onboardingPausedByError',
  "document.querySelectorAll('[data-install-cancel]')",
  "document.querySelectorAll('[data-install-reinstall]')",
]) {
  if (!startupJs.includes(contract)) fail(`ui/startup.js: 首次安装取消/重试流程缺少契约 ${contract}`);
}
if (!startupJs.includes("generation: installGeneration")
    || !read('src-tauri/src/app_state/mod.rs').includes('pub install_generation: u64')) {
  fail('首次安装取消必须使用带引导轮次的显式业务状态');
}
for (const contract of [
  'id="service-choice-box"',
  'id="btn-connect-external"',
  'id="btn-start-local"',
  'id="btn-use-local"',
]) {
  if (!startupHtml.includes(contract)) fail(`ui/index.html: 外部服务选择流程缺少契约 ${contract}`);
}
const dshLifecycle = read('src-tauri/src/dsh.rs');
if (!startupJs.includes("invoke('choose_service'")
    || !read('src-tauri/src/commands/mod.rs').includes('choose_service,')
    || !dshLifecycle.includes('"host.describe"')
    || !dshLifecycle.includes('config.port = 0;')
    || dshLifecycle.includes('PORT_SCAN_WINDOW')) {
  fail('服务归属必须经过显式选择与官方 RPC 校验，端口回退必须交给系统分配');
}
const updater = read('src-tauri/src/updater/mod.rs');
const restartNavigation = read('src-tauri/src/webview/navigation.rs');
const restartService = updater.indexOf('fn restart_service_locked(');
const restartStatus = updater.indexOf('emit_status(app, BootPhase::Starting', restartService);
const enterRestartView = updater.indexOf('enter_restart_view(app, resume_url.is_some())', restartStatus);
const stopRestartedService = updater.indexOf('dsh::shutdown(app)', enterRestartView);
if (restartService < 0 || restartStatus < restartService || enterRestartView < restartStatus
    || stopRestartedService < enterRestartView
    || !restartNavigation.includes('navigate(app, &local_app_entry_url(dev_origin.as_ref()))')) {
  fail('重启服务必须先进入与普通启动相同的内置加载页，再停止托管服务');
}

const statusbar = read('ui/statusbar.js');
if (statusbar.includes('.errorKind')) fail('ui/statusbar.js: BalancePayload 契约字段必须使用 error_kind');
const authBranch = statusbar.indexOf("b.error_kind === 'no_key'");
const genericFailure = statusbar.indexOf('if (!b.ok)', authBranch);
if (authBranch < 0 || genericFailure < authBranch) {
  fail('ui/statusbar.js: 凭据错误必须在通用余额失败分支之前处理');
}
if (!statusbar.includes('!payload.error_kind && lastBalance && lastBalance.ok')) {
  fail('ui/statusbar.js: 只有瞬时错误可保留 stale 余额，凭据错误必须立即替换');
}
const settingsCommands = read('src-tauri/src/commands/settings.rs');
const commandRegistry = read('src-tauri/src/commands/mod.rs');
if (!modelUi.includes("invoke('set_deepseek_api_key'")
    || !settingsCommands.includes('pub fn set_deepseek_api_key(')
    || !commandRegistry.includes('settings::set_deepseek_api_key,')) {
  fail('DeepSeek API Key 设置入口的前后端命令契约不完整');
}

const editContextScript = read('ui/edit-context.js');
const menuScript = read('src-tauri/resources/injections/context-menu.js');
const navigation = read('src-tauri/src/webview/navigation.rs');
if (!navigation.includes('include_str!("../../resources/injections/context-menu.js")')) {
  fail('src-tauri/src/webview/navigation.rs: 右键菜单资源未通过 include_str! 嵌入');
} else if (!navigation.includes('include_str!("../../../ui/edit-context.js")')) {
  fail('src-tauri/src/webview/navigation.rs: 文本编辑菜单核心未与 dsh 注入页共用');
} else {
  try {
    new vm.Script(`(function(){${editContextScript}\n${menuScript}\n})();`, { filename: 'context-menu.js' });
  } catch (error) {
    fail(`MENU_INJECT JavaScript 语法失败: ${error.message}`);
  }
  for (const contract of [
    "T('打开文件', 'Open file')",
    "T('复制路径', 'Copy path')",
    "T('复制文件内容', 'Copy file contents')",
    'contextSequence',
    "dshdUrl('normalize'",
    'prefers-reduced-motion',
    "document.addEventListener('contextmenu', onCtx)",
  ]) {
    if (!menuScript.includes(contract)) fail(`MENU_INJECT 缺少兼容契约: ${contract}`);
  }
  if (!navigation.includes('delete window.__dshdProtocolToken')) fail('注入后未清除全局协议令牌');
}

const pkg = JSON.parse(read('package.json'));
const npmLock = JSON.parse(read('package-lock.json'));
const tauri = JSON.parse(read('src-tauri/tauri.conf.json'));
const cargo = read('src-tauri/Cargo.toml').match(/^version\s*=\s*"([^"]+)"/m)?.[1];
const lockVersion = read('src-tauri/Cargo.lock').match(/\[\[package\]\]\s*\nname = "dsh-box"\s*\nversion = "([^"]+)"/m)?.[1];
const npmLockRootVersion = npmLock.packages?.['']?.version;
if (!cargo || pkg.version !== cargo || npmLock.version !== cargo || npmLockRootVersion !== cargo
    || tauri.version !== cargo || lockVersion !== cargo) {
  fail(`版本不一致: Cargo=${cargo} package=${pkg.version} npm-lock=${npmLock.version}/${npmLockRootVersion} tauri=${tauri.version} cargo-lock=${lockVersion}`);
}
if (pkg.engines?.node !== '^22.19.0 || >=24.0.0') fail('package.json: Node.js 工具链范围未与 dsh 官方兼容范围对齐');

const buildWorkflow = read('.github/workflows/build.yml');
if (!/permissions:\s*\n\s+contents: read/.test(buildWorkflow)) {
  fail('.github/workflows/build.yml: 默认 GITHUB_TOKEN 必须保持 contents: read');
}
if (buildWorkflow.includes('if-no-files-found: warn')) {
  fail('.github/workflows/build.yml: 发布产物缺失必须失败，不能只告警');
}
if (buildWorkflow.includes('branches: [main, master]')) {
  fail('.github/workflows/build.yml: 默认分支只允许 main');
}
if (buildWorkflow.includes("github.event_name != 'pull_request'")) {
  fail('.github/workflows/build.yml: 普通 main push 不得构建或上传发布附件');
}
if (!buildWorkflow.includes('^v[0-9]+\\.[0-9]+\\.[0-9]+$')
    || !buildWorkflow.includes('startsWith(github.ref, \'refs/tags/\')')) {
  fail('.github/workflows/build.yml: 发布必须受严格版本 tag 与 tag-only 条件约束');
}
if (read('dev-run.ps1').includes('<title>DeepSeek Harness Box</title>')) {
  fail('dev-run.ps1: 开发服务器健康检查仍引用旧产品标题');
}

function packageNameFromSpec(spec) {
  const plain = String(spec || '').split('#')[0].trim();
  if (!plain || plain.includes('://') || plain.startsWith('git+') || plain.startsWith('.')) return '';
  if (plain.startsWith('@')) {
    const slash = plain.indexOf('/');
    if (slash < 2) return '';
    const versionAt = plain.lastIndexOf('@');
    return versionAt > slash ? plain.slice(0, versionAt) : plain;
  }
  const versionAt = plain.lastIndexOf('@');
  return versionAt > 0 ? plain.slice(0, versionAt) : plain;
}

const presets = JSON.parse(read('src-tauri/resources/builtin-plugins.json'));
const presetIds = new Set();
for (const preset of presets) {
  if (!preset.id || !preset.spec || !preset.name || !preset.description_zh || !preset.description_en || !preset.homepage) {
    fail(`builtin-plugins.json: 条目字段不完整: ${JSON.stringify(preset)}`);
  }
  if (presetIds.has(preset.id)) fail(`builtin-plugins.json: 重复 id=${preset.id}`);
  presetIds.add(preset.id);
  if (packageNameFromSpec(preset.spec) !== preset.id) {
    fail(`builtin-plugins.json: id 与安装 spec 的包名不一致: ${preset.id} / ${preset.spec}`);
  }
  if (!/^https:\/\/github\.com\//.test(preset.homepage || '')) fail(`builtin-plugins.json: 非 GitHub HTTPS 地址: ${preset.homepage}`);
}

const recommended = JSON.parse(read('src-tauri/resources/recommended-plugins.json'));
const recommendedIds = new Set();
for (const plugin of recommended) {
  if (!plugin.id || !plugin.spec || !plugin.name || !plugin.description_zh || !plugin.description_en || !plugin.homepage) {
    fail(`recommended-plugins.json: 条目字段不完整: ${JSON.stringify(plugin)}`);
  }
  if (recommendedIds.has(plugin.id)) fail(`recommended-plugins.json: 重复 id=${plugin.id}`);
  recommendedIds.add(plugin.id);
  if (presetIds.has(plugin.id)) {
    fail(`插件不能同时出现在内置与社区清单: ${plugin.id}`);
  }
  if (packageNameFromSpec(plugin.spec) !== plugin.id) {
    fail(`recommended-plugins.json: id 与安装 spec 的包名不一致: ${plugin.id} / ${plugin.spec}`);
  }
  if (!/^https:\/\/github\.com\//.test(plugin.homepage)) {
    fail(`recommended-plugins.json: 非 GitHub HTTPS 地址: ${plugin.homepage}`);
  }
}

for (const file of tracked.filter((file) => file.endsWith('.ps1'))) {
  const bytes = fs.readFileSync(path.join(root, file));
  if (!(bytes[0] === 0xef && bytes[1] === 0xbb && bytes[2] === 0xbf)) fail(`${file}: 缺少 UTF-8 BOM`);
}

for (const file of tracked.filter((file) => file.endsWith('.md'))) {
  const text = read(file);
  for (const match of text.matchAll(/!?\[[^\]]*\]\(([^)]+)\)/g)) {
    let target = match[1].trim().replace(/^<|>$/g, '');
    if (!target || /^(?:#|[a-z]+:|\/\/)/i.test(target)) continue;
    target = decodeURIComponent(target.split('#')[0]);
    if (!fs.existsSync(path.resolve(root, path.dirname(file), target))) fail(`${file}: 本地链接不存在: ${target}`);
  }
}

const pngExpectations = new Map([
  ['src-tauri/icons/32x32.png', [32, 32]],
  ['src-tauri/icons/40x40.png', [40, 40]],
  ['src-tauri/icons/48x48.png', [48, 48]],
  ['src-tauri/icons/64x64.png', [64, 64]],
  ['src-tauri/icons/128x128.png', [128, 128]],
  ['src-tauri/icons/128x128@2x.png', [256, 256]],
  ['src-tauri/icons/256x256.png', [256, 256]],
  ['src-tauri/icons/tray-16.png', [16, 16]],
  ['src-tauri/icons/tray-20.png', [20, 20]],
  ['src-tauri/icons/tray-24.png', [24, 24]],
  ['src-tauri/icons/tray-32.png', [32, 32]],
]);
for (const [file, [expectedWidth, expectedHeight]] of pngExpectations) {
  const bytes = fs.readFileSync(path.join(root, file));
  const signature = bytes.subarray(0, 8).toString('hex');
  const width = bytes.readUInt32BE(16);
  const height = bytes.readUInt32BE(20);
  if (signature !== '89504e470d0a1a0a' || width !== expectedWidth || height !== expectedHeight) {
    fail(`${file}: PNG 规格异常 (${width}x${height})`);
  }
}
if (!read('scripts/gen-icons.mjs').includes("assets', 'brand', 'deepseek-mark.svg")) {
  fail('图标生成脚本未使用统一品牌源 assets/brand/deepseek-mark.svg');
}
const ico = fs.readFileSync(path.join(root, 'src-tauri/icons/icon.ico'));
if (ico.subarray(0, 4).toString('hex') !== '00000100') fail('src-tauri/icons/icon.ico: ICO 文件头异常');
const icns = fs.readFileSync(path.join(root, 'src-tauri/icons/icon.icns'));
if (icns.subarray(0, 4).toString('ascii') !== 'icns') fail('src-tauri/icons/icon.icns: ICNS 文件头异常');

if (failures.length) {
  console.error(`项目检查失败（${failures.length} 项）：`);
  for (const failure of failures) console.error(`- ${failure}`);
  process.exit(1);
}

console.log(`项目检查通过：${tracked.length} 个受控文件，JS/HTML/i18n/版本/配置/文档链接/图标/右键契约均有效。`);
